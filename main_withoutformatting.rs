use anyhow::{anyhow, Context, Result};
use clap::Parser;
use itoa::Buffer as Itoa;
use memmap2::Mmap;
use rayon::prelude::*;
use regex::Regex;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;
use walkdir::WalkDir;

/// CLI arguments
#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Ultra-fast ASN.1 DER Decoder -> JSONL (Rust, hex-only values, structure-correct)",
    long_about = None
)]
struct Cli {
    /// ASN.1 schema file
    #[arg(long = "schema")]
    schema: PathBuf,

    /// Root ASN.1 type (e.g. PGWRecord). Use "auto" to infer.
    #[arg(long = "root-type")]
    root_type: String,

    /// Output directory
    #[arg(long = "output-dir")]
    output_dir: PathBuf,

    /// Optional: only decode files matching these extensions (comma-separated), e.g. "dat,bin"
    #[arg(long = "ext")]
    ext: Option<String>,

    /// DER-encoded input files or directories (directories scanned recursively)
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
}

/// Field info for SEQUENCE / SET
#[derive(Debug, Clone)]
struct FieldSpec {
    name: String,
    field_type: String,
    #[allow(dead_code)]
    optional: bool,
    is_sequence_of: bool,
}

/// Schema representation
#[derive(Debug, Default)]
struct Asn1Schema {
    // type_name -> tag -> (field_name, field_type)
    // NOTE: untagged CHOICE alternatives are stored with synthetic tags SYNTH_CHOICE_BASE + idx
    choices: HashMap<String, HashMap<u32, (String, String)>>,
    sequences: HashMap<String, HashMap<u32, FieldSpec>>,
    sets: HashMap<String, HashMap<u32, FieldSpec>>,
    primitives: HashMap<String, String>, // type_name -> primitive kind
}

// Synthetic tags reserved for untagged CHOICE alternatives.
const SYNTH_CHOICE_BASE: u32 = 0xFFFF_FF00;

#[inline]
fn is_synth_choice_tag(t: u32) -> bool {
    t >= SYNTH_CHOICE_BASE
}

impl Asn1Schema {
    fn parse(schema_text: &str) -> Result<Self> {
        let comment_strip_re = Regex::new(r"--.*?(?:\n|$)")?;

        // One-line-ish matcher for TypeName ::= KIND { ... }
        let type_assign_re = Regex::new(
            r"(?s)([\w-]+)\s*::=\s*(CHOICE|SEQUENCE|SET|ENUMERATED|INTEGER|OCTET STRING|BIT STRING|IA5String|UTF8String|BOOLEAN|NULL|TBCD-STRING)\s*(?:\(([^)]*)\))?\s*(\{[^}]*\})?",
        )?;

        // Allow '-' in identifiers.
        // IMPORTANT: for the right-hand side type, allow multi-word like "OCTET STRING".
        let choice_tagged_re = Regex::new(r"([\w-]+)\s+\[(\d+)\]\s+([\w-]+(?:\s+[\w-]+)?)")?;
        let choice_untagged_re = Regex::new(r"([\w-]+)\s+([\w-]+(?:\s+[\w-]+)?)")?;

        let sequence_body_re = Regex::new(
            r"([\w-]+)\s+\[(\d+)\]\s+([\w-]+(?:\s+OF\s+[\w-]+)?)\s*(OPTIONAL)?",
        )?;

        let mut schema = Asn1Schema::default();
        let stripped = comment_strip_re.replace_all(schema_text, "");

        for caps in type_assign_re.captures_iter(&stripped) {
            let type_name = caps.get(1).unwrap().as_str().to_string();
            let type_kind = caps.get(2).unwrap().as_str();
            let body = caps.get(4).map(|m| m.as_str()).unwrap_or("");

            match type_kind {
                "CHOICE" => {
                    let mut alts = HashMap::new();

                    // Tagged alternatives: name [n] Type
                    for c in choice_tagged_re.captures_iter(body) {
                        let field_name = c.get(1).unwrap().as_str().to_string();
                        let tag: u32 = c.get(2).unwrap().as_str().parse()?;
                        let field_type = c.get(3).unwrap().as_str().trim().to_string();
                        alts.insert(tag, (field_name, field_type));
                    }

                    // Untagged alternatives: name Type (store under synthetic tags)
                    if alts.is_empty() {
                        let mut idx: u32 = 0;
                        for c in choice_untagged_re.captures_iter(body) {
                            let field_name = c.get(1).unwrap().as_str().to_string();
                            let field_type = c.get(2).unwrap().as_str().trim().to_string();
                            if field_name.is_empty() || field_type.is_empty() {
                                continue;
                            }
                            alts.insert(SYNTH_CHOICE_BASE + idx, (field_name, field_type));
                            idx += 1;
                            if idx >= 255 {
                                break;
                            }
                        }
                    }

                    schema.choices.insert(type_name, alts);
                }
                "SEQUENCE" | "SET" => {
                    let mut fields = HashMap::new();
                    for c in sequence_body_re.captures_iter(body) {
                        let field_name = c.get(1).unwrap().as_str().to_string();
                        let tag: u32 = c.get(2).unwrap().as_str().parse()?;
                        let type_spec = c.get(3).unwrap().as_str().to_string();
                        let optional = c.get(4).is_some();

                        let mut is_sequence_of = false;
                        let mut element_type = type_spec.clone();
                        if let Some(pos) = type_spec.find(" OF ") {
                            is_sequence_of = true;
                            element_type = type_spec[pos + 4..].trim().to_string();
                        }

                        fields.insert(
                            tag,
                            FieldSpec {
                                name: field_name,
                                field_type: element_type,
                                optional,
                                is_sequence_of,
                            },
                        );
                    }
                    if type_kind == "SEQUENCE" {
                        schema.sequences.insert(type_name, fields);
                    } else {
                        schema.sets.insert(type_name, fields);
                    }
                }
                // Everything else treated as primitive alias in hex-only mode
                kind => {
                    schema.primitives.insert(type_name, kind.to_string());
                }
            }
        }

        Ok(schema)
    }

