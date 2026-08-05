# Transcriba Core Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Rust library plus thin CLI that turns any common audio file into a readable `.docx` and `.pdf` transcript, fully offline.

**Architecture:** Cargo workspace with a `transcriba-core` library and a `transcriba-cli` binary. The library exposes five independent modules — `decode`, `model_store`, `transcribe`, `reflow`, `render` — each a pure function over data where possible. Plan 2 adds the Tauri GUI on top of this same library, so nothing here may depend on Tauri.

**Tech Stack:** Rust 2021, `symphonia` (decode), `rubato` (resample), `audiopus` (Opus), `whisper-rs` (transcribe), `docx-rs` (DOCX), `genpdf` (PDF), `clap` (CLI).

## Global Constraints

- Library must not depend on Tauri or any GUI crate — Plan 2 consumes it as-is.
- All processing local. No network calls except the one model download in `model_store`.
- Model: `ggml-large-v3-turbo-q5_0.bin`, ~574MB, verified by size and `ggml` magic bytes.
- Model cache: `~/.local/share/transcriba/models/` (Linux), `%LOCALAPPDATA%\transcriba\models\` (Windows). Override via `TRANSCRIBA_MODEL_PATH`.
- Default thread count: `num_cpus - 2`, minimum 1.
- Reflow parameters, copied verbatim from the validated Python: paragraph break on gap `> 1.0s` or length `> 700` chars; section heading every `600s`; sentence close on `[.!?…]` optionally followed by `'`, `"`, `)`, or `]`.
- Output written beside the input file; existing files get a numeric suffix, never overwritten.
- Target platforms: Linux and Windows. CI must run both by end of Plan 2; Plan 1 CI is Linux-only.
- Tests must never require the 574MB model. Integration tests use `ggml-tiny`.

---

### Task 1: Workspace scaffold and CI

