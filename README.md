

---

# 🦀 ASN.1 DER Decoder → JSONL (Rust)

A **high-performance, schema-based ASN.1 DER decoder written in Rust** that converts binary DER records into **JSON Lines (JSONL)** format.

This tool is designed for:

* ⚡ **Speed** (memory-mapped I/O + parallel decoding + **zero-latency schema loading**)
* ✅ **Correctness** (strict schema-driven decoding)
* 👁️ **Usability** (human-readable JSON output)
* 🏭 **Production workloads** (batch processing millions of records)

---

## ✨ Features

* 🚀 **Ultra-fast decoding**
* Memory-mapped input files for zero-copy reads
* Parallel file processing with Rayon


* 💾 **Schema Compilation (New!)**
* **Parse once, run forever:** Compile text ASN.1 schemas into optimized binary format.
* **Instant startup:** Skip regex parsing overhead for batch jobs (up to **80% faster** startup on small batches).


* 📐 **Schema-based decoding**
* Supports `SEQUENCE`, `SET`, `CHOICE`, `COMPONENTS OF`, and primitive types.
* Auto-resolves `IMPLICIT`/`EXPLICIT` tags and type aliases.


* 🧾 **JSON Lines output**
* One decoded record per line.
* Ideal for streaming to Spark, BigQuery, or log pipelines.


* 🔢 **Hex-only values**
* Values are preserved exactly as encoded (no lossy decimal conversion).



---

## 📦 Installation

### Prerequisites

* Rust **1.70+**
* Cargo (comes with Rust)

### Build from source

```bash
git clone https://github.com/HasibNirjhar07/asn1-der-decoder.git
cd asn1-der-decoder
cargo build --release

```

The binary will be located at: `target/release/asn1-der-decoder`

---

## 🚀 Usage

There are two ways to run the decoder: **Direct Mode** (standard) and **Compiled Mode** (optimized).

### 1️⃣ Standard Mode (Parse Text Schema)

Good for one-off runs or testing changes to the schema.

```bash
asn1-der-decoder \
  --schema schema.asn \
  --root-type CallEventRecord \
  --output-dir output \
  input.dat

```

### 2️⃣ Optimized Mode (Compile & Load) ⚡

Recommended for production batch scripts. Eliminates schema parsing overhead.

**Step A: Compile the Schema (Run Once)**

```bash
asn1-der-decoder \
  --schema schema.asn \
  --compile-schema schema.bin \
  --root-type CallEventRecord \
  --output-dir dummy_output \
  input.dat

```

**Step B: Fast Decode (Run Many Times)**

```bash
asn1-der-decoder \
  --load-compiled schema.bin \
  --root-type CallEventRecord \
  --output-dir output \
  input_directory/

```

---

## ⚙️ Command Line Arguments

| Argument | Description | Required? |
| --- | --- | --- |
| `--schema` | Path to the text ASN.1 schema file (`.asn`). | Yes* |
| `--load-compiled` | Path to a pre-compiled binary schema (`.bin`). | Yes* |
| `--compile-schema` | Path to **save** the compiled binary schema. | No |
| `--root-type` | Root ASN.1 type name to decode (e.g., `CallEventRecord`). | Yes |
| `--output-dir` | Directory where `.jsonl` files will be written. | Yes |
| `inputs` | One or more input files or directories. | Yes |

**You must provide either `--schema` OR `--load-compiled`.*

### Optional flags

```bash
--ext dat,bin

```

Decode only files matching specific extensions (e.g., ignore `.tmp` files).

---

## 📊 Performance Notes

### Why Compile Schemas?

Parsing complex ASN.1 text definitions using Regex is CPU-intensive. By saving the parsed structure to disk (`--compile-schema`), you can reuse it later.

| File Type | Records | Text Parse Time | Binary Load Time | Improvement |
| --- | --- | --- | --- | --- |
| **Large (GGSN)** | 14k+ | 0.257s | **0.226s** | ~12% Faster |
| **Small (TAP)** | 1 | 0.039s | **0.015s** | ~61% Faster |
| **Tiny (NRTRDE)** | 1 | 0.021s | **0.004s** | **~81% Faster** |

*Benchmarks based on production CDR data.*

---

## 📤 Output Format

* Output files are written as **JSON Lines (`.jsonl`)**
* Each line represents **one ASN.1 record**

**Example:**

```json
{
  "recordType": "01",
  "servedIMSI": "9f23ab...",
  "chargingID": "0018c4e2",
  "list-Of-Traffic-Volumes": [
    { "dataVolumeGPRSUplink": "000004d2" },
    { "dataVolumeGPRSDownlink": "0000162e" }
  ]
}

```

---

## 🏗️ Project Structure

```text
.
├── src/
│   └── main.rs        # Core decoder implementation
├── docs/
│   └── main.tex       # Documentation
├── Cargo.toml         # Dependencies (Serde, Bincode, Rayon, etc.)
├── README.md

```

---

## 🛡️ Safety & Correctness

* **Defensive Parsing:** Bounds checking prevents panics on malformed data.
* **Fallbacks:** ambiguous or unknown tags are preserved as `"unknown_tag_XX": "HEX_VALUE"` rather than crashing.
* **Concurrency:** Thread-safe processing using Rust's ownership model and Rayon.

---
