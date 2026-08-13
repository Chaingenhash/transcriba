# Transcriba

Transcriba turns an audio recording into a readable transcript. Drop in a meeting
recording and it writes a `.docx` and a `.pdf` beside the file — no terminal, no upload,
nothing leaves your machine.

## For users

### Install

- **Linux:** download the `.AppImage` from the release, make it executable
  (`chmod +x Transcriba_*.AppImage`), and run it. No installation step — it's a single
  file you can put anywhere.
- **Windows:** download and run the `.exe`/`.msi` installer from the release.

  Windows will very likely show a blue screen titled **"Windows protected your PC"**, with
  a greyed-out button. This is Windows SmartScreen reacting to an installer that isn't
  code-signed — Transcriba is an internal tool for a handful of colleagues, and a
  code-signing certificate runs €200–400/year, which isn't worth it for that audience. The
  installer is not malicious; it's just unrecognized. To proceed:

  1. Click **"More info"** (small text near the bottom of the warning).
  2. Click the **"Run anyway"** button that appears.

  You should only need to do this once, the first time you run the installer.

### Use it

1. Open Transcriba.
2. Drop an audio file onto the window (or use the file picker).
3. Pick the spoken language, or leave it on auto-detect.
4. Wait. **Budget roughly 25 minutes per hour of audio** on a typical laptop — a
   90-minute meeting takes about 35–40 minutes. The progress bar moves the whole time;
   if it looks stuck at a percentage for a minute or two, that's normal, not a hang. Don't
   kill it.
5. When it finishes, a `.docx` and a `.pdf` appear next to the original file (e.g.
   `meeting.mp3` produces `meeting.docx` and `meeting.pdf`). Existing files are never
   overwritten — a new one gets a numbered suffix instead.

### First run: the model download

Transcriba does not ship with its transcription model, to keep the installer small. On
the very first run it downloads one (`large-v3-turbo`, about **574MB**) from Hugging
Face and caches it locally; every run after that is instant on this front.

If your network blocks Hugging Face — common on corporate networks — that download will
fail, and it'll happen at the worst possible time (first launch). If that happens, get a
copy of the model file (`ggml-large-v3-turbo-q5_0.bin`) from a colleague or an internal
file share, and point Transcriba at it with the `TRANSCRIBA_MODEL_PATH` environment
variable instead of letting it download:

```bash
# Linux
TRANSCRIBA_MODEL_PATH=/home/you/models/ggml-large-v3-turbo-q5_0.bin ./Transcriba_*.AppImage
```

```powershell
# Windows (PowerShell)
$env:TRANSCRIBA_MODEL_PATH = "C:\Users\you\models\ggml-large-v3-turbo-q5_0.bin"
& "C:\Program Files\Transcriba\transcriba.exe"
```

When this variable is set and the file passes verification, no download is attempted at
all.

## For developers

### Prerequisites

- Rust (stable)
- `cmake` and `libclang` (needed to build `whisper-rs`'s vendored whisper.cpp)
- Node.js and npm
- On Linux: `webkit2gtk-4.1`, `libsoup-3.0`, and `javascriptcoregtk-4.1` (Tauri's
  webview dependencies)

### Build and test

```bash
cargo test --workspace
cd app && npx tauri dev      # run the desktop app
cd app && npx tauri build --bundles appimage   # produce a Linux AppImage
```

### The CLI binary is not the supported artifact

`crates/cli` builds a standalone `transcriba` binary, useful for local development, but
it resolves its PDF fonts relative to `CARGO_MANIFEST_DIR` at compile time
(`crates/cli/src/main.rs`). That path only exists on the machine it was built on — copy
the binary anywhere else, or install it with `cargo install`, and PDF rendering fails
because the font directory isn't there. The Tauri app does not have this problem: it
resolves fonts through Tauri's bundled-resource path at runtime, and that path travels
with the app.

**Hand colleagues the app, not the CLI binary.**

### More detail

- Findings and constraints carried over from earlier implementation work:
  [`docs/superpowers/plan-2-carry-forward.md`](docs/superpowers/plan-2-carry-forward.md)
- Full design spec:
  [`docs/superpowers/specs/2026-08-05-local-transcription-app-design.md`](docs/superpowers/specs/2026-08-05-local-transcription-app-design.md)
