# Beginner-Friendly Documentation for `main.rs`

This document explains the Rust program in **very simple language**.

The program’s job, in plain words:

1. Read an **ASN.1 schema file** (a text file that describes the structure of records).
2. Find one or more **input files** (DER-encoded binary files) in the paths you give.
3. For each input file:
   - Read the raw bytes very quickly.
   - Split the bytes into DER “chunks” called **TLV** items (Tag–Length–Value).
   - Use the schema to turn those bytes into **JSON**.
   - Write one JSON object per line into an output file (`.jsonl` format).

Important beginner notes:

- **DER** is a strict binary format used with ASN.1. You do *not* need to understand DER deeply to follow this code.
- A **TLV** (Tag–Length–Value) is a common way to store data:
  - **Tag**: what kind of thing this is
  - **Length**: how many bytes the value uses
  - **Value**: the raw bytes
- This program intentionally prints most values as **hex strings** (example: `"0a1bff"`) instead of turning them into decimal numbers. That is a design choice for speed and safety.

---

## How to run the program (high level)

The program is a command-line tool. You run it like this (example):

```bash
./decoder --schema path/to/schema.asn1 --root-type PGWRecord --output-dir out/ input1.dat input_folder/
```

- `--schema`: path to the ASN.1 schema file
- `--root-type`: the top-level ASN.1 type to decode (or `"auto"` to guess)
- `--output-dir`: where `.jsonl` files will be written
- inputs: one or more files or folders (folders are scanned recursively)

---

## Big-picture flow (how the pieces work together)

### Step 1 — `main()`
`main()` reads command-line options, loads the schema, builds a decoder, finds input files, and processes them (in parallel).

### Step 2 — schema parsing (`Asn1Schema::parse`)
The schema text file is scanned with regular expressions and turned into easy-to-use maps:
- `choices`: CHOICE types with possible alternatives
- `sequences` / `sets`: SEQUENCE / SET types with tagged fields
- `primitives`: primitive types (INTEGER, OCTET STRING, etc.)
- `aliases`: type aliases (TypeA ::= TypeB)

### Step 3 — decoding bytes into TLVs (`DerDecoder::parse_tlv`)
When decoding a file, the decoder repeatedly reads:
- one TLV
- moves the “cursor” forward
- repeats until the file ends

### Step 4 — writing JSON (`write_type`, `write_sequence`, `write_choice`, etc.)
Depending on the schema type:
- SEQUENCE/SET → JSON object (`{...}`)
- SEQUENCE OF → JSON array (`[...]`)
- CHOICE → JSON object with exactly one key (the chosen alternative)
- primitive/unknown → hex string

### Step 5 — output (`process_file`)
Each input file becomes one output file named like `inputfile.ext.jsonl`.
Each decoded record is written as **one JSON line**, then a newline.

---

## Function-by-function explanations (beginner level)

### `is_synth_choice_tag(t: u32) -> bool`
**Purpose:**  
The schema parser sometimes has to invent (“synthesize”) tag numbers for CHOICE alternatives that are not explicitly tagged in the schema. This helper checks whether a tag number is one of those invented ones.

**Input:**  
- `t`: a tag number (an unsigned 32-bit integer)

**Output:**  
- `true` if `t` is in the special “synthetic” range  
- `false` otherwise

**How it works:**  
- It compares `t` to a constant base value (`SYNTH_CHOICE_BASE`).
- Any tag number at or above that base is treated as synthetic.

---

### `Asn1Schema::parse(schema_text: &str) -> Result<Asn1Schema>`
**Purpose:**  
Read ASN.1 schema *text* and turn it into structured data the decoder can use.

**Input:**  
- `schema_text`: the full contents of the schema file as a string

**Output:**  
- `Ok(schema)` if parsing succeeds
- `Err(...)` if something goes wrong (for example, a regex fails to compile, or a tag number cannot be parsed as a number)

**Step-by-step:**
1. Build several **regular expressions** (patterns) that can find important parts of the schema.
2. Remove ASN.1 comments (lines starting with `--`).
3. Find **aliases** like `TypeA ::= TypeB` and store them in `schema.aliases`.
4. Find **type assignments**:
   - If the type is `CHOICE`, read its alternatives and store them in `schema.choices`.
   - If the type is `SEQUENCE` or `SET`, read its tagged fields and store them in `schema.sequences` or `schema.sets`.
   - Otherwise, treat it like a “primitive” and store it in `schema.primitives`.
5. Return the filled `schema`.

**What happens if you change it:**  
- If the regex patterns are wrong, the decoder will misunderstand the schema, which leads to wrong JSON output.
- If comment stripping is removed, patterns might match things inside comments and create garbage types/fields.

---

### `Asn1Schema::resolve_alias(&self, t: &str) -> &str`
**Purpose:**  
If the schema says `TypeA ::= TypeB`, then `TypeA` is just another name for `TypeB`. This function follows those renames until it reaches the “real” type.

**Input:**  
- `t`: a type name

**Output:**  
- a type name that is not an alias anymore (or the original if there is no alias)

**How it works:**  
- It looks up `t` in `self.aliases`.
- If found, it replaces `t` with the alias target and repeats.
- It stops after a small fixed number of steps (16) to avoid infinite loops.

---

### `Asn1Schema::knows_type(&self, t: &str) -> bool`
**Purpose:**  
Check whether the schema contains information about a type name (directly or via alias).

**Input:**  
- `t`: a type name

**Output:**  
- `true` if the type is in `choices`, `sequences`, `sets`, or `primitives`
- `false` if the decoder has no idea what this type is

---

### `write_json_key(w, key) -> Result<()>`
**Purpose:**  
Write a JSON object key safely, with minimal escaping.

**Input:**  
- `w`: something you can write bytes into (like a buffered file writer)
- `key`: the key text

**Output:**  
- `Ok(())` on success, `Err(...)` on write failure

**How it works:**  
- Writes an opening quote `"`.
- Walks through each byte of the key and:
  - escapes quotes, backslashes, and a few control characters
  - writes normal characters as-is
- Writes the closing quote `"`.

---

### `hex_encode_into(bytes, scratch) -> &[u8]`
**Purpose:**  
Convert raw bytes into lowercase hex characters (fast), reusing a buffer to avoid repeated allocations.

**Input:**  
- `bytes`: the raw data
- `scratch`: a reusable `Vec<u8>` buffer

**Output:**  
- a slice pointing into `scratch` that contains the hex text

**How it works:**  
- Clears and resizes `scratch` to exactly `bytes.len() * 2` (each byte becomes 2 hex characters).
- Fills `scratch` by mapping each half-byte (“nibble”) to `0..f`.
- Returns the used part of `scratch`.

**Note for beginners:**  
This uses `unsafe` for speed. `unsafe` means Rust is not checking some safety rules here, so the code must be extra careful.

---

### `write_hex_json(w, data, scratch) -> Result<()>`
**Purpose:**  
Write a JSON string value that is the hex representation of `data`.

