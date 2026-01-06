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
    about = "Ultra-fast ASN.1 DER Decoder -> JSONL (Rust, hex-only values + enum name enrichment)",
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

    /// INTEGER/ENUMERATED named values: type_name -> (value -> name)
    named_ints: HashMap<String, HashMap<i64, String>>,

    /// Type aliases like: GSNAddress ::= IPAddress
    aliases: HashMap<String, String>,
}

const SYNTH_CHOICE_BASE: u32 = 0xFFFF_FF00;
fn is_synth_choice_tag(t: u32) -> bool {
    t >= SYNTH_CHOICE_BASE
}

impl Asn1Schema {
    fn parse(schema_text: &str) -> Result<Self> {
        let comment_strip_re = Regex::new(r"--.*?(?:\n|$)")?;

        // Match "TypeName ::= KIND { ... }" where KIND includes INTEGER/ENUMERATED
        let type_assign_re = Regex::new(
            r"(?s)([\w-]+)\s*::=\s*(CHOICE|SEQUENCE|SET|ENUMERATED|INTEGER|OCTET STRING|BIT STRING|IA5String|UTF8String|BOOLEAN|NULL|TBCD-STRING)\s*(?:\(([^)]*)\))?\s*(\{.*?\})?",
        )?;

        // Alias: TypeA ::= TypeB   (no braces)
        let alias_re = Regex::new(r"(?m)^\s*([\w-]+)\s*::=\s*([\w-]+)\s*$")?;

        // Allow '-' in identifiers
        let choice_tagged_re = Regex::new(r"([\w-]+)\s+\[(\d+)\]\s+([\w-]+)")?;
        let choice_untagged_re = Regex::new(r"([\w-]+)\s+([\w-]+)")?;

        let sequence_body_re = Regex::new(
            r"([\w-]+)\s+\[(\d+)\]\s+([\w-]+(?:\s+OF\s+[\w-]+)?)\s*(OPTIONAL)?",
        )?;

        // Enum/int named values inside "{ name (num), ... }"
        let named_val_re = Regex::new(r"([\w-]+)\s*\(\s*(-?\d+)\s*\)")?;

        let mut schema = Asn1Schema::default();
        let stripped = comment_strip_re.replace_all(schema_text, "");

        // 1) Parse aliases first (GSNAddress ::= IPAddress)
        for cap in alias_re.captures_iter(&stripped) {
            let lhs = cap.get(1).unwrap().as_str().to_string();
            let rhs = cap.get(2).unwrap().as_str().to_string();

            // Ignore if RHS is a keyword (we only want bare identifier aliases)
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

        // 2) Parse normal type assignments
        for caps in type_assign_re.captures_iter(&stripped) {
            let type_name = caps.get(1).unwrap().as_str().to_string();
            let type_kind = caps.get(2).unwrap().as_str();
            let body = caps.get(4).map(|m| m.as_str()).unwrap_or("");

            match type_kind {
                "CHOICE" => {
                    let mut alts = HashMap::new();

                    // Tagged CHOICE alternatives: name [n] Type
                    for c in choice_tagged_re.captures_iter(body) {
                        let field_name = c.get(1).unwrap().as_str().to_string();
                        let tag: u32 = c.get(2).unwrap().as_str().parse()?;
                        let field_type = c.get(3).unwrap().as_str().to_string();
                        alts.insert(tag, (field_name, field_type));
                    }

                    // Untagged CHOICE: name Type (store synthetic tags)
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
                "ENUMERATED" | "INTEGER" => {
                    schema.primitives.insert(type_name.clone(), type_kind.to_string());

                    if !body.is_empty() {
                        let mut map = HashMap::new();
                        for c in named_val_re.captures_iter(body) {
                            let name = c.get(1).unwrap().as_str().to_string();
                            let val: i64 = c.get(2).unwrap().as_str().parse()?;
                            map.insert(val, name);
                        }
                        if !map.is_empty() {
                            schema.named_ints.insert(type_name, map);
                        }
                    }
                }
                kind => {
                    schema.primitives.insert(type_name, kind.to_string());
                }
            }
        }

        Ok(schema)
    }

    /// Follow aliases like GSNAddress -> IPAddress -> ...
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

fn write_json_string<W: Write>(w: &mut W, s: &str) -> Result<()> {
    w.write_all(b"\"")?;
    for b in s.bytes() {
        match b {
            b'\"' => w.write_all(b"\\\"")?,
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
            c => w.write_all(&[c])?,
        }
    }
    w.write_all(b"\"")?;
    Ok(())
}

fn write_hex_json<W: Write>(w: &mut W, data: &[u8]) -> Result<()> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    w.write_all(b"\"")?;
    for &b in data {
        w.write_all(&[HEX[(b >> 4) as usize], HEX[(b & 0x0F) as usize]])?;
    }
    w.write_all(b"\"")?;
    Ok(())
}

/// decode DER INTEGER/ENUM content bytes (two's complement) -> i64
fn decode_int_i64(bytes: &[u8]) -> Option<i64> {
    if bytes.is_empty() {
        return Some(0);
    }
    if bytes.len() > 8 {
        return None;
    }

    let negative = (bytes[0] & 0x80) != 0;
    let mut v: i64 = 0;
    for &b in bytes {
        v = (v << 8) | (b as i64);
    }
    if !negative {
        return Some(v);
    }

    let bits = (bytes.len() * 8) as u32;
    let mask: i64 = (1i64 << bits) - 1;
    let signed = -(((!v) & mask) + 1);
    Some(signed)
}

fn lower_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
    }
}

