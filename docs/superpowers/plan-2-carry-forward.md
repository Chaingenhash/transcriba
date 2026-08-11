# Plan 2 Carry-Forward

Findings from implementing Plan 1 (the core pipeline) that Plan 2 — the Tauri v2 GUI,
Windows CI and installers — must account for. Extracted from the per-task review ledger
before that scratch workspace was deleted.

**Verified state at the end of Plan 1:** 16 commits, 42 tests, `cargo fmt` and
`cargo clippy -D warnings` clean, whole-branch review passed. End-to-end verified twice on a
real 58m30s stereo 48kHz Portuguese recording — 7305 words against the manual pipeline's
7273, exact duration, verbatim opening and closing text.

## Blockers — must be solved in Plan 2

**libopus links dynamically; distribution needs static.**
`symphonia-adapter-libopus` is depended on with `default-features = false`, because its
`bundled` feature vendors and statically links libopus but requires `cmake` at build time.
Fine for a developer CLI; not fine for an installer handed to a colleague with no libopus.
Either enable `bundled` (cmake is now installed locally, and CI declares it) or bundle the
shared library into the AppImage and Windows installer. On Windows,
`vcpkg install opus:x64-windows-static` is the documented route.

**Vulkan on Windows is expected to fail.**
whisper.cpp has an open bug where the Vulkan backend silently fails to register on Windows
MSVC static builds — a static-init race throws inside `ggml_vk_instance_init()`, the backend
never registers, and whisper reports "no GPU found" while devices are visible. Verify
empirically; ship Windows CPU-only if confirmed. GPU must be opportunistic with the backend
it actually used displayed, because there are three documented silent-fallback paths.

**The CLI binary is not relocatable.**
`crates/cli/src/main.rs` resolves the font directory through `env!("CARGO_MANIFEST_DIR")`,
so a copied or `cargo install`ed binary fails at PDF render. Replace with Tauri's resource
resolver. Until then, do not hand anyone the bare binary.

**`whisper-rs` 0.16.0's `set_abort_callback_safe` is unsound — do not call it.**
It installs `trampoline::<F>` over the caller's closure type while the stored value is a
`Box<dyn FnMut() -> bool>` fat pointer, so it reinterprets a trait object's data pointer as
whatever the closure captured. The sibling `set_progress_callback_safe` is written correctly,
which is why the bug is not obvious. `transcribe.rs` works around it with the raw
`set_abort_callback` / `set_abort_callback_user_data` API and a `*const AtomicBool`; three
reviewers verified that workaround sound against vendored whisper.cpp and ggml-cpu sources.
A `#[cfg(test)]` counter inside the callback now asserts it actually fires. **Re-check this on
any version bump** — and consider filing upstream.

## API changes worth making before a GUI depends on the current shape

**`transcribe()` cannot report which backend ran.** It returns a bare `Vec<Cue>`, while
`DocumentMeta.backend` is a string the *caller* invents (`main.rs` hardcodes
`format!("CPU ({threads} threads)")`). The spec calls backend reporting "the substance of"
the GPU feature — so the component that knows cannot report, and the one that reports does
not know. Return `Transcript { cues, backend }` instead. Five minutes now, a breaking change
once a GUI is built on `Vec<Cue>`.

**Add `model_store::ensure_available(&mut dyn FnMut(u64, Option<u64>)) -> Result<PathBuf>`.**
The resolve-or-download sequence currently lives only in `main.rs`, so the GUI would
reimplement it. It is the only pipeline orchestration outside the library, and extracting it
gains test coverage for free.

**Consider `pipeline::run(input, opts, &mut dyn Progress)`** carrying the spec's documented
progress bands (decode 0-5%, transcribe 5-95%, render 95-100%). Nothing implements that
banding today — the CLI forwards whisper's raw percentage. This is also where the docx/pdf
pair should share one suffix: `unique_path` is currently called twice independently, so the
two outputs mismatch if a user deletes one.

**Unify the five error enums.** All five are hand-rolled `Enum(String)` with hand-written
`Display` and no `source()`, so structure is destroyed at every boundary and the CLI flattens
everything to `Box<dyn Error>`. A GUI must map errors to actions ("Retry", "Convert to MP3",
"Choose another file") and would be reduced to string-matching. `thiserror` with `#[from]`
would preserve the chain. Concretely, `ModelError::Io(String)` has already discarded the
`io::ErrorKind` that would have made one fix cleaner.