**Input:**  
- `data`: raw bytes
- `scratch`: reusable buffer for hex encoding

**Output:**  
- `Ok(())` or an error if writing fails

---

### `lower_first(s) -> String`
**Purpose:**  
Make the first character lowercase. This is used when the program generates a JSON key from a type name in auto mode.

---

### `DerDecoder::new(schema) -> DerDecoder`
**Purpose:**  
Create a decoder from the parsed schema, and build an index used for “auto” root-type guessing.

**How it works (simplified):**
- Looks through schema CHOICE definitions.
- For a specific family of tags (context-specific tags), stores a mapping from tag number → possible type name.
- Saves both `schema` and the index in the returned `DerDecoder`.

---

### `DerDecoder::parse_tlv(data, offset) -> Option<(Tlv, usize)>`
**Purpose:**  
Read exactly one DER TLV item starting at `offset`.

**Input:**  
- `data`: the full byte buffer
- `offset`: where to start reading

**Output:**  
- `Some((tlv, new_offset))` if parsing works
- `None` if there isn’t enough data or something looks invalid

**Step-by-step (high level):**
1. Read the first tag byte.
2. Split that byte into:
   - tag class
   - constructed flag
   - tag number (or “extended tag number” if it is `0x1F`)
3. Read the length:
   - short form: one byte
   - long form: first length byte says how many more bytes form the length
4. Slice out the `value` bytes.
5. Also slice out `raw` bytes (tag + length + value).
6. Return the `Tlv` plus the new offset (pointing right after it).

---

### `DerDecoder::write_type(...)`
**Purpose:**  
Given some bytes and an ASN.1 type name, write the correct JSON representation.

**What it does:**
- If the type is a CHOICE: call `write_choice`
- If SEQUENCE/SET: call `write_sequence`
- Otherwise: treat it as primitive/unknown and write hex

---

### `DerDecoder::write_sequence(...)`
**Purpose:**  
Decode a SEQUENCE/SET value (a list of tagged fields) into a JSON object.

**Key idea:**  
It keeps reading TLVs inside the SEQUENCE, and for each TLV:
- If its tag is known in the schema → write the field under that field name
- Otherwise → write an `"unknown_tag_<n>"` key with hex

---

### `DerDecoder::write_sequence_of(...)`
**Purpose:**  
Decode a SEQUENCE OF (a list of repeated elements) into a JSON array.

---

### `DerDecoder::choice_alt_matches_tlv(...)`
**Purpose:**  
For untagged CHOICE alternatives, the program has to guess which alternative matches by looking at the TLV’s tag class / tag number and whether it is constructed.

---

### `DerDecoder::write_choice(...)`
**Purpose:**  
Decode a CHOICE into a JSON object with one key.

**How it works (simplified):**
- Parse the “outer” TLV.
- Try a few “candidate” TLVs (outer, or inner if it is wrapped).
- If any candidate tag matches a tagged CHOICE alternative, use that.
- Otherwise, try synthetic (untagged) alternatives by testing “does this TLV look like that type?”
- If still unknown, output a fallback object containing hex.

---

### `DerDecoder::write_root_tlv_with_type(...)`
**Purpose:**  
At the very top level, decide whether to decode using a named root type, or fall back to auto mode.

---

### `DerDecoder::write_auto_record(...)`
**Purpose:**  
When root type is `"auto"`, try to pick the record type based on a tag index built at startup.

---

### `expand_inputs(inputs, allowed_exts) -> Result<Vec<PathBuf>>`
**Purpose:**  
Turn the list of input paths (files and/or directories) into a sorted list of real files to process.

---

### `should_include(path, allowed_exts) -> bool`
**Purpose:**  
Filter files by extension (if the user provided `--ext`).

---

### `process_file(decoder, root_type, in_path, out_dir) -> Result<usize>`
**Purpose:**  
Decode one input file into one output `.jsonl` file.

**Steps:**
1. Open the input file and memory-map it for speed.
2. Create an output file with a big buffered writer.
3. Loop:
   - parse a TLV at the current offset
   - decode it into JSON (auto or typed)
   - write newline
   - move offset forward
4. Return how many records were written.

---

### `main() -> Result<()>`
**Purpose:**  
Tie everything together:
- parse CLI args
- read schema
- find input files
- process files in parallel
- print a summary

---

## Line-by-line explanation (every line)

Below, every line of `main.rs` is explained in a beginner-friendly way.

##### Line 1: `use anyhow::{anyhow, Context, Result};`
- **What it does:** This imports names (types/functions/macros) from another crate or from Rust’s standard library.
- **Why it is needed:** Without importing, you would have to write longer names (like `std::fs::File` everywhere) and some features would be unavailable.
- **If removed/changed:** If removed, any later code that uses the imported names will fail to compile unless you replace them with full paths.


##### Line 2: `use clap::Parser;`
- **What it does:** This imports names (types/functions/macros) from another crate or from Rust’s standard library.
- **Why it is needed:** Without importing, you would have to write longer names (like `std::fs::File` everywhere) and some features would be unavailable.
- **If removed/changed:** If removed, any later code that uses the imported names will fail to compile unless you replace them with full paths.


##### Line 3: `use memmap2::Mmap;`
- **What it does:** This imports names (types/functions/macros) from another crate or from Rust’s standard library.
- **Why it is needed:** Without importing, you would have to write longer names (like `std::fs::File` everywhere) and some features would be unavailable.
- **If removed/changed:** If removed, any later code that uses the imported names will fail to compile unless you replace them with full paths.


##### Line 4: `use rayon::prelude::*;`
- **What it does:** This imports names (types/functions/macros) from another crate or from Rust’s standard library.
- **Why it is needed:** Without importing, you would have to write longer names (like `std::fs::File` everywhere) and some features would be unavailable.
- **If removed/changed:** If removed, any later code that uses the imported names will fail to compile unless you replace them with full paths.


##### Line 5: `use regex::Regex;`
- **What it does:** This imports names (types/functions/macros) from another crate or from Rust’s standard library.
- **Why it is needed:** Without importing, you would have to write longer names (like `std::fs::File` everywhere) and some features would be unavailable.
- **If removed/changed:** If removed, any later code that uses the imported names will fail to compile unless you replace them with full paths.


##### Line 6: `use std::collections::{HashMap, HashSet};`
- **What it does:** This imports names (types/functions/macros) from another crate or from Rust’s standard library.
- **Why it is needed:** Without importing, you would have to write longer names (like `std::fs::File` everywhere) and some features would be unavailable.
- **If removed/changed:** If removed, any later code that uses the imported names will fail to compile unless you replace them with full paths.


##### Line 7: `use std::fs::File;`
- **What it does:** This imports names (types/functions/macros) from another crate or from Rust’s standard library.
- **Why it is needed:** Without importing, you would have to write longer names (like `std::fs::File` everywhere) and some features would be unavailable.
- **If removed/changed:** If removed, any later code that uses the imported names will fail to compile unless you replace them with full paths.


