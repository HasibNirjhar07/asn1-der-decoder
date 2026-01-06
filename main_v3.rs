use anyhow::{anyhow, Context, Result};
use clap::Parser;
use memmap2::Mmap;
use rayon::prelude::*;
use regex::Regex;
use serde_json::{json, Map as JsonMap, Value as JsonValue};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// CLI arguments
#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Ultra-fast ASN.1 DER Decoder -> JSONL (Rust)",
    long_about = None
)]
struct Cli {
    /// ASN.1 schema file
    #[arg(long = "schema")]
    schema: PathBuf,

    /// Root ASN.1 type (e.g. CallEventDataFile)
    /// Use "auto" to let the decoder infer record type like Python auto_decode_record
    #[arg(long = "root-type")]
    root_type: String,

    /// Output directory
    #[arg(long = "output-dir")]
    output_dir: PathBuf,

    /// DER-encoded input files
    #[arg(required = true)]
    der_files: Vec<PathBuf>,
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

/// Schema representation (subset of your Python version)
#[derive(Debug, Default)]
struct Asn1Schema {
    choices: HashMap<String, HashMap<u32, (String, String)>>, // type_name -> tag -> (field_name, field_type)
    sequences: HashMap<String, HashMap<u32, FieldSpec>>,
    sets: HashMap<String, HashMap<u32, FieldSpec>>,
    enumerations: HashMap<String, HashMap<u32, String>>, // type_name -> value -> name
    bitstrings: HashMap<String, HashMap<u32, String>>,   // type_name -> bitpos -> name
    primitives: HashMap<String, String>,                 // type_name -> primitive kind
}

impl Asn1Schema {
    fn parse(schema_text: &str) -> Result<Self> {
        let comment_strip_re = Regex::new(r"--.*?(?:\n|$)")?;

        // IMPORTANT: this must be ONE line, no embedded newlines
        let type_assign_re = Regex::new(
            r"(?s)([\w-]+)\s*::=\s*(CHOICE|SEQUENCE|SET|ENUMERATED|INTEGER|OCTET STRING|BIT STRING|IA5String|UTF8String|BOOLEAN|NULL|TBCD-STRING)\s*(?:\(([^)]*)\))?\s*(\{[^}]*\})?"
        )?;

        let choice_body_re = Regex::new(r"(\w+)\s+\[(\d+)\]\s+(\w+)")?;
        let bitstring_body_re = Regex::new(r"(\w+)\s*\(\s*(\d+)\s*\)")?;
        let sequence_body_re =
            Regex::new(r"(\w+)\s+\[(\d+)\]\s+(\w+(?:\s+OF\s+\w+)?)\s*(OPTIONAL)?")?;
        let enum_body_re = Regex::new(r"(\w+)\s*\((\d+)\)")?;

        let mut schema = Asn1Schema::default();

        let stripped = comment_strip_re.replace_all(schema_text, "");

        for caps in type_assign_re.captures_iter(&stripped) {
            let type_name = caps.get(1).unwrap().as_str().to_string();
            let type_kind = caps.get(2).unwrap().as_str();
            let _constraints = caps.get(3).map(|m| m.as_str());
            let body = caps.get(4).map(|m| m.as_str()).unwrap_or("");

            match type_kind {
                "CHOICE" => {
                    let mut alts = HashMap::new();
                    for c in choice_body_re.captures_iter(body) {
                        let field_name = c.get(1).unwrap().as_str().to_string();
                        let tag: u32 = c.get(2).unwrap().as_str().parse()?;
                        let field_type = c.get(3).unwrap().as_str().to_string();
                        alts.insert(tag, (field_name, field_type));
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
                "ENUMERATED" => {
                    let mut values = HashMap::new();
                    for c in enum_body_re.captures_iter(body) {
                        let name = c.get(1).unwrap().as_str().to_string();
                        let val: u32 = c.get(2).unwrap().as_str().parse()?;
                        values.insert(val, name);
                    }
                    schema.enumerations.insert(type_name, values);
                }
                "BIT STRING" => {
                    let mut values = HashMap::new();
                    for c in bitstring_body_re.captures_iter(body) {
                        let name = c.get(1).unwrap().as_str().to_string();
                        let val: u32 = c.get(2).unwrap().as_str().parse()?;
                        values.insert(val, name);
                    }
                    schema.bitstrings.insert(type_name, values);
                }
                // primitive aliases
                kind => {
                    schema.primitives.insert(type_name, kind.to_string());
                }
            }
        }

        Ok(schema)
    }

    /// Check if a type is known in this schema
    fn knows_type(&self, t: &str) -> bool {
        self.choices.contains_key(t)
            || self.sequences.contains_key(t)
            || self.sets.contains_key(t)
            || self.enumerations.contains_key(t)
            || self.bitstrings.contains_key(t)
            || self.primitives.contains_key(t)
    }
}

/// A parsed TLV
#[derive(Debug)]
struct Tlv<'a> {
    tag_class: u8,
    constructed: bool,
    tag_num: u32,
    #[allow(dead_code)]
    length: usize,
    value: &'a [u8],
    raw: &'a [u8],
}

/// Helper: lower-case first character, like Python's camelization of top-level record key
fn lower_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
    }
}