    #[inline]
    fn knows_type(&self, t: &str) -> bool {
        self.choices.contains_key(t)
            || self.sequences.contains_key(t)
            || self.sets.contains_key(t)
            || self.primitives.contains_key(t)
    }
}

/// A parsed TLV
#[derive(Debug, Clone)]
struct Tlv<'a> {
    tag_class: u8,
    constructed: bool,
    tag_num: u32,
    #[allow(dead_code)]
    length: usize,
    value: &'a [u8],
    raw: &'a [u8],
}

/* ===========================
   FAST OUTPUT PRIMITIVES
   =========================== */

thread_local! {
    static HEX_BUF: RefCell<Vec<u8>> = RefCell::new(Vec::new());
}

#[inline]
fn write_json_string<W: Write>(w: &mut W, s: &str) -> Result<()> {
    w.write_all(b"\"")?;

    // Chunked writing: write contiguous "safe" spans in one go.
    let bytes = s.as_bytes();
    let mut i = 0usize;
    let mut start = 0usize;

    while i < bytes.len() {
        let b = bytes[i];
        let needs_escape = matches!(b, b'"' | b'\\' | b'\n' | b'\r' | b'\t') || b < 0x20;

        if needs_escape {
            if start < i {
                w.write_all(&bytes[start..i])?;
            }
            match b {
                b'"' => w.write_all(b"\\\"")?,
                b'\\' => w.write_all(b"\\\\")?,
                b'\n' => w.write_all(b"\\n")?,
                b'\r' => w.write_all(b"\\r")?,
                b'\t' => w.write_all(b"\\t")?,
                c if c < 0x20 => {
                    const HEX: &[u8; 16] = b"0123456789abcdef";
                    let mut esc = [b'\\', b'u', b'0', b'0', 0u8, 0u8];
                    esc[4] = HEX[((c >> 4) & 0x0F) as usize];
                    esc[5] = HEX[(c & 0x0F) as usize];
                    w.write_all(&esc)?;
                }
                _ => {}
            }
            i += 1;
            start = i;
            continue;
        }

        i += 1;
    }

    if start < bytes.len() {
        w.write_all(&bytes[start..])?;
    }

    w.write_all(b"\"")?;
    Ok(())
}

#[inline]
fn write_hex_json<W: Write>(w: &mut W, data: &[u8]) -> Result<()> {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    HEX_BUF.with(|cell| {
        let mut buf = cell.borrow_mut();
        buf.clear();
        buf.reserve(2 + data.len() * 2);

        buf.push(b'"');
        for &b in data {
            buf.push(HEX[(b >> 4) as usize]);
            buf.push(HEX[(b & 0x0F) as usize]);
        }
        buf.push(b'"');

        w.write_all(&buf)?;
        Ok(())
    })
}