##### Line 8: `use std::io::{BufWriter, Write};`
- **What it does:** This imports names (types/functions/macros) from another crate or from Rust’s standard library.
- **Why it is needed:** Without importing, you would have to write longer names (like `std::fs::File` everywhere) and some features would be unavailable.
- **If removed/changed:** If removed, any later code that uses the imported names will fail to compile unless you replace them with full paths.


##### Line 9: `use std::path::{Path, PathBuf};`
- **What it does:** This imports names (types/functions/macros) from another crate or from Rust’s standard library.
- **Why it is needed:** Without importing, you would have to write longer names (like `std::fs::File` everywhere) and some features would be unavailable.
- **If removed/changed:** If removed, any later code that uses the imported names will fail to compile unless you replace them with full paths.


##### Line 10: `use std::time::Instant;`
- **What it does:** This imports names (types/functions/macros) from another crate or from Rust’s standard library.
- **Why it is needed:** Without importing, you would have to write longer names (like `std::fs::File` everywhere) and some features would be unavailable.
- **If removed/changed:** If removed, any later code that uses the imported names will fail to compile unless you replace them with full paths.


##### Line 11: `use walkdir::WalkDir;`
- **What it does:** This imports names (types/functions/macros) from another crate or from Rust’s standard library.
- **Why it is needed:** Without importing, you would have to write longer names (like `std::fs::File` everywhere) and some features would be unavailable.
- **If removed/changed:** If removed, any later code that uses the imported names will fail to compile unless you replace them with full paths.


##### Line 12: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 13: `/// CLI arguments`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 14: `#[derive(Parser, Debug)]`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 15: `#[command(`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 16: `    author,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 17: `    version,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 18: `    about = "Ultra-fast ASN.1 DER Decoder -> JSONL (schema-based, hex-only values, no decimal conversion)",`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 19: `    long_about = None`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 20: `)]`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 21: `struct Cli {`
- **What it does:** This starts defining a `struct`, which is a custom data type that groups several pieces of data together.
- **Why it is needed:** The program needs a structured way to store related values (like CLI options or schema fields).
- **If removed/changed:** If removed, code that creates or uses this type will not compile; you would need a different way to store the same information.


##### Line 22: `    /// ASN.1 schema file`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 23: `    #[arg(long = "schema")]`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 24: `    schema: PathBuf,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 25: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 26: `    /// Root ASN.1 type (e.g. PGWRecord). Use "auto" to infer.`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 27: `    #[arg(long = "root-type")]`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 28: `    root_type: String,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 29: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 30: `    /// Output directory`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 31: `    #[arg(long = "output-dir")]`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 32: `    output_dir: PathBuf,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 33: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 34: `    /// Optional: only decode files matching these extensions (comma-separated), e.g. "dat,bin"`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 35: `    #[arg(long = "ext")]`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 36: `    ext: Option<String>,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 37: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 38: `    /// DER-encoded input files or directories (directories scanned recursively)`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 39: `    #[arg(required = true)]`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 40: `    inputs: Vec<PathBuf>,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 41: `}`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 42: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 43: `/// Field info for SEQUENCE / SET`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 44: `#[derive(Debug, Clone)]`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 45: `struct FieldSpec {`
- **What it does:** This starts defining a `struct`, which is a custom data type that groups several pieces of data together.
- **Why it is needed:** The program needs a structured way to store related values (like CLI options or schema fields).
- **If removed/changed:** If removed, code that creates or uses this type will not compile; you would need a different way to store the same information.


##### Line 46: `    name: String,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 47: `    field_type: String,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 48: `    #[allow(dead_code)]`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 49: `    optional: bool,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 50: `    is_sequence_of: bool,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 51: `}`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 52: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 53: `/// Schema representation`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 54: `#[derive(Debug, Default)]`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 55: `struct Asn1Schema {`
- **What it does:** This starts defining a `struct`, which is a custom data type that groups several pieces of data together.
- **Why it is needed:** The program needs a structured way to store related values (like CLI options or schema fields).
- **If removed/changed:** If removed, code that creates or uses this type will not compile; you would need a different way to store the same information.


##### Line 56: `    choices: HashMap<String, HashMap<u32, (String, String)>>,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 57: `    sequences: HashMap<String, HashMap<u32, FieldSpec>>,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 58: `    sets: HashMap<String, HashMap<u32, FieldSpec>>,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 59: `    primitives: HashMap<String, String>, // type_name -> primitive kind`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 60: `    aliases: HashMap<String, String>,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 61: `}`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 62: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 63: `const SYNTH_CHOICE_BASE: u32 = 0xFFFF_FF00;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 64: `#[inline]`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 65: `fn is_synth_choice_tag(t: u32) -> bool {`
- **What it does:** This starts defining a function, which is a named block of code you can run.
- **Why it is needed:** Functions keep code organized and reusable, and they make the main flow easier to follow.
- **If removed/changed:** If removed, any place that calls this function will not compile; you would need to inline its logic elsewhere.


##### Line 66: `    t >= SYNTH_CHOICE_BASE`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 67: `}`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 68: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 69: `impl Asn1Schema {`
- **What it does:** This starts an `impl` block, which is where methods (functions attached to a type) are defined.
- **Why it is needed:** It groups behavior (functions) that belong to a particular struct, like parsing or decoding.
- **If removed/changed:** If removed, the methods inside disappear and the rest of the program won’t compile.


##### Line 70: `    fn parse(schema_text: &str) -> Result<Self> {`
- **What it does:** This starts defining a function, which is a named block of code you can run.
- **Why it is needed:** Functions keep code organized and reusable, and they make the main flow easier to follow.
- **If removed/changed:** If removed, any place that calls this function will not compile; you would need to inline its logic elsewhere.