/// DER / BER decoder with heuristic decoding (similar to Python)
struct DerDecoder {
    schema: Asn1Schema,
    record_like_types: Vec<String>,
    cs_choice_index: HashMap<u32, String>,
}

impl DerDecoder {
    fn new(schema: Asn1Schema) -> Self {
        // Record-like types: SEQUENCE/SET names ending in "Record" or "DataFile"
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

        // Context-specific CHOICE index: tag -> alternative type
        let mut cs_choice_index: HashMap<u32, String> = HashMap::new();
        for (_choice_name, alts) in &schema.choices {
            for (tag, (_field_name, field_type)) in alts {
                cs_choice_index.entry(*tag).or_insert(field_type.clone());
            }
        }

        Self {
            schema,
            record_like_types,
            cs_choice_index,
        }
    }

    /// Generic TLV decoder used as fallback when we don't know the schema type.
    fn decode_generic(&self, tlv: &Tlv) -> JsonValue {
        if tlv.constructed {
            // pre-allocate a bit to reduce reallocation
            let mut obj = JsonMap::with_capacity(8);
            let mut offset = 0usize;
            let data = tlv.value;
            let mut idx = 0usize;
            while offset < data.len() {
                let (inner, new_off) = match self.parse_tlv(data, offset) {
                    Some(t) => t,
                    None => break,
                };
                if new_off <= offset {
                    break;
                }
                let key = format!("field_{}", idx);
                let val = self.decode_generic(&inner);
                obj.insert(key, val);
                offset = new_off;
                idx += 1;
            }
            JsonValue::Object(obj)
        } else {
            let d = tlv.value;
            // Try small integer for short values
            if !d.is_empty() && d.len() <= 8 {
                let mut val: u64 = 0;
                for &b in d {
                    val = (val << 8) | b as u64;
                }
                if val < 1_000_000_000_000 {
                    return json!(val);
                }
            }
            // Try UTF-8 string
            if let Ok(s) = std::str::from_utf8(d) {
                if s.chars()
                    .all(|c| !c.is_control() || c == '\n' || c == '\r' || c == '\t')
                {
                    return json!(s.to_string());
                }
            }
            // Fallback: hex
            json!(hex::encode(d))
        }
    }

    /// Parse a single TLV from data[offset..]
    #[inline]
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

        // Long-form tag
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

    /// Decode TLV as given root type (or generic if unknown)
    fn decode_root_tlv_with_type(&self, tlv: &Tlv, root_type: &str) -> JsonValue {
        // If the type isn't known in the schema, fall back to auto record decoding
        if !self.schema.knows_type(root_type) {
            eprintln!(
                "WARNING: root-type '{}' not found in schema; using auto record decoding",
                root_type
            );
            return self.auto_decode_record(tlv);
        }

        // CHOICE uses raw TLV, SEQUENCE/SET uses value only
        if self.schema.choices.contains_key(root_type) {
            return self.decode_type(tlv.raw, root_type);
        }
        if self.schema.sequences.contains_key(root_type)
            || self.schema.sets.contains_key(root_type)
        {
            return self.decode_type(tlv.value, root_type);
        }

        self.decode_type(tlv.value, root_type)
    }