#[inline]
fn write_field_key<W: Write>(out: &mut W, idx: usize) -> Result<()> {
    out.write_all(b"\"field_")?;
    let mut b = Itoa::new();
    out.write_all(b.format(idx).as_bytes())?;
    out.write_all(b"\"")?;
    Ok(())
}

#[inline]
fn write_unknown_tag_key<W: Write>(out: &mut W, tag: u32) -> Result<()> {
    out.write_all(b"\"unknown_tag_")?;
    let mut b = Itoa::new();
    out.write_all(b.format(tag).as_bytes())?;
    out.write_all(b"\"")?;
    Ok(())
}

/* ===========================
   DECODER
   =========================== */

struct DerDecoder {
    schema: Asn1Schema,
    cs_choice_index: HashMap<u32, String>,
}

impl DerDecoder {
    fn new(schema: Asn1Schema) -> Self {
        // Build context-specific CHOICE index from tagged CHOICE alts only (ignore synthetic)
        let mut cs_choice_index = HashMap::new();
        for (_choice_name, alts) in &schema.choices {
            for (tag, (_fname, ftype)) in alts {
                if is_synth_choice_tag(*tag) {
                    continue;
                }
                cs_choice_index.entry(*tag).or_insert(ftype.clone());
            }
        }

        Self { schema, cs_choice_index }
    }