struct DerDecoder {
    schema: Asn1Schema,
    record_like_types: Vec<String>,
    cs_choice_index: HashMap<u32, String>,
}

impl DerDecoder {
    fn new(schema: Asn1Schema) -> Self {
        let mut record_like_types = Vec::new();
        for name in schema.sequences.keys() {
            if name.ends_with("Record") || name.ends_with("DataFile") {
                record_like_types.push(name.clone());
            }
        }
        for name in schema.sets.keys() {
            if name.ends_with("Record") || name.ends_with("DataFile") {
                record_like_types.push(name.clone());
            }
        }

        let mut cs_choice_index: HashMap<u32, String> = HashMap::new();
        for (_choice_name, alts) in &schema.choices {
            for (tag, (_field_name, field_type)) in alts {
                if is_synth_choice_tag(*tag) {
                    continue;
                }
                cs_choice_index.entry(*tag).or_insert(field_type.clone());
            }
        }

        Self {
            schema,
            record_like_types,
            cs_choice_index,
        }
    }

    // ------------------- Dynamic primitive writing (matches your correct JSON) -------------------

    fn primitive_kind(&self, type_name: &str) -> Option<&str> {
        let rt = self.schema.resolve_alias(type_name);
        self.schema.primitives.get(rt).map(|s| s.as_str())
    }

    fn lookup_named_int(&self, type_name: &str, v: i64) -> Option<&str> {
        let rt = self.schema.resolve_alias(type_name);
        self.schema
            .named_ints
            .get(rt)
            .and_then(|m| m.get(&v))
            .map(|s| s.as_str())
    }

    /// Write ENUMERATED as {"value": <int>, "name": "<name>"?}
    fn write_enumerated_obj<W: Write>(&self, out: &mut W, type_name: &str, value_bytes: &[u8]) -> Result<()> {
        let v = match decode_int_i64(value_bytes) {
            Some(x) => x,
            None => {
                // fallback if can't decode
                write_hex_json(out, value_bytes)?;
                return Ok(());
            }
        };

        out.write_all(b"{")?;
        write_json_string(out, "value")?;
        out.write_all(b":")?;
        write!(out, "{}", v)?;

        if let Some(name) = self.lookup_named_int(type_name, v) {
            out.write_all(b",")?;
            write_json_string(out, "name")?;
            out.write_all(b":")?;
            write_json_string(out, name)?;
        }

        out.write_all(b"}")?;
        Ok(())
    }

    /// BIT STRING => {"valueHex":"...","unusedBits":N}
    fn write_bit_string_obj<W: Write>(&self, out: &mut W, value_bytes: &[u8]) -> Result<()> {
        if value_bytes.is_empty() {
            out.write_all(b"{")?;
            write_json_string(out, "valueHex")?;
            out.write_all(b":")?;
            write_hex_json(out, &[])?;
            out.write_all(b",")?;
            write_json_string(out, "unusedBits")?;
            out.write_all(b":0")?;
            out.write_all(b"}")?;
            return Ok(());
        }

        let unused_bits = value_bytes[0] as u32;
        let bits = &value_bytes[1..];

        out.write_all(b"{")?;
        write_json_string(out, "valueHex")?;
        out.write_all(b":")?;
        write_hex_json(out, bits)?;
        out.write_all(b",")?;
        write_json_string(out, "unusedBits")?;
        out.write_all(b":")?;
        write!(out, "{}", unused_bits)?;
        out.write_all(b"}")?;
        Ok(())
    }

