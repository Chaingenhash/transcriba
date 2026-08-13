# Plan 3 Carry-Forward

What Plan 2 (the desktop app) closed, what it left, and what the whole-branch review
identified as the next round's work. Supersedes `plan-2-carry-forward.md`, which remains as
the record of what Plan 1 handed over.

**State at the end of Plan 2:** 54 tests under default features plus 13 under the `vulkan`
feature. `cargo fmt` and `cargo clippy -D warnings` clean. A working 104MB Linux AppImage with
its bundled fonts verified by extraction. CI covers Linux and Windows; a release workflow
publishes installers on a `v*` tag.

## Closed by Plan 2

- **libopus links statically.** `symphonia-adapter-libopus` uses default features; `ldd` shows
  no dynamic opus link. An installer no longer assumes libopus on the target machine.
- **`transcribe()` reports the backend that ran.** `Transcript { cues, backend }`, with GPU
  reported only on an observed positive signal from whisper.cpp's own log line — never
  inferred from the feature flag.
- **`model_store::ensure_available()`** exists, so no front end reimplements resolve-or-download.
- **Font resolution from bundled resources**, replacing `CARGO_MANIFEST_DIR` in the app.
- **The docx/pdf output pair shares one suffix** via `output::unique_path_set`.

## Verified 2026-08-13 — no longer open

**The AppImage works end to end.** A human ran
`Transcriba_0.1.0_amd64.AppImage --appimage-extract-and-run`, dropped an audio file, and got
both a `.docx` and a `.pdf`. Three things that had never been exercised were confirmed at once:

- **Bundled font resolution.** The list form of `resources` mangled `../` into literal `_up_`
  segments and broke PDF rendering in every installed copy until two commits before merge. That
  bug is unreachable from `tauri dev`, which reads fonts from the source tree. A rendered PDF
  from the bundle is the only proof that exists, and now it does.
- **`model_store::download()`**, which has zero test coverage — no mock server, and tests must
  not fetch 574MB. It streamed the model into
  `~/.local/share/transcriba/models/ggml-large-v3-turbo-q5_0.bin`, verified it, renamed it into
  place, and the app loaded it. First attempt, in production.
- **The whole GUI flow**: drop → decode → transcribe → reflow → render → two files beside the
  input.

The app correctly reported `use gpu = 0` / `no GPU found`, confirming installers are CPU-only.

**Still open:** one real Windows NSIS install, to close the `%LOCALAPPDATA%\Transcriba` install
path claim and the CSP added in the final fix wave (both verified statically only).

## Enable the GPU feature — measured, not predicted

Benchmarked 2026-08-13 on a 5-minute Portuguese clip with the production model, same machine
(Intel Meteor Lake iGPU, Mesa, 12 threads):

| Build | Wall clock | Reported backend | Words |
|---|---|---|---|
| default (CPU) | **431s** | `CPU (12 threads)` | 638 |
| `--features vulkan` | **110s** | `GPU (Vulkan0)` | 637 |

**3.9× faster.** The plan predicted 1.5–2× on integrated graphics and set an off-ramp at
"under 20%, leave it off". That was wrong in the good direction: a 58-minute recording drops
from ~25 minutes to roughly 6. Word counts differ by one, so nothing is traded for the speed.

This also validated `gpu_detect` on real hardware: whisper.cpp logged
`whisper_backend_init_gpu: using Vulkan0 backend` and the log-line detection reported
`GPU (Vulkan0)` in the document header — the mechanism works outside unit tests.

`vulkan-headers` is now installed, so `--features vulkan` builds with no `VULKAN_SDK`
workaround.

**The remaining decision is per-platform, not a flag flip.** whisper.cpp's Vulkan backend is
still expected to fail to register on Windows MSVC static builds, so enabling it means: Linux
releases with `vulkan`, Windows CPU-only until proven otherwise, and a runtime force-CPU
setting for colleagues with misbehaving drivers (which the spec promises and nothing
implements).

## Memory: the >8GB assumption is optimistic, not safe

A GUI run was OOM-killed on this 15GB machine (`anon-rss` 1.37GB, `oom_score_adj` 200 because
it was launched from Nautilus, which marks its children as preferred victims). The immediate
cause was concurrent load — a release build and two benchmarks running alongside it — but the
underlying exposure is real:

- `decode` copies the whole decoded signal for resampling (`let input_data = vec![input.to_vec()]`),
  so peak is roughly **2× the sample data** on top of the model. A 90-minute recording is ~2.4GB.
- A colleague on an 8GB laptop with Teams and a browser open is genuinely at risk, and the
  failure mode is a silent kill with no error message.