    /// Auto-decoding of a top-level record, like Python auto_decode_record
    fn auto_decode_record(&self, tlv: &Tlv) -> JsonValue {
        // Context-specific CHOICE-alternative
        if tlv.tag_class == 2 {
            if let Some(alt_type) = self.cs_choice_index.get(&tlv.tag_num) {
                let decoded = self.decode_type(tlv.value, alt_type);
                let mut obj = JsonMap::new();
                let key = lower_first(alt_type);
                obj.insert(key, decoded);
                return JsonValue::Object(obj);
            }
        }

        // Universal SEQUENCE/SET with record-like type
        if tlv.tag_class == 0 && tlv.constructed && (tlv.tag_num == 16 || tlv.tag_num == 17) {
            if self.record_like_types.len() == 1 {
                let type_name = &self.record_like_types[0];
                let decoded = self.decode_type(tlv.value, type_name);
                let mut obj = JsonMap::new();
                let key = lower_first(type_name);
                obj.insert(key, decoded);
                return JsonValue::Object(obj);
            }
        }

        // Fallback: generic
        let mut obj = JsonMap::new();
        obj.insert("unknown".to_string(), self.decode_generic(tlv));
        JsonValue::Object(obj)
    }

    /// Main type dispatch (with heuristics for unknown types)
    fn decode_type(&self, data: &[u8], type_name: &str) -> JsonValue {
        let lname = type_name.to_ascii_lowercase();

        // special handlers by type name (like Python _special_handlers / decode_inferred)
        match lname.as_str() {
            "ipaddress" | "gsnaddress" => return self.decode_ipaddress_choice(data),
            "pdpaddress" => return self.decode_pdpaddress_choice(data),
            "timestamp" => return self.decode_timestamp(data),
            "plmn-id" => return self.decode_plmn_id(data),
            _ => {}
        }

        // CHOICE
        if let Some(alts) = self.schema.choices.get(type_name) {
            return self.decode_choice(data, alts);
        }

        // SEQUENCE
        if let Some(fields) = self.schema.sequences.get(type_name) {
            return self.decode_sequence(data, fields);
        }

        // SET (same as sequence for us)
        if let Some(fields) = self.schema.sets.get(type_name) {
            return self.decode_sequence(data, fields);
        }

        // ENUM
        if let Some(enumvals) = self.schema.enumerations.get(type_name) {
            return self.decode_enum(data, enumvals);
        }

        // BIT STRING type
        if let Some(bitnames) = self.schema.bitstrings.get(type_name) {
            return self.decode_bitstring(data, bitnames);
        }

        // Primitive alias
        if let Some(prim) = self.schema.primitives.get(type_name) {
            return self.decode_primitive(data, prim);
        }

        // Fallback: heuristic decode like Python's decode_inferred
        self.decode_inferred(data, type_name)
    }

    fn decode_sequence(
        &self,
        data: &[u8],
        field_spec: &HashMap<u32, FieldSpec>,
    ) -> JsonValue {
        // pre-allocate map for roughly number of fields
        let mut obj = JsonMap::with_capacity(field_spec.len());
        let mut offset = 0usize;
        let len = data.len();

        while offset < len {
            let (tlv, new_off) = match self.parse_tlv(data, offset) {
                Some(t) => t,
                None => break,
            };
            if new_off <= offset {
                break;
            }

            let tag = tlv.tag_num;

            if let Some(field) = field_spec.get(&tag) {
                let fname = &field.name;
                let ftype = &field.field_type;

                let value = if field.is_sequence_of {
                    self.decode_sequence_of(tlv.value, ftype)
                } else if tlv.constructed {
                    self.decode_type(tlv.value, ftype)
                } else {
                    // leaf primitive
                    let tname = ftype.to_uppercase();
                    if tname == "OCTET STRING" {
                        self.decode_primitive(tlv.value, "OCTET STRING")
                    } else if tname == "TBCD-STRING" {
                        self.decode_tbcd(tlv.value)
                    } else {
                        self.decode_type(tlv.value, ftype)
                    }
                };

                obj.insert(fname.clone(), value);
            } else {
                // unknown tag: keep as hex under unknown_tag_X (like Python when not in fast-mode)
                let key = format!("unknown_tag_{}", tag);
                obj.insert(key, json!(hex::encode(tlv.value)));
            }

            offset = new_off;
        }

        JsonValue::Object(obj)
    }