##### Line 71: `        let comment_strip_re = Regex::new(r"--.*?(?:\n|$)")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 72: `        let type_assign_re = Regex::new(`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 73: `            r"(?s)([\w-]+)\s*::=\s*(CHOICE|SEQUENCE|SET|ENUMERATED|INTEGER|OCTET STRING|BIT STRING|IA5String|UTF8String|BOOLEAN|NULL|TBCD-STRING)\s*(?:\(([^)]*)\))?\s*(\{.*?\})?",`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 74: `        )?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 75: `        let alias_re = Regex::new(r"(?m)^\s*([\w-]+)\s*::=\s*([\w-]+)\s*$")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 76: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 77: `        let choice_tagged_re = Regex::new(r"([\w-]+)\s+\[(\d+)\]\s+([\w-]+)")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 78: `        let choice_untagged_re = Regex::new(r"([\w-]+)\s+([\w-]+)")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 79: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 80: `        let sequence_body_re = Regex::new(`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 81: `            r"([\w-]+)\s+\[(\d+)\]\s+([\w-]+(?:\s+OF\s+[\w-]+)?)\s*(OPTIONAL)?",`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 82: `        )?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 83: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 84: `        let mut schema = Asn1Schema::default();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 85: `        let stripped = comment_strip_re.replace_all(schema_text, "");`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 86: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 87: `        // aliases`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 88: `        for cap in alias_re.captures_iter(&stripped) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 89: `            let lhs = cap.get(1).unwrap().as_str().to_string();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 90: `            let rhs = cap.get(2).unwrap().as_str().to_string();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 91: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 92: `            let rhs_upper = rhs.to_ascii_uppercase();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 93: `            let is_keyword = matches!(`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 94: `                rhs_upper.as_str(),`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 95: `                "CHOICE"`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 96: `                    | "SEQUENCE"`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 97: `                    | "SET"`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 98: `                    | "ENUMERATED"`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 99: `                    | "INTEGER"`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 100: `                    | "OCTET"`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 101: `                    | "BIT"`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 102: `                    | "IA5STRING"`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 103: `                    | "UTF8STRING"`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 104: `                    | "BOOLEAN"`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 105: `                    | "NULL"`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 106: `                    | "TBCD-STRING"`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 107: `                    | "OCTET STRING"`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 108: `                    | "BIT STRING"`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 109: `            );`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 110: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 111: `            if !is_keyword && lhs != rhs {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 112: `                schema.aliases.insert(lhs, rhs);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 113: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 114: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 115: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 116: `        // type assignments`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 117: `        for caps in type_assign_re.captures_iter(&stripped) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 118: `            let type_name = caps.get(1).unwrap().as_str().to_string();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 119: `            let type_kind = caps.get(2).unwrap().as_str();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 120: `            let body = caps.get(4).map(|m| m.as_str()).unwrap_or("");`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 121: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 122: `            match type_kind {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 123: `                "CHOICE" => {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 124: `                    let mut alts = HashMap::new();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 125: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 126: `                    for c in choice_tagged_re.captures_iter(body) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 127: `                        let field_name = c.get(1).unwrap().as_str().to_string();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 128: `                        let tag: u32 = c.get(2).unwrap().as_str().parse()?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 129: `                        let field_type = c.get(3).unwrap().as_str().to_string();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 130: `                        alts.insert(tag, (field_name, field_type));`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 131: `                    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 132: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 133: `                    if alts.is_empty() {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 134: `                        let mut idx: u32 = 0;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 135: `                        for c in choice_untagged_re.captures_iter(body) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 136: `                            let field_name = c.get(1).unwrap().as_str().to_string();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 137: `                            let field_type = c.get(2).unwrap().as_str().to_string();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 138: `                            if field_name.is_empty() || field_type.is_empty() {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 139: `                                continue;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 140: `                            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 141: `                            alts.insert(SYNTH_CHOICE_BASE + idx, (field_name, field_type));`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 142: `                            idx += 1;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 143: `                            if idx >= 255 {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 144: `                                break;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 145: `                            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 146: `                        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 147: `                    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 148: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 149: `                    schema.choices.insert(type_name, alts);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 150: `                }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 151: `                "SEQUENCE" | "SET" => {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 152: `                    let mut fields = HashMap::new();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 153: `                    for c in sequence_body_re.captures_iter(body) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 154: `                        let field_name = c.get(1).unwrap().as_str().to_string();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 155: `                        let tag: u32 = c.get(2).unwrap().as_str().parse()?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 156: `                        let type_spec = c.get(3).unwrap().as_str().to_string();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 157: `                        let optional = c.get(4).is_some();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 158: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 159: `                        let mut is_sequence_of = false;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 160: `                        let mut element_type = type_spec.clone();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 161: `                        if let Some(pos) = type_spec.find(" OF ") {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 162: `                            is_sequence_of = true;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 163: `                            element_type = type_spec[pos + 4..].trim().to_string();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 164: `                        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 165: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 166: `                        fields.insert(`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 167: `                            tag,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 168: `                            FieldSpec {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 169: `                                name: field_name,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 170: `                                field_type: element_type,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 171: `                                optional,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 172: `                                is_sequence_of,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 173: `                            },`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 174: `                        );`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 175: `                    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 176: `                    if type_kind == "SEQUENCE" {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 177: `                        schema.sequences.insert(type_name, fields);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 178: `                    } else {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 179: `                        schema.sets.insert(type_name, fields);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 180: `                    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 181: `                }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 182: `                kind => {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 183: `                    schema.primitives.insert(type_name, kind.to_string());`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 184: `                }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 185: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 186: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 187: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 188: `        Ok(schema)`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 189: `    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 190: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 191: `    #[inline]`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 192: `    fn resolve_alias<'a>(&'a self, mut t: &'a str) -> &'a str {`
- **What it does:** This starts defining a function, which is a named block of code you can run.
- **Why it is needed:** Functions keep code organized and reusable, and they make the main flow easier to follow.
- **If removed/changed:** If removed, any place that calls this function will not compile; you would need to inline its logic elsewhere.


##### Line 193: `        for _ in 0..16 {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 194: `            if let Some(next) = self.aliases.get(t) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 195: `                t = next;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 196: `            } else {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 197: `                break;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 198: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 199: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 200: `        t`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 201: `    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 202: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 203: `    #[inline]`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 204: `    fn knows_type(&self, t: &str) -> bool {`
- **What it does:** This starts defining a function, which is a named block of code you can run.
- **Why it is needed:** Functions keep code organized and reusable, and they make the main flow easier to follow.
- **If removed/changed:** If removed, any place that calls this function will not compile; you would need to inline its logic elsewhere.


##### Line 205: `        let rt = self.resolve_alias(t);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 206: `        self.choices.contains_key(rt)`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 207: `            || self.sequences.contains_key(rt)`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 208: `            || self.sets.contains_key(rt)`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 209: `            || self.primitives.contains_key(rt)`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 210: `    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 211: `}`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 212: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 213: `/// A parsed TLV`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 214: `#[derive(Debug, Clone)]`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 215: `struct Tlv<'a> {`
- **What it does:** This starts defining a `struct`, which is a custom data type that groups several pieces of data together.
- **Why it is needed:** The program needs a structured way to store related values (like CLI options or schema fields).
- **If removed/changed:** If removed, code that creates or uses this type will not compile; you would need a different way to store the same information.


##### Line 216: `    tag_class: u8,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 217: `    constructed: bool,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 218: `    tag_num: u32,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 219: `    #[allow(dead_code)]`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 220: `    length: usize,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 221: `    value: &'a [u8],`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 222: `    raw: &'a [u8],`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 223: `}`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 224: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 225: `#[inline]`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 226: `fn write_json_key<W: Write>(w: &mut W, key: &str) -> Result<()> {`
- **What it does:** This starts defining a function, which is a named block of code you can run.
- **Why it is needed:** Functions keep code organized and reusable, and they make the main flow easier to follow.
- **If removed/changed:** If removed, any place that calls this function will not compile; you would need to inline its logic elsewhere.


