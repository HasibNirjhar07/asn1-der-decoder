use anyhow::{anyhow, Context, Result};
use clap::Parser;
use memmap2::Mmap;
use rayon::prelude::*;
use regex::Regex;
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
    about = "Ultra-fast ASN.1 DER Decoder -> JSONL (schema-based, hex-only values, no decimal conversion)",
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
    choices: HashMap<String, HashMap<u32, (String, String)>>,
    sequences: HashMap<String, HashMap<u32, FieldSpec>>,
    sets: HashMap<String, HashMap<u32, FieldSpec>>,
    primitives: HashMap<String, String>, // type_name -> primitive kind
    aliases: HashMap<String, String>,
}

const SYNTH_CHOICE_BASE: u32 = 0xFFFF_FF00;
#[inline]
fn is_synth_choice_tag(t: u32) -> bool {
    t >= SYNTH_CHOICE_BASE
}

impl Asn1Schema {
    fn parse(schema_text: &str) -> Result<Self> {
        let comment_strip_re = Regex::new(r"--.*?(?:\n|$)")?;
        let type_assign_re = Regex::new(
            r"(?s)([\w-]+)\s*::=\s*(CHOICE|SEQUENCE|SET|ENUMERATED|INTEGER|OCTET STRING|BIT STRING|IA5String|UTF8String|BOOLEAN|NULL|TBCD-STRING)\s*(?:\(([^)]*)\))?\s*(\{.*?\})?",
        )?;
        let alias_re = Regex::new(r"(?m)^\s*([\w-]+)\s*::=\s*([\w-]+)\s*$")?;

        let choice_tagged_re = Regex::new(r"([\w-]+)\s+\[(\d+)\]\s+([\w-]+)")?;
        let choice_untagged_re = Regex::new(r"([\w-]+)\s+([\w-]+)")?;

        let sequence_body_re = Regex::new(
            r"([\w-]+)\s+\[(\d+)\]\s+([\w-]+(?:\s+OF\s+[\w-]+)?)\s*(OPTIONAL)?",
        )?;

        let mut schema = Asn1Schema::default();
        let stripped = comment_strip_re.replace_all(schema_text, "");

        // aliases
        for cap in alias_re.captures_iter(&stripped) {
            let lhs = cap.get(1).unwrap().as_str().to_string();
            let rhs = cap.get(2).unwrap().as_str().to_string();

            let rhs_upper = rhs.to_ascii_uppercase();
            let is_keyword = matches!(
                rhs_upper.as_str(),
                "CHOICE"
                    | "SEQUENCE"
                    | "SET"
                    | "ENUMERATED"
                    | "INTEGER"
                    | "OCTET"
                    | "BIT"
                    | "IA5STRING"
                    | "UTF8STRING"
                    | "BOOLEAN"
                    | "NULL"
                    | "TBCD-STRING"
                    | "OCTET STRING"
                    | "BIT STRING"
            );

            if !is_keyword && lhs != rhs {
                schema.aliases.insert(lhs, rhs);
            }
        }

        // type assignments
        for caps in type_assign_re.captures_iter(&stripped) {
            let type_name = caps.get(1).unwrap().as_str().to_string();
            let type_kind = caps.get(2).unwrap().as_str();
            let body = caps.get(4).map(|m| m.as_str()).unwrap_or("");

            match type_kind {
                "CHOICE" => {
                    let mut alts = HashMap::new();

                    for c in choice_tagged_re.captures_iter(body) {
                        let field_name = c.get(1).unwrap().as_str().to_string();
                        let tag: u32 = c.get(2).unwrap().as_str().parse()?;
                        let field_type = c.get(3).unwrap().as_str().to_string();
                        alts.insert(tag, (field_name, field_type));
                    }

                    if alts.is_empty() {
                        let mut idx: u32 = 0;
                        for c in choice_untagged_re.captures_iter(body) {
                            let field_name = c.get(1).unwrap().as_str().to_string();
                            let field_type = c.get(2).unwrap().as_str().to_string();
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
                kind => {
                    schema.primitives.insert(type_name, kind.to_string());
                }
            }
        }

        Ok(schema)
    }

    #[inline]
    fn resolve_alias<'a>(&'a self, mut t: &'a str) -> &'a str {
        for _ in 0..16 {
            if let Some(next) = self.aliases.get(t) {
                t = next;
            } else {
                break;
            }
        }
        t
    }

    #[inline]
    fn knows_type(&self, t: &str) -> bool {
        let rt = self.resolve_alias(t);
        self.choices.contains_key(rt)
            || self.sequences.contains_key(rt)
            || self.sets.contains_key(rt)
            || self.primitives.contains_key(rt)
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

#[inline]
fn write_json_key<W: Write>(w: &mut W, key: &str) -> Result<()> {
    // minimal escaping: schema keys should be safe; do a cheap escape for quotes/backslash/control
    w.write_all(b"\"")?;
    for &b in key.as_bytes() {
        match b {
            b'"' => w.write_all(b"\\\"")?,
            b'\\' => w.write_all(b"\\\\")?,
            b'\n' => w.write_all(b"\\n")?,
            b'\r' => w.write_all(b"\\r")?,
            b'\t' => w.write_all(b"\\t")?,
            c if c < 0x20 => {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                let esc = [b'\\', b'u', b'0', b'0', HEX[(c >> 4) as usize], HEX[(c & 0x0F) as usize]];
                w.write_all(&esc)?;
            }
            c => w.write_all(&[c])?,
        }
    }
    w.write_all(b"\"")?;
    Ok(())
}

/// Ultra-fast hex encoder into a reusable per-file scratch.
/// Returns slice to encoded hex.
#[inline(always)]
fn hex_encode_into<'a>(bytes: &[u8], scratch: &'a mut Vec<u8>) -> &'a [u8] {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    scratch.clear();
    scratch.reserve(bytes.len() * 2);
    unsafe { scratch.set_len(bytes.len() * 2) };

    let mut j = 0usize;
    for &b in bytes {
        scratch[j] = HEX[(b >> 4) as usize];
        scratch[j + 1] = HEX[(b & 0x0F) as usize];
        j += 2;
    }
    &scratch[..j]
}

#[inline]
fn write_hex_json<W: Write>(w: &mut W, data: &[u8], scratch: &mut Vec<u8>) -> Result<()> {
    w.write_all(b"\"")?;
    let hex = hex_encode_into(data, scratch);
    w.write_all(hex)?;
    w.write_all(b"\"")?;
    Ok(())
}

#[inline]
fn lower_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
    }
}

struct DerDecoder {
    schema: Asn1Schema,
    cs_choice_index: HashMap<u32, String>,
}

impl DerDecoder {
    fn new(schema: Asn1Schema) -> Self {
        let mut cs_choice_index: HashMap<u32, String> = HashMap::new();
        for (_choice_name, alts) in &schema.choices {
            for (tag, (_field_name, field_type)) in alts {
                if is_synth_choice_tag(*tag) {
                    continue;
                }
                cs_choice_index.entry(*tag).or_insert(field_type.clone());
            }
        }
        Self { schema, cs_choice_index }
    }

