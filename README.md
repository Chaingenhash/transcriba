# Transcriba

Transcriba turns an audio recording into a readable transcript. Drop in a meeting
recording and it writes a `.docx` and a `.pdf` beside the file — no terminal, no upload,
nothing leaves your machine.

## For users

### Install

- **Linux:** download the `.AppImage` from the release, make it executable
  (`chmod +x Transcriba_*.AppImage`), and run it. No installation step — it's a single
  file you can put anywhere.
- **Windows:** download and run the `.exe` installer from the release.

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
4. Wait. How long depends on whether your machine has a usable GPU:

   | | Roughly, per hour of audio |
   |---|---|
   | Linux with a working GPU | **6–7 minutes** |
   | Anything on CPU | **25 minutes** |

   The Linux build uses your GPU when it can and falls back to CPU when it can't; the
   Windows build is CPU-only. Either way the finished document's header says which one
   actually ran, so you never have to guess — look for `GPU (…)` or `CPU (N threads)`.

   The progress bar moves the whole time. If it sits on a percentage for a minute or two
   that's normal, not a hang. Don't kill it.
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
& "$env:LOCALAPPDATA\Transcriba\Transcriba.exe"
```

(The installer's default mode is per-user, so it installs under `%LOCALAPPDATA%\Transcriba`,
not `C:\Program Files`.)

When this variable is set and the file passes verification, no download is attempted at
all.

## For developers

### Prerequisites

- Rust (stable)
- `cmake` and `libclang` (needed to build `whisper-rs`'s vendored whisper.cpp)
- Node.js and npm
- On Linux, the exact packages `ci.yml` installs on the runner (Tauri's webview
  dependencies, plus the AppImage icon/tray libraries):

  ```bash
  sudo apt-get update
  sudo apt-get install -y cmake libclang-dev \
    libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev
  ```

### Build and test

```bash
cargo test --workspace   # skips the whisper integration tests — see below
cd app && npm ci                                # install pinned frontend deps
cd app && npx tauri dev                          # run the desktop app
cd app && npx tauri build --bundles appimage    # produce a Linux AppImage
```

`cargo test --workspace` alone does not exercise the whisper integration tests —
they need a real model file and are skipped otherwise, silently, with no failure
to flag it. Point `TRANSCRIBA_TEST_MODEL` at a `ggml-tiny.bin` (or any valid ggml
model) to run the full suite, matching what `ci.yml` does:

```bash
TRANSCRIBA_TEST_MODEL=/path/to/ggml-tiny.bin cargo test --workspace
```

If the AppImage build fails with `strip`/`.relr.dyn` errors, retry with
`NO_STRIP=1 npx tauri build --bundles appimage`. `linuxdeploy`'s vendored `strip`
predates the `SHT_RELR` relocation format newer binutils emit by default, so it can't
strip the system libraries it copies in — this isn't specific to one machine and can hit
CI runners too. See `docs/superpowers/plan-2-carry-forward.md` for the full story.

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