##### Line 227: `    // minimal escaping: schema keys should be safe; do a cheap escape for quotes/backslash/control`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 228: `    w.write_all(b"\"")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 229: `    for &b in key.as_bytes() {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 230: `        match b {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 231: `            b'"' => w.write_all(b"\\\"")?,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 232: `            b'\\' => w.write_all(b"\\\\")?,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 233: `            b'\n' => w.write_all(b"\\n")?,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 234: `            b'\r' => w.write_all(b"\\r")?,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 235: `            b'\t' => w.write_all(b"\\t")?,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 236: `            c if c < 0x20 => {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 237: `                const HEX: &[u8; 16] = b"0123456789abcdef";`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 238: `                let esc = [b'\\', b'u', b'0', b'0', HEX[(c >> 4) as usize], HEX[(c & 0x0F) as usize]];`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 239: `                w.write_all(&esc)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 240: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 241: `            c => w.write_all(&[c])?,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 242: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 243: `    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 244: `    w.write_all(b"\"")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 245: `    Ok(())`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 246: `}`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 247: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 248: `/// Ultra-fast hex encoder into a reusable per-file scratch.`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 249: `/// Returns slice to encoded hex.`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 250: `#[inline(always)]`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 251: `fn hex_encode_into<'a>(bytes: &[u8], scratch: &'a mut Vec<u8>) -> &'a [u8] {`
- **What it does:** This starts defining a function, which is a named block of code you can run.
- **Why it is needed:** Functions keep code organized and reusable, and they make the main flow easier to follow.
- **If removed/changed:** If removed, any place that calls this function will not compile; you would need to inline its logic elsewhere.


##### Line 252: `    const HEX: &[u8; 16] = b"0123456789abcdef";`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 253: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 254: `    scratch.clear();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 255: `    scratch.reserve(bytes.len() * 2);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 256: `    unsafe { scratch.set_len(bytes.len() * 2) };`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 257: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 258: `    let mut j = 0usize;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 259: `    for &b in bytes {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 260: `        scratch[j] = HEX[(b >> 4) as usize];`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 261: `        scratch[j + 1] = HEX[(b & 0x0F) as usize];`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 262: `        j += 2;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 263: `    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 264: `    &scratch[..j]`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 265: `}`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 266: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 267: `#[inline]`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 268: `fn write_hex_json<W: Write>(w: &mut W, data: &[u8], scratch: &mut Vec<u8>) -> Result<()> {`
- **What it does:** This starts defining a function, which is a named block of code you can run.
- **Why it is needed:** Functions keep code organized and reusable, and they make the main flow easier to follow.
- **If removed/changed:** If removed, any place that calls this function will not compile; you would need to inline its logic elsewhere.


##### Line 269: `    w.write_all(b"\"")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 270: `    let hex = hex_encode_into(data, scratch);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 271: `    w.write_all(hex)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 272: `    w.write_all(b"\"")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 273: `    Ok(())`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 274: `}`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 275: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 276: `#[inline]`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 277: `fn lower_first(s: &str) -> String {`
- **What it does:** This starts defining a function, which is a named block of code you can run.
- **Why it is needed:** Functions keep code organized and reusable, and they make the main flow easier to follow.
- **If removed/changed:** If removed, any place that calls this function will not compile; you would need to inline its logic elsewhere.


##### Line 278: `    let mut chars = s.chars();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 279: `    match chars.next() {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 280: `        None => String::new(),`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 281: `        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 282: `    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 283: `}`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 284: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 285: `struct DerDecoder {`
- **What it does:** This starts defining a `struct`, which is a custom data type that groups several pieces of data together.
- **Why it is needed:** The program needs a structured way to store related values (like CLI options or schema fields).
- **If removed/changed:** If removed, code that creates or uses this type will not compile; you would need a different way to store the same information.


##### Line 286: `    schema: Asn1Schema,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 287: `    cs_choice_index: HashMap<u32, String>,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 288: `}`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 289: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 290: `impl DerDecoder {`
- **What it does:** This starts an `impl` block, which is where methods (functions attached to a type) are defined.
- **Why it is needed:** It groups behavior (functions) that belong to a particular struct, like parsing or decoding.
- **If removed/changed:** If removed, the methods inside disappear and the rest of the program won’t compile.


##### Line 291: `    fn new(schema: Asn1Schema) -> Self {`
- **What it does:** This starts defining a function, which is a named block of code you can run.
- **Why it is needed:** Functions keep code organized and reusable, and they make the main flow easier to follow.
- **If removed/changed:** If removed, any place that calls this function will not compile; you would need to inline its logic elsewhere.


##### Line 292: `        let mut cs_choice_index: HashMap<u32, String> = HashMap::new();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 293: `        for (_choice_name, alts) in &schema.choices {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 294: `            for (tag, (_field_name, field_type)) in alts {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 295: `                if is_synth_choice_tag(*tag) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 296: `                    continue;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 297: `                }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 298: `                cs_choice_index.entry(*tag).or_insert(field_type.clone());`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 299: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 300: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 301: `        Self { schema, cs_choice_index }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 302: `    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 303: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 304: `    #[inline(always)]`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 305: `    fn parse_tlv<'a>(&self, data: &'a [u8], mut offset: usize) -> Option<(Tlv<'a>, usize)> {`
- **What it does:** This starts defining a function, which is a named block of code you can run.
- **Why it is needed:** Functions keep code organized and reusable, and they make the main flow easier to follow.
- **If removed/changed:** If removed, any place that calls this function will not compile; you would need to inline its logic elsewhere.


##### Line 306: `        let data_len = data.len();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 307: `        if offset >= data_len {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 308: `            return None;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 309: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 310: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 311: `        let start = offset;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 312: `        let tag_byte = data[offset];`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 313: `        offset += 1;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 314: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 315: `        let tag_class = (tag_byte >> 6) & 0x03;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 316: `        let constructed = ((tag_byte >> 5) & 0x01) != 0;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 317: `        let mut tag_num = (tag_byte & 0x1F) as u32;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 318: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 319: `        if tag_num == 0x1F {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 320: `            tag_num = 0;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 321: `            while offset < data_len {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 322: `                let b = data[offset];`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 323: `                offset += 1;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 324: `                tag_num = (tag_num << 7) | (b & 0x7F) as u32;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 325: `                if (b & 0x80) == 0 {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 326: `                    break;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 327: `                }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 328: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 329: `            if offset >= data_len {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 330: `                return None;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 331: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 332: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 333: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 334: `        if offset >= data_len {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 335: `            return None;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 336: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 337: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 338: `        let length_byte = data[offset];`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 339: `        offset += 1;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 340: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 341: `        let length: usize;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 342: `        if (length_byte & 0x80) != 0 {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 343: `            let num_octets = (length_byte & 0x7F) as usize;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 344: `            if num_octets == 0 || offset + num_octets > data_len {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 345: `                return None;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 346: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 347: `            let mut l: usize = 0;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 348: `            let end_len = offset + num_octets;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 349: `            while offset < end_len {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 350: `                l = (l << 8) | data[offset] as usize;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 351: `                offset += 1;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 352: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 353: `            length = l;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 354: `        } else {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 355: `            length = length_byte as usize;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 356: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 357: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 358: `        if offset + length > data_len {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 359: `            return None;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 360: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 361: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 362: `        let value = &data[offset..offset + length];`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 363: `        offset += length;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 364: `        let raw = &data[start..offset];`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 365: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 366: `        Some((`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 367: `            Tlv {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 368: `                tag_class,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 369: `                constructed,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 370: `                tag_num,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 371: `                length,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 372: `                value,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 373: `                raw,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 374: `            },`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 375: `            offset,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 376: `        ))`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 377: `    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 378: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 379: `    #[inline]`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 380: `    fn write_type<W: Write>(&self, data: &[u8], type_name: &str, out: &mut W, scratch: &mut Vec<u8>) -> Result<()> {`
- **What it does:** This starts defining a function, which is a named block of code you can run.
- **Why it is needed:** Functions keep code organized and reusable, and they make the main flow easier to follow.
- **If removed/changed:** If removed, any place that calls this function will not compile; you would need to inline its logic elsewhere.


