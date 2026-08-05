# Transcriba — Local Audio Transcription App

**Date:** 2026-08-05
**Status:** Approved design, ready for implementation planning

## Problem

Transcribing a meeting recording currently requires a terminal, a manually installed
`whisper-cpp`, a manually downloaded model, an `ffmpeg` invocation with exactly the right
flags, and a Python script to turn subtitle cues into readable prose. This works for one
person on one Arch laptop. It does not work for colleagues.

Colleagues need to transcribe occasional long recordings (30–90 minute meetings) and get
back a document they can read and edit. They are not technical, they run Linux and Windows,
and the audio must never leave their machine.

## Goals

- One-click install, no terminal, no dependencies for the end user
- Fully local processing — no audio or transcript leaves the machine
- Linux and Windows
- Input: any common audio file. Output: a readable `.docx` and `.pdf`
- Portuguese preselected; any language whisper supports is selectable from a dropdown,
  plus an "auto-detect" entry
- Usable while a job runs — must not monopolise the machine

## Non-goals

These were considered and explicitly excluded:

- **Speaker labels / diarization.** Requires a second model and roughly doubles scope.
- **Summaries or action items.** Requires an LLM, conflicts with local-only.
- **Raw `.txt` / `.srt` delivery.** Intermediate only; not written to disk.
- **Cloud transcription.** Rejected on privacy grounds. Cost was never the objection —
  Groq runs the same `large-v3-turbo` model for ~$0.04/hour of audio. The decision is
  about GDPR exposure once colleagues upload recordings we do not control.
- **Auto-updates.** Tauri's updater needs hosting and signing keys. Revisit if adopted.
- **A server or shared instance.** Each colleague runs it locally.

## Constraints and decisions

| Decision | Rationale |
|---|---|
| Tauri v2, Rust backend | Small installers, first-class bundling. Author wants to learn Rust. |
| `large-v3-turbo` q5 model (574MB) | Jobs are long and occasional, so quality beats speed. Measured 0.25× realtime on 14 threads. |
| Model downloaded on first run | Keeps the installer small. Model is not redistributed. |
| Pure-Rust pipeline, no sidecars | No per-platform binary bundling. More Rust to write, which serves the learning goal. |
| Output beside the input file | "Where did it go?" is the most common support question. Designed out. |
| Ship unsigned | Informal tool for a handful of colleagues; a certificate is €200–400/year. |
| CPU is the contract, GPU is a bonus | Three documented silent-fallback bugs in whisper.cpp's Vulkan backend. |
| Assume >8GB RAM | Explicit project assumption. Removes the need for chunked decoding. |

## Architecture

```
┌─ Frontend (TypeScript) ────────────────────────┐
│  drop zone · language picker · progress · done │
└───────────────── Tauri IPC ────────────────────┘
┌─ Rust core (src-tauri) ────────────────────────┐
│  model_store   fetch model, cache, verify      │
│  decode        any audio → 16kHz mono f32      │
│  transcribe    whisper-rs, progress callback   │
│  reflow        cues → sentences → paragraphs   │
│  render        docx + pdf                      │
└────────────────────────────────────────────────┘
```

Five modules with narrow interfaces. `decode` takes a path and returns samples. `reflow`
takes cues and returns paragraphs. `render` takes paragraphs and returns bytes. None
requires the others to be tested.

### Dependencies

| Job | Crate | Notes |
|---|---|---|
| Transcribe | `whisper-rs` 0.16 | Static-links whisper.cpp. Exposes whisper's progress callback. Has Windows build instructions. |
| Decode | `symphonia` | MP3, M4A/MP4, WAV, FLAC, OGG/Vorbis. **Not Opus.** |
| Opus decode | `symphonia-adapter-libopus` | Registers libopus as a codec inside symphonia. Revised 2026-08-05 from `audiopus`, whose only release is a pre-release, and which would have needed a separate Ogg parser. |
| Resample | `rubato` | 48kHz → 16kHz. Replaces the `ffmpeg` step. |
| DOCX | `docx-rs` | Writer only, which is all that is needed. |
| PDF | `genpdf` | Pure Rust, layout over `printpdf`. See risks. |

