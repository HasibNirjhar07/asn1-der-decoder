
---

# 🦀 ASN.1 DER Decoder → JSONL (Rust)

A **high-performance, schema-based ASN.1 DER decoder written in Rust** that converts binary DER records into **JSON Lines (JSONL)** format.

This tool is designed for:

* speed (memory-mapped I/O + parallel decoding)
* correctness (schema-driven decoding)
* usability (human-readable JSON output)
* large files and production workloads

---

## ✨ Features

* 🚀 **Ultra-fast decoding**

  * Memory-mapped input files
  * Parallel file processing with Rayon
* 📐 **Schema-based decoding**

  * Supports `SEQUENCE`, `SET`, `CHOICE`, and primitive ASN.1 types
  * Type aliases are automatically resolved
* 🧾 **JSON Lines output**

  * One decoded record per line
  * Ideal for streaming, indexing, and big-data tools
* 🔢 **Hex-only values**

  * All values are preserved exactly as encoded (no lossy decimal conversion)
* 📁 **Batch processing**

  * Decode entire directories recursively
  * Optional file-extension filtering
* 🧠 **Auto-decode mode**

  * Decode records even when the root type is unknown

---

## 📦 Installation

### Prerequisites

* Rust **1.70+**
* Cargo (comes with Rust)

Install Rust from:
👉 [https://www.rust-lang.org/tools/install](https://www.rust-lang.org/tools/install)

### Build from source

```bash
git clone https://github.com/HasibNirjhar07/asn1-der-decoder.git
cd asn1-der-decoder
cargo build --release
```

The binary will be located at:

```text
target/release/asn1-der-decoder
```

---

## 🚀 Usage

```bash
asn1-der-decoder \
  --schema schema.asn1 \
  --root-type PGWRecord \
  --output-dir output \
  input.dat
```

### Required arguments

| Argument       | Description                                    |
| -------------- | ---------------------------------------------- |
| `--schema`     | Path to the ASN.1 schema file                  |
| `--root-type`  | Root ASN.1 type name (or `auto`)               |
| `--output-dir` | Directory where `.jsonl` files will be written |
| `inputs`       | One or more input files or directories         |

---

## 🔧 Optional flags

```bash
--ext dat,bin
```

Decode only files matching specific extensions.

---

## 🧠 Auto mode

If you don’t know the root ASN.1 type:

```bash
asn1-der-decoder \
  --schema schema.asn1 \
  --root-type auto \
  --output-dir output \
  input_dir/
```

The decoder will:

* infer the structure when possible
* fall back to raw hex output when necessary
* never discard data

---

## 📤 Output format

* Output files are written as **JSON Lines (`.jsonl`)**
* Each line represents **one ASN.1 record**
* Example:

```json
{"recordType":"01","subscriberId":"9f23ab...","timestamp":"0018c4e2"}
```

### Why JSONL?

* Stream-friendly
* Easy to process with:

  * `jq`
  * Spark
  * BigQuery
  * Python / Pandas
  * Log pipelines

---

## 📚 Documentation

A **complete, beginner-friendly, line-by-line explanation** of the code is provided as a LaTeX document:

* Explains:

  * ASN.1 TLV decoding
  * Schema parsing
  * JSON generation
  * Data flow through the program
* Suitable for:

  * Non-Rust developers
  * New contributors
  * Code reviews and audits

📄 See: `docs/main.tex` (or compiled PDF)

---

## 🏗️ Project structure

```text
.
├── src/
│   └── main.rs        # Core decoder implementation
├── docs/
│   └── main.tex       # Beginner-friendly LaTeX documentation
├── Cargo.toml
├── README.md
```

---

## ⚙️ Performance notes

* Uses **memory-mapped I/O** for zero-copy reads
* Uses **buffered output writers** for large JSON files
* Uses **parallel decoding** for multiple input files
* Designed for very large ASN.1 datasets

---

## 🛡️ Safety & correctness

* Defensive bounds checking during TLV parsing
* Graceful handling of malformed records
* No unsafe decoding assumptions
* Raw bytes are always preserved as hex when decoding is ambiguous

---

