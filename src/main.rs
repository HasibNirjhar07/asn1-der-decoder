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
use serde::{Serialize, Deserialize};

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Ultra-fast ASN.1 DER/BER Decoder -> JSONL (schema-based, hex-only values)",
    long_about = None
)]
struct Cli {
    #[arg(long = "schema")]
    schema: Option<PathBuf>,

    // New flag: Path to save the compiled binary schema
    #[arg(long = "compile-schema")]
    compile_schema: Option<PathBuf>,

    // New flag: Path to load a pre-compiled binary schema
    #[arg(long = "load-compiled")]
    load_compiled: Option<PathBuf>,

    #[arg(long = "root-type")]
    root_type: String,

    #[arg(long = "output-dir")]
    output_dir: PathBuf,

    #[arg(long = "ext")]
    ext: Option<String>,

    #[arg(required = true)]
    inputs: Vec<PathBuf>,
}

type TagKey = (u8, u32);
const SYNTH_CHOICE_BASE: u32 = 0xFFFF_FF00;

#[inline]
fn is_synth_choice_tag(t: u32) -> bool {
    t >= SYNTH_CHOICE_BASE
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FieldSpec {
    name: String,
    field_type: String,
    #[allow(dead_code)]
    optional: bool,
    is_sequence_of: bool,
    is_set_of: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Asn1Schema {
    choices: HashMap<String, HashMap<TagKey, (String, String)>>,
    sequences: HashMap<String, HashMap<TagKey, FieldSpec>>,
    sets: HashMap<String, HashMap<TagKey, FieldSpec>>,

    seq_of_types: HashMap<String, String>,
    set_of_types: HashMap<String, String>,

    primitives: HashMap<String, String>,
    aliases: HashMap<String, String>,

    type_outer_tag: HashMap<String, TagKey>,
}

#[inline]
fn tag_class_from_word(word: Option<&str>) -> u8 {
    match word.map(|s| s.to_ascii_uppercase()) {
        Some(w) if w == "APPLICATION" => 1,
        Some(w) if w == "UNIVERSAL" => 0,
        Some(w) if w == "PRIVATE" => 3,
        Some(w) if w == "CONTEXT" || w == "CONTEXT-SPECIFIC" || w == "CONTEXTSPECIFIC" => 2,
        None => 2, // Default to Context-Specific if only a number is given [x]
        _ => 2,
    }
}

impl Asn1Schema {
    fn parse(schema_text: &str) -> Result<Self> {
        let snacc_directive_re = Regex::new(r"(?is)--\s*snacc\b.*?--")?;
        let comment_strip_re = Regex::new(r"(?m)--.*?$")?;
        let no_snacc = snacc_directive_re.replace_all(schema_text, " ");
        let stripped = comment_strip_re.replace_all(&no_snacc, "");

        // Updated regex to handle (IMPLICIT|EXPLICIT) and any identifier type
        let type_assign_re = Regex::new(
            r"(?s)([\w-]+)\s*::=\s*(?:\[\s*(?:(APPLICATION|UNIVERSAL|PRIVATE|CONTEXT|CONTEXT-SPECIFIC)\s+)?(\d+)\s*\]\s*)?(?:IMPLICIT|EXPLICIT)?\s*(CHOICE|SEQUENCE|SET|ENUMERATED|INTEGER|OCTET STRING|BIT STRING|IA5String|UTF8String|BOOLEAN|NULL|TBCD-STRING|OBJECT IDENTIFIER|[\w-]+)\s*(?:OF\s+([\w-]+))?\s*(?:\(([^)]*)\))?\s*(\{.*?\})?",
        )?;

        let alias_re = Regex::new(r"(?m)^\s*([\w-]+)\s*::=\s*([\w-]+)\s*$")?;

        // Updated choice regex to allow 0 whitespace before '[' e.g. "sIP-URI[0]"
        let choice_tagged_re = Regex::new(
            r"([\w-]+)\s*\[\s*(?:(APPLICATION|UNIVERSAL|PRIVATE|CONTEXT|CONTEXT-SPECIFIC)\s+)?(\d+)\s*\]\s*([\w-]+)",
        )?;
        let choice_untagged_re = Regex::new(r"([\w-]+)\s+([\w-]+)")?;

        // Updated field regex to handle optional IMPLICIT/EXPLICIT and tags
        let field_re = Regex::new(
            r"(?m)^\s*([\w-]+)\s*(?:\[\s*(?:(APPLICATION|UNIVERSAL|PRIVATE|CONTEXT|CONTEXT-SPECIFIC)\s+)?(\d+)\s*\])?\s*(?:IMPLICIT|EXPLICIT)?\s+((?:SET|SEQUENCE)\s+OF\s+[\w-]+|[\w-]+)\s*(?:DEFAULT\s+[^,\n]+)?\s*(OPTIONAL)?",
        )?;
        
        // Handle COMPONENTS OF (simple inheritance)
        let components_of_re = Regex::new(r"(?m)^\s*COMPONENTS\s+OF\s+([\w-]+)")?;

        let mut schema = Asn1Schema::default();

        // 1. Parse Aliases
        for cap in alias_re.captures_iter(&stripped) {
            let lhs = cap.get(1).unwrap().as_str().to_string();
            let rhs = cap.get(2).unwrap().as_str().to_string();
            let rhs_upper = rhs.to_ascii_uppercase();
            // Filter out keywords
            let is_keyword = matches!(
                rhs_upper.as_str(),
                "CHOICE" | "SEQUENCE" | "SET" | "ENUMERATED" | "INTEGER" | "OCTET" | "BIT" 
                | "IA5STRING" | "UTF8STRING" | "BOOLEAN" | "NULL" | "OBJECT" | "IDENTIFIER" | "BEGIN" | "END"
            );
            if !is_keyword && lhs != rhs {
                schema.aliases.insert(lhs, rhs);
            }
        }

        #[derive(Clone)]
        struct Def {
            type_name: String,
            type_kind: String,
            of_type: Option<String>,
            body: String,
        }
        let mut defs: Vec<Def> = Vec::new();

        // 2. Parse Type Definitions
        for caps in type_assign_re.captures_iter(&stripped) {
            let type_name = caps.get(1).unwrap().as_str().to_string();
            let tag_class_word = caps.get(2).map(|m| m.as_str());
            let tag_num_opt = caps.get(3).map(|m| m.as_str());
            let type_kind = caps.get(4).unwrap().as_str().trim().to_string();
            let of_type = caps.get(5).map(|m| m.as_str().to_string());
            let body = caps.get(7).map(|m| m.as_str()).unwrap_or("").to_string();

            if let Some(tag_num_str) = tag_num_opt {
                if let Ok(num) = tag_num_str.parse::<u32>() {
                    let cls = tag_class_from_word(tag_class_word);
                    schema.type_outer_tag.insert(type_name.clone(), (cls, num));
                }
            }

            match type_kind.as_str() {
                "CHOICE" | "SEQUENCE" | "SET" => {}
                kind => {
                    schema.primitives.insert(type_name.clone(), kind.to_string());
                }
            }

            defs.push(Def {
                type_name,
                type_kind,
                of_type,
                body,
            });
        }

        let mut components_queue: Vec<(String, String)> = Vec::new();

        // 3. Process Structures
        for d in defs {
            match d.type_kind.as_str() {
                "SEQUENCE" | "SET" => {
                    let is_set = d.type_kind == "SET";
                    if let Some(elem) = d.of_type.clone() {
                        if is_set {
                            schema.set_of_types.insert(d.type_name, elem);
                        } else {
                            schema.seq_of_types.insert(d.type_name, elem);
                        }
                        continue;
                    }

                    let mut fields: HashMap<TagKey, FieldSpec> = HashMap::new();
                    for c in field_re.captures_iter(&d.body) {
                        let field_name = c.get(1).unwrap().as_str().to_string();
                        let cls_word = c.get(2).map(|m| m.as_str());
                        let tag_opt = c.get(3).map(|m| m.as_str());
                        let type_spec = c.get(4).unwrap().as_str().trim().to_string();
                        let optional = c.get(5).is_some();

                        let mut is_sequence_of = false;
                        let mut is_set_of = false;
                        let mut element_type = type_spec.clone();

                        if let Some(rest) = type_spec.strip_prefix("SEQUENCE OF ") {
                            is_sequence_of = true;
                            element_type = rest.trim().to_string();
                        } else if let Some(rest) = type_spec.strip_prefix("SET OF ") {
                            is_set_of = true;
                            element_type = rest.trim().to_string();
                        }

                        let key: TagKey = if let Some(tag_str) = tag_opt {
                            let cls = tag_class_from_word(cls_word);
                            (cls, tag_str.parse::<u32>()?)
                        } else {
                            match schema.tag_for_type(&element_type) {
                                Some(tk) => tk,
                                None => continue,
                            }
                        };

                        fields.insert(
                            key,
                            FieldSpec {
                                name: field_name,
                                field_type: element_type,
                                optional,
                                is_sequence_of,
                                is_set_of,
                            },
                        );
                    }
                    
                    for c in components_of_re.captures_iter(&d.body) {
                        let source_type = c.get(1).unwrap().as_str().to_string();
                        components_queue.push((d.type_name.clone(), source_type));
                    }

                    if is_set {
                        schema.sets.insert(d.type_name, fields);
                    } else {
                        schema.sequences.insert(d.type_name, fields);
                    }
                }
                "CHOICE" => {
                    let mut alts: HashMap<TagKey, (String, String)> = HashMap::new();

                    for c in choice_tagged_re.captures_iter(&d.body) {
                        let field_name = c.get(1).unwrap().as_str().to_string();
                        let cls_word = c.get(2).map(|m| m.as_str());
                        let tag: u32 = c.get(3).unwrap().as_str().parse()?;
                        let field_type = c.get(4).unwrap().as_str().to_string();
                        let cls = tag_class_from_word(cls_word);
                        alts.insert((cls, tag), (field_name, field_type));
                    }

                    if alts.is_empty() {
                        let mut idx: u32 = 0;
                        for c in choice_untagged_re.captures_iter(&d.body) {
                            let field_name = c.get(1).unwrap().as_str().to_string();
                            let field_type = c.get(2).unwrap().as_str().to_string();
                            if field_name == "isPdu" || field_name == "TRUE" { continue; }
                            if !field_name.is_empty() && !field_type.is_empty() {
                                alts.insert((3u8, SYNTH_CHOICE_BASE + idx), (field_name, field_type));
                                idx += 1;
                            }
                        }
                    }

                    schema.choices.insert(d.type_name, alts);
                }
                _ => {}
            }
        }
        
        // 4. Resolve COMPONENTS OF
        for (target, source) in components_queue {
            let source_fields = if let Some(f) = schema.sequences.get(&source) {
                Some(f.clone())
            } else if let Some(f) = schema.sets.get(&source) {
                Some(f.clone())
            } else {
                None
            };
            
            if let Some(src) = source_fields {
                if let Some(tgt) = schema.sequences.get_mut(&target) {
                    tgt.extend(src);
                } else if let Some(tgt) = schema.sets.get_mut(&target) {
                    tgt.extend(src);
                }
            }
        }

        Ok(schema)
    }

    #[inline]
    fn resolve_alias<'a>(&'a self, mut t: &'a str) -> &'a str {
        for _ in 0..32 {
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
            || self.seq_of_types.contains_key(rt)
            || self.set_of_types.contains_key(rt)
            || self.primitives.contains_key(rt)
    }

    #[inline]
    fn tag_for_type(&self, t: &str) -> Option<TagKey> {
        let rt = self.resolve_alias(t);
        if let Some(tk) = self.type_outer_tag.get(rt) {
            return Some(*tk);
        }
        self.universal_tag_for_type(rt)
    }

    #[inline]
    fn universal_tag_for_type(&self, t: &str) -> Option<TagKey> {
        let rt = self.resolve_alias(t);

        if self.sequences.contains_key(rt) || self.seq_of_types.contains_key(rt) {
            return Some((0u8, 16u32));
        }
        if self.sets.contains_key(rt) || self.set_of_types.contains_key(rt) {
            return Some((0u8, 17u32));
        }
        if self.choices.contains_key(rt) {
            return None;
        }

        let kind = self.primitives.get(rt).map(|s| s.as_str()).unwrap_or(rt);

        match kind {
            "INTEGER" => Some((0u8, 2u32)),
            "OCTET STRING" => Some((0u8, 4u32)),
            "BIT STRING" => Some((0u8, 3u32)),
            "BOOLEAN" => Some((0u8, 1u32)),
            "NULL" => Some((0u8, 5u32)),
            "ENUMERATED" => Some((0u8, 10u32)),
            "IA5String" => Some((0u8, 22u32)),
            "UTF8String" => Some((0u8, 12u32)),
            "OBJECT IDENTIFIER" => Some((0u8, 6u32)),
            "TBCD-STRING" => Some((0u8, 4u32)),
            "GraphicString" => Some((0u8, 25u32)),
            "VisibleString" => Some((0u8, 26u32)),
            _ => None,
        }
    }
}

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

#[inline(always)]
fn find_eoc(data: &[u8], mut off: usize) -> Option<usize> {
    let mut depth: i32 = 1;
    while off + 1 < data.len() {
        if data[off] == 0x00 && data[off + 1] == 0x00 {
            depth -= 1;
            off += 2;
            if depth == 0 {
                return Some(off);
            }
            continue;
        }

        let start = off;
        let tag_byte = *data.get(off)?;
        off += 1;

        let constructed = ((tag_byte >> 5) & 0x01) != 0;
        let mut tag_num = (tag_byte & 0x1F) as u32;

        if tag_num == 0x1F {
            tag_num = 0;
            while off < data.len() {
                let b = data[off];
                off += 1;
                tag_num = (tag_num << 7) | (b & 0x7F) as u32;
                if (b & 0x80) == 0 {
                    break;
                }
            }
        }

        let len_byte = *data.get(off)?;
        off += 1;

        if len_byte == 0x80 {
            if !constructed {
                return None;
            }
            depth += 1;
            continue;
        }

        let len: usize;
        if (len_byte & 0x80) != 0 {
            let n = (len_byte & 0x7F) as usize;
            if n == 0 || off + n > data.len() {
                return None;
            }
            let mut l = 0usize;
            for _ in 0..n {
                l = (l << 8) | data[off] as usize;
                off += 1;
            }
            len = l;
        } else {
            len = len_byte as usize;
        }

        if off + len > data.len() {
            return None;
        }
        off += len;

        if off <= start {
            return None;
        }
    }
    None
}

struct DerDecoder {
    schema: Asn1Schema,
}

impl DerDecoder {
    fn new(schema: Asn1Schema) -> Self {
        Self { schema }
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

        if length_byte == 0x80 {
            if !constructed {
                return None;
            }
            let content_start = offset;
            let eoc_end = find_eoc(data, offset)?;
            let content_end = eoc_end.checked_sub(2)?;
            let length = content_end.checked_sub(content_start)?;
            let value = &data[content_start..content_end];
            let raw = &data[start..eoc_end];
            return Some((
                Tlv {
                    tag_class,
                    constructed,
                    tag_num,
                    length,
                    value,
                    raw,
                },
                eoc_end,
            ));
        }

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

    fn choice_alt_matches_tlv(&self, alt_type: &str, tlv: &Tlv) -> bool {
        let rt = self.schema.resolve_alias(alt_type);

        if let Some((cls, tag)) = self.schema.type_outer_tag.get(rt) {
            return tlv.tag_class == *cls && tlv.tag_num == *tag;
        }

        if let Some(sub_alts) = self.schema.choices.get(rt) {
            if sub_alts.contains_key(&(tlv.tag_class, tlv.tag_num)) {
                return true;
            }
        }

        if self.schema.sequences.contains_key(rt) || self.schema.seq_of_types.contains_key(rt) {
            return tlv.tag_class == 0 && tlv.constructed && tlv.tag_num == 16;
        }
        if self.schema.sets.contains_key(rt) || self.schema.set_of_types.contains_key(rt) {
            return tlv.tag_class == 0 && tlv.constructed && tlv.tag_num == 17;
        }
        
        // Match Universal tags
        if let Some((cls, tag)) = self.schema.universal_tag_for_type(rt) {
             if tlv.tag_class == cls && tlv.tag_num == tag {
                 return true;
             }
        }

        false
    }

    #[inline]
    fn tlv_matches_root(&self, tlv: &Tlv, root_type: &str) -> bool {
        let rt = self.schema.resolve_alias(root_type);

        if let Some((cls, num)) = self.schema.type_outer_tag.get(rt) {
            return tlv.tag_class == *cls && tlv.tag_num == *num;
        }

        if let Some(alts) = self.schema.choices.get(rt) {
            if alts.contains_key(&(tlv.tag_class, tlv.tag_num)) {
                return true;
            }
            for ((cls, tag), (_fname, ftype)) in alts {
                if *cls == 3u8 && is_synth_choice_tag(*tag) {
                    if self.choice_alt_matches_tlv(ftype, tlv) {
                        return true;
                    }
                }
            }
            return false;
        }

        if self.schema.sequences.contains_key(rt) || self.schema.seq_of_types.contains_key(rt) {
            return tlv.tag_class == 0 && tlv.constructed && tlv.tag_num == 16;
        }
        if self.schema.sets.contains_key(rt) || self.schema.set_of_types.contains_key(rt) {
            return tlv.tag_class == 0 && tlv.constructed && tlv.tag_num == 17;
        }

        self.schema.primitives.contains_key(rt)
    }

    fn find_next_root_tlv<'a>(&self, data: &'a [u8], mut start: usize, root_type: &str) -> Option<(Tlv<'a>, usize)> {
        while start < data.len() {
            if let Some((tlv, end)) = self.parse_tlv(data, start) {
                if end > start && self.tlv_matches_root(&tlv, root_type) {
                    return Some((tlv, end));
                }
            }
            start += 1;
        }
        None
    }

    #[inline]
    fn write_type<W: Write>(&self, data: &[u8], type_name: &str, out: &mut W, scratch: &mut Vec<u8>) -> Result<()> {
        let rt = self.schema.resolve_alias(type_name);

        if let Some(elem) = self.schema.seq_of_types.get(rt) {
            self.write_sequence_of(data, elem, out, scratch)?;
            return Ok(());
        }
        if let Some(elem) = self.schema.set_of_types.get(rt) {
            self.write_sequence_of(data, elem, out, scratch)?;
            return Ok(());
        }

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

        write_hex_json(out, data, scratch)?;
        Ok(())
    }

    fn write_sequence<W: Write>(
        &self,
        data: &[u8],
        field_spec: &HashMap<TagKey, FieldSpec>,
        out: &mut W,
        scratch: &mut Vec<u8>,
    ) -> Result<()> {
        out.write_all(b"{")?;
        let mut offset = 0usize;
        let mut first = true;

        let mut itoa_buf = itoa::Buffer::new();
        let mut itoa_buf2 = itoa::Buffer::new();

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

            let key: TagKey = (tlv.tag_class, tlv.tag_num);

            if let Some(field) = field_spec.get(&key) {
                write_json_key(out, &field.name)?;
                out.write_all(b":")?;

                let resolved_field_type = self.schema.resolve_alias(&field.field_type);

                if field.is_sequence_of || field.is_set_of {
                    self.write_sequence_of(tlv.value, &field.field_type, out, scratch)?;
                } else if self.schema.choices.contains_key(resolved_field_type) {
                    // CHOICE special handling: 
                    // If the CHOICE field itself has a tag (Context 101), that tag is EXPLICIT.
                    // Meaning the content `tlv.value` contains the *inner* TLV (e.g. Context 1).
                    // We must pass `tlv.raw` so `write_choice` can parse the wrapper (if it matches)
                    // OR if `tlv` is the wrapper, `write_choice` needs to peel it.
                    // Actually, `write_choice` looks at `candidates`. 
                    // If we pass `tlv.raw` (the wrapper), `candidates[0]` is wrapper, 
                    // `candidates[1]` is inner.
                    self.write_type(tlv.raw, &field.field_type, out, scratch)?;
                } else if tlv.constructed {
                    self.write_type(tlv.value, &field.field_type, out, scratch)?;
                } else {
                    write_hex_json(out, tlv.value, scratch)?;
                }
            } else {
                out.write_all(b"\"unknown_tag_")?;
                out.write_all(itoa_buf.format(tlv.tag_class as u32).as_bytes())?;
                out.write_all(b"_")?;
                out.write_all(itoa_buf2.format(tlv.tag_num).as_bytes())?;
                out.write_all(b"\":")?;
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

            if !arr_first {
                out.write_all(b",")?;
            }
            arr_first = false;

            if is_choice {
                // For Sequence Of Choice, the items are direct choices.
                // We pass `tlv.raw` because the tag we found (e.g. [1]) IS the choice tag.
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

    fn write_choice<W: Write>(
        &self,
        data: &[u8],
        alts: &HashMap<TagKey, (String, String)>,
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

        let mut candidates: [Option<Tlv>; 3] = [None, None, None];
        candidates[0] = Some(outer.clone());

        // If the outer tag is a constructed wrapper (Explicit tagging), look inside.
        if outer.constructed {
            candidates[1] = self.parse_tlv(outer.value, 0).map(|(inner, _)| inner);
        }
        // Special case for TAP: sometimes double wrapped?
        if outer.tag_class == 0 && !outer.constructed && outer.tag_num == 4 {
             if !outer.value.is_empty() && outer.value[0] != 0x00 {
                candidates[2] = self.parse_tlv(outer.value, 0).map(|(inner, _)| inner);
             }
        }

        out.write_all(b"{")?;

        // 1. Tagged CHOICE: direct match
        for cand in candidates.iter().flatten() {
            if let Some((field_name, type_name)) = alts.get(&(cand.tag_class, cand.tag_num)) {
                write_json_key(out, field_name)?;
                out.write_all(b":")?;
                self.write_type(cand.value, type_name, out, scratch)?;
                out.write_all(b"}")?;
                return Ok(());
            }
        }

        // 2. Untagged CHOICE (Synthetic)
        let mut synth_keys: Vec<u32> = alts
            .keys()
            .filter(|(cls, tag)| *cls == 3u8 && is_synth_choice_tag(*tag))
            .map(|(_, tag)| *tag)
            .collect();
        synth_keys.sort_unstable();

        for k in synth_keys {
            let (fname, ftype) = &alts[&(3u8, k)];
            let f_rt = self.schema.resolve_alias(ftype);

            for cand in candidates.iter().flatten() {
                if self.choice_alt_matches_tlv(ftype, cand) {
                    write_json_key(out, fname)?;
                    out.write_all(b":")?;
                    
                    if self.schema.type_outer_tag.contains_key(f_rt) {
                        self.write_type(cand.value, ftype, out, scratch)?;
                    } else if self.schema.choices.contains_key(f_rt) {
                         self.write_type(cand.raw, ftype, out, scratch)?;
                    } else {
                        self.write_type(cand.value, ftype, out, scratch)?;
                    }

                    out.write_all(b"}")?;
                    return Ok(());
                }
            }
        }

        write_json_key(out, "unknown_alternative")?;
        out.write_all(b":")?;
        write_hex_json(out, outer.raw, scratch)?;
        out.write_all(b"}")?;
        Ok(())
    }

    fn write_root_tlv_with_type<W: Write>(&self, tlv: &Tlv, root_type: &str, out: &mut W, scratch: &mut Vec<u8>) -> Result<()> {
        let rt = self.schema.resolve_alias(root_type);

        if !self.schema.knows_type(rt) {
            return Err(anyhow!("root-type '{}' not found in schema", root_type));
        }

        if self.schema.type_outer_tag.contains_key(rt) {
            self.write_type(tlv.value, root_type, out, scratch)?;
            return Ok(());
        }

        if self.schema.choices.contains_key(rt) {
            self.write_type(tlv.raw, root_type, out, scratch)?;
        } else {
            self.write_type(tlv.value, root_type, out, scratch)?;
        }
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
    let Some(set) = allowed_exts else { return true; };
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else { return false; };
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

    let mut writer = BufWriter::with_capacity(64 * 1024 * 1024, out_file);
    let mut hex_scratch: Vec<u8> = Vec::with_capacity(8 * 1024 * 1024);

    let mut offset = 0usize;
    let mut count = 0usize;

    while offset < data.len() {
        let (tlv, new_off) = match decoder.find_next_root_tlv(data, offset, root_type) {
            Some(t) => t,
            None => break,
        };

        decoder.write_root_tlv_with_type(&tlv, root_type, &mut writer, &mut hex_scratch)?;
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

    // LOGIC: Decide whether to Load Binary or Parse Text
    let schema = if let Some(bin_path) = &cli.load_compiled {
        // FAST PATH: Load from binary
        println!("Loading pre-compiled schema from {:?}", bin_path);
        let file = File::open(bin_path).with_context(|| "Failed to open compiled schema")?;
        let decoded: Asn1Schema = bincode::deserialize_from(file)
            .with_context(|| "Failed to deserialize schema")?;
        decoded
    } else if let Some(text_path) = &cli.schema {
        // SLOW PATH: Parse text
        println!("Parsing text schema from {:?}", text_path);
        let schema_text = std::fs::read_to_string(text_path)
            .with_context(|| format!("Failed to read schema file {:?}", text_path))?;
        let parsed = Asn1Schema::parse(&schema_text)?;

        // OPTIONAL: Save to binary if requested
        if let Some(save_path) = &cli.compile_schema {
            println!("Saving compiled schema to {:?}", save_path);
            let file = File::create(save_path).with_context(|| "Failed to create schema dump file")?;
            bincode::serialize_into(file, &parsed).with_context(|| "Failed to serialize schema")?;
            println!("Schema saved. You can now use --load-compiled next time.");
        }
        parsed
    } else {
        return Err(anyhow!("You must provide either --schema or --load-compiled"));
    };

    let decoder = DerDecoder::new(schema);

    std::fs::create_dir_all(&cli.output_dir)?;

    let root_type = cli.root_type.clone();
    if !decoder.schema.knows_type(&root_type) {
        return Err(anyhow!(
            "root-type '{}' does not appear in parsed schema (check spelling / module).",
            root_type
        ));
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