A C toolchain is required at **build** time because `whisper-rs` compiles whisper.cpp.
Since that cost is already paid, `audiopus` adds no new category of dependency.

## Data flow

### First run

Check the model cache; download if absent. Cache lives in Tauri's `app_local_data_dir()` —
`~/.local/share/transcriba/models/` on Linux, `%LOCALAPPDATA%\transcriba\models\` on
Windows.

The download is resumable, shows progress, and is verified by file size and `ggml` magic
bytes before being accepted. A truncated download that passes as valid is the most
annoying possible failure, so verification failure deletes the file and re-fetches rather
than handing a corrupt model to whisper.

The model source must be overridable by the `TRANSCRIBA_MODEL_PATH` environment variable,
pointing at an already-downloaded model file. If set and the file verifies, no download is
attempted. Corporate proxies block HuggingFace more often than expected, and the failure
would land on first launch — the worst moment for adoption. The override lets the model be
placed on an internal share and pointed at instead.

### Per job

```
pick/drop file
  → decode      symphonia + rubato → 16kHz mono f32    ~5s,     progress 0-5%
  → transcribe  whisper-rs                             minutes, progress 5-95%
  → reflow      cues → sentences → paragraphs          instant
  → render      docx + pdf beside the input            ~1s,     progress 95-100%