    /// Write a primitive value as JSON based on schema primitive kind (dynamic, no hardcoded fields).
    ///
    /// Returns: (extra_name_to_emit)
    /// - For INTEGER with named mapping, we emit value as number, and return Some(enum_name)
    ///   so caller can add `<fieldName>Name`.
    fn write_primitive_value<W: Write>(
        &self,
        out: &mut W,
        type_name: &str,
        value_bytes: &[u8],
    ) -> Result<Option<String>> {
        let kind = self.primitive_kind(type_name);

        match kind {
            Some("BOOLEAN") => {
                let b = value_bytes.first().copied().unwrap_or(0) != 0;
                if b {
                    out.write_all(b"true")?;
                } else {
                    out.write_all(b"false")?;
                }
                Ok(None)
            }
            Some("NULL") => {
                out.write_all(b"null")?;
                Ok(None)
            }
            Some("IA5String") | Some("UTF8String") => {
                match std::str::from_utf8(value_bytes) {
                    Ok(s) => {
                        write_json_string(out, s)?;
                        Ok(None)
                    }
                    Err(_) => {
                        // fallback to hex if invalid
                        write_hex_json(out, value_bytes)?;
                        Ok(None)
                    }
                }
            }
            Some("BIT STRING") => {
                self.write_bit_string_obj(out, value_bytes)?;
                Ok(None)
            }
            Some("ENUMERATED") => {
                self.write_enumerated_obj(out, type_name, value_bytes)?;
                Ok(None)
            }
            Some("INTEGER") => {
                if let Some(v) = decode_int_i64(value_bytes) {
                    write!(out, "{}", v)?;

                    // If schema provides a name mapping, return it so caller can emit `<fieldName>Name`
                    if let Some(name) = self.lookup_named_int(type_name, v) {
                        return Ok(Some(name.to_string()));
                    }
                    Ok(None)
                } else {
                    // fallback
                    write_hex_json(out, value_bytes)?;
                    Ok(None)
                }
            }
            // Other primitives: OCTET STRING, TBCD-STRING, etc => keep hex (matches your sample)
            _ => {
                write_hex_json(out, value_bytes)?;
                Ok(None)
            }
        }
    }

    // ------------------------------------ TLV parsing ------------------------------------

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
                if b & 0x80 == 0 {
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
        if length_byte & 0x80 != 0 {
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

    fn is_universal_octet_string(tlv: &Tlv) -> bool {
        tlv.tag_class == 0 && !tlv.constructed && tlv.tag_num == 4
    }

    fn unwrap_octet_string_containing_tlv<'a>(&self, tlv: &Tlv<'a>) -> Option<Tlv<'a>> {
        if !Self::is_universal_octet_string(tlv) {
            return None;
        }
        self.parse_tlv(tlv.value, 0).map(|(inner, _)| inner)
    }

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

                let key = format!("field_{}", idx);
                write_json_string(out, &key)?;
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
        let rt = self.schema.resolve_alias(type_name);

        if let Some(alts) = self.schema.choices.get(rt) {
            self.write_choice(data, alts, out)?;
            return Ok(());
        }

        if let Some(fields) = self.schema.sequences.get(rt) {
            self.write_sequence(data, fields, out)?;
            return Ok(());
        }

        if let Some(fields) = self.schema.sets.get(rt) {
            self.write_sequence(data, fields, out)?;
            return Ok(());
        }