**Files:**
- Create: `Cargo.toml`, `.gitignore`, `crates/core/Cargo.toml`, `crates/core/src/lib.rs`, `crates/cli/Cargo.toml`, `crates/cli/src/main.rs`, `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: nothing
- Produces: workspace members `transcriba-core` and `transcriba-cli`; `transcriba_core::VERSION: &str`

- [ ] **Step 1: Create the workspace root**

`Cargo.toml`:

```toml
[workspace]
members = ["crates/core", "crates/cli"]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT"
```

- [ ] **Step 2: Create `.gitignore`**

```
/target
/crates/*/target
*.docx
*.pdf
/tests/fixtures/*.bin
```

- [ ] **Step 3: Create the core crate**

`crates/core/Cargo.toml`:

```toml
[package]
name = "transcriba-core"
version.workspace = true
edition.workspace = true

[dependencies]

[dev-dependencies]
```

`crates/core/src/lib.rs`:

```rust
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_not_empty() {
        assert!(!super::VERSION.is_empty());
    }
}
```

- [ ] **Step 4: Create the CLI crate**

`crates/cli/Cargo.toml`:

```toml
[package]
name = "transcriba-cli"
version.workspace = true
edition.workspace = true

[[bin]]
name = "transcriba"
path = "src/main.rs"

[dependencies]
transcriba-core = { path = "../core" }
```

`crates/cli/src/main.rs`:

```rust
fn main() {
    println!("transcriba {}", transcriba_core::VERSION);
}
```

- [ ] **Step 5: Verify the workspace builds and tests pass**

Run: `cargo test --workspace`
Expected: PASS, 1 test (`version_is_not_empty`).

- [ ] **Step 6: Verify dependency API versions before later tasks depend on them**

Run: `cargo add --dry-run --package transcriba-core symphonia rubato whisper-rs docx-rs genpdf`

Record the resolved versions in a comment at the top of `crates/core/src/lib.rs`. This matters because `rubato` renamed its resamplers — this plan is written against the API where the FFT resampler is `rubato::Fft<f32>`, not the older `SincFixedIn`. If the resolved version exposes the older names, adapt Task 5's code accordingly and note it.

- [ ] **Step 7: Add Linux CI**

`.github/workflows/ci.yml`:

```yaml
name: ci
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt
      - run: cargo fmt --all -- --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace
```

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml .gitignore crates .github
git commit -m "feat: scaffold cargo workspace and Linux CI"
```

---

### Task 2: Reflow module

The highest-value first task: pure logic, no dependencies, and a direct port of code already validated on a real 58-minute recording. Good Rust practice on structs, enums, iterators and tests.

**Files:**
- Create: `crates/core/src/reflow.rs`
- Modify: `crates/core/src/lib.rs`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `pub struct Cue { pub start: f64, pub end: f64, pub text: String }`
  - `pub enum Block { Heading { from: f64, to: f64 }, Para { start: f64, text: String } }`
  - `pub fn reflow(cues: &[Cue]) -> Vec<Block>`
  - `pub fn format_timestamp(seconds: f64) -> String`

- [ ] **Step 1: Write the failing tests**

`crates/core/src/reflow.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn cue(start: f64, end: f64, text: &str) -> Cue {
        Cue { start, end, text: text.to_string() }
    }

    #[test]
    fn merges_cues_into_one_sentence_until_terminal_punctuation() {
        let cues = vec![
            cue(0.0, 1.0, "Muito boa tarde"),
            cue(1.0, 2.0, "a todos."),
        ];
        let blocks = reflow(&cues);
        let paras: Vec<_> = blocks.iter().filter_map(|b| match b {
            Block::Para { text, .. } => Some(text.as_str()),
            _ => None,
        }).collect();
        assert_eq!(paras, vec!["Muito boa tarde a todos."]);
    }

    #[test]
    fn breaks_paragraph_on_pause_longer_than_one_second() {
        let cues = vec![
            cue(0.0, 1.0, "Primeira frase."),
            cue(3.0, 4.0, "Segunda frase."),
        ];
        let paras = para_texts(&reflow(&cues));
        assert_eq!(paras.len(), 2);
    }

    #[test]
    fn keeps_sentences_together_when_pause_is_short() {
        let cues = vec![
            cue(0.0, 1.0, "Primeira frase."),
            cue(1.2, 2.0, "Segunda frase."),
        ];
        let paras = para_texts(&reflow(&cues));
        assert_eq!(paras.len(), 1);
    }

    #[test]
    fn breaks_paragraph_when_over_seven_hundred_chars() {
        let long = "palavra ".repeat(100);
        let cues = vec![
            cue(0.0, 1.0, &format!("{}.", long)),
            cue(1.1, 2.0, "Curta."),
        ];
        let paras = para_texts(&reflow(&cues));
        assert_eq!(paras.len(), 2);
    }

    #[test]
    fn emits_a_heading_every_ten_minutes() {
        let cues = vec![
            cue(0.0, 1.0, "Inicio."),
            cue(650.0, 651.0, "Depois."),
        ];
        let headings: Vec<_> = reflow(&cues).into_iter().filter(|b| matches!(b, Block::Heading { .. })).collect();
        assert_eq!(headings.len(), 2);
    }

    #[test]
    fn unterminated_trailing_cues_still_produce_a_paragraph() {
        // Production audio produced long stretches averaging 2.3 words per cue
        // with no sentence terminator at all. Nothing may be dropped.
        let cues = vec![
            cue(0.0, 0.5, "que tem toda"),
            cue(0.5, 1.0, "a legitimidade"),
            cue(1.0, 1.5, "para tal"),
        ];
        let paras = para_texts(&reflow(&cues));
        assert_eq!(paras, vec!["que tem toda a legitimidade para tal"]);
    }

    #[test]
    fn empty_input_produces_no_blocks() {
        assert!(reflow(&[]).is_empty());
    }

    #[test]
    fn formats_timestamps_with_and_without_hours() {
        assert_eq!(format_timestamp(65.0), "1:05");
        assert_eq!(format_timestamp(3725.0), "1:02:05");
    }

    fn para_texts(blocks: &[Block]) -> Vec<String> {
        blocks.iter().filter_map(|b| match b {
            Block::Para { text, .. } => Some(text.clone()),
            _ => None,
        }).collect()
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --package transcriba-core reflow`
Expected: FAIL — `cannot find type Cue`, `cannot find function reflow`.

- [ ] **Step 3: Implement the module**

Prepend to `crates/core/src/reflow.rs`:

```rust
//! Turns whisper's short subtitle cues into readable paragraphs.
//!
//! Ported from a Python implementation validated on a 58m30s Portuguese
//! recording: 2553 cues collapsed to 141 sentences and 58 paragraphs.

const PAUSE_BREAK_SECS: f64 = 1.0;
const MAX_PARA_CHARS: usize = 700;
const SECTION_SECS: f64 = 600.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading { from: f64, to: f64 },
    Para { start: f64, text: String },
}

struct Sentence {
    start: f64,
    end: f64,
    text: String,
}

/// True when `text` ends a sentence, allowing one trailing closing quote or bracket.
fn closes_sentence(text: &str) -> bool {
    let trimmed = text.trim_end_matches(['\'', '"', ')', ']']);
    matches!(trimmed.chars().last(), Some('.') | Some('!') | Some('?') | Some('…'))
}

fn to_sentences(cues: &[Cue]) -> Vec<Sentence> {
    let mut out = Vec::new();
    let mut buf: Vec<&str> = Vec::new();
    let mut start = None;
    let mut end = 0.0;

    for cue in cues {
        if start.is_none() {
            start = Some(cue.start);
        }
        end = cue.end;
        buf.push(cue.text.trim());
        if closes_sentence(&cue.text) {
            out.push(Sentence { start: start.take().unwrap(), end, text: buf.join(" ") });
            buf.clear();
        }
    }
    // Trailing cues with no terminator must not be dropped.
    if let Some(s) = start {
        out.push(Sentence { start: s, end, text: buf.join(" ") });
    }
    out
}

pub fn reflow(cues: &[Cue]) -> Vec<Block> {
    let sentences = to_sentences(cues);
    let mut blocks = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut para_start = 0.0;
    let mut prev_end: Option<f64> = None;
    let mut section: i64 = -1;

    // Captures nothing, so it needs no `mut` — `-D warnings` in CI rejects an unused mut.
    let flush = |blocks: &mut Vec<Block>, current: &mut Vec<String>, start: f64| {
        if !current.is_empty() {
            blocks.push(Block::Para { start, text: current.join(" ") });
            current.clear();
        }
    };

    for s in &sentences {
        let gap = prev_end.map_or(0.0, |p| s.start - p);
        let too_long = current.iter().map(|t| t.len()).sum::<usize>() > MAX_PARA_CHARS;

        if !current.is_empty() && (gap > PAUSE_BREAK_SECS || too_long) {
            flush(&mut blocks, &mut current, para_start);
        }
        if current.is_empty() {
            para_start = s.start;
            let block = (s.start / SECTION_SECS) as i64;
            if block != section {
                section = block;
                let from = block as f64 * SECTION_SECS;
                blocks.push(Block::Heading { from, to: from + SECTION_SECS });
            }
        }
        current.push(s.text.clone());
        prev_end = Some(s.end);
    }
    flush(&mut blocks, &mut current, para_start);
    blocks
}

pub fn format_timestamp(seconds: f64) -> String {
    let total = seconds as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}
```

Add to `crates/core/src/lib.rs`:

```rust
pub mod reflow;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --package transcriba-core reflow`
Expected: PASS, 8 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/reflow.rs crates/core/src/lib.rs
git commit -m "feat(core): reflow whisper cues into readable paragraphs"
```

---

### Task 3: DOCX rendering

**Files:**
- Create: `crates/core/src/render/mod.rs`, `crates/core/src/render/docx.rs`
- Modify: `crates/core/src/lib.rs`, `crates/core/Cargo.toml`

**Interfaces:**
- Consumes: `reflow::{Block, format_timestamp}`
- Produces:
  - `pub struct DocumentMeta { pub title: String, pub duration: f64, pub backend: String }`
  - `pub fn render_docx(blocks: &[Block], meta: &DocumentMeta) -> Result<Vec<u8>, RenderError>`
  - `pub enum RenderError { Docx(String), Pdf(String) }`

- [ ] **Step 1: Add the dependency**

In `crates/core/Cargo.toml` under `[dependencies]`:

```toml
docx-rs = "0.4"
```

In `[dev-dependencies]`:

```toml
zip = "2"
```

- [ ] **Step 2: Write the failing test**

`crates/core/src/render/docx.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflow::Block;
    use std::io::Read;

    fn meta() -> DocumentMeta {
        DocumentMeta {
            title: "Reuniao de teste".to_string(),
            duration: 3510.88,
            backend: "CPU (12 threads)".to_string(),
        }
    }

    /// Unzips the .docx and returns the raw document.xml. Byte-level proof the
    /// text survived, rather than trusting the builder silently produced content.
    fn document_xml(bytes: &[u8]) -> String {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("valid zip");
        let mut file = archive.by_name("word/document.xml").expect("has document.xml");
        let mut xml = String::new();
        file.read_to_string(&mut xml).expect("readable xml");
        xml
    }

    #[test]
    fn writes_paragraph_text_into_the_document() {
        let blocks = vec![Block::Para { start: 5.0, text: "Muito boa tarde a todos.".into() }];
        let bytes = render_docx(&blocks, &meta()).expect("renders");
        let xml = document_xml(&bytes);
        assert!(xml.contains("Muito boa tarde a todos."));
    }

    #[test]
    fn includes_a_timestamp_marker_per_paragraph() {
        let blocks = vec![Block::Para { start: 65.0, text: "Texto.".into() }];
        let xml = document_xml(&render_docx(&blocks, &meta()).unwrap());
        assert!(xml.contains("[1:05]"));
    }

    #[test]
    fn includes_headings_and_title_and_backend() {
        let blocks = vec![
            Block::Heading { from: 0.0, to: 600.0 },
            Block::Para { start: 1.0, text: "Texto.".into() },
        ];
        let xml = document_xml(&render_docx(&blocks, &meta()).unwrap());
        assert!(xml.contains("0:00"));
        assert!(xml.contains("Reuniao de teste"));
        assert!(xml.contains("CPU (12 threads)"));
    }

    #[test]
    fn preserves_portuguese_diacritics() {
        let blocks = vec![Block::Para { start: 0.0, text: "Assembleia de Freguesia, aprovação.".into() }];
        let xml = document_xml(&render_docx(&blocks, &meta()).unwrap());
        assert!(xml.contains("aprovação"));
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test --package transcriba-core render::docx`
Expected: FAIL — `cannot find function render_docx`.

- [ ] **Step 4: Implement**

`crates/core/src/render/mod.rs`:

```rust
pub mod docx;
pub mod pdf;

/// Metadata rendered into the document header.
#[derive(Debug, Clone)]
pub struct DocumentMeta {
    pub title: String,
    pub duration: f64,
    pub backend: String,
}

#[derive(Debug)]
pub enum RenderError {
    Docx(String),
    Pdf(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::Docx(m) => write!(f, "could not build the Word document: {m}"),
            RenderError::Pdf(m) => write!(f, "could not build the PDF: {m}"),
        }
    }
}

impl std::error::Error for RenderError {}

/// Shared disclaimer. Machine output is never presented as verified.
pub fn subtitle(meta: &DocumentMeta) -> String {
    format!(
        "Duração {} · transcrição automática ({}) · texto não revisto — \
verificar nomes, valores e votações contra o áudio original.",
        crate::reflow::format_timestamp(meta.duration),
        meta.backend,
    )
}
```

Prepend to `crates/core/src/render/docx.rs`:

```rust
use super::{subtitle, DocumentMeta, RenderError};
use crate::reflow::{format_timestamp, Block};
use docx_rs::*;

pub fn render_docx(blocks: &[Block], meta: &DocumentMeta) -> Result<Vec<u8>, RenderError> {
    let mut doc = Docx::new()
        .add_paragraph(
            Paragraph::new()
                .add_run(Run::new().add_text(&meta.title).bold().size(36)),
        )
        .add_paragraph(
            Paragraph::new().add_run(Run::new().add_text(subtitle(meta)).size(18)),
        );

    for block in blocks {
        doc = match block {
            Block::Heading { from, to } => doc.add_paragraph(
                Paragraph::new().add_run(
                    Run::new()
                        .add_text(format!("{} – {}", format_timestamp(*from), format_timestamp(*to)))
                        .bold()
                        .size(24),
                ),
            ),
            Block::Para { start, text } => doc.add_paragraph(
                Paragraph::new()
                    .add_run(Run::new().add_text(format!("[{}] ", format_timestamp(*start))).size(16))
                    .add_run(Run::new().add_text(text).size(22)),
            ),
        };
    }

    let mut buf = Vec::new();
    doc.build()
        .pack(std::io::Cursor::new(&mut buf))
        .map_err(|e| RenderError::Docx(e.to_string()))?;
    Ok(buf)
}
```

Add to `crates/core/src/lib.rs`:

```rust
pub mod render;
```

In `crates/core/src/render/mod.rs`, declare only the docx module for now:

```rust
pub mod docx;
```

Task 4 adds `pub mod pdf;` alongside it. Do not create `pdf.rs` yet — an empty module declared here would not compile.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --package transcriba-core render::docx`
Expected: PASS, 4 tests. If `pack` has a different name in the resolved `docx-rs` version, check `Docx::build()`'s returned `XMLDocx` type in the docs and use its write method.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/render crates/core/src/lib.rs crates/core/Cargo.toml
git commit -m "feat(core): render transcript to docx"
```

---

### Task 4: PDF rendering

`genpdf`'s builtin fonts only cover Windows-1252. Portuguese fits, but bundling Liberation fonts makes output identical on Linux and Windows and keeps other languages working.

**Files:**
- Create: `crates/core/src/render/pdf.rs` (replacing the stub), `assets/fonts/README.md`
- Modify: `crates/core/Cargo.toml`

**Interfaces:**
- Consumes: `render::{DocumentMeta, RenderError, subtitle}`, `reflow::{Block, format_timestamp}`
- Produces: `pub fn render_pdf(blocks: &[Block], meta: &DocumentMeta, font_dir: &std::path::Path) -> Result<Vec<u8>, RenderError>`

- [ ] **Step 1: Add the dependency**

In `crates/core/Cargo.toml` under `[dependencies]`:

```toml
genpdf = "0.2"
```

In `[dev-dependencies]`:

```toml
pdf-extract = "0.7"
```

- [ ] **Step 2: Vendor the fonts**

```bash
mkdir -p assets/fonts
cp /usr/share/fonts/liberation/LiberationSerif-{Regular,Bold,Italic,BoldItalic}.ttf assets/fonts/
```

If that path does not exist on this machine, find them with `fc-list | grep -i liberationserif`. `genpdf::fonts::from_files` requires all four variants with this exact naming convention.

`assets/fonts/README.md`:

```markdown
# Bundled fonts

Liberation Serif, SIL Open Font License 1.1. Bundled so PDF output is identical
on Linux and Windows, and so glyphs outside Windows-1252 render correctly.
`genpdf::fonts::from_files` requires all four variants present.
```

- [ ] **Step 3: Write the failing test**

Append to `crates/core/src/render/pdf.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflow::Block;

    fn meta() -> DocumentMeta {
        DocumentMeta {
            title: "Reuniao de teste".to_string(),
            duration: 3510.88,
            backend: "CPU (12 threads)".to_string(),
        }
    }

    fn font_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts")
    }

    #[test]
    fn produces_a_valid_pdf_header() {
        let blocks = vec![Block::Para { start: 0.0, text: "Texto.".into() }];
        let bytes = render_pdf(&blocks, &meta(), &font_dir()).expect("renders");
        assert!(bytes.starts_with(b"%PDF-"), "output must be a PDF");
    }

    #[test]
    fn paragraph_text_is_extractable() {
        let blocks = vec![Block::Para { start: 0.0, text: "Muito boa tarde a todos.".into() }];
        let bytes = render_pdf(&blocks, &meta(), &font_dir()).unwrap();
        let text = pdf_extract::extract_text_from_mem(&bytes).expect("extractable");
        assert!(text.contains("Muito boa tarde"));
    }

    #[test]
    fn long_transcripts_keep_all_content() {
        // 300 paragraphs cannot fit one page, so this also proves pagination did
        // not silently drop the overflow — the failure mode that matters.
        let blocks: Vec<_> = (0..300)
            .map(|i| Block::Para { start: i as f64, text: format!("Paragrafo numero {i} deste teste.") })
            .collect();
        let bytes = render_pdf(&blocks, &meta(), &font_dir()).unwrap();
        let text = pdf_extract::extract_text_from_mem(&bytes).expect("extractable");
        assert!(text.contains("Paragrafo numero 0 "), "first paragraph missing");
        assert!(text.contains("Paragrafo numero 299 "), "last paragraph missing");
    }
}
```

- [ ] **Step 3a: Run test to verify it fails**

Run: `cargo test --package transcriba-core render::pdf`
Expected: FAIL — `cannot find function render_pdf`.

- [ ] **Step 4: Implement**

Replace the stub in `crates/core/src/render/pdf.rs` with this, keeping the test module:

```rust
use super::{subtitle, DocumentMeta, RenderError};
use crate::reflow::{format_timestamp, Block};
use genpdf::{elements, fonts, style, Document, SimplePageDecorator};
use std::path::Path;

pub fn render_pdf(
    blocks: &[Block],
    meta: &DocumentMeta,
    font_dir: &Path,
) -> Result<Vec<u8>, RenderError> {
    let family = fonts::from_files(font_dir, "LiberationSerif", None)
        .map_err(|e| RenderError::Pdf(format!("could not load fonts from {font_dir:?}: {e}")))?;

    let mut doc = Document::new(family);
    doc.set_title(&meta.title);
    doc.set_minimal_conformance();

    let mut decorator = SimplePageDecorator::new();
    decorator.set_margins(20);
    doc.set_page_decorator(decorator);

    doc.push(elements::Paragraph::new(&meta.title).styled(style::Style::new().bold().with_font_size(18)));
    doc.push(elements::Paragraph::new(subtitle(meta)).styled(style::Style::new().with_font_size(8)));
    doc.push(elements::Break::new(1));

    for block in blocks {
        match block {
            Block::Heading { from, to } => {
                doc.push(elements::Break::new(1));
                doc.push(
                    elements::Paragraph::new(format!(
                        "{} – {}",
                        format_timestamp(*from),
                        format_timestamp(*to)
                    ))
                    .styled(style::Style::new().bold().with_font_size(12)),
                );
            }
            Block::Para { start, text } => {
                doc.push(
                    elements::Paragraph::new(format!("[{}] {}", format_timestamp(*start), text))
                        .styled(style::Style::new().with_font_size(11)),
                );
            }
        }
    }

    let mut buf = Vec::new();
    doc.render(&mut buf).map_err(|e| RenderError::Pdf(e.to_string()))?;
    Ok(buf)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --package transcriba-core render::pdf`
Expected: PASS, 3 tests.

If `set_minimal_conformance` or `styled` are absent in the resolved `genpdf` version, drop them — they are conveniences, not requirements. **If `genpdf` proves unworkable here, stop and report rather than fighting it:** the spec flags it as a maintenance risk at 0.1.1 with a dormant upstream, and `typst` embedded as a library is the sanctioned fallback.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/render/pdf.rs assets/fonts crates/core/Cargo.toml
git commit -m "feat(core): render transcript to pdf with bundled fonts"
```

---

### Task 5: Audio decoding and resampling

**Files:**
- Create: `crates/core/src/decode.rs`, `tests/fixtures/README.md`
- Modify: `crates/core/src/lib.rs`, `crates/core/Cargo.toml`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `pub const TARGET_RATE: u32 = 16_000;`
  - `pub struct Audio { pub samples: Vec<f32>, pub duration: f64 }`
  - `pub fn decode(path: &std::path::Path) -> Result<Audio, DecodeError>`
  - `pub enum DecodeError { Open(String), UnsupportedCodec(String), Empty, Decode(String) }`

- [ ] **Step 1: Add dependencies**

In `crates/core/Cargo.toml`:

```toml
symphonia = { version = "0.5", features = ["mp3", "aac", "isomp4", "ogg", "vorbis", "flac", "wav", "pcm"] }
rubato = "0.16"
```

- [ ] **Step 2: Create test fixtures**

```bash
mkdir -p tests/fixtures
ffmpeg -y -f lavfi -i "sine=frequency=440:duration=2" -ar 44100 -ac 2 tests/fixtures/tone.mp3
ffmpeg -y -f lavfi -i "sine=frequency=440:duration=2" -ar 48000 -ac 1 tests/fixtures/tone.wav
ffmpeg -y -f lavfi -i "sine=frequency=440:duration=2" -ar 44100 -ac 2 tests/fixtures/tone.m4a
```

`tests/fixtures/README.md`:

```markdown
# Test fixtures

Two-second 440Hz synthetic tones, generated with ffmpeg (see the plan). They
exercise the decode paths — stereo→mono, 44.1k/48k→16k — without shipping real
audio. `ffmpeg` is a developer-only dependency; the app does not use it.
```

Remove `/tests/fixtures/*.bin` from `.gitignore` only if fixtures must be committed; the tone files are small enough to commit and `.bin` refers to models.

- [ ] **Step 3: Write the failing tests**

`crates/core/src/decode.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures").join(name)
    }

    #[test]
    fn decodes_mp3_to_sixteen_khz_mono() {
        let audio = decode(&fixture("tone.mp3")).expect("decodes");
        assert!((audio.duration - 2.0).abs() < 0.2, "duration was {}", audio.duration);
        let expected = (2.0 * TARGET_RATE as f64) as usize;
        let delta = (audio.samples.len() as i64 - expected as i64).abs();
        assert!(delta < TARGET_RATE as i64 / 5, "got {} samples", audio.samples.len());
    }

    #[test]
    fn decodes_wav() {
        let audio = decode(&fixture("tone.wav")).expect("decodes");
        assert!(!audio.samples.is_empty());
    }

    #[test]
    fn decodes_m4a() {
        let audio = decode(&fixture("tone.m4a")).expect("decodes");
        assert!(!audio.samples.is_empty());
    }

    #[test]
    fn samples_are_within_valid_range() {
        let audio = decode(&fixture("tone.wav")).unwrap();
        assert!(audio.samples.iter().all(|s| s.is_finite() && s.abs() <= 1.01));
    }

    #[test]
    fn missing_file_reports_open_error() {
        assert!(matches!(decode(Path::new("/nonexistent.mp3")), Err(DecodeError::Open(_))));
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test --package transcriba-core decode`
Expected: FAIL — `cannot find function decode`.

- [ ] **Step 5: Implement**

Prepend to `crates/core/src/decode.rs`:

```rust
//! Decodes any supported audio file to the 16kHz mono f32 whisper requires.
//!
//! Replaces the `ffmpeg -ar 16000 -ac 1 -c:a pcm_s16le` step of the manual pipeline.

use rubato::{Fft, Resampler};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use std::path::Path;

pub const TARGET_RATE: u32 = 16_000;

#[derive(Debug)]
pub struct Audio {
    pub samples: Vec<f32>,
    pub duration: f64,
}

#[derive(Debug)]
pub enum DecodeError {
    Open(String),
    UnsupportedCodec(String),
    Empty,
    Decode(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Open(m) => write!(f, "could not open the file: {m}"),
            DecodeError::UnsupportedCodec(c) => write!(
                f,
                "this file uses {c} audio, which isn't supported yet. \
Convert it to MP3 and try again."
            ),
            DecodeError::Empty => write!(f, "the file contains no audio"),
            DecodeError::Decode(m) => write!(f, "the audio could not be decoded: {m}"),
        }
    }
}

impl std::error::Error for DecodeError {}

pub fn decode(path: &Path) -> Result<Audio, DecodeError> {
    let file = std::fs::File::open(path).map_err(|e| DecodeError::Open(e.to_string()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| DecodeError::UnsupportedCodec(e.to_string()))?;
    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or(DecodeError::Empty)?;
    let track_id = track.id;
    let source_rate = track.codec_params.sample_rate.unwrap_or(TARGET_RATE);
    let channels = track.codec_params.channels.map_or(1, |c| c.count());

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| DecodeError::UnsupportedCodec(e.to_string()))?;

    let mut mono = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            // Symphonia signals end-of-stream as an IO error.
            Err(SymphoniaError::IoError(_)) => break,
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(DecodeError::Decode(e.to_string())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                // SampleBuffer converts any AudioBufferRef variant to interleaved f32,
                // so we don't match on every sample format ourselves.
                let spec = *buf.spec();
                let mut sb = SampleBuffer::<f32>::new(buf.capacity() as u64, spec);
                sb.copy_interleaved_ref(buf);
                for frame in sb.samples().chunks(channels) {
                    mono.push(frame.iter().sum::<f32>() / channels as f32);
                }
            }
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(DecodeError::Decode(e.to_string())),
        }
    }

    if mono.is_empty() {
        return Err(DecodeError::Empty);
    }

    let samples = if source_rate == TARGET_RATE {
        mono
    } else {
        resample(&mono, source_rate)?
    };
    let duration = samples.len() as f64 / TARGET_RATE as f64;
    Ok(Audio { samples, duration })
}

fn resample(input: &[f32], source_rate: u32) -> Result<Vec<f32>, DecodeError> {
    const CHUNK: usize = 1024;
    let mut resampler = Fft::<f32>::new(
        source_rate as usize,
        TARGET_RATE as usize,
        CHUNK,
        2,
        1,
    )
    .map_err(|e| DecodeError::Decode(e.to_string()))?;

    let mut out = Vec::with_capacity(input.len() * TARGET_RATE as usize / source_rate as usize + CHUNK);
    for chunk in input.chunks(CHUNK) {
        let mut padded = chunk.to_vec();
        padded.resize(CHUNK, 0.0);
        let resampled = resampler
            .process(&[padded], None)
            .map_err(|e| DecodeError::Decode(e.to_string()))?;
        out.extend_from_slice(&resampled[0]);
    }
    Ok(out)
}
```

Add to `crates/core/src/lib.rs`:

```rust
pub mod decode;
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --package transcriba-core decode`
Expected: PASS, 5 tests.

`rubato` is the likeliest source of a signature mismatch — Task 1 Step 6 recorded the resolved version. If `Fft::<f32>::new` takes different arguments, consult `cargo doc --open --package rubato` and adapt; the contract is fixed-ratio resampling of one mono channel.

- [ ] **Step 7: Commit**

```bash
git add crates/core/src/decode.rs crates/core/src/lib.rs crates/core/Cargo.toml tests/fixtures
git commit -m "feat(core): decode and resample audio to 16kHz mono"
```

---

### Task 6: Opus support

Separable from Task 5 and independently rejectable: WhatsApp voice notes are frequently Opus, and Symphonia's Opus codec is not production-ready. `whisper-rs` already requires a C toolchain, so libopus bindings add no new class of dependency.

**Files:**
- Modify: `crates/core/src/decode.rs`, `crates/core/Cargo.toml`, `tests/fixtures/README.md`

**Interfaces:**
- Consumes: `decode::{Audio, DecodeError, TARGET_RATE}`
- Produces: no new public API — `decode()` gains Opus handling internally.

- [ ] **Step 1: Try Symphonia's Opus support first**

```bash
ffmpeg -y -f lavfi -i "sine=frequency=440:duration=2" -c:a libopus tests/fixtures/tone.opus
cargo test --package transcriba-core decode
```

Add this test to `crates/core/src/decode.rs`'s test module:

```rust
#[test]
fn decodes_opus() {
    let audio = decode(&fixture("tone.opus")).expect("decodes opus");
    assert!(!audio.samples.is_empty());
    assert!((audio.duration - 2.0).abs() < 0.2);
}
```

Run: `cargo test --package transcriba-core decode::tests::decodes_opus`

**If it passes, Symphonia handles Opus and this task is done — commit the fixture and test and skip the remaining steps.** Verify this before adding a C dependency you may not need.

- [ ] **Step 2: If it failed, add libopus bindings**

In `crates/core/Cargo.toml`:

```toml
audiopus = "0.3"
ogg = "0.9"
```

- [ ] **Step 3: Route Opus files to a dedicated decoder**

In `decode()`, before the Symphonia probe:

```rust
    if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("opus")) {
        return decode_opus(path);
    }
```

Add:

```rust
/// Decodes Ogg-contained Opus via libopus. Symphonia's Opus codec is not
/// production-ready, and WhatsApp voice notes are commonly Opus.
fn decode_opus(path: &Path) -> Result<Audio, DecodeError> {
    use audiopus::{coder::Decoder as OpusDecoder, Channels, SampleRate};

    let file = std::fs::File::open(path).map_err(|e| DecodeError::Open(e.to_string()))?;
    let mut reader = ogg::PacketReader::new(std::io::BufReader::new(file));

    // libopus decodes natively at 16kHz, so no resampling is needed.
    let mut decoder = OpusDecoder::new(SampleRate::Hz16000, Channels::Mono)
        .map_err(|e| DecodeError::Decode(e.to_string()))?;

    let mut samples: Vec<f32> = Vec::new();
    let mut frame = vec![0f32; 1920]; // 120ms at 16kHz, the largest Opus frame
    let mut index = 0usize;

    while let Some(packet) = reader
        .read_packet()
        .map_err(|e| DecodeError::Decode(e.to_string()))?
    {
        index += 1;
        // The first two Ogg packets are the OpusHead and OpusTags headers.
        if index <= 2 {
            continue;
        }
        match decoder.decode_float(Some(&packet.data), &mut frame[..], false) {
            Ok(written) => samples.extend_from_slice(&frame[..written]),
            Err(_) => continue,
        }
    }

    if samples.is_empty() {
        return Err(DecodeError::Empty);
    }
    let duration = samples.len() as f64 / TARGET_RATE as f64;
    Ok(Audio { samples, duration })
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --package transcriba-core decode`
Expected: PASS, 6 tests.

If `audiopus` fails to build for lack of libopus, install it (`sudo pacman -S opus`) and note in the commit that CI needs it too — this becomes a CI dependency on both Linux and Windows.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/decode.rs crates/core/Cargo.toml tests/fixtures
git commit -m "feat(core): decode opus audio via libopus"
```

---

### Task 7: Model store

**Files:**
- Create: `crates/core/src/model_store.rs`
- Modify: `crates/core/src/lib.rs`, `crates/core/Cargo.toml`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `pub const MODEL_FILENAME: &str = "ggml-large-v3-turbo-q5_0.bin";`
  - `pub const MODEL_URL: &str`, `pub const MIN_MODEL_BYTES: u64 = 500_000_000;`
  - `pub fn cache_dir() -> Result<PathBuf, ModelError>`
  - `pub fn resolve() -> Result<Option<PathBuf>, ModelError>` — Some when a valid model exists, None when it must be downloaded
  - `pub fn verify(path: &Path) -> Result<(), ModelError>`
  - `pub fn download(dest: &Path, on_progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<(), ModelError>`
  - `pub enum ModelError { Io(String), Network(String), Invalid(String) }`

- [ ] **Step 1: Add dependencies**

In `crates/core/Cargo.toml`:

```toml
ureq = "2"
dirs = "5"
```

- [ ] **Step 2: Write the failing tests**

`crates/core/src/model_store.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("transcriba-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(bytes).unwrap();
        path
    }

    #[test]
    fn rejects_a_file_that_is_too_small() {
        let path = temp_file("small.bin", b"ggml and nothing else");
        assert!(matches!(verify(&path), Err(ModelError::Invalid(_))));
    }

    #[test]
    fn rejects_a_file_without_ggml_magic() {
        let mut bytes = vec![0u8; MIN_MODEL_BYTES as usize + 1];
        bytes[..4].copy_from_slice(b"junk");
        let path = temp_file("nomagic.bin", &bytes);
        assert!(matches!(verify(&path), Err(ModelError::Invalid(_))));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn accepts_a_file_with_ggml_magic_and_sufficient_size() {
        let mut bytes = vec![0u8; MIN_MODEL_BYTES as usize + 1];
        bytes[..4].copy_from_slice(b"lmgg");
        let path = temp_file("good.bin", &bytes);
        assert!(verify(&path).is_ok());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn missing_file_is_reported_as_io_error() {
        assert!(matches!(verify(std::path::Path::new("/nonexistent")), Err(ModelError::Io(_))));
    }

    #[test]
    fn env_override_takes_precedence_when_valid() {
        let mut bytes = vec![0u8; MIN_MODEL_BYTES as usize + 1];
        bytes[..4].copy_from_slice(b"lmgg");
        let path = temp_file("override.bin", &bytes);
        std::env::set_var("TRANSCRIBA_MODEL_PATH", &path);
        assert_eq!(resolve().unwrap(), Some(path.clone()));
        std::env::remove_var("TRANSCRIBA_MODEL_PATH");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn cache_dir_ends_with_expected_path() {
        let dir = cache_dir().unwrap();
        assert!(dir.ends_with("transcriba/models"));
    }
}
```

Note: the two tests that allocate `MIN_MODEL_BYTES` write ~500MB each to the temp directory. That is deliberate — it is the only way to test the real size threshold — and they clean up after themselves.

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --package transcriba-core model_store`
Expected: FAIL — `cannot find function verify`.

- [ ] **Step 4: Implement**

Prepend to `crates/core/src/model_store.rs`:

```rust
//! Locates, verifies and downloads the whisper model.
//!
//! A truncated download that passes as valid is the most annoying possible
//! failure, so verification is strict and failure deletes the file.

use std::io::Read;
use std::path::{Path, PathBuf};

pub const MODEL_FILENAME: &str = "ggml-large-v3-turbo-q5_0.bin";
pub const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin?download=true";
/// The real file is ~574MB. Anything materially smaller is truncated.
pub const MIN_MODEL_BYTES: u64 = 500_000_000;

#[derive(Debug)]
pub enum ModelError {
    Io(String),
    Network(String),
    Invalid(String),
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelError::Io(m) => write!(f, "could not read or write the model file: {m}"),
            ModelError::Network(m) => write!(
                f,
                "could not download the speech model: {m}. Check your internet connection, \
or set TRANSCRIBA_MODEL_PATH to a copy of {MODEL_FILENAME}."
            ),
            ModelError::Invalid(m) => write!(f, "the model file is not valid: {m}"),
        }
    }
}

impl std::error::Error for ModelError {}

pub fn cache_dir() -> Result<PathBuf, ModelError> {
    let base = dirs::data_local_dir()
        .ok_or_else(|| ModelError::Io("no local data directory for this user".into()))?;
    Ok(base.join("transcriba").join("models"))
}

/// Checks size and the ggml magic bytes. ggml files begin with the little-endian
/// bytes of "ggml", which read as `lmgg`.
pub fn verify(path: &Path) -> Result<(), ModelError> {
    let meta = std::fs::metadata(path).map_err(|e| ModelError::Io(e.to_string()))?;
    if meta.len() < MIN_MODEL_BYTES {
        return Err(ModelError::Invalid(format!(
            "expected at least {MIN_MODEL_BYTES} bytes, found {}",
            meta.len()
        )));
    }
    let mut magic = [0u8; 4];
    std::fs::File::open(path)
        .map_err(|e| ModelError::Io(e.to_string()))?
        .read_exact(&mut magic)
        .map_err(|e| ModelError::Io(e.to_string()))?;
    if &magic != b"lmgg" {
        return Err(ModelError::Invalid("missing ggml magic bytes".into()));
    }
    Ok(())
}

/// Returns the model path if one is already available, or None if it must be downloaded.
pub fn resolve() -> Result<Option<PathBuf>, ModelError> {
    if let Some(override_path) = std::env::var_os("TRANSCRIBA_MODEL_PATH") {
        let path = PathBuf::from(override_path);
        verify(&path)?;
        return Ok(Some(path));
    }
    let cached = cache_dir()?.join(MODEL_FILENAME);
    match verify(&cached) {
        Ok(()) => Ok(Some(cached)),
        // A corrupt cached file is deleted so the next run re-downloads cleanly.
        Err(ModelError::Invalid(_)) => {
            std::fs::remove_file(&cached).ok();
            Ok(None)
        }
        Err(ModelError::Io(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn download(
    dest: &Path,
    on_progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<(), ModelError> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ModelError::Io(e.to_string()))?;
    }
    let response = ureq::get(MODEL_URL)
        .call()
        .map_err(|e| ModelError::Network(e.to_string()))?;
    let total: Option<u64> = response
        .header("Content-Length")
        .and_then(|v| v.parse().ok());

    // Download to a .part file so an interrupted transfer is never mistaken for a model.
    let part = dest.with_extension("part");
    let mut file = std::fs::File::create(&part).map_err(|e| ModelError::Io(e.to_string()))?;
    let mut reader = response.into_reader();
    let mut buf = vec![0u8; 1 << 16];
    let mut written = 0u64;

    loop {
        let n = reader.read(&mut buf).map_err(|e| ModelError::Network(e.to_string()))?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n])
            .map_err(|e| ModelError::Io(e.to_string()))?;
        written += n as u64;
        on_progress(written, total);
    }
    drop(file);

    verify(&part).inspect_err(|_| {
        std::fs::remove_file(&part).ok();
    })?;
    std::fs::rename(&part, dest).map_err(|e| ModelError::Io(e.to_string()))?;
    Ok(())
}
```

Add to `crates/core/src/lib.rs`:

```rust
pub mod model_store;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --package transcriba-core model_store`
Expected: PASS, 6 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/model_store.rs crates/core/src/lib.rs crates/core/Cargo.toml
git commit -m "feat(core): locate, verify and download the whisper model"
```

---

### Task 8: Transcription

**Files:**
- Create: `crates/core/src/transcribe.rs`
- Modify: `crates/core/src/lib.rs`, `crates/core/Cargo.toml`

**Interfaces:**
- Consumes: `decode::Audio`, `reflow::Cue`
- Produces:
  - `pub struct Options { pub language: String, pub threads: usize, pub model_path: PathBuf }`
  - `pub fn default_threads() -> usize`
  - `pub struct Cancelled;`
  - `pub fn transcribe(audio: &Audio, opts: &Options, on_progress: &mut dyn FnMut(i32), should_cancel: &dyn Fn() -> bool) -> Result<Vec<Cue>, TranscribeError>`
  - `pub enum TranscribeError { Model(String), Run(String), Cancelled, NoSpeech }`

- [ ] **Step 1: Add dependencies**

In `crates/core/Cargo.toml`:

```toml
whisper-rs = "0.16"
num_cpus = "1"
```

Build now — `whisper-rs` compiles whisper.cpp and needs a C toolchain, so surface that before writing code:

Run: `cargo build --package transcriba-core`
Expected: succeeds, slowly. If it fails, install a C/C++ toolchain (`sudo pacman -S base-devel cmake`) and retry.

- [ ] **Step 2: Write the failing tests**

`crates/core/src/transcribe.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::Audio;

    fn silence(seconds: f64) -> Audio {
        Audio {
            samples: vec![0.0; (seconds * crate::decode::TARGET_RATE as f64) as usize],
            duration: seconds,
        }
    }

    fn tiny_model_opts() -> Option<Options> {
        // Integration tests use ggml-tiny (75MB), never the 574MB production model:
        // this exercises plumbing, not accuracy.
        let path = std::env::var_os("TRANSCRIBA_TEST_MODEL")?;
        Some(Options {
            language: "pt".into(),
            threads: 2,
            model_path: std::path::PathBuf::from(path),
        })
    }

    #[test]
    fn default_threads_leaves_headroom_and_is_at_least_one() {
        let t = default_threads();
        assert!(t >= 1);
        assert!(t <= num_cpus::get().saturating_sub(2).max(1));
    }

    #[test]
    fn missing_model_reports_model_error() {
        let opts = Options {
            language: "pt".into(),
            threads: 1,
            model_path: "/nonexistent-model.bin".into(),
        };
        let err = transcribe(&silence(0.5), &opts, &mut |_| {}, &|| false);
        assert!(matches!(err, Err(TranscribeError::Model(_))));
    }

    #[test]
    fn cancellation_before_work_returns_cancelled() {
        let Some(opts) = tiny_model_opts() else { return };
        let err = transcribe(&silence(2.0), &opts, &mut |_| {}, &|| true);
        assert!(matches!(err, Err(TranscribeError::Cancelled)));
    }

    #[test]
    fn silence_reports_no_speech() {
        let Some(opts) = tiny_model_opts() else { return };
        let err = transcribe(&silence(3.0), &opts, &mut |_| {}, &|| false);
        assert!(matches!(err, Err(TranscribeError::NoSpeech)));
    }
}
```

Tests needing a model skip cleanly when `TRANSCRIBA_TEST_MODEL` is unset, so `cargo test` works on a fresh clone. CI sets it after fetching `ggml-tiny`.

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --package transcriba-core transcribe`
Expected: FAIL — `cannot find function transcribe`.

- [ ] **Step 4: Implement**

Prepend to `crates/core/src/transcribe.rs`:

```rust
//! Runs whisper over decoded audio and returns timestamped cues.

use crate::decode::Audio;
use crate::reflow::Cue;
use std::path::PathBuf;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

#[derive(Debug, Clone)]
pub struct Options {
    pub language: String,
    pub threads: usize,
    pub model_path: PathBuf,
}

#[derive(Debug)]
pub enum TranscribeError {
    Model(String),
    Run(String),
    Cancelled,
    NoSpeech,
}

impl std::fmt::Display for TranscribeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TranscribeError::Model(m) => write!(f, "the speech model could not be loaded: {m}"),
            TranscribeError::Run(m) => write!(f, "transcription failed: {m}"),
            TranscribeError::Cancelled => write!(f, "transcription was cancelled"),
            TranscribeError::NoSpeech => write!(f, "no speech was detected in this recording"),
        }
    }
}

impl std::error::Error for TranscribeError {}

/// Leaves two cores free so the machine stays usable during a long job.
pub fn default_threads() -> usize {
    num_cpus::get().saturating_sub(2).max(1)
}

pub fn transcribe(
    audio: &Audio,
    opts: &Options,
    on_progress: &mut dyn FnMut(i32),
    should_cancel: &dyn Fn() -> bool,
) -> Result<Vec<Cue>, TranscribeError> {
    if should_cancel() {
        return Err(TranscribeError::Cancelled);
    }

    let ctx = WhisperContext::new_with_params(
        &opts.model_path.to_string_lossy(),
        WhisperContextParameters::default(),
    )
    .map_err(|e| TranscribeError::Model(e.to_string()))?;

    let mut state = ctx
        .create_state()
        .map_err(|e| TranscribeError::Model(e.to_string()))?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(opts.threads as i32);
    params.set_language(Some(&opts.language));
    params.set_print_progress(false);
    params.set_print_special(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);

    // whisper.cpp exposes both callbacks; verified present in whisper-rs 0.16.
    params.set_progress_callback_safe(|p: i32| on_progress(p));
    params.set_abort_callback_safe(|| should_cancel());

    state
        .full(params, &audio.samples)
        .map_err(|e| TranscribeError::Run(e.to_string()))?;

    if should_cancel() {
        return Err(TranscribeError::Cancelled);
    }

    let mut cues = Vec::new();
    for segment in state.as_iter() {
        let text = segment.to_str_lossy().unwrap_or_default().trim().to_string();
        if text.is_empty() {
            continue;
        }
        // whisper timestamps are in centiseconds.
        cues.push(Cue {
            start: segment.start_timestamp() as f64 / 100.0,
            end: segment.end_timestamp() as f64 / 100.0,
            text,
        });
    }

    if cues.is_empty() {
        return Err(TranscribeError::NoSpeech);
    }
    Ok(cues)
}
```

Add to `crates/core/src/lib.rs`:

```rust
pub mod transcribe;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --package transcriba-core transcribe`
Expected: PASS. Model-dependent tests skip unless `TRANSCRIBA_TEST_MODEL` is set.

**This is the least certain code in the plan — expect to adapt it.** `whisper-rs` documents itself as only ~52% covered, so the exact names `as_iter`, `to_str_lossy`, `set_progress_callback_safe` and `set_abort_callback_safe` must be confirmed against `cargo doc --open --package whisper-rs`. The *shapes* are verified and should not change: a context built from a path, a state from the context, `FullParams` with `SamplingStrategy`, segment iteration yielding text plus centisecond timestamps, and both a progress and an abort callback.

Expect one specific fight. The two callbacks capture `on_progress` (an `&mut dyn FnMut`) and `should_cancel` (a `&dyn Fn`), and the callback setters may demand `'static` or a plain `Fn` rather than `FnMut`. If the borrow checker rejects it, the standard fix is to move the shared state behind an `Arc<Mutex<..>>` for progress and an `Arc<AtomicBool>` for cancellation, then have the closures touch only those. That is also the shape Plan 2's GUI needs, since progress must cross a thread boundary to reach the webview — so if you hit this, solving it with `Arc` is progress, not a workaround.

- [ ] **Step 6: Fetch the tiny model and verify end-to-end**

```bash
mkdir -p tests/fixtures
curl -L -o tests/fixtures/ggml-tiny.bin \
  "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin?download=true"
TRANSCRIBA_TEST_MODEL=tests/fixtures/ggml-tiny.bin cargo test --package transcriba-core transcribe
```

Expected: all four tests run, none skipped.

- [ ] **Step 7: Commit**

```bash
git add crates/core/src/transcribe.rs crates/core/src/lib.rs crates/core/Cargo.toml
git commit -m "feat(core): transcribe audio with whisper-rs"
```

---

### Task 9: CLI wiring

Makes the library usable end-to-end and replaces the manual `ffmpeg` + `whisper-cli` + Python pipeline.

**Files:**
- Create: `crates/core/src/output.rs`
- Modify: `crates/cli/src/main.rs`, `crates/cli/Cargo.toml`, `crates/core/src/lib.rs`, `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: every module above
- Produces: `pub fn unique_path(preferred: &Path) -> PathBuf`; binary `transcriba <FILE> [--lang pt] [--threads N]`

- [ ] **Step 1: Write the failing test for non-overwriting output paths**

`crates/core/src/output.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_the_preferred_path_when_free() {
        let dir = std::env::temp_dir().join("transcriba-output-tests-a");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("meeting.docx");
        std::fs::remove_file(&target).ok();
        assert_eq!(unique_path(&target), target);
    }

    #[test]
    fn adds_a_numeric_suffix_when_the_file_exists() {
        let dir = std::env::temp_dir().join("transcriba-output-tests-b");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("meeting.docx");
        std::fs::write(&target, b"x").unwrap();
        let next = unique_path(&target);
        assert_eq!(next.file_name().unwrap(), "meeting-2.docx");
        std::fs::remove_file(&target).ok();
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --package transcriba-core output`
Expected: FAIL — `cannot find function unique_path`.

- [ ] **Step 3: Implement**

Prepend to `crates/core/src/output.rs`:

```rust
//! Chooses output paths that never overwrite an existing file.

use std::path::{Path, PathBuf};

pub fn unique_path(preferred: &Path) -> PathBuf {
    if !preferred.exists() {
        return preferred.to_path_buf();
    }
    let stem = preferred.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = preferred.extension().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let parent = preferred.parent().unwrap_or(Path::new("."));
    for n in 2..1000 {
        let candidate = parent.join(format!("{stem}-{n}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!("{stem}-{}.{ext}", std::process::id()))
}
```

Add to `crates/core/src/lib.rs`:

```rust
pub mod output;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --package transcriba-core output`
Expected: PASS, 2 tests.

- [ ] **Step 5: Add the CLI dependency**

In `crates/cli/Cargo.toml`:

```toml
clap = { version = "4", features = ["derive"] }
```

- [ ] **Step 6: Write the CLI**

`crates/cli/src/main.rs`:

```rust
use clap::Parser;
use std::path::PathBuf;
use transcriba_core::{decode, model_store, output, reflow, render, transcribe};

#[derive(Parser)]
#[command(name = "transcriba", about = "Transcribe an audio file to docx and pdf, fully offline")]
struct Args {
    /// Audio file to transcribe
    file: PathBuf,
    /// Language code, or "auto" to detect
    #[arg(long, default_value = "pt")]
    lang: String,
    /// Worker threads. Defaults to CPU count minus two.
    #[arg(long)]
    threads: Option<usize>,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let model_path = match model_store::resolve()? {
        Some(p) => p,
        None => {
            let dest = model_store::cache_dir()?.join(model_store::MODEL_FILENAME);
            eprintln!("Downloading the speech model once (~574MB)...");
            let mut last = 0u64;
            model_store::download(&dest, &mut |done, total| {
                let pct = total.map(|t| done * 100 / t.max(1)).unwrap_or(0);
                if pct != last {
                    last = pct;
                    eprint!("\r  {pct}%");
                }
            })?;
            eprintln!("\r  done.");
            dest
        }
    };

    eprintln!("Decoding {}...", args.file.display());
    let audio = decode::decode(&args.file)?;
    eprintln!("  {:.1} minutes of audio", audio.duration / 60.0);

    let threads = args.threads.unwrap_or_else(transcribe::default_threads);
    eprintln!("Transcribing with {threads} threads...");
    let opts = transcribe::Options { language: args.lang.clone(), threads, model_path };

    let mut last_pct = -1;
    let cues = transcribe::transcribe(
        &audio,
        &opts,
        &mut |p| {
            if p != last_pct {
                last_pct = p;
                eprint!("\r  {p}%");
            }
        },
        &|| false,
    )?;
    eprintln!("\r  done — {} segments", cues.len());

    let blocks = reflow::reflow(&cues);
    let title = args.file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let meta = render::DocumentMeta {
        title,
        duration: audio.duration,
        backend: format!("CPU ({threads} threads)"),
    };

    let docx_path = output::unique_path(&args.file.with_extension("docx"));
    std::fs::write(&docx_path, render::docx::render_docx(&blocks, &meta)?)?;

    let font_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts");
    let pdf_path = output::unique_path(&args.file.with_extension("pdf"));
    std::fs::write(&pdf_path, render::pdf::render_pdf(&blocks, &meta, &font_dir)?)?;

    eprintln!("Wrote {}", docx_path.display());
    eprintln!("Wrote {}", pdf_path.display());
    Ok(())
}
```

- [ ] **Step 7: Verify end-to-end against real audio**

```bash
cargo build --release
cp "/home/guilherme/Downloads/WhatsApp Audio 2026-08-04 at 11.30.31.mpeg" /tmp/meeting.mp3
TRANSCRIBA_MODEL_PATH="$HOME/.local/share/whisper-models/ggml-large-v3-turbo-q5_0.bin" \
  ./target/release/transcriba /tmp/meeting.mp3 --lang pt
```

The env override reuses the already-downloaded model instead of fetching another 574MB.

Expected: `/tmp/meeting.docx` and `/tmp/meeting.pdf` exist. Compare against the known-good baseline from the manual pipeline: 58m30s duration, roughly 2553 segments, about 58 paragraphs, ~12 A4 pages. Segment count will differ somewhat — whisper is not bit-deterministic across thread counts — but a result off by more than about 20% means something is wrong.

- [ ] **Step 8: Add the model-dependent tests to CI**

In `.github/workflows/ci.yml`, before `cargo test`:

```yaml
      - name: Fetch tiny model for integration tests
        run: |
          mkdir -p tests/fixtures
          curl -sL -o tests/fixtures/ggml-tiny.bin \
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin?download=true"
      - name: Install audio fixtures toolchain
        run: sudo apt-get update && sudo apt-get install -y ffmpeg libopus-dev
```

and change the test step to:

```yaml
      - run: cargo test --workspace
        env:
          TRANSCRIBA_TEST_MODEL: tests/fixtures/ggml-tiny.bin
```

- [ ] **Step 9: Commit**

```bash
git add crates/core/src/output.rs crates/core/src/lib.rs crates/cli .github/workflows/ci.yml
git commit -m "feat(cli): wire the full pipeline into a transcriba binary"
```

---

## Deferred to Plan 2

- Tauri v2 app, frontend, drag-and-drop, progress UI, language dropdown
- Vulkan feature flag and the "Transcribed using GPU/CPU" backend display
- Windows CI runner, `.msi`/`.exe` and AppImage bundling
- Font directory resolution from bundled resources rather than `CARGO_MANIFEST_DIR`

The font path in Task 9 uses `CARGO_MANIFEST_DIR`, which works only from a source checkout. Plan 2 must replace it with Tauri's resource resolver; noted here so it is not mistaken for finished work.