    #[inline]
    fn parse_tlv<'a>(&self, data: &'a [u8], mut offset: usize) -> Option<(Tlv<'a>, usize)> {
        let n = data.len();
        if offset >= n {
            return None;
        }
        let start = offset;

        let tag_byte = data[offset];
        offset += 1;

        let tag_class = (tag_byte >> 6) & 0x03;
        let constructed = ((tag_byte >> 5) & 0x01) != 0;
        let mut tag_num = (tag_byte & 0x1F) as u32;

        if tag_num == 0x1F {
            tag_num = 0;
            while offset < n {
                let b = data[offset];
                offset += 1;
                tag_num = (tag_num << 7) | (b & 0x7F) as u32;
                if b & 0x80 == 0 {
                    break;
                }
            }
            if offset >= n {
                return None;
            }
        }

        if offset >= n {
            return None;
        }

        let length_byte = data[offset];
        offset += 1;

        let length: usize;
        if (length_byte & 0x80) != 0 {
            let num_octets = (length_byte & 0x7F) as usize;
            if num_octets == 0 || offset + num_octets > n {
                return None;
            }
            let mut l = 0usize;
            for _ in 0..num_octets {
                l = (l << 8) | data[offset] as usize;
                offset += 1;
            }
            length = l;
        } else {
            length = length_byte as usize;
        }

        if offset + length > n {
            return None;
        }

        let value = &data[offset..offset + length];
        offset += length;
        let raw = &data[start..offset];

        Some((
            Tlv {
                tag_class,
                constructed,
                tag_num,
                length,
                value,
                raw,
            },
            offset,
        ))
    }

    #[inline]
    fn is_universal_octet_string(tlv: &Tlv) -> bool {
        tlv.tag_class == 0 && !tlv.constructed && tlv.tag_num == 4
    }

    #[inline]
    fn unwrap_octet_string_containing_tlv<'a>(&self, tlv: &Tlv<'a>) -> Option<Tlv<'a>> {
        if !Self::is_universal_octet_string(tlv) {
            return None;
        }
        self.parse_tlv(tlv.value, 0).map(|(inner, _)| inner)
    }

    #[inline]
    fn unwrap_constructed_containing_tlv<'a>(&self, tlv: &Tlv<'a>) -> Option<Tlv<'a>> {
        if !tlv.constructed {
            return None;
        }
        self.parse_tlv(tlv.value, 0).map(|(inner, _)| inner)
    }

    fn write_generic_value<W: Write>(&self, tlv: &Tlv, out: &mut W) -> Result<()> {
        if tlv.constructed {
            out.write_all(b"{")?;
            let mut offset = 0usize;
            let mut idx = 0usize;
            let mut first = true;

            while offset < tlv.value.len() {
                let (inner, new_off) = match self.parse_tlv(tlv.value, offset) {
                    Some(t) => t,
                    None => break,
                };
                if new_off <= offset {
                    break;
                }

                if !first {
                    out.write_all(b",")?;
                }
                first = false;

                write_field_key(out, idx)?;
                out.write_all(b":")?;
                self.write_generic_value(&inner, out)?;

                offset = new_off;
                idx += 1;
            }

            out.write_all(b"}")?;
        } else {
            write_hex_json(out, tlv.value)?;
        }
        Ok(())
    }

    fn write_type<W: Write>(&self, data: &[u8], type_name: &str, out: &mut W) -> Result<()> {
        if let Some(alts) = self.schema.choices.get(type_name) {
            self.write_choice(data, alts, out)?;
            return Ok(());
        }

        if let Some(fields) = self.schema.sequences.get(type_name) {
            self.write_sequence(data, fields, out)?;
            return Ok(());
        }

        if let Some(fields) = self.schema.sets.get(type_name) {
            self.write_sequence(data, fields, out)?;
            return Ok(());
        }

        write_hex_json(out, data)?;
        Ok(())
    }

    fn write_sequence<W: Write>(
        &self,
        data: &[u8],
        field_spec: &HashMap<u32, FieldSpec>,
        out: &mut W,
    ) -> Result<()> {
        out.write_all(b"{")?;
        let mut offset = 0usize;
        let mut first = true;

        while offset < data.len() {
            let (tlv, new_off) = match self.parse_tlv(data, offset) {
                Some(t) => t,
                None => break,
            };
            if new_off <= offset {
                break;
            }

            if let Some(field) = field_spec.get(&tlv.tag_num) {
                if !first {
                    out.write_all(b",")?;
                }
                first = false;

                write_json_string(out, &field.name)?;
                out.write_all(b":")?;

                if field.is_sequence_of {
                    self.write_sequence_of(tlv.value, &field.field_type, out)?;
                } else if self.schema.choices.contains_key(&field.field_type) {
                    // CHOICE needs raw TLV so it can see the tag (wrapper)
                    self.write_type(tlv.raw, &field.field_type, out)?;
                } else if tlv.constructed {
                    self.write_type(tlv.value, &field.field_type, out)?;
                } else {
                    write_hex_json(out, tlv.value)?;
                }
            } else {
                if !first {
                    out.write_all(b",")?;
                }
                first = false;

                write_unknown_tag_key(out, tlv.tag_num)?;
                out.write_all(b":")?;
                write_hex_json(out, tlv.value)?;
            }

            offset = new_off;
        }

        out.write_all(b"}")?;
        Ok(())
    }

    fn write_sequence_of<W: Write>(&self, data: &[u8], element_type: &str, out: &mut W) -> Result<()> {
        out.write_all(b"[")?;
        let mut offset = 0usize;
        let mut first = true;

        let is_choice = self.schema.choices.contains_key(element_type);

        while offset < data.len() {
            let (tlv, new_off) = match self.parse_tlv(data, offset) {
                Some(t) => t,
                None => break,
            };
            if new_off <= offset {
                break;
            }

            if !first {
                out.write_all(b",")?;
            }
            first = false;

            if is_choice {
                self.write_type(tlv.raw, element_type, out)?;
            } else if tlv.constructed {
                self.write_type(tlv.value, element_type, out)?;
            } else {
                write_hex_json(out, tlv.value)?;
            }

            offset = new_off;
        }

        out.write_all(b"]")?;
        Ok(())
    }

    /// Probe helper for untagged CHOICE:
    /// Return true if this tlv can be an encoding for alt_type.
    #[inline]
    fn choice_alt_matches_tlv(&self, alt_type: &str, tlv: &Tlv) -> bool {
        // If alt_type is a CHOICE with tagged alts (like IPBinaryAddress),
        // then the incoming tlv.tag_num should match one of its real tags.
        if let Some(sub_alts) = self.schema.choices.get(alt_type) {
            if sub_alts.contains_key(&tlv.tag_num) {
                return true;
            }
        }

        // If alt_type is SEQUENCE / SET
        if self.schema.sequences.contains_key(alt_type) {
            return tlv.tag_class == 0 && tlv.constructed && tlv.tag_num == 16;
        }
        if self.schema.sets.contains_key(alt_type) {
            return tlv.tag_class == 0 && tlv.constructed && tlv.tag_num == 17;
        }

        false
    }

    fn write_choice<W: Write>(
        &self,
        data: &[u8],
        alts: &HashMap<u32, (String, String)>,
        out: &mut W,
    ) -> Result<()> {
        // Parse outer TLV from data
        let (outer, _) = match self.parse_tlv(data, 0) {
            Some(t) => t,
            None => {
                out.write_all(b"null")?;
                return Ok(());
            }
        };

        // Fixed-size candidates (no Vec alloc)
        let inner_constructed = self.unwrap_constructed_containing_tlv(&outer);
        let inner_octet = self.unwrap_octet_string_containing_tlv(&outer);

        // Choose a "probe" candidate for untagged CHOICE (prefer unwrapped)
        let probe = inner_octet
            .as_ref()
            .or(inner_constructed.as_ref())
            .unwrap_or(&outer);

        out.write_all(b"{")?;

        // 1) Tagged CHOICE resolution (try outer, then unwrapped candidates)
        // Try outer
        if let Some((field_name, type_name)) = alts.get(&outer.tag_num) {
            write_json_string(out, field_name)?;
            out.write_all(b":")?;
            self.write_type(outer.value, type_name, out)?;
            out.write_all(b"}")?;
            return Ok(());
        }

        // Try constructed inner
        if let Some(inner) = inner_constructed.as_ref() {
            if let Some((field_name, type_name)) = alts.get(&inner.tag_num) {
                write_json_string(out, field_name)?;
                out.write_all(b":")?;
                self.write_type(inner.value, type_name, out)?;
                out.write_all(b"}")?;
                return Ok(());
            }
        }

        // Try octet-wrapped inner
        if let Some(inner) = inner_octet.as_ref() {
            if let Some((field_name, type_name)) = alts.get(&inner.tag_num) {
                write_json_string(out, field_name)?;
                out.write_all(b":")?;
                self.write_type(inner.value, type_name, out)?;
                out.write_all(b"}")?;
                return Ok(());
            }
        }

        // 2) Untagged CHOICE: probe synthetic alternatives without allocating/sorting keys.
        // Synthetic tags were inserted as SYNTH_CHOICE_BASE + idx sequentially.
        for idx in 0u32..255 {
            let k = SYNTH_CHOICE_BASE + idx;
            let Some((fname, ftype)) = alts.get(&k) else { continue; };

            if self.choice_alt_matches_tlv(ftype, probe) {
                write_json_string(out, fname)?;
                out.write_all(b":")?;

                // For untagged CHOICE, bytes are exactly the alternative TLV.
                if self.schema.choices.contains_key(ftype) {
                    self.write_type(probe.raw, ftype, out)?;
                } else {
                    self.write_type(probe.value, ftype, out)?;
                }

                out.write_all(b"}")?;
                return Ok(());
            }
        }

        // Fallback: show the probe raw for debugging
        write_json_string(out, "unknown_alternative")?;
        out.write_all(b":")?;
        write_hex_json(out, probe.raw)?;
        out.write_all(b"}")?;
        Ok(())
    }

    fn write_root_tlv_with_type<W: Write>(&self, tlv: &Tlv, root_type: &str, out: &mut W) -> Result<()> {
        if !self.schema.knows_type(root_type) {
            self.write_auto_record(tlv, out)?;
            return Ok(());
        }

        if self.schema.choices.contains_key(root_type) {
            self.write_type(tlv.raw, root_type, out)?;
        } else {
            self.write_type(tlv.value, root_type, out)?;
        }
        Ok(())
    }

    fn write_auto_record<W: Write>(&self, tlv: &Tlv, out: &mut W) -> Result<()> {
        if tlv.tag_class == 2 {
            if let Some(alt_type) = self.cs_choice_index.get(&tlv.tag_num) {
                out.write_all(b"{")?;
                write_json_string(out, &lower_first(alt_type))?;
                out.write_all(b":")?;
                self.write_type(tlv.value, alt_type, out)?;
                out.write_all(b"}")?;
                return Ok(());
            }
        }

        out.write_all(b"{")?;
        write_json_string(out, "unknown")?;
        out.write_all(b":")?;
        self.write_generic_value(tlv, out)?;
        out.write_all(b"}")?;
        Ok(())
    }
}