        // If it's a primitive type name, render by kind
        if self.schema.primitives.contains_key(rt) {
            // data here is already the content bytes
            let _ = self.write_primitive_value(out, rt, data)?;
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

                let resolved_field_type = self.schema.resolve_alias(&field.field_type);

                if field.is_sequence_of {
                    self.write_sequence_of(tlv.value, &field.field_type, out)?;
                } else if self.schema.choices.contains_key(resolved_field_type) {
                    // CHOICE needs raw TLV so it can see the tag
                    self.write_type(tlv.raw, &field.field_type, out)?;
                } else if tlv.constructed {
                    self.write_type(tlv.value, &field.field_type, out)?;
                } else if self.schema.primitives.contains_key(resolved_field_type) {
                    // Primitive scalar: render by kind, and if INTEGER has mapping emit <fieldName>Name
                    if let Some(name) = self.write_primitive_value(out, &field.field_type, tlv.value)? {
                        out.write_all(b",")?;
                        let name_key = format!("{}Name", field.name);
                        write_json_string(out, &name_key)?;
                        out.write_all(b":")?;
                        write_json_string(out, &name)?;
                    }
                } else {
                    // Unknown primitive => keep hex
                    write_hex_json(out, tlv.value)?;
                }
            } else {
                if !first {
                    out.write_all(b",")?;
                }
                first = false;

                let key = format!("unknown_tag_{}", tlv.tag_num);
                write_json_string(out, &key)?;
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
        let mut arr_first = true;
        let mut offset = 0usize;
        let len = data.len();

        let is_choice = self.schema.choices.contains_key(self.schema.resolve_alias(element_type));
        let elem_rt = self.schema.resolve_alias(element_type);
        let is_primitive = self.schema.primitives.contains_key(elem_rt);

        while offset < len {
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
                self.write_type(tlv.raw, element_type, out)?;
            } else if tlv.constructed {
                self.write_type(tlv.value, element_type, out)?;
            } else if is_primitive {
                // For SEQUENCE OF ENUMERATED your correct output expects objects {value,name}
                // For SEQUENCE OF INTEGER it expects numbers (and NO extra Name keys inside array)
                let kind = self.primitive_kind(element_type);
                match kind {
                    Some("ENUMERATED") => {
                        self.write_enumerated_obj(out, element_type, tlv.value)?;
                    }
                    Some("INTEGER") => {
                        if let Some(v) = decode_int_i64(tlv.value) {
                            write!(out, "{}", v)?;
                        } else {
                            write_hex_json(out, tlv.value)?;
                        }
                    }
                    Some("BOOLEAN") => {
                        let b = tlv.value.first().copied().unwrap_or(0) != 0;
                        if b {
                            out.write_all(b"true")?;
                        } else {
                            out.write_all(b"false")?;
                        }
                    }
                    Some("IA5String") | Some("UTF8String") => {
                        match std::str::from_utf8(tlv.value) {
                            Ok(s) => write_json_string(out, s)?,
                            Err(_) => write_hex_json(out, tlv.value)?,
                        }
                    }
                    Some("BIT STRING") => {
                        self.write_bit_string_obj(out, tlv.value)?;
                    }
                    Some("NULL") => {
                        out.write_all(b"null")?;
                    }
                    _ => {
                        write_hex_json(out, tlv.value)?;
                    }
                }
            } else {
                write_hex_json(out, tlv.value)?;
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
    ) -> Result<()> {
        let (outer, _) = match self.parse_tlv(data, 0) {
            Some(t) => t,
            None => {
                out.write_all(b"null")?;
                return Ok(());
            }
        };

        let mut candidates: Vec<Tlv> = Vec::new();
        candidates.push(outer.clone());
        if let Some(inner) = self.unwrap_constructed_containing_tlv(&outer) {
            candidates.push(inner);
        }
        if let Some(inner) = self.unwrap_octet_string_containing_tlv(&outer) {
            candidates.push(inner);
        }

        out.write_all(b"{")?;

        // Tagged CHOICE
        for cand in &candidates {
            if let Some((field_name, type_name)) = alts.get(&cand.tag_num) {
                write_json_string(out, field_name)?;
                out.write_all(b":")?;
                self.write_type(cand.value, type_name, out)?;
                out.write_all(b"}")?;
                return Ok(());
            }
        }

        // Untagged CHOICE: probe synthetic alts
        let mut synth_keys: Vec<u32> = alts
            .keys()
            .copied()
            .filter(|t| is_synth_choice_tag(*t))
            .collect();
        synth_keys.sort_unstable();

        let probe = candidates.last().unwrap();

        for k in synth_keys {
            let (fname, ftype) = &alts[&k];
            if self.choice_alt_matches_tlv(ftype, probe) {
                write_json_string(out, fname)?;
                out.write_all(b":")?;

                let rt = self.schema.resolve_alias(ftype);

                // For untagged CHOICE, pass full TLV to nested CHOICE types
                if self.schema.choices.contains_key(rt) {
                    self.write_type(probe.raw, ftype, out)?;
                } else {
                    self.write_type(probe.value, ftype, out)?;
                }
                out.write_all(b"}")?;
                return Ok(());
            }
        }

        write_json_string(out, "unknown_alternative")?;
        out.write_all(b":")?;
        write_hex_json(out, probe.raw)?;
        out.write_all(b"}")?;
        Ok(())
    }

    fn write_root_tlv_with_type<W: Write>(&self, tlv: &Tlv, root_type: &str, out: &mut W) -> Result<()> {
        let rt = self.schema.resolve_alias(root_type);

        if !self.schema.knows_type(rt) {
            self.write_auto_record(tlv, out)?;
            return Ok(());
        }

        if self.schema.choices.contains_key(rt) {
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
                let key = lower_first(alt_type);
                write_json_string(out, &key)?;
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