    fn decode_sequence_of(&self, data: &[u8], element_type: &str) -> JsonValue {
        // small initial capacity to reduce reallocations
        let mut arr = Vec::<JsonValue>::with_capacity(16);
        let mut offset = 0usize;
        let len = data.len();

        let is_choice = self.schema.choices.contains_key(element_type);

        while offset < len {
            let (tlv, new_off) = match self.parse_tlv(data, offset) {
                Some(t) => t,
                None => break,
            };
            if new_off <= offset {
                break;
            }

            let v = if is_choice {
                self.decode_type(tlv.raw, element_type)
            } else {
                self.decode_type(tlv.value, element_type)
            };

            arr.push(v);
            offset = new_off;
        }

        JsonValue::Array(arr)
    }

    fn decode_choice(
        &self,
        data: &[u8],
        alts: &HashMap<u32, (String, String)>,
    ) -> JsonValue {
        let (tlv, _) = match self.parse_tlv(data, 0) {
            Some(t) => t,
            None => return JsonValue::Null,
        };

        if let Some((field_name, type_name)) = alts.get(&tlv.tag_num) {
            let v = self.decode_type(tlv.value, type_name);
            let mut obj = JsonMap::new();
            obj.insert(field_name.clone(), v);
            JsonValue::Object(obj)
        } else {
            let mut obj = JsonMap::new();
            obj.insert(
                "unknown_alternative".to_string(),
                json!(hex::encode(tlv.value)),
            );
            JsonValue::Object(obj)
        }
    }

    fn decode_enum(&self, data: &[u8], enumvals: &HashMap<u32, String>) -> JsonValue {
        let v = self.decode_integer(data);
        let u = v as u32;

        if let Some(name) = enumvals.get(&u) {
            json!(name)
        } else {
            json!(format!("Unknown({})", u))
        }
    }

    fn decode_bitstring(&self, data: &[u8], bitnames: &HashMap<u32, String>) -> JsonValue {
        if data.is_empty() {
            return JsonValue::Array(vec![]);
        }

        let unused = data[0] as usize;
        let bits_bytes = &data[1..];
        let total_bits = (bits_bytes.len() * 8).saturating_sub(unused);

        if total_bits == 0 {
            return JsonValue::Array(vec![]);
        }

        let mut intval: u128 = 0;
        for &b in bits_bytes {
            intval = (intval << 8) | b as u128;
        }

        let mut names = Vec::new();
        for (bitpos, name) in bitnames {
            let pos = *bitpos as usize;
            if pos >= total_bits {
                continue;
            }
            let shift = total_bits - 1 - pos;
            if (intval & (1u128 << shift)) != 0 {
                names.push(JsonValue::String(name.clone()));
            }
        }

        JsonValue::Array(names)
    }

    #[inline]
    fn decode_integer(&self, data: &[u8]) -> i64 {
        if data.is_empty() {
            return 0;
        }

        // big-endian signed
        let mut val: i128 = 0;
        let sign = (data[0] & 0x80) != 0;
        for &b in data {
            val = (val << 8) | (b as i128);
        }
        if sign {
            let bits = data.len() * 8;
            let mask: i128 = 1i128 << bits;
            val -= mask;
        }
        val as i64
    }