#[inline]
fn lower_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
    }
}

/// Expand inputs: any arg can be a file or a directory (directories scanned recursively).
fn expand_inputs(inputs: &[PathBuf], allowed_exts: Option<&HashSet<String>>) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = Vec::new();

    for p in inputs {
        if p.is_file() {
            if should_include(p, allowed_exts) {
                files.push(p.clone());
            }
        } else if p.is_dir() {
            for entry in WalkDir::new(p).follow_links(false) {
                let entry = entry?;
                let path = entry.path();
                if path.is_file() && should_include(path, allowed_exts) {
                    files.push(path.to_path_buf());
                }
            }
        } else {
            return Err(anyhow!("Input path is not a file or directory: {:?}", p));
        }
    }

    files.sort();
    files.dedup();
    Ok(files)
}

#[inline]
fn should_include(path: &Path, allowed_exts: Option<&HashSet<String>>) -> bool {
    let Some(set) = allowed_exts else {
        return true;
    };

    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };

    set.contains(&ext.to_ascii_lowercase())
}

fn process_file(decoder: &DerDecoder, root_type: &str, in_path: &Path, out_dir: &Path) -> Result<usize> {
    let file = File::open(in_path).with_context(|| format!("Failed to open input file {:?}", in_path))?;
    let mmap = unsafe { Mmap::map(&file)? };
    let data: &[u8] = &mmap;

    if data.is_empty() {
        return Ok(0);
    }

    let file_name = in_path
        .file_name()
        .ok_or_else(|| anyhow!("Input path has no filename: {:?}", in_path))?
        .to_string_lossy()
        .to_string();

    let out_path = out_dir.join(format!("{}.jsonl", file_name));

    let out_file = File::create(&out_path).with_context(|| format!("Failed to create output file {:?}", out_path))?;
    let mut writer = BufWriter::with_capacity(16 * 1024 * 1024, out_file);

    let mut offset = 0usize;
    let mut count = 0usize;

    let use_auto = root_type.eq_ignore_ascii_case("auto") || root_type.is_empty();

    while offset < data.len() {
        let (tlv, new_off) = match decoder.parse_tlv(data, offset) {
            Some(t) => t,
            None => break,
        };
        if new_off <= offset {
            break;
        }

        if use_auto {
            decoder.write_auto_record(&tlv, &mut writer)?;
        } else {
            decoder.write_root_tlv_with_type(&tlv, root_type, &mut writer)?;
        }

        writer.write_all(b"\n")?;
        offset = new_off;
        count += 1;
    }

    writer.flush()?;
    Ok(count)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let overall_start = Instant::now();

    let allowed_exts: Option<HashSet<String>> = cli.ext.as_ref().map(|s| {
        s.split(',')
            .map(|x| x.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|x| !x.is_empty())
            .collect()
    });

    let schema_text =
        std::fs::read_to_string(&cli.schema).with_context(|| format!("Failed to read schema file {:?}", cli.schema))?;
    let schema = Asn1Schema::parse(&schema_text)?;
    let decoder = DerDecoder::new(schema);

    std::fs::create_dir_all(&cli.output_dir)?;

    let mut root_type = cli.root_type.clone();
    if !root_type.eq_ignore_ascii_case("auto") && !decoder.schema.knows_type(&root_type) {
        eprintln!(
            "WARNING: root-type '{}' does not appear in parsed schema. Falling back to auto mode.",
            root_type
        );
        root_type = "auto".to_string();
    }

    let input_files = expand_inputs(&cli.inputs, allowed_exts.as_ref())
        .with_context(|| "Failed to expand input files/directories")?;

    if input_files.is_empty() {
        eprintln!("No input files found.");
        return Ok(());
    }

    println!("Found {} input files", input_files.len());

    let out_dir = cli.output_dir.clone();
    let results: Vec<(PathBuf, Result<usize>)> = input_files
        .par_iter()
        .map(|p| {
            let count = process_file(&decoder, &root_type, p, &out_dir);
            (p.clone(), count)
        })
        .collect();

    let mut total_records = 0usize;
    for (path, res) in results {
        match res {
            Ok(count) => {
                total_records += count;
                println!(
                    "Decoded {} records from {:?} -> {}",
                    count,
                    path,
                    out_dir
                        .join(format!("{}.jsonl", path.file_name().unwrap().to_string_lossy()))
                        .display()
                );
            }
            Err(e) => {
                eprintln!("Decoding failed for {:?}: {:#}", path, e);
            }
        }
    }

    println!("Total decoded records: {}", total_records);
    println!("Total elapsed wall time: {:.3} s", overall_start.elapsed().as_secs_f64());
    Ok(())
}