Fix: take the `Vec<f32>` by value and move it into the resampler instead of cloning. Also worth
considering: launching from a file manager inherits `oom_score_adj=200`, so the app is the
kernel's first choice under pressure — a note in the README, or an explicit `oom_score_adj`
reset, would help.

## Architecture — the top item

**Extract `pipeline::run(input, opts, &mut dyn Progress)` into `transcriba-core`.**
`app/src-tauri/src/commands.rs` currently *is* the pipeline: ensure-model → decode →
transcribe → reflow → render → write → count paragraphs. Its own doc comment claims all
pipeline logic lives in core; that is no longer true.

The predicted consequence has already materialised — the CLI and the app have diverged:

| | app | CLI |
|---|---|---|
| Output pair suffix | shared (`unique_path_set`) | independent, can mismatch |
| Whisper percentage | clamped | unclamped |
| Font directory | bundled resource | `CARGO_MANIFEST_DIR`, not relocatable |
| Thread control | none | `--threads` |

A third front end would reimplement the orchestration, the phase→percentage banding (currently
*split* between `commands.rs` phases and `main.ts` percentages), the paragraph count, and the
audio-extension allowlist.

**Enforce the single-job invariant in Rust.** `main.ts`'s `running` flag is the only guard. The
backend will happily run N concurrent transcriptions, each spawning `num_cpus - 2` threads —
against the spec's "must not monopolise the machine". Reachable today by reloading the webview
mid-job. Reject in `transcribe_file` when `Jobs` is non-empty.

## Errors — now producing user-visible defects

Plan 2 added a tagged `CommandError { kind: "cancelled" | "failed", message }` so cancellation
stops being rendered as a red error. That was the minimum fix. The underlying problem stands:

**The five hand-rolled `Enum(String)` error types destroy structure at every boundary.** The
frontend receives prose and cannot translate it or map it to an action. So a Portuguese UI aimed
at Portuguese-speaking colleagues reports `Erro: no speech was detected in this recording` and
`Erro: this file uses ... audio, which isn't supported yet` in English.

Adopt `thiserror` with `#[from]` and `source()`, carry a typed kind across IPC, and let the
frontend own the Portuguese strings and the "Retry" / "Convert to MP3" / "Choose another file"
actions. Note `ModelError::Io(String)` has already discarded the `io::ErrorKind` that would make
one existing fix cleaner.

The spec's "Every error names the file, says what went wrong, and offers one action" is still
unmet in the GUI — it never names the file and offers no action.

## Decide what to do about the GPU feature

`release.yml` builds default features, so **every installer is CPU-only** and the backend line
will always read "CPU (N threads)". The honest-reporting machinery works and currently reports
on a feature no user has. Nothing in the README says releases are CPU-only.

Also unresolved: the spec promises "a setting to force CPU covers colleagues with misbehaving
drivers", and with `vulkan` compiled in there is no runtime way to opt out.

Blocking measurement: the system `vulkan-headers` package is not installed, so
`--features vulkan` needs `VULKAN_SDK` pointed at a headers tree. Install
`vulkan-headers` (available in `extra`), then measure on `/tmp/meeting5.mp3` with the production
model. Expectation to test against: integrated graphics share memory bandwidth with the CPU, so
1.5–2× at best. **If the gain is under ~20%, leave the feature off and say so** — a build-time
complication that buys nothing is worth rejecting.

## A settings surface

Model path, thread count, and force-CPU are all compile-time or environment-variable-only, in a
product whose first stated goal is "no terminal". The `TRANSCRIBA_MODEL_PATH` escape hatch —
the documented remedy when a corporate network blocks HuggingFace — currently requires
launching the app from a terminal. A "point me at a model file" button would serve the same need
without contradicting the premise.

## Smaller items

- **`whisper-rs` 0.16.0's `set_abort_callback_safe` is unsound** — it installs
  `trampoline::<F>` over the caller's closure type while the stored value is a
  `Box<dyn FnMut() -> bool>` fat pointer. `transcribe.rs` works around it with the raw
  `set_abort_callback` API. **Re-check on any version bump**; a `#[cfg(test)]` counter inside
  the callback asserts it actually fires. Worth filing upstream.
- **`gpu_detect::observe` restores the *default* whisper log callback, not the previous one**, so
  as public API it silently destroys a consumer's hook. Whisper's C API offers no way to read the
  prior callback; document the limitation.
- **`CAPTURE_LOCK` serializes observers, not loggers.** With two concurrent `transcribe` calls
  through the public API, a matching log line on another thread writes into this thread's slot
  unsynchronized. Unreachable in the shipped app (one job at a time, feature off), but real.
  An `AtomicPtr` or a `Mutex<Option<String>>` slot closes it. `observe` is also not re-entrant.