##### Line 381: `        let rt = self.schema.resolve_alias(type_name);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 382: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 383: `        if let Some(alts) = self.schema.choices.get(rt) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 384: `            self.write_choice(data, alts, out, scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 385: `            return Ok(());`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 386: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 387: `        if let Some(fields) = self.schema.sequences.get(rt) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 388: `            self.write_sequence(data, fields, out, scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 389: `            return Ok(());`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 390: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 391: `        if let Some(fields) = self.schema.sets.get(rt) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 392: `            self.write_sequence(data, fields, out, scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 393: `            return Ok(());`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 394: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 395: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 396: `        // primitive or unknown: ALWAYS hex (you requested no decimal conversion)`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 397: `        write_hex_json(out, data, scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 398: `        Ok(())`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 399: `    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 400: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 401: `    fn write_sequence<W: Write>(`
- **What it does:** This starts defining a function, which is a named block of code you can run.
- **Why it is needed:** Functions keep code organized and reusable, and they make the main flow easier to follow.
- **If removed/changed:** If removed, any place that calls this function will not compile; you would need to inline its logic elsewhere.


##### Line 402: `        &self,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 403: `        data: &[u8],`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 404: `        field_spec: &HashMap<u32, FieldSpec>,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 405: `        out: &mut W,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 406: `        scratch: &mut Vec<u8>,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 407: `    ) -> Result<()> {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 408: `        out.write_all(b"{")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 409: `        let mut offset = 0usize;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 410: `        let mut first = true;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 411: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 412: `        let mut itoa_buf = itoa::Buffer::new();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 413: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 414: `        while offset < data.len() {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 415: `            let (tlv, new_off) = match self.parse_tlv(data, offset) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 416: `                Some(t) => t,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 417: `                None => break,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 418: `            };`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 419: `            if new_off <= offset {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 420: `                break;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 421: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 422: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 423: `            if !first { out.write_all(b",")?; }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 424: `            first = false;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 425: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 426: `            if let Some(field) = field_spec.get(&tlv.tag_num) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 427: `                write_json_key(out, &field.name)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 428: `                out.write_all(b":")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 429: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 430: `                let resolved_field_type = self.schema.resolve_alias(&field.field_type);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 431: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 432: `                if field.is_sequence_of {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 433: `                    self.write_sequence_of(tlv.value, &field.field_type, out, scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 434: `                } else if self.schema.choices.contains_key(resolved_field_type) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 435: `                    // CHOICE needs raw TLV so it can see tag`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 436: `                    self.write_type(tlv.raw, &field.field_type, out, scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 437: `                } else if tlv.constructed {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 438: `                    self.write_type(tlv.value, &field.field_type, out, scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 439: `                } else {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 440: `                    // scalar primitive: hex only`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 441: `                    write_hex_json(out, tlv.value, scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 442: `                }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 443: `            } else {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 444: `                // unknown_tag_<n>: hex`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 445: `                out.write_all(b"\"unknown_tag_")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 446: `                out.write_all(itoa_buf.format(tlv.tag_num).as_bytes())?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 447: `                out.write_all(b"\":")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 448: `                    // Debug to stderr`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 449: `    eprintln!("Unknown tag {}, Raw: {:02x?}", tlv.tag_num, &tlv.raw[..tlv.raw.len().min(32)]);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 450: `                write_hex_json(out, tlv.value, scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 451: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 452: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 453: `            offset = new_off;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 454: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 455: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 456: `        out.write_all(b"}")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 457: `        Ok(())`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 458: `    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 459: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 460: `    fn write_sequence_of<W: Write>(&self, data: &[u8], element_type: &str, out: &mut W, scratch: &mut Vec<u8>) -> Result<()> {`
- **What it does:** This starts defining a function, which is a named block of code you can run.
- **Why it is needed:** Functions keep code organized and reusable, and they make the main flow easier to follow.
- **If removed/changed:** If removed, any place that calls this function will not compile; you would need to inline its logic elsewhere.


##### Line 461: `        out.write_all(b"[")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 462: `        let mut arr_first = true;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 463: `        let mut offset = 0usize;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 464: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 465: `        let is_choice = self.schema.choices.contains_key(self.schema.resolve_alias(element_type));`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 466: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 467: `        while offset < data.len() {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 468: `            let (tlv, new_off) = match self.parse_tlv(data, offset) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 469: `                Some(t) => t,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 470: `                None => break,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 471: `            };`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 472: `            if new_off <= offset {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 473: `                break;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 474: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 475: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 476: `            if !arr_first { out.write_all(b",")?; }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 477: `            arr_first = false;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 478: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 479: `            if is_choice {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 480: `                self.write_type(tlv.raw, element_type, out, scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 481: `            } else if tlv.constructed {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 482: `                self.write_type(tlv.value, element_type, out, scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 483: `            } else {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 484: `                write_hex_json(out, tlv.value, scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 485: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 486: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 487: `            offset = new_off;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 488: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 489: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 490: `        out.write_all(b"]")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 491: `        Ok(())`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 492: `    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 493: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 494: `    fn choice_alt_matches_tlv(&self, alt_type: &str, tlv: &Tlv) -> bool {`
- **What it does:** This starts defining a function, which is a named block of code you can run.
- **Why it is needed:** Functions keep code organized and reusable, and they make the main flow easier to follow.
- **If removed/changed:** If removed, any place that calls this function will not compile; you would need to inline its logic elsewhere.


##### Line 495: `        let rt = self.schema.resolve_alias(alt_type);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 496: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 497: `        if let Some(sub_alts) = self.schema.choices.get(rt) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 498: `            if sub_alts.contains_key(&tlv.tag_num) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 499: `                return true;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 500: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 501: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 502: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 503: `        if self.schema.sequences.contains_key(rt) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 504: `            return tlv.tag_class == 0 && tlv.constructed && tlv.tag_num == 16;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 505: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 506: `        if self.schema.sets.contains_key(rt) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 507: `            return tlv.tag_class == 0 && tlv.constructed && tlv.tag_num == 17;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 508: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 509: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 510: `        false`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 511: `    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 512: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 513: `    fn write_choice<W: Write>(`
- **What it does:** This starts defining a function, which is a named block of code you can run.
- **Why it is needed:** Functions keep code organized and reusable, and they make the main flow easier to follow.
- **If removed/changed:** If removed, any place that calls this function will not compile; you would need to inline its logic elsewhere.