    #[inline]
    fn decode_primitive(&self, data: &[u8], prim_type: &str) -> JsonValue {
        match prim_type {
            "INTEGER" => json!(self.decode_integer(data)),
            "OCTET STRING" => json!(hex::encode(data)),
            "TBCD-STRING" => self.decode_tbcd(data),
            "BIT STRING" => {
                if data.len() >= 1 {
                    json!(hex::encode(&data[1..]))
                } else {
                    json!(hex::encode(data))
                }
            }
            "IA5String" | "UTF8String" => {
                let s = String::from_utf8_lossy(data).to_string();
                json!(s)
            }
            "BOOLEAN" => json!(!data.is_empty() && data[0] != 0),
            "NULL" => JsonValue::Null,
            _ => json!(hex::encode(data)),
        }
    }

    /// TBCD decoding (telecom digits)
    #[inline]
    fn decode_tbcd(&self, octets: &[u8]) -> JsonValue {
        if octets.is_empty() {
            return json!("");
        }
        // each byte can produce up to 2 digits
        let mut digits = String::with_capacity(octets.len() * 2);
        for &b in octets {
            let low = b & 0x0F;
            let high = (b >> 4) & 0x0F;
            if low <= 9 {
                digits.push(char::from(b'0' + low));
            }
            if high <= 9 {
                digits.push(char::from(b'0' + high));
            }
        }
        json!(digits)
    }

    /// Timestamp decoding similar to Python decode_timestamp
    fn decode_timestamp(&self, ts_bytes: &[u8]) -> JsonValue {
        if ts_bytes.len() < 6 {
            return json!(hex::encode(ts_bytes));
        }

        let b0 = ts_bytes[0];
        let b1 = ts_bytes[1];
        let b2 = ts_bytes[2];
        let b3 = ts_bytes[3];
        let b4 = ts_bytes[4];
        let b5 = ts_bytes[5];

        let yy = format!("{}{}", (b0 >> 4) & 0x0F, b0 & 0x0F);
        let mm = format!("{}{}", (b1 >> 4) & 0x0F, b1 & 0x0F);
        let dd = format!("{}{}", (b2 >> 4) & 0x0F, b2 & 0x0F);
        let hh = format!("{}{}", (b3 >> 4) & 0x0F, b3 & 0x0F);
        let mn = format!("{}{}", (b4 >> 4) & 0x0F, b4 & 0x0F);
        let ss = format!("{}{}", (b5 >> 4) & 0x0F, b5 & 0x0F);

        let mut tz_part = String::new();
        if ts_bytes.len() >= 9 {
            let b6 = ts_bytes[6];
            let sign = match b6 {
                b'-' => '-',
                b'+' => '+',
                _ => '+',
            };
            let b7 = ts_bytes[7];
            let b8 = ts_bytes[8];
            let tz_hh = format!("{}{}", (b7 >> 4) & 0x0F, b7 & 0x0F);
            let tz_mm = format!("{}{}", (b8 >> 4) & 0x0F, b8 & 0x0F);
            tz_part = format!(" {}{}:{}", sign, tz_hh, tz_mm);
        }

        let s = format!(
            "20{}-{}-{} {}:{}:{}{}",
            yy, mm, dd, hh, mn, ss, tz_part
        );
        json!(s)
    }

    fn decode_plmn_id(&self, plmn_bytes: &[u8]) -> JsonValue {
        if plmn_bytes.len() != 3 {
            return json!(hex::encode(plmn_bytes));
        }
        let b0 = plmn_bytes[0];
        let b1 = plmn_bytes[1];
        let b2 = plmn_bytes[2];

        let mcc_digit2 = b0 & 0x0F;
        let mcc_digit1 = (b0 >> 4) & 0x0F;
        let mcc_digit3 = b1 & 0x0F;

        let mnc_digit3 = (b1 >> 4) & 0x0F;
        let mnc_digit2 = b2 & 0x0F;
        let mnc_digit1 = (b2 >> 4) & 0x0F;

        let mcc = format!("{}{}{}", mcc_digit1, mcc_digit2, mcc_digit3);
        let mnc = if mnc_digit3 == 0x0F {
            format!("{}{}", mnc_digit1, mnc_digit2)
        } else {
            format!("{}{}{}", mnc_digit1, mnc_digit2, mnc_digit3)
        };

        json!(format!("{}-{}", mcc, mnc))
    }