**Document or enforce `Audio`'s invariant.** `Audio { samples, duration }` has public fields
and no rate marker; the 16kHz-mono guarantee is upheld by convention only, and `transcribe`
never validates it. Also `duration` is fully derivable from `samples.len()`, and `transcribe`
ignores it.

## Known gaps

**The spec promises a resumable download; the implementation is not one.** `download()`
always does `File::create` on the `.part` file and sends no `Range` header, so every retry
restarts from zero. The spec also promises "Retry, preserving partial progress." This is a
plan defect — the promise was written and the requirement dropped. Either implement resume
(`.part` length → `Range: bytes=N-`, append rather than create) or amend the spec.

**`download()` has no test coverage** — the streaming loop, progress callback, `.part` rename
and error mapping are verified only by source reading. Extract
`stream_to_part(reader: &mut dyn Read, part: &Path, total: Option<u64>, on_progress)` and
drive it with a `Cursor`; that covers everything except the `ureq::get` call itself.

**No README, and most of the public API is undocumented.** `lib.rs` opens with a version
comment rather than a `//!` block. For a library whose stated purpose is consumption by a
second plan, this is the largest production-readiness gap.

**Two upstream per-call leaks**, negligible for a one-shot CLI but unbounded across a
long-lived GUI session, because `FullParams` has no `Drop` impl:
`set_progress_callback_safe` leaks its boxed closure, and `set_language` leaks a `CString`
via `into_raw()`.

**No fixture covers the `source_rate == TARGET_RATE` skip-resample branch** — the only
uncovered branch in `decode`, and the one a 16kHz input takes. One `ffmpeg -ar 16000` line.

**`model_store::resolve()` has a theoretical TOCTOU** between `exists()` and `verify()`.
Single-user local cache; not worth fixing unless it bites.

## Operational note — verifying this tool

A full run on a 58-minute recording takes ~25 minutes, which exceeds every available
foreground timeout, and plain background jobs are reaped with their parent shell — three
attempts died at 22%, 37% and mid-run, including one under `setsid nohup`. Only a transient
systemd user unit survived:

```bash
systemd-run --user --collect --unit=transcriba-e2e \
  --setenv=TRANSCRIBA_MODEL_PATH="$HOME/.local/share/whisper-models/ggml-large-v3-turbo-q5_0.bin" \
  --property=StandardError=file:/tmp/e2e_err.log \
  --working-directory="$PWD" \
  ./target/release/transcriba /tmp/meeting.mp3 --lang pt
```

Whoever wires release verification needs this, or a real terminal.

## Process notes

**Record resolved dependency versions before anything depends on them.** Every version in
Plan 1's original text was wrong — `symphonia` 0.5→0.6, `rubato` 0.16→4.0, `pdf-extract`
0.7→0.12 (0.7 does not exist), `audiopus` 0.3→only a 0.3.0-rc.0 pre-release, `ureq` 2→3,
`dirs` 5→6. The symphonia error was the dangerous one: 0.6 signals end-of-stream as
`Ok(None)` rather than an IO error, so the planned decode loop would have errored on every
successful file.

**The escalation test that worked:** *did the plan decide this, or fail to consider it?*
Nine findings were labeled plan-mandated; each was an oversight contradicting the plan's own
stated intent, so fixing increased compliance rather than overriding a decision. One case —
where the plan's whole Opus *approach* was unworkable — was escalated to the human and
approved. That line held, but only because the plan's author was adjudicating. **An
implementer working someone else's plan should escalate more readily**, since they cannot
distinguish oversight from unstated context.

**Prefer a test over a comment for an upstream workaround.** A comment cannot fail CI.

**Plan 1's plan body was never corrected**, only annotated in the scratch ledger. If Plan 2
reuses any of it, note that Task 5's code block still shows `symphonia = "0.5"`,
`rubato = "0.16"`, the old `Fft::new` signature and `IoError => break`, and Task 6 Step 1
omits `default-features = false`. Read the code, not the plan.