- **`model_store::download()` has no test coverage.** Extract
  `stream_to_part(&mut dyn Read, ...)` and drive it with a `Cursor`.
- **Two tests write ~500MB temp files each** to exercise the real `MIN_MODEL_BYTES` threshold,
  now ~1GB concurrently and never measured on a CI runner. Make the threshold injectable.
- **Nothing asserts `TRANSCRIBA_TEST_MODEL` is set in CI.** A typo in the workflow silently
  no-ops six whisper integration tests and CI goes green having tested none of the pipeline. One
  test that fails when `CI` is set and the variable is not costs three lines.
- **No test asserts the Rust↔TypeScript wire format.** The `Progress`/`Outcome` serde shapes are
  the whole IPC contract, verified only by humans reading both sides. Three
  `assert_eq!(serde_json::to_value(..), json!({..}))` tests make a rename fail CI instead of
  failing in a colleague's hands. `commands.rs` has zero tests.
- **CI never compiles `--features vulkan` and never runs `tauri build`.** The whole `gpu_detect`
  module and its 13 tests are invisible to CI and can rot. Add a non-blocking
  `cargo check -p transcriba-core --features vulkan` once headers are available, and a
  `tauri build` smoke step.
- **The language picker offers 7 languages**; the spec and the plan's own constraints both say
  every language whisper supports, plus auto-detect. A plan defect, not an implementation one —
  the plan's code block hardcoded seven. `auto` works, so a Dutch recording transcribes, it just
  cannot be selected explicitly. Generating ~99 `<option>`s is mechanical.
- **The frontend's extension allowlist pre-empts a better error.** Dropping a `.wma` yields
  "não parece ser áudio" when `DecodeError::UnsupportedCodec` would have named the actual codec
  and suggested MP3 — which is what the spec asks for. Let the backend adjudicate anything with
  an extension; keep the allowlist for the picker's filter only.
- **45 of 52 committed icon files** are iOS/Android/UWP assets unreachable by the appimage/nsis
  bundlers. Prune when someone next touches icons.
- **The CLI still has the mismatched output-pair bug** (`crates/cli/src/main.rs`). The helper now
  exists; adopting it is two lines.
- **`app/src-tauri/src/lib.rs`** is a comment-only stub with a `staticlib`/`cdylib`/`rlib`
  crate-type, so every build links three flavours of an empty library. Trim to `["rlib"]` until
  mobile actually arrives.
- **The Linux system-package line is duplicated** between `ci.yml` and `release.yml` and has
  already drifted once. A composite action would stop it.
- **Cosmetic:** "1 minutos" for a one-minute file; a drop during a running job is silently
  ignored; extra dropped files beyond the first are discarded silently; `Preparing` reports 0%
  for the whole download when the server omits `Content-Length`.

## Process notes

**Record resolved dependency versions before anything depends on them.** Every version in Plan
1's original text was wrong. Plan 2's were mostly right because Plan 1's Task 1 established the
habit.

**Prose claims got challenged; code blocks did not.** Three of Plan 2's stated mechanisms were
disproved by implementers reading vendored source — the `cancel_job` delivery path, the
drag-drop payload cast, and the CI step-ordering justification. But the whole-branch review then
found a Critical defect copied verbatim from a plan *code block*: release globs pointing at
`app/src-tauri/target/` when the workspace target is at the root, masked by
`fail_on_unmatched_files: false`. **Paths and globs in plan snippets are unverified until
something matches them.** State that in Plan 3's preamble, and make every plan that writes a
glob prove it with an `ls`.

**Verifying this tool needs to escape the session cgroup.** A full run on a 58-minute recording
takes ~25 minutes, exceeding every available foreground timeout, and plain background jobs are
reaped with their parent shell. Only a transient systemd unit survived:

```bash
systemd-run --user --collect --unit=transcriba-e2e \
  --setenv=TRANSCRIBA_MODEL_PATH="$HOME/.local/share/whisper-models/ggml-large-v3-turbo-q5_0.bin" \
  --property=StandardError=file:/tmp/e2e.log --working-directory="$PWD" \
  ./target/release/transcriba /tmp/meeting.mp3 --lang pt
```

**AppImage bundling needs `NO_STRIP=1`** on this machine and plausibly on `ubuntu-latest`:
linuxdeploy's vendored `strip` cannot parse the `SHT_RELR` sections current binutils emits. The
release workflow sets it. Consequence: the bundle ships unstripped libraries, hence 104MB. Pin a
newer linuxdeploy to fix properly.