    /// IP address decoding similar to Python decode_ip_address
    #[inline]
    fn decode_ip_address(&self, data: &[u8]) -> JsonValue {
        if data.len() == 4 {
            let s = format!("{}.{}.{}.{}", data[0], data[1], data[2], data[3]);
            return json!(s);
        }

        if data.len() == 16 {
            // naive IPv6 hex groups
            let mut parts = Vec::with_capacity(8);
            for i in (0..16).step_by(2) {
                parts.push(format!("{:02x}{:02x}", data[i], data[i + 1]));
            }
            return json!(parts.join(":"));
        }

        // Try to interpret as TLV wrapping IP
        if let Some((tlv, _)) = self.parse_tlv(data, 0) {
            let val = tlv.value;
            if val.len() == 4 {
                let s = format!("{}.{}.{}.{}", val[0], val[1], val[2], val[3]);
                return json!(s);
            }
            if val.len() == 16 {
                let mut parts = Vec::with_capacity(8);
                for i in (0..16).step_by(2) {
                    parts.push(format!("{:02x}{:02x}", val[i], val[i + 1]));
                }
                return json!(parts.join(":"));
            }
        }

        json!(hex::encode(data))
    }

    /// IPAddress / GSNAddress CHOICE
    fn decode_ipaddress_choice(&self, data: &[u8]) -> JsonValue {
        let (tlv, _) = match self.parse_tlv(data, 0) {
            Some(t) => t,
            None => return self.decode_ip_address(data),
        };

        let tnum = tlv.tag_num;
        let val = tlv.value;

        match tnum {
            0 | 1 => self.decode_ip_address(val),
            2 | 3 => {
                let s = String::from_utf8_lossy(val).to_string();
                json!(s)
            }
            _ => {
                // if constructed, peek inside
                if tlv.constructed {
                    if let Some((inner, _)) = self.parse_tlv(val, 0) {
                        let inner_val = inner.value;
                        let inner_tag = inner.tag_num;
                        if inner_tag == 0 || inner_tag == 1 {
                            return self.decode_ip_address(inner_val);
                        }
                        if inner_tag == 2 || inner_tag == 3 {
                            let s = String::from_utf8_lossy(inner_val).to_string();
                            return json!(s);
                        }
                    }
                }
                self.decode_ip_address(val)
            }
        }
    }

    /// PDPAddress CHOICE-like decoding
    fn decode_pdpaddress_choice(&self, data: &[u8]) -> JsonValue {
        let (tlv, _) = match self.parse_tlv(data, 0) {
            Some(t) => t,
            None => return json!(hex::encode(data)),
        };

        let tnum = tlv.tag_num;
        let val = tlv.value;

        match tnum {
            0 => self.decode_ipaddress_choice(val),
            1 => self.decode_tbcd(val),
            _ => {
                if val.len() == 4 {
                    let s = format!("{}.{}.{}.{}", val[0], val[1], val[2], val[3]);
                    json!(s)
                } else if val.len() == 16 {
                    let mut parts = Vec::with_capacity(8);
                    for i in (0..16).step_by(2) {
                        parts.push(format!("{:02x}{:02x}", val[i], val[i + 1]));
                    }
                    json!(parts.join(":"))
                } else {
                    json!(hex::encode(val))
                }
            }
        }
    }