##### Line 514: `        &self,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 515: `        data: &[u8],`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 516: `        alts: &HashMap<u32, (String, String)>,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 517: `        out: &mut W,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 518: `        scratch: &mut Vec<u8>,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 519: `    ) -> Result<()> {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 520: `        let (outer, _) = match self.parse_tlv(data, 0) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 521: `            Some(t) => t,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 522: `            None => {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 523: `                out.write_all(b"null")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 524: `                return Ok(());`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 525: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 526: `        };`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 527: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 528: `        // Candidate: outer only + unwrap constructed + unwrap octet-string`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 529: `        let mut candidates: [Option<Tlv>; 3] = [None, None, None];`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 530: `        candidates[0] = Some(outer.clone());`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 531: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 532: `        if outer.constructed {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 533: `            candidates[1] = self.parse_tlv(outer.value, 0).map(|(inner, _)| inner);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 534: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 535: `        if outer.tag_class == 0 && !outer.constructed && outer.tag_num == 4 {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 536: `            candidates[2] = self.parse_tlv(outer.value, 0).map(|(inner, _)| inner);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 537: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 538: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 539: `        out.write_all(b"{")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 540: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 541: `        // Tagged CHOICE`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 542: `        for cand in candidates.iter().flatten() {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 543: `            if let Some((field_name, type_name)) = alts.get(&cand.tag_num) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 544: `                write_json_key(out, field_name)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 545: `                out.write_all(b":")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 546: `                self.write_type(cand.value, type_name, out, scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 547: `                out.write_all(b"}")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 548: `                return Ok(());`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 549: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 550: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 551: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 552: `        // Untagged CHOICE: probe synthetic alts`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 553: `        let mut synth_keys: Vec<u32> = alts.keys().copied().filter(|t| is_synth_choice_tag(*t)).collect();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 554: `        synth_keys.sort_unstable();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 555: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 556: `        let probe = candidates.iter().flatten().last().unwrap();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 557: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 558: `        for k in synth_keys {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 559: `            let (fname, ftype) = &alts[&k];`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 560: `            if self.choice_alt_matches_tlv(ftype, probe) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 561: `                write_json_key(out, fname)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 562: `                out.write_all(b":")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 563: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 564: `                let rt = self.schema.resolve_alias(ftype);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 565: `                if self.schema.choices.contains_key(rt) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 566: `                    self.write_type(probe.raw, ftype, out, scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 567: `                } else {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 568: `                    self.write_type(probe.value, ftype, out, scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 569: `                }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 570: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 571: `                out.write_all(b"}")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 572: `                return Ok(());`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 573: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 574: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 575: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 576: `        write_json_key(out, "unknown_alternative")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 577: `        out.write_all(b":")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 578: `        write_hex_json(out, probe.raw, scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 579: `        out.write_all(b"}")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 580: `        Ok(())`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 581: `    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 582: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 583: `    fn write_root_tlv_with_type<W: Write>(&self, tlv: &Tlv, root_type: &str, out: &mut W, scratch: &mut Vec<u8>) -> Result<()> {`
- **What it does:** This starts defining a function, which is a named block of code you can run.
- **Why it is needed:** Functions keep code organized and reusable, and they make the main flow easier to follow.
- **If removed/changed:** If removed, any place that calls this function will not compile; you would need to inline its logic elsewhere.


##### Line 584: `        let rt = self.schema.resolve_alias(root_type);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 585: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 586: `        if !self.schema.knows_type(rt) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 587: `            self.write_auto_record(tlv, out, scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 588: `            return Ok(());`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 589: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 590: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 591: `        if self.schema.choices.contains_key(rt) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 592: `            self.write_type(tlv.raw, root_type, out, scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 593: `        } else {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 594: `            self.write_type(tlv.value, root_type, out, scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 595: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 596: `        Ok(())`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 597: `    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 598: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 599: `    fn write_auto_record<W: Write>(&self, tlv: &Tlv, out: &mut W, scratch: &mut Vec<u8>) -> Result<()> {`
- **What it does:** This starts defining a function, which is a named block of code you can run.
- **Why it is needed:** Functions keep code organized and reusable, and they make the main flow easier to follow.
- **If removed/changed:** If removed, any place that calls this function will not compile; you would need to inline its logic elsewhere.


##### Line 600: `        if tlv.tag_class == 2 {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 601: `            if let Some(alt_type) = self.cs_choice_index.get(&tlv.tag_num) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 602: `                out.write_all(b"{")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 603: `                let key = lower_first(alt_type);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 604: `                write_json_key(out, &key)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 605: `                out.write_all(b":")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 606: `                self.write_type(tlv.value, alt_type, out, scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 607: `                out.write_all(b"}")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 608: `                return Ok(());`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 609: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 610: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 611: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 612: `        // fallback: dump raw TLV hex`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 613: `        out.write_all(b"{")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 614: `        write_json_key(out, "unknown")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 615: `        out.write_all(b":")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 616: `        write_hex_json(out, tlv.raw, scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 617: `        out.write_all(b"}")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 618: `        Ok(())`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 619: `    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 620: `}`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 621: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 622: `fn expand_inputs(inputs: &[PathBuf], allowed_exts: Option<&HashSet<String>>) -> Result<Vec<PathBuf>> {`
- **What it does:** This starts defining a function, which is a named block of code you can run.
- **Why it is needed:** Functions keep code organized and reusable, and they make the main flow easier to follow.
- **If removed/changed:** If removed, any place that calls this function will not compile; you would need to inline its logic elsewhere.


##### Line 623: `    let mut files: Vec<PathBuf> = Vec::new();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 624: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 625: `    for p in inputs {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 626: `        if p.is_file() {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 627: `            if should_include(p, allowed_exts) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 628: `                files.push(p.clone());`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 629: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 630: `        } else if p.is_dir() {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 631: `            for entry in WalkDir::new(p).follow_links(false) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 632: `                let entry = entry?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 633: `                let path = entry.path();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 634: `                if path.is_file() && should_include(path, allowed_exts) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 635: `                    files.push(path.to_path_buf());`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 636: `                }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 637: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 638: `        } else {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 639: `            return Err(anyhow!("Input path is not a file or directory: {:?}", p));`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 640: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 641: `    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 642: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 643: `    files.sort();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 644: `    files.dedup();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 645: `    Ok(files)`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 646: `}`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 647: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 648: `#[inline]`
- **What it does:** This is an attribute (a little instruction to Rust and/or a library).
- **Why it is needed:** Attributes are commonly used to auto-generate code or add metadata (here mostly for the command-line parser).
- **If removed/changed:** If removed or changed, the related library features may stop working (for example CLI parsing), and the code may not compile.


