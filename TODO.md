# TODO

Ordered by what blocks what. Reasoning and evidence for most of these lives in
[`docs/superpowers/plan-3-carry-forward.md`](docs/superpowers/plan-3-carry-forward.md);
this file is the short actionable version.

## Uncommitted right now

- [ ] **Commit the `GGML_NATIVE: "OFF"` fix** in `ci.yml` and `release.yml`. Until it lands,
      CI stays red and every installer is built for the runner's CPU. This is the most
      important thing in this file — see the next section for why.

## Before tagging v0.1.0

- [ ] **Confirm CI is green on both platforms** after the `GGML_NATIVE` fix. It failed on the
      first runs with `SIGILL: illegal instruction` (`AMX is not ready to be used!`) because
      `whisper-rs-sys` never passes `GGML_NATIVE`, so ggml defaulted to `-march=native` and
      `rust-cache` restored an AMX-tuned `libwhisper` onto a runner without AMX. The same
      mechanism would bake the build runner's CPU into shipped installers, so a colleague on
      an older machine gets a crash rather than a transcript. The `prefix-key` bump matters
      as much as the env var: nothing else invalidates a cached native build.
- [ ] **Install the Windows build on a real Windows machine.** Two things are verified only
      statically: that NSIS installs to `%LOCALAPPDATA%\Transcriba` rather than
      `Program Files` (the README's model-path escape hatch depends on it), and the CSP added
      in the final fix wave. Confirm the SmartScreen walkthrough in the README matches what
      actually appears.
- [ ] **Watch one real job on the new waiting screen.** The section spine was designed against
      headless screenshots with representative data, not live progress. Check the sections
      fill in step with the audio and that the time-remaining estimate is not wildly wrong
      early on.
- [ ] **Decide whether the ~2× memory peak is acceptable to ship.** `decode` copies the whole
      decoded signal for resampling, so peak is roughly twice the sample data on top of the
      model — about 2.4GB for a 90-minute recording. An 8GB laptop running Teams is exposed,
      and the failure mode is a silent OOM kill with no message. The fix is small: move the
      `Vec<f32>` into the resampler instead of cloning it.

## Then, in rough order of value

- [ ] **Extract `pipeline::run(input, opts, &mut dyn Progress)` into `transcriba-core`.**
      `app/src-tauri/src/commands.rs` currently *is* the pipeline, and the CLI and app have
      already diverged on four points (output-pair suffixes, percentage clamping, font
      resolution, thread control). Retires that drift and makes the Tauri command genuinely
      thin.
- [ ] **Typed errors across IPC.** The five hand-rolled `Enum(String)` types destroy structure
      at every boundary, so a Portuguese UI reports failures in English
      (`Erro: no speech was detected in this recording`). `thiserror` with `#[from]`, a typed
      kind over IPC, and the frontend owning the Portuguese strings and the
      Retry / Convert-to-MP3 / Choose-another-file actions.
- [ ] **A settings surface.** Model path, thread count, and force-CPU are all compile-time or
      environment-variable-only, in a product whose premise is "no terminal". The
      `TRANSCRIBA_MODEL_PATH` escape hatch currently requires launching from a shell; a
      "point me at a model file" button serves the same need without contradicting that.
      Force-CPU is also promised in the spec and implemented nowhere.
- [ ] **Enforce the single-job invariant in Rust.** `main.ts`'s `running` flag is the only
      guard; the backend will run N concurrent transcriptions, each spawning `num_cpus - 2`
      threads. Reachable today by reloading the webview mid-job.
- [ ] **Expand the language list.** Seven entries are hardcoded; the spec asks for every
      language whisper supports plus auto-detect. `auto` works, so nothing is broken — a Dutch
      recording just cannot be chosen explicitly.

## Smaller, worth doing when nearby

- [ ] Test `model_store::download()` by extracting `stream_to_part(&mut dyn Read, ...)` and
      driving it with a `Cursor`. It works — it fetched the model on the first real run — but
      has no coverage at all.
- [ ] Make `MIN_MODEL_BYTES` injectable so two tests stop writing ~500MB temp files each
      (~1GB concurrently, never measured on a CI runner).
- [ ] Assert `TRANSCRIBA_TEST_MODEL` is set when `CI` is. A typo in the workflow silently
      no-ops six whisper integration tests and CI goes green having tested none of the
      pipeline.
- [ ] Add three `assert_eq!(serde_json::to_value(..), json!({..}))` tests for the
      `Progress`/`Outcome` wire shapes. They are the whole IPC contract and are currently
      verified only by humans reading both sides; `commands.rs` has no tests.
- [ ] Let the backend adjudicate unknown extensions. The frontend allowlist means a `.wma`
      gets "não parece ser uma gravação" when `DecodeError::UnsupportedCodec` would have named
      the actual codec and suggested MP3 — which is what the spec asks for.
- [ ] Adopt `output::unique_path_set` in the CLI (`crates/cli/src/main.rs`), which still has
      the mismatched output-pair bug the app fixed.
- [ ] Pin or vendor a newer `linuxdeploy` so `NO_STRIP=1` can go. The bundle currently ships
      unstripped libraries, hence 104MB.
- [ ] Prune the 45 of 52 committed icon files that are iOS/Android/UWP assets no bundler here
      reads.
- [ ] Trim `app/src-tauri/src/lib.rs`'s `crate-type` to `["rlib"]` until mobile is real —
      it currently links three flavours of a comment-only library.
- [ ] Cosmetic: "1 minutos" for a one-minute file; a drop during a running job is silently
      ignored; extra dropped files beyond the first are discarded silently.

## Upstream

- [ ] **File the `whisper-rs` 0.16 `set_abort_callback_safe` bug.** It installs
      `trampoline::<F>` over the caller's closure type while the stored value is a
      `Box<dyn FnMut() -> bool>` fat pointer, so it reinterprets a trait object's data pointer
      as whatever the closure captured — unsound for every caller, and inert in practice. This
      repo works around it with the raw `set_abort_callback` API; a `#[cfg(test)]` counter
      asserts the callback actually fires. **Re-check on any version bump.**
- [ ] Consider reporting the linuxdeploy RELR issue, if it is not already known.