    /// Heuristic decode similar to Python decode_inferred
    #[inline]
    fn decode_inferred(&self, data: &[u8], type_name: &str) -> JsonValue {
        let lname = type_name.to_ascii_lowercase();

        if lname == "gsnaddress" {
            return self.decode_ipaddress_choice(data);
        }
        if lname == "pdpaddress" {
            return self.decode_pdpaddress_choice(data);
        }
        if lname == "timestamp" {
            return self.decode_timestamp(data);
        }
        if lname == "imsi" || lname == "imei" {
            return self.decode_tbcd(data);
        }
        if lname == "msisdn" {
            return self.decode_tbcd(data);
        }
        if lname == "plmn-id" {
            return self.decode_plmn_id(data);
        }

        if !data.is_empty() {
            let dlen = data.len();

            if dlen == 9 && (lname.contains("time") || lname.contains("timestamp")) {
                return self.decode_timestamp(data);
            }

            if (dlen == 4 || dlen == 16)
                && (lname.contains("address") || lname.contains("ip"))
            {
                return self.decode_ip_address(data);
            }
        }

        if lname.contains("imsi") || lname.contains("imei") {
            return self.decode_tbcd(data);
        }
        if lname.contains("msisdn") {
            return self.decode_tbcd(data);
        }
        if lname.contains("address") && !lname.contains("ip") {
            return self.decode_tbcd(data);
        }
        if lname.contains("plmn") {
            return self.decode_plmn_id(data);
        }

        // small id -> integer
        if lname.contains("id") && data.len() <= 8 {
            let mut val: u64 = 0;
            for &b in data {
                val = (val << 8) | b as u64;
            }
            return json!(val);
        }

        if lname.contains("string") {
            let s = String::from_utf8_lossy(data).to_string();
            return json!(s);
        }

        if lname.contains("flag") || lname.contains("boolean") {
            return json!(!data.is_empty() && data[0] != 0);
        }

        // last resort: small bytes => integer
        if data.len() <= 8 {
            let mut val: u64 = 0;
            for &b in data {
                val = (val << 8) | b as u64;
            }
            return json!(val);
        }

        json!(hex::encode(data))
    }
}

/// Process a single file into JSONL
fn process_file(
    decoder: &DerDecoder,
    root_type: &str,
    in_path: &Path,
    out_dir: &Path,
) -> Result<usize> {
    let file = File::open(in_path)
        .with_context(|| format!("Failed to open input file {:?}", in_path))?;
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

    let out_file = File::create(&out_path)
        .with_context(|| format!("Failed to create output file {:?}", out_path))?;
    // Larger buffer to reduce syscalls
    let mut writer = BufWriter::with_capacity(16 * 1024 * 1024, out_file);

    let mut offset = 0usize;
    let total_len = data.len();
    let mut count = 0usize;

    let use_auto = root_type.eq_ignore_ascii_case("auto") || root_type.is_empty();

    while offset < total_len {
        let (tlv, new_off) = match decoder.parse_tlv(data, offset) {
            Some(t) => t,
            None => break,
        };
        if new_off <= offset {
            break;
        }

        let decoded = if use_auto {
            decoder.auto_decode_record(&tlv)
        } else {
            decoder.decode_root_tlv_with_type(&tlv, root_type)
        };

        serde_json::to_writer(&mut writer, &decoded)?;
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

    // Read and parse schema
    let schema_text = std::fs::read_to_string(&cli.schema)
        .with_context(|| format!("Failed to read schema file {:?}", cli.schema))?;
    let schema = Asn1Schema::parse(&schema_text)?;
    let decoder = DerDecoder::new(schema);

    // Ensure output dir
    std::fs::create_dir_all(&cli.output_dir)?;

    // Determine effective root type (fall back to auto if unknown)
    let mut root_type = cli.root_type.clone();
    if !root_type.eq_ignore_ascii_case("auto") && !decoder.schema.knows_type(&root_type) {
        eprintln!(
            "WARNING: root-type '{}' does not appear in parsed schema. Falling back to auto mode.",
            root_type
        );
        root_type = "auto".to_string();
    }

    // Process all files in parallel
    let out_dir = cli.output_dir.clone();

    let results: Vec<(PathBuf, Result<usize>)> = cli
        .der_files
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
                        .join(format!(
                            "{}.jsonl",
                            path.file_name().unwrap().to_string_lossy()
                        ))
                        .display()
                );
            }
            Err(e) => {
                eprintln!("Decoding failed for {:?}: {:#}", path, e);
            }
        }
    }

    println!("Total decoded records: {}", total_records);

    // end timer and print total runtime
    let elapsed = overall_start.elapsed().as_secs_f64();
    println!("Total elapsed wall time: {:.3} s", elapsed);
    Ok(())
}