```

Transcription is CPU-bound and blocking, so it runs on a dedicated thread and reports
through Tauri events. The webview never blocks.

Thread count defaults to `num_cpus - 2`, not all cores. A colleague must be able to keep
working while a 90-minute recording processes. Pinning every core on someone's work laptop
for an hour is how a tool gets uninstalled.

Progress comes from whisper's own callback, so the bar reflects real work. On slower
hardware this is the difference between "working" and "frozen".

Outputs land beside the input: `meeting.mp3` produces `meeting.docx` and `meeting.pdf`.
Existing files get a numeric suffix; nothing is overwritten.

### Reflow algorithm

A direct port of the validated Python implementation:

1. Merge consecutive cues into sentences, closing on terminal punctuation (`.!?…`).
2. Start a new paragraph when the gap between sentences exceeds 1.0s, or the paragraph
   exceeds 700 characters.
3. Emit a section heading every 10 minutes of audio.
4. Prefix each paragraph with an `[mm:ss]` marker so the reader can find the audio.

Pure function, no I/O.

## GPU acceleration

Build with the `vulkan` feature. Vulkan — not CUDA — because the target hardware is office
laptops with Intel integrated graphics; one Vulkan build covers Intel, AMD and NVIDIA,
while CUDA requires the toolkit at build time and only helps rare NVIDIA machines.

GPU is **opportunistic**: attempt it, fall back to CPU, and **display which backend
actually ran** — "Transcribed using GPU" versus "Transcribed using CPU (12 threads)".

That display is the substance of this feature. whisper.cpp has three documented
silent-fallback problems:

- Vulkan silently fails to register on Windows MSVC static builds — a static-init race
  throws inside `ggml_vk_instance_init()`, the backend never registers, and whisper
  reports "no GPU found" while devices are visible. This is exactly our configuration.
- Intel GPUs below Gen8 lack `VK_KHR_16bit_storage` and fail cryptically instead of
  falling back cleanly.
- Silent CPU fallback is a recurring complaint on AMD/Linux.

Without backend reporting there is no way to know whether GPU ever engaged. A setting to
force CPU covers colleagues with misbehaving drivers.

Expectations: on integrated graphics the GPU shares memory bandwidth with the CPU, so
realistic gains are 1.5–2×, not the 10× a discrete card gives. If the Windows registration
bug bites, Windows ships CPU-only with no redesign needed.

## Error handling

Every error names the file, says what went wrong, and offers one action.

| Failure | User-facing behaviour |
|---|---|
| Unsupported codec | Names the actual codec, not "unsupported format" |
| Model download failed | Retry, preserving partial progress. Distinguishes offline from disk-full. |
| Corrupt/truncated model | Delete and re-download. Never retried against a bad file. |
| No speech detected | Says so explicitly, rather than producing an empty document |
| Output file locked | Writes a suffixed name and reports it. Common on Windows with Word open. |

## Testing

- **`reflow`** — table-driven unit tests over cue sequences: pause boundaries, the 700-char
  cap, and missing punctuation. The real logic risk lives here; production audio has
  produced stretches averaging 2.3 words per cue with no sentence terminators.
- **`decode`** — one short fixture per format, asserting sample rate, channel count and
  duration.
- **`render`** — unzip the `.docx` and assert paragraph text survived; assert page count and
  extractable text for the `.pdf`.
- **Integration** — a ~10 second fixture end-to-end, asserting known words appear in the
  `.docx`. Uses **`ggml-tiny` (75MB), not `large-v3-turbo`** — this tests plumbing, not
  accuracy, and keeps CI usable.
- **CI** — GitHub Actions matrix on Linux and Windows. Development happens on Arch and half
  the users are on Windows; an untested Windows build is a broken Windows build.

Transcription accuracy is deliberately not tested. It belongs to whisper, and asserting on
model output produces brittle tests.

## Packaging and distribution

Tauri v2's bundler produces a Windows `.exe`/`.msi` and a Linux `AppImage`. AppImage is
chosen over `.deb` so it runs across distros without per-distro packaging.

The Windows target cannot realistically be built from Arch — cross-compiling Tauri is
awkward, and `whisper-rs` compiling whisper.cpp through a C toolchain makes it worse. A
GitHub Actions matrix with a real `windows-latest` runner handles it. This is the same CI
the tests need, so it is one setup rather than two, but it does mean the release path lives
in CI from day one.

Installers ship unsigned. Windows will show "Windows protected your PC" with the Run button
behind *More info*. Colleagues need a one-time walkthrough. If the tool becomes something
the company depends on, revisit — IT may already hold a certificate.

## Risks and open questions

Items to resolve during implementation rather than design:

1. ~~**Cancellation.**~~ **Resolved 2026-08-05:** `whisper-rs` exposes both
   `WhisperProgressCallback` and `WhisperAbortCallback`, so Cancel can abort mid-run rather
   than waiting for a segment boundary. The remaining wrinkle is a borrow-checker one — the
   callbacks likely need `Arc<AtomicBool>` for cancellation and `Arc<Mutex<..>>` for
   progress rather than borrowed closures, which is the shape the GUI needs anyway.
2. **`genpdf` maintenance.** At 0.1.1 with the original repository dormant and several
   forks circulating. Adequate for headings and paragraphs, which is all a transcript is.
   If typography matters more later, `typst` embedded as a library is the upgrade path.
3. **Vulkan on Windows.** Expected to fail per the issue above. Verify empirically; ship
   Windows CPU-only if confirmed.
4. **libopus linkage on Windows.** Opus support goes through `symphonia-adapter-libopus`,
   which needs libopus at build time. The development machine has it (1.6.1) so Linux CI is
   fine, but the Windows runner arrives only in Plan 2 and must solve linkage there —
   `vcpkg install opus:x64-windows-static`, or a `-sys` crate that vendors and statically
   links it as `audiopus_sys` reportedly does on Windows. Deferred deliberately, with the
   human partner's agreement; it is a Plan 2 blocker, not a Plan 1 one.

5. **Colleague hardware.** 0.25× realtime was measured on a Core Ultra 7 with 14 threads.
   An older laptop at 0.8× realtime turns a 90-minute recording into a 72-minute wait.
   Measure on a real colleague machine early — it may justify offering a smaller model as
   a "faster, less accurate" option.

## Reference: the validated manual pipeline

The behaviour this app must reproduce, verified end-to-end on a 58m30s Portuguese recording
(14.8 minutes of compute, 2553 cues → 141 sentences → 58 paragraphs → 12 A4 pages):

```bash
ffmpeg -y -i INPUT -ar 16000 -ac 1 -c:a pcm_s16le /tmp/a.wav
whisper-cli -m ggml-large-v3-turbo-q5_0.bin -f /tmp/a.wav \
  -l pt -t 12 -pp -otxt -osrt -of OUTPUT
```