    #[inline(always)]
    fn parse_tlv<'a>(&self, data: &'a [u8], mut offset: usize) -> Option<(Tlv<'a>, usize)> {
        let data_len = data.len();
        if offset >= data_len {
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
            while offset < data_len {
                let b = data[offset];
                offset += 1;
                tag_num = (tag_num << 7) | (b & 0x7F) as u32;
                if (b & 0x80) == 0 {
                    break;
                }
            }
            if offset >= data_len {
                return None;
            }
        }

        if offset >= data_len {
            return None;
        }

        let length_byte = data[offset];
        offset += 1;

        let length: usize;
        if (length_byte & 0x80) != 0 {
            let num_octets = (length_byte & 0x7F) as usize;
            if num_octets == 0 || offset + num_octets > data_len {
                return None;
            }
            let mut l: usize = 0;
            let end_len = offset + num_octets;
            while offset < end_len {
                l = (l << 8) | data[offset] as usize;
                offset += 1;
            }
            length = l;
        } else {
            length = length_byte as usize;
        }

        if offset + length > data_len {
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
    fn write_type<W: Write>(&self, data: &[u8], type_name: &str, out: &mut W, scratch: &mut Vec<u8>) -> Result<()> {
        let rt = self.schema.resolve_alias(type_name);

        if let Some(alts) = self.schema.choices.get(rt) {
            self.write_choice(data, alts, out, scratch)?;
            return Ok(());
        }
        if let Some(fields) = self.schema.sequences.get(rt) {
            self.write_sequence(data, fields, out, scratch)?;
            return Ok(());
        }
        if let Some(fields) = self.schema.sets.get(rt) {
            self.write_sequence(data, fields, out, scratch)?;
            return Ok(());
        }

        // primitive or unknown: ALWAYS hex (you requested no decimal conversion)
        write_hex_json(out, data, scratch)?;
        Ok(())
    }

    fn write_sequence<W: Write>(
        &self,
        data: &[u8],
        field_spec: &HashMap<u32, FieldSpec>,
        out: &mut W,
        scratch: &mut Vec<u8>,
    ) -> Result<()> {
        out.write_all(b"{")?;
        let mut offset = 0usize;
        let mut first = true;

        let mut itoa_buf = itoa::Buffer::new();

        while offset < data.len() {
            let (tlv, new_off) = match self.parse_tlv(data, offset) {
                Some(t) => t,
                None => break,
            };
            if new_off <= offset {
                break;
            }

            if !first { out.write_all(b",")?; }
            first = false;

            if let Some(field) = field_spec.get(&tlv.tag_num) {
                write_json_key(out, &field.name)?;
                out.write_all(b":")?;

                let resolved_field_type = self.schema.resolve_alias(&field.field_type);

                if field.is_sequence_of {
                    self.write_sequence_of(tlv.value, &field.field_type, out, scratch)?;
                } else if self.schema.choices.contains_key(resolved_field_type) {
                    // CHOICE needs raw TLV so it can see tag
                    self.write_type(tlv.raw, &field.field_type, out, scratch)?;
                } else if tlv.constructed {
                    self.write_type(tlv.value, &field.field_type, out, scratch)?;
                } else {
                    // scalar primitive: hex only
                    write_hex_json(out, tlv.value, scratch)?;
                }
            } else {
                // unknown_tag_<n>: hex
                out.write_all(b"\"unknown_tag_")?;
                out.write_all(itoa_buf.format(tlv.tag_num).as_bytes())?;
                out.write_all(b"\":")?;
                    // Debug to stderr
    eprintln!("Unknown tag {}, Raw: {:02x?}", tlv.tag_num, &tlv.raw[..tlv.raw.len().min(32)]);
                write_hex_json(out, tlv.value, scratch)?;
            }

            offset = new_off;
        }

        out.write_all(b"}")?;
        Ok(())
    }

    fn write_sequence_of<W: Write>(&self, data: &[u8], element_type: &str, out: &mut W, scratch: &mut Vec<u8>) -> Result<()> {
        out.write_all(b"[")?;
        let mut arr_first = true;
        let mut offset = 0usize;

        let is_choice = self.schema.choices.contains_key(self.schema.resolve_alias(element_type));

        while offset < data.len() {
            let (tlv, new_off) = match self.parse_tlv(data, offset) {
                Some(t) => t,
                None => break,
            };
            if new_off <= offset {
                break;
            }

            if !arr_first { out.write_all(b",")?; }
            arr_first = false;

            if is_choice {
                self.write_type(tlv.raw, element_type, out, scratch)?;
            } else if tlv.constructed {
                self.write_type(tlv.value, element_type, out, scratch)?;
            } else {
                write_hex_json(out, tlv.value, scratch)?;
            }

            offset = new_off;
        }

        out.write_all(b"]")?;
        Ok(())
    }

    fn choice_alt_matches_tlv(&self, alt_type: &str, tlv: &Tlv) -> bool {
        let rt = self.schema.resolve_alias(alt_type);

        if let Some(sub_alts) = self.schema.choices.get(rt) {
            if sub_alts.contains_key(&tlv.tag_num) {
                return true;
            }
        }

        if self.schema.sequences.contains_key(rt) {
            return tlv.tag_class == 0 && tlv.constructed && tlv.tag_num == 16;
        }
        if self.schema.sets.contains_key(rt) {
            return tlv.tag_class == 0 && tlv.constructed && tlv.tag_num == 17;
        }

        false
    }

    fn write_choice<W: Write>(
        &self,
        data: &[u8],
        alts: &HashMap<u32, (String, String)>,
        out: &mut W,
        scratch: &mut Vec<u8>,
    ) -> Result<()> {
        let (outer, _) = match self.parse_tlv(data, 0) {
            Some(t) => t,
            None => {
                out.write_all(b"null")?;
                return Ok(());
            }
        };

        // Candidate: outer only + unwrap constructed + unwrap octet-string
        let mut candidates: [Option<Tlv>; 3] = [None, None, None];
        candidates[0] = Some(outer.clone());

        if outer.constructed {
            candidates[1] = self.parse_tlv(outer.value, 0).map(|(inner, _)| inner);
        }
        if outer.tag_class == 0 && !outer.constructed && outer.tag_num == 4 {
            candidates[2] = self.parse_tlv(outer.value, 0).map(|(inner, _)| inner);
        }

        out.write_all(b"{")?;

        // Tagged CHOICE
        for cand in candidates.iter().flatten() {
            if let Some((field_name, type_name)) = alts.get(&cand.tag_num) {
                write_json_key(out, field_name)?;
                out.write_all(b":")?;
                self.write_type(cand.value, type_name, out, scratch)?;
                out.write_all(b"}")?;
                return Ok(());
            }
        }

        // Untagged CHOICE: probe synthetic alts
        let mut synth_keys: Vec<u32> = alts.keys().copied().filter(|t| is_synth_choice_tag(*t)).collect();
        synth_keys.sort_unstable();

        let probe = candidates.iter().flatten().last().unwrap();

        for k in synth_keys {
            let (fname, ftype) = &alts[&k];
            if self.choice_alt_matches_tlv(ftype, probe) {
                write_json_key(out, fname)?;
                out.write_all(b":")?;

                let rt = self.schema.resolve_alias(ftype);
                if self.schema.choices.contains_key(rt) {
                    self.write_type(probe.raw, ftype, out, scratch)?;
                } else {
                    self.write_type(probe.value, ftype, out, scratch)?;
                }

                out.write_all(b"}")?;
                return Ok(());
            }
        }

        write_json_key(out, "unknown_alternative")?;
        out.write_all(b":")?;
        write_hex_json(out, probe.raw, scratch)?;
        out.write_all(b"}")?;
        Ok(())
    }

    fn write_root_tlv_with_type<W: Write>(&self, tlv: &Tlv, root_type: &str, out: &mut W, scratch: &mut Vec<u8>) -> Result<()> {
        let rt = self.schema.resolve_alias(root_type);

        if !self.schema.knows_type(rt) {
            self.write_auto_record(tlv, out, scratch)?;
            return Ok(());
        }

        if self.schema.choices.contains_key(rt) {
            self.write_type(tlv.raw, root_type, out, scratch)?;
        } else {
            self.write_type(tlv.value, root_type, out, scratch)?;
        }
        Ok(())
    }

    fn write_auto_record<W: Write>(&self, tlv: &Tlv, out: &mut W, scratch: &mut Vec<u8>) -> Result<()> {
        if tlv.tag_class == 2 {
            if let Some(alt_type) = self.cs_choice_index.get(&tlv.tag_num) {
                out.write_all(b"{")?;
                let key = lower_first(alt_type);
                write_json_key(out, &key)?;
                out.write_all(b":")?;
                self.write_type(tlv.value, alt_type, out, scratch)?;
                out.write_all(b"}")?;
                return Ok(());
            }
        }

        // fallback: dump raw TLV hex
        out.write_all(b"{")?;
        write_json_key(out, "unknown")?;
        out.write_all(b":")?;
        write_hex_json(out, tlv.raw, scratch)?;
        out.write_all(b"}")?;
        Ok(())
    }
}

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

    // Bigger buffer helps a lot when JSONL is large
    let mut writer = BufWriter::with_capacity(64 * 1024 * 1024, out_file);

    // per-file reusable hex buffer (critical for speed)
    let mut hex_scratch: Vec<u8> = Vec::with_capacity(8 * 1024 * 1024);

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
            decoder.write_auto_record(&tlv, &mut writer, &mut hex_scratch)?;
        } else {
            decoder.write_root_tlv_with_type(&tlv, root_type, &mut writer, &mut hex_scratch)?;
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
        .map(|p| (p.clone(), process_file(&decoder, &root_type, p, &out_dir)))
        .collect();

    let mut total_records = 0usize;
    for (path, res) in results {
        match res {
            Ok(count) => {
                total_records += count;
                println!("Decoded {} records from {:?}", count, path);
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