##### Line 649: `fn should_include(path: &Path, allowed_exts: Option<&HashSet<String>>) -> bool {`
- **What it does:** This starts defining a function, which is a named block of code you can run.
- **Why it is needed:** Functions keep code organized and reusable, and they make the main flow easier to follow.
- **If removed/changed:** If removed, any place that calls this function will not compile; you would need to inline its logic elsewhere.


##### Line 650: `    let Some(set) = allowed_exts else {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 651: `        return true;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 652: `    };`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 653: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 654: `    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 655: `        return false;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 656: `    };`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 657: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 658: `    set.contains(&ext.to_ascii_lowercase())`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 659: `}`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 660: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 661: `fn process_file(decoder: &DerDecoder, root_type: &str, in_path: &Path, out_dir: &Path) -> Result<usize> {`
- **What it does:** This starts defining a function, which is a named block of code you can run.
- **Why it is needed:** Functions keep code organized and reusable, and they make the main flow easier to follow.
- **If removed/changed:** If removed, any place that calls this function will not compile; you would need to inline its logic elsewhere.


##### Line 662: `    let file = File::open(in_path).with_context(|| format!("Failed to open input file {:?}", in_path))?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 663: `    let mmap = unsafe { Mmap::map(&file)? };`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 664: `    let data: &[u8] = &mmap;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 665: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 666: `    if data.is_empty() {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 667: `        return Ok(0);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 668: `    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 669: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 670: `    let file_name = in_path`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 671: `        .file_name()`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 672: `        .ok_or_else(|| anyhow!("Input path has no filename: {:?}", in_path))?`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 673: `        .to_string_lossy()`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 674: `        .to_string();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 675: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 676: `    let out_path = out_dir.join(format!("{}.jsonl", file_name));`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 677: `    let out_file = File::create(&out_path).with_context(|| format!("Failed to create output file {:?}", out_path))?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 678: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 679: `    // Bigger buffer helps a lot when JSONL is large`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 680: `    let mut writer = BufWriter::with_capacity(64 * 1024 * 1024, out_file);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 681: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 682: `    // per-file reusable hex buffer (critical for speed)`
- **What it does:** This is a comment. Rust ignores comments when running the program.
- **Why it is needed:** It explains the code for humans.
- **If removed/changed:** If removed, the program still works the same, but future readers lose helpful explanation.


##### Line 683: `    let mut hex_scratch: Vec<u8> = Vec::with_capacity(8 * 1024 * 1024);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 684: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 685: `    let mut offset = 0usize;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 686: `    let mut count = 0usize;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 687: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 688: `    let use_auto = root_type.eq_ignore_ascii_case("auto") || root_type.is_empty();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 689: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 690: `    while offset < data.len() {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 691: `        let (tlv, new_off) = match decoder.parse_tlv(data, offset) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 692: `            Some(t) => t,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 693: `            None => break,`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 694: `        };`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 695: `        if new_off <= offset {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 696: `            break;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 697: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 698: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 699: `        if use_auto {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 700: `            decoder.write_auto_record(&tlv, &mut writer, &mut hex_scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 701: `        } else {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 702: `            decoder.write_root_tlv_with_type(&tlv, root_type, &mut writer, &mut hex_scratch)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 703: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 704: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 705: `        writer.write_all(b"\n")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 706: `        offset = new_off;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 707: `        count += 1;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 708: `    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 709: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 710: `    writer.flush()?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 711: `    Ok(count)`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 712: `}`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 713: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 714: `fn main() -> Result<()> {`
- **What it does:** This starts defining a function, which is a named block of code you can run.
- **Why it is needed:** Functions keep code organized and reusable, and they make the main flow easier to follow.
- **If removed/changed:** If removed, any place that calls this function will not compile; you would need to inline its logic elsewhere.


##### Line 715: `    let cli = Cli::parse();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 716: `    let overall_start = Instant::now();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 717: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 718: `    let allowed_exts: Option<HashSet<String>> = cli.ext.as_ref().map(|s| {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 719: `        s.split(',')`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 720: `            .map(|x| x.trim().trim_start_matches('.').to_ascii_lowercase())`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 721: `            .filter(|x| !x.is_empty())`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 722: `            .collect()`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 723: `    });`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 724: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 725: `    let schema_text =`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 726: `        std::fs::read_to_string(&cli.schema).with_context(|| format!("Failed to read schema file {:?}", cli.schema))?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 727: `    let schema = Asn1Schema::parse(&schema_text)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 728: `    let decoder = DerDecoder::new(schema);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 729: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 730: `    std::fs::create_dir_all(&cli.output_dir)?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 731: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 732: `    let mut root_type = cli.root_type.clone();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 733: `    if !root_type.eq_ignore_ascii_case("auto") && !decoder.schema.knows_type(&root_type) {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 734: `        eprintln!(`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 735: `            "WARNING: root-type '{}' does not appear in parsed schema. Falling back to auto mode.",`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 736: `            root_type`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 737: `        );`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 738: `        root_type = "auto".to_string();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 739: `    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 740: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 741: `    let input_files = expand_inputs(&cli.inputs, allowed_exts.as_ref())`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 742: `        .with_context(|| "Failed to expand input files/directories")?;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 743: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 744: `    if input_files.is_empty() {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 745: `        eprintln!("No input files found.");`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 746: `        return Ok(());`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 747: `    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 748: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 749: `    println!("Found {} input files", input_files.len());`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 750: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 751: `    let out_dir = cli.output_dir.clone();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 752: `    let results: Vec<(PathBuf, Result<usize>)> = input_files`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 753: `        .par_iter()`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 754: `        .map(|p| (p.clone(), process_file(&decoder, &root_type, p, &out_dir)))`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 755: `        .collect();`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 756: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 757: `    let mut total_records = 0usize;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 758: `    for (path, res) in results {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 759: `        match res {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 760: `            Ok(count) => {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 761: `                total_records += count;`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 762: `                println!("Decoded {} records from {:?}", count, path);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 763: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 764: `            Err(e) => {`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 765: `                eprintln!("Decoding failed for {:?}: {:#}", path, e);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 766: `            }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 767: `        }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 768: `    }`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 769: ``
- **What it does:** This is a blank line (an empty line).
- **Why it is needed:** It separates blocks of code so the file is easier to read.
- **If removed/changed:** If removed, the program still works the same, but the code becomes harder to read.


##### Line 770: `    println!("Total decoded records: {}", total_records);`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 771: `    println!("Total elapsed wall time: {:.3} s", overall_start.elapsed().as_secs_f64());`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 772: `    Ok(())`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).


##### Line 773: `}`
- **What it does:** This line is part of the program logic.
- **Why it is needed:** It helps the program move toward its goal: reading DER data and writing JSONL output.
- **If removed/changed:** If removed or changed, the program may not compile, or it may behave differently (for example decoding incorrectly or producing different output).

