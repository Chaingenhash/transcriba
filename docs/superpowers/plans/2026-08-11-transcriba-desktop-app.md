# Transcriba Desktop App Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Tauri v2 desktop app that a non-technical colleague installs, drops an audio file into, and gets back a readable `.docx` and `.pdf` — fully offline, on Linux and Windows.

**Architecture:** A new `app/` directory holds the Tauri shell (`app/src-tauri`, Rust) and a vanilla-TypeScript frontend (`app/src`), added as a third workspace member. The shell is thin: one async command that calls `transcriba-core` and streams progress back over a Tauri channel. No pipeline logic lives in the shell — the GUI and the existing CLI must both be replaceable front ends over the same library.

**Tech Stack:** Tauri 2.11, `@tauri-apps/api` 2.11, Vite + vanilla TypeScript (no UI framework — the interface is a drop zone, a select, a progress bar and a result line), `tauri-plugin-dialog` for the file-picker fallback.

## Global Constraints

- `transcriba-core` must never depend on Tauri or any GUI crate. The shell depends on core; never the reverse.
- All processing local. The only network call remains the one model download in `model_store`.
- Output written beside the input file; existing files get a numeric suffix, never overwritten.
- CI runs `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --locked -- -D warnings`; any warning fails the build.
- `Cargo.lock` is tracked. Dependency additions must include its update.
- Default thread count stays `num_cpus - 2`, minimum 1 — a colleague must keep working during a 90-minute job.
- Target platforms: Linux and Windows. By the end of this plan CI must build and test both.
- Installers ship **unsigned** — the human partner accepted the Windows SmartScreen warning rather than a €200–400/year certificate.
- Portuguese is preselected in the language picker; every language whisper supports is selectable, plus an auto-detect entry.
- GPU is **opportunistic**: try it, fall back to CPU, and always display which backend actually ran.
- The verified baseline for end-to-end checks: a 58m30s Portuguese recording yields ~7305 words, duration 58:30, 6 section headings, 9 PDF pages.

## Read First

- `docs/superpowers/plan-2-carry-forward.md` — every blocker and API change this plan acts on, written at the end of Plan 1.
- `docs/superpowers/specs/2026-08-05-local-transcription-app-design.md` — the approved design.

## File Structure

```
crates/core/            existing library — modified in Tasks 1-2 only
crates/cli/             existing CLI — updated to the new core API in Task 1
app/
  package.json          frontend deps and scripts
  vite.config.ts        dev server on a fixed port for Tauri
  index.html            single page
  src/
    main.ts             wiring: drop zone, picker, language, progress, results
    api.ts              the only file that touches @tauri-apps — typed wrappers
    styles.css
  src-tauri/
    Cargo.toml
    tauri.conf.json     window, bundle targets, resources
    build.rs
    capabilities/default.json
    icons/              generated
    src/
      main.rs           builder, plugins, command registration — thin
      commands.rs       the transcribe command and its payload types
      jobs.rs           cancellation registry (job id -> AtomicBool)
assets/fonts/           existing — becomes a bundled Tauri resource
```

`api.ts` exists so that every Tauri import sits behind one typed boundary; `main.ts` never imports `@tauri-apps` directly. That keeps the DOM wiring testable in isolation and makes the drag-drop quirk (below) a single-file concern.

---

### Task 1: Return the backend from `transcribe`, and add `ensure_available`

Two API changes the carry-forward doc flags as cheap now and breaking later. Do these before any GUI exists.

**Files:**
- Modify: `crates/core/src/transcribe.rs`, `crates/core/src/lib.rs`, `crates/core/src/model_store.rs`, `crates/cli/src/main.rs`

**Interfaces:**
- Consumes: `reflow::Cue`, `decode::Audio`
- Produces:
  - `pub enum Backend { Cpu { threads: usize }, Gpu { name: String } }`
  - `impl std::fmt::Display for Backend` — `"CPU (12 threads)"` / `"GPU (name)"`
  - `pub struct Transcript { pub cues: Vec<Cue>, pub backend: Backend }`
  - `pub fn transcribe(...) -> Result<Transcript, TranscribeError>` (same parameters as before)
  - `pub fn model_store::ensure_available(on_progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<PathBuf, ModelError>`

- [ ] **Step 1: Write the failing tests**

Add to the test module in `crates/core/src/transcribe.rs`:

```rust
    #[test]
    fn backend_displays_cpu_thread_count() {
        assert_eq!(Backend::Cpu { threads: 12 }.to_string(), "CPU (12 threads)");
        assert_eq!(Backend::Cpu { threads: 1 }.to_string(), "CPU (1 thread)");
    }

    #[test]
    fn backend_displays_gpu_name() {
        assert_eq!(
            Backend::Gpu { name: "Intel Graphics".into() }.to_string(),
            "GPU (Intel Graphics)"
        );
    }

    #[test]
    fn transcript_reports_the_cpu_backend_it_ran_on() {
        let Some(opts) = tiny_model_opts() else { return };
        let t = transcribe(&tone(2.0, 0.2), &opts, &mut |_| {}, &|| false);
        // Either it transcribed something or it found no speech; both are fine here.
        // What matters is that a success carries a backend describing this run.
        if let Ok(transcript) = t {
            assert!(matches!(transcript.backend, Backend::Cpu { threads } if threads == opts.threads));
        }
    }
```

Note the third test reuses the `tone` helper added in Plan 1's final fix wave (`tone(secs, amplitude)`); confirm its exact signature in the file before writing.

Add to the test module in `crates/core/src/model_store.rs`:

```rust
    #[test]
    fn ensure_available_returns_the_override_without_downloading() {
        let mut bytes = vec![0u8; MIN_MODEL_BYTES as usize + 1];
        bytes[..4].copy_from_slice(b"lmgg");
        let path = temp_file("ensure-override.bin", &bytes);
        std::env::set_var("TRANSCRIBA_MODEL_PATH", &path);
        let mut called = false;
        let got = ensure_available(&mut |_, _| called = true).expect("resolves");
        std::env::remove_var("TRANSCRIBA_MODEL_PATH");
        assert_eq!(got, path);
        assert!(!called, "must not report download progress when the model already exists");
        std::fs::remove_file(path).ok();
    }
```

Reuse the existing `temp_file` helper. Consider using `File::set_len` for the large allocation as the final fix wave did elsewhere; check what that module currently does and match it.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --package transcriba-core backend_ ensure_available`
Expected: FAIL — `cannot find type Backend` (E0412), `cannot find function ensure_available` (E0425). Capture the literal output.

- [ ] **Step 3: Implement `Backend` and `Transcript`**

In `crates/core/src/transcribe.rs`, add above `transcribe`:

```rust
/// Which compute backend actually executed a run.
///
/// whisper.cpp has documented paths where a GPU build silently falls back to
/// CPU, so this is reported rather than assumed — the value is produced by the
/// code that ran, not by the caller.
#[derive(Debug, Clone, PartialEq)]
pub enum Backend {
    Cpu { threads: usize },
    Gpu { name: String },
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Backend::Cpu { threads: 1 } => write!(f, "CPU (1 thread)"),
            Backend::Cpu { threads } => write!(f, "CPU ({threads} threads)"),
            Backend::Gpu { name } => write!(f, "GPU ({name})"),
        }
    }
}

/// A finished transcription plus the backend that produced it.
#[derive(Debug)]
pub struct Transcript {
    pub cues: Vec<Cue>,
    pub backend: Backend,
}
```

Change `transcribe`'s return type to `Result<Transcript, TranscribeError>`. At the end, where it currently returns `Ok(cues)`, return:

```rust
    Ok(Transcript {
        cues,
        backend: Backend::Cpu { threads: opts.threads },
    })
```

Task 7 replaces that hardcoded `Cpu` with real detection. Leaving it as `Cpu` now is correct: this build has no GPU feature enabled, so CPU is the truth today.

- [ ] **Step 4: Implement `ensure_available`**

In `crates/core/src/model_store.rs`:

```rust
/// Returns a usable model path, downloading it first if necessary.
///
/// This is the whole find-or-fetch sequence in one call so that every front end
/// — the CLI and the desktop app — shares it rather than reimplementing it.
pub fn ensure_available(
    on_progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<PathBuf, ModelError> {
    if let Some(path) = resolve()? {
        return Ok(path);
    }
    let dest = cache_dir()?.join(MODEL_FILENAME);
    download(&dest, on_progress)?;
    Ok(dest)
}
```

- [ ] **Step 5: Update the CLI to the new API**

In `crates/cli/src/main.rs`, replace the inline resolve-or-download block with `model_store::ensure_available`, and take the backend from the transcript instead of inventing the string:

```rust
    eprintln!("Preparing the speech model...");
    let mut last = u64::MAX;
    let model_path = model_store::ensure_available(&mut |done, total| {
        let pct = total.map(|t| done * 100 / t.max(1)).unwrap_or(0);
        if pct != last {
            last = pct;
            eprint!("\r  {pct}%");
        }
    })?;
    if last != u64::MAX {
        eprintln!("\r  done.");
    }
```

and after transcription:

```rust
    let transcript = transcribe::transcribe(&audio, &opts, &mut progress, &|| false)?;
    eprintln!("\r  done — {} segments", transcript.cues.len());

    let blocks = reflow::reflow(&transcript.cues);
    let meta = render::DocumentMeta {
        title,
        duration: audio.duration,
        backend: transcript.backend.to_string(),
    };
```

Keep `DocumentMeta.backend` a `String` — the renderers only ever print it, and making them depend on `transcribe::Backend` would couple two modules that have no other relationship.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `TRANSCRIBA_TEST_MODEL=$PWD/tests/fixtures/ggml-tiny.bin cargo test --workspace --locked`
Expected: PASS, 45 tests (42 prior + 3 new).

Then `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --locked -- -D warnings`.

- [ ] **Step 7: Commit**

```bash
git add crates/core/src/transcribe.rs crates/core/src/model_store.rs crates/core/src/lib.rs crates/cli/src/main.rs Cargo.lock
git commit -m "feat(core): report the backend that ran, add model_store::ensure_available"
```

---

### Task 2: Link libopus statically

Plan 1's carry-forward doc lists this as a distribution blocker: the app currently links against whatever libopus is on the build machine, so an installer handed to a colleague without it fails at Opus decoding. `cmake` is now installed, which is what the `bundled` feature needed.

**Files:**
- Modify: `crates/core/Cargo.toml`

**Interfaces:** no API change.

- [ ] **Step 1: Enable the bundled feature**

In `crates/core/Cargo.toml`, replace the `symphonia-adapter-libopus` line and its explanatory comment with:

```toml
# `bundled` vendors libopus and links it statically, so a shipped installer does
# not depend on the target machine having libopus. It needs `cmake` at build
# time (and therefore in CI). Plan 1 shipped with default-features = false
# because cmake was unavailable then; see docs/superpowers/plan-2-carry-forward.md.
symphonia-adapter-libopus = "0.3"
```

- [ ] **Step 2: Verify it builds and Opus still decodes**

Run: `cargo build --package transcriba-core`
Expected: succeeds, slower than usual — libopus now compiles from source.

Run: `cargo test --package transcriba-core decode`
Expected: PASS, all 7 decode tests including `decodes_opus`.

If the build fails for a missing tool, report what is missing rather than reverting the feature. Do not silently go back to dynamic linking — that would re-open the blocker this task exists to close.

- [ ] **Step 3: Prove the link is actually static**

Run: `ldd target/debug/transcriba 2>/dev/null | grep -i opus || echo "no dynamic opus link"`
Expected: `no dynamic opus link`.

This is the actual acceptance criterion. A successful build proves nothing about linkage, so capture this output in your report — it is the only evidence that distinguishes the fix from the bug.

- [ ] **Step 4: Verify the whole suite and commit**

Run: `TRANSCRIBA_TEST_MODEL=$PWD/tests/fixtures/ggml-tiny.bin cargo test --workspace --locked`, then fmt and clippy.

```bash
git add crates/core/Cargo.toml Cargo.lock
git commit -m "build(core): vendor and statically link libopus"
```

---

### Task 3: Scaffold the Tauri app

**Files:**
- Create: `app/package.json`, `app/vite.config.ts`, `app/index.html`, `app/src/main.ts`, `app/src/styles.css`, `app/src-tauri/Cargo.toml`, `app/src-tauri/build.rs`, `app/src-tauri/tauri.conf.json`, `app/src-tauri/capabilities/default.json`, `app/src-tauri/src/main.rs`, `app/src-tauri/icons/` (generated)
- Modify: `Cargo.toml` (workspace members), `.gitignore`

**Interfaces:**
- Consumes: nothing yet
- Produces: a runnable `cargo tauri dev` window; workspace member `transcriba-app`

- [ ] **Step 1: Add the frontend**

`app/package.json`:

```json
{
  "name": "transcriba-app",
  "private": true,
  "version": "0.1.0",
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "tsc --noEmit && vite build",
    "preview": "vite preview"
  },
  "dependencies": {
    "@tauri-apps/api": "^2.11.1",
    "@tauri-apps/plugin-dialog": "^2"
  },
  "devDependencies": {
    "@tauri-apps/cli": "^2.11.4",
    "typescript": "^5",
    "vite": "^6"
  }
}
```

`app/vite.config.ts`:

```ts
import { defineConfig } from "vite";

// Tauri expects a fixed port and must not have vite pick another one silently.
// es2022 (not es2021) because main.ts uses top-level await.
export default defineConfig({
  server: { port: 5173, strictPort: true },
  build: { target: "es2022", emptyOutDir: true },
});
```

`app/tsconfig.json` — required, since `npm run build` runs `tsc --noEmit`:

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "lib": ["ES2022", "DOM", "DOM.Iterable"],
    "strict": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "noFallthroughCasesInSwitch": true,
    "skipLibCheck": true,
    "noEmit": true,
    "isolatedModules": true,
    "verbatimModuleSyntax": true
  },
  "include": ["src"]
}
```

`app/index.html`:

```html
<!doctype html>
<html lang="pt">
  <head>
    <meta charset="UTF-8" />
    <title>Transcriba</title>
    <link rel="stylesheet" href="/src/styles.css" />
  </head>
  <body>
    <main id="app">
      <div id="drop" class="drop">
        <p class="drop-title">Arraste um ficheiro de áudio para aqui</p>
        <button id="pick" type="button">Escolher ficheiro</button>
      </div>
      <section id="controls" class="controls">
        <label for="lang">Idioma</label>
        <select id="lang"></select>
      </section>
      <section id="status" class="status" hidden>
        <p id="phase"></p>
        <progress id="bar" max="100" value="0"></progress>
        <button id="cancel" type="button">Cancelar</button>
      </section>
      <section id="result" class="result" hidden></section>
    </main>
    <script type="module" src="/src/main.ts"></script>
  </body>
</html>
```

`app/src/styles.css` — the complete stylesheet, written once here so Task 5 does not need to touch it:

```css
:root {
  --fg: #1c1c1e;
  --muted: #6b6b70;
  --line: #cfcfd4;
  --accent: #2f6f4f;
  --error: #8c2f2f;
  --bg: #fbfbfc;
  font-family: system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
}

* { box-sizing: border-box; }

body {
  margin: 0;
  color: var(--fg);
  background: var(--bg);
}

#app {
  max-width: 640px;
  margin: 0 auto;
  padding: 24px 20px;
  display: flex;
  flex-direction: column;
  gap: 18px;
}

.drop {
  border: 2px dashed var(--line);
  border-radius: 10px;
  padding: 36px 20px;
  text-align: center;
  background: #fff;
  transition: border-color 120ms, background 120ms;
}

.drop.busy { opacity: 0.6; pointer-events: none; }
.drop-title { margin: 0 0 14px; color: var(--muted); }

button {
  font: inherit;
  padding: 8px 16px;
  border: 1px solid var(--line);
  border-radius: 6px;
  background: #fff;
  cursor: pointer;
}

button:hover { border-color: var(--fg); }

.controls { display: flex; align-items: center; gap: 10px; }
.controls label { color: var(--muted); }
select { font: inherit; padding: 6px 8px; border: 1px solid var(--line); border-radius: 6px; }

.status { display: flex; flex-direction: column; gap: 10px; }
#phase { margin: 0; }
progress { width: 100%; height: 14px; }

.result { padding: 14px 16px; border-radius: 8px; border: 1px solid var(--line); background: #fff; }
.result p { margin: 0 0 8px; }
.result p:last-child { margin-bottom: 0; }
.result.ok { border-left: 4px solid var(--accent); }
.result.error { border-left: 4px solid var(--error); color: var(--error); }
.paths { font-family: ui-monospace, monospace; font-size: 12px; color: var(--muted); word-break: break-all; }
```

Colour is never the only signal — the success and error states differ in wording as well as in the left border.

`app/src/main.ts` for this task only:

```ts
const langs = document.querySelector<HTMLSelectElement>("#lang")!;
for (const [code, label] of [["pt", "Português"], ["en", "English"], ["auto", "Detetar automaticamente"]]) {
  const opt = document.createElement("option");
  opt.value = code;
  opt.textContent = label;
  langs.append(opt);
}
langs.value = "pt";
```

Task 5 replaces this with the full wiring and the complete language list.

- [ ] **Step 2: Install and confirm the frontend builds**

```bash
cd app && npm install && npm run build
```
Expected: a `dist/` directory, no TypeScript errors.

- [ ] **Step 3: Add the Tauri crate**

`app/src-tauri/Cargo.toml`:

```toml
[package]
name = "transcriba-app"
version.workspace = true
edition.workspace = true

[lib]
name = "transcriba_app_lib"
crate-type = ["staticlib", "cdylib", "rlib"]

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
tauri = { version = "2", features = [] }
tauri-plugin-dialog = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
transcriba-core = { path = "../../crates/core" }
```

`app/src-tauri/build.rs`:

```rust
fn main() {
    tauri_build::build()
}
```

`app/src-tauri/src/main.rs`:

```rust
// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .run(tauri::generate_context!())
        .expect("error while running the Transcriba app");
}
```

Add `"app/src-tauri"` to the workspace `members` in the root `Cargo.toml`.

- [ ] **Step 4: Write `tauri.conf.json`**

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "Transcriba",
  "version": "0.1.0",
  "identifier": "pt.chaingenhash.transcriba",
  "build": {
    "beforeDevCommand": "npm run dev",
    "beforeBuildCommand": "npm run build",
    "devUrl": "http://localhost:5173",
    "frontendDist": "../dist"
  },
  "app": {
    "windows": [
      {
        "title": "Transcriba",
        "width": 720,
        "height": 560,
        "minWidth": 520,
        "minHeight": 420,
        "dragDropEnabled": true
      }
    ],
    "security": { "csp": null }
  },
  "bundle": {
    "active": true,
    "targets": ["appimage", "nsis"],
    "icon": ["icons/32x32.png", "icons/128x128.png", "icons/icon.icns", "icons/icon.ico"],
    "resources": ["../../assets/fonts/*"]
  }
}
```

`dragDropEnabled` is stated explicitly even though it defaults to true, because Task 5 depends on it and its behaviour is counter-intuitive (see that task).

**Verify the `targets` values against `cargo tauri build --help` or the v2 config schema before relying on them** — the accepted identifiers have changed between Tauri versions, and a wrong value fails only at bundle time, in Task 8.

- [ ] **Step 5: Generate icons**

Create any 1024×1024 PNG as `app/icon-source.png` (a plain coloured square with a "T" is fine — this is a placeholder, not a design task), then:

```bash
cd app && npx tauri icon icon-source.png
```

This writes `src-tauri/icons/`. Commit the generated icons; do not gitignore them, since the bundler needs them and CI must not regenerate them.

- [ ] **Step 6: Write the capability file**

`app/src-tauri/capabilities/default.json`:

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "Capabilities the Transcriba window needs",
  "windows": ["main"],
  "permissions": ["core:default", "dialog:allow-open"]
}
```

Tauri v2 denies everything not listed here. If the app later fails at runtime with a permission error, this file is why — add the specific permission rather than widening to a wildcard.

- [ ] **Step 7: Update `.gitignore`**

Append:

```
/app/node_modules
/app/dist
/app/src-tauri/gen
/app/src-tauri/target
```

- [ ] **Step 8: Verify the app builds and runs**

Run: `cargo build --package transcriba-app`
Expected: succeeds.

Run: `cd app && npx tauri dev`
Expected: a window titled "Transcriba" with the drop zone and a language select containing three entries. Close it.

If you cannot open a window in this environment, say so in your report and confirm `cargo build --package transcriba-app` plus `npm run build` instead — do not claim you saw a window you did not see.

- [ ] **Step 9: Verify the workspace is still green and commit**

Run: `TRANSCRIBA_TEST_MODEL=$PWD/tests/fixtures/ggml-tiny.bin cargo test --workspace --locked`, then fmt and clippy across the whole workspace including the new crate.

```bash
git add app Cargo.toml Cargo.lock .gitignore
git commit -m "feat(app): scaffold the Tauri desktop shell"
```

Confirm `app/package-lock.json` is included — Task 8's CI uses `npm ci`, which requires it. Confirm `app/node_modules` and `app/dist` are NOT, per the `.gitignore` additions.

---

### Task 4: The transcribe command with streamed progress

**Files:**
- Create: `app/src-tauri/src/commands.rs`, `app/src-tauri/src/jobs.rs`
- Modify: `app/src-tauri/src/main.rs`

**Interfaces:**
- Consumes: `transcriba_core::{decode, model_store, reflow, render, output, transcribe}`; `transcribe::Transcript`, `Backend`; `model_store::ensure_available`
- Produces:
  - `#[derive(Serialize)] pub enum Progress { Preparing { pct: u8 }, Decoding, Transcribing { pct: u8 }, Rendering }`
  - `#[derive(Serialize)] pub struct Outcome { pub docx: String, pub pdf: String, pub backend: String, pub duration_secs: f64, pub paragraphs: usize }`
  - `#[tauri::command] pub async fn transcribe_file(app: AppHandle, jobs: State<'_, Jobs>, path: String, language: String, job_id: String, progress: Channel<Progress>) -> Result<Outcome, String>`
  - `#[tauri::command] pub fn cancel_job(jobs: State<'_, Jobs>, job_id: String)`
  - `pub struct Jobs` with `fn flag(&self, id: &str) -> Arc<AtomicBool>`, `fn cancel(&self, id: &str)`, `fn finish(&self, id: &str)`

- [ ] **Step 1: Write the cancellation registry with its tests**

`app/src-tauri/src/jobs.rs`:

```rust
//! Tracks the cancel flag for each running job so a `cancel_job` command can
//! reach a transcription already in flight.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct Jobs {
    inner: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl Jobs {
    /// Registers `id` and returns its cancel flag, replacing any prior entry.
    pub fn flag(&self, id: &str) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.inner
            .lock()
            .expect("jobs mutex poisoned")
            .insert(id.to_string(), Arc::clone(&flag));
        flag
    }

    /// Requests cancellation. Unknown ids are ignored — the job may have just finished.
    pub fn cancel(&self, id: &str) {
        if let Some(flag) = self.inner.lock().expect("jobs mutex poisoned").get(id) {
            flag.store(true, Ordering::Relaxed);
        }
    }

    pub fn finish(&self, id: &str) {
        self.inner.lock().expect("jobs mutex poisoned").remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelling_a_registered_job_sets_its_flag() {
        let jobs = Jobs::default();
        let flag = jobs.flag("a");
        assert!(!flag.load(Ordering::Relaxed));
        jobs.cancel("a");
        assert!(flag.load(Ordering::Relaxed));
    }

    #[test]
    fn cancelling_an_unknown_job_is_a_no_op() {
        let jobs = Jobs::default();
        jobs.cancel("nope");
    }

    #[test]
    fn finished_jobs_are_forgotten_and_no_longer_cancellable() {
        let jobs = Jobs::default();
        let flag = jobs.flag("a");
        jobs.finish("a");
        jobs.cancel("a");
        assert!(!flag.load(Ordering::Relaxed));
    }

    #[test]
    fn two_jobs_have_independent_flags() {
        let jobs = Jobs::default();
        let a = jobs.flag("a");
        let b = jobs.flag("b");
        jobs.cancel("a");
        assert!(a.load(Ordering::Relaxed));
        assert!(!b.load(Ordering::Relaxed));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --package transcriba-app jobs`
Expected: FAIL — the module does not exist yet if you have not added `mod jobs;`. Add it to `main.rs`, re-run, and capture the literal failing output before the implementation compiles.

- [ ] **Step 3: Confirm they pass**

Run: `cargo test --package transcriba-app jobs`
Expected: PASS, 4 tests.

- [ ] **Step 4: Write the command**

`app/src-tauri/src/commands.rs`:

```rust
//! The single command the frontend calls. All pipeline logic lives in
//! `transcriba-core`; this file only adapts it to Tauri's IPC.

use crate::jobs::Jobs;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::ipc::Channel;
use tauri::path::BaseDirectory;
use tauri::{AppHandle, Manager, State};
use transcriba_core::{decode, model_store, output, reflow, render, transcribe};

/// Progress phases, mapped to the spec's bands: preparing and decoding occupy
/// the first 5%, transcription 5-95%, rendering the last 5%.
#[derive(Serialize, Clone)]
#[serde(tag = "phase", rename_all = "camelCase")]
pub enum Progress {
    Preparing { pct: u8 },
    Decoding,
    Transcribing { pct: u8 },
    Rendering,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub docx: String,
    pub pdf: String,
    pub backend: String,
    pub duration_secs: f64,
    pub paragraphs: usize,
}

fn font_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .resolve("assets/fonts", BaseDirectory::Resource)
        .map_err(|e| format!("could not locate the bundled fonts: {e}"))
}

#[tauri::command]
pub async fn transcribe_file(
    app: AppHandle,
    jobs: State<'_, Jobs>,
    path: String,
    language: String,
    job_id: String,
    progress: Channel<Progress>,
) -> Result<Outcome, String> {
    let cancel = jobs.flag(&job_id);
    let result = run(&app, &path, &language, &cancel, &progress);
    jobs.finish(&job_id);
    result
}

fn run(
    app: &AppHandle,
    path: &str,
    language: &str,
    cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    progress: &Channel<Progress>,
) -> Result<Outcome, String> {
    use std::sync::atomic::Ordering;
    let input = Path::new(path);

    let mut last = u8::MAX;
    let model_path = model_store::ensure_available(&mut |done, total| {
        let pct = total.map_or(0, |t| (done * 100 / t.max(1)) as u8);
        if pct != last {
            last = pct;
            let _ = progress.send(Progress::Preparing { pct });
        }
    })
    .map_err(|e| e.to_string())?;

    let _ = progress.send(Progress::Decoding);
    let audio = decode::decode(input).map_err(|e| e.to_string())?;

    let opts = transcribe::Options {
        language: language.to_string(),
        threads: transcribe::default_threads(),
        model_path,
    };
    let mut last_pct = i32::MIN;
    let transcript = transcribe::transcribe(
        &audio,
        &opts,
        &mut |p| {
            if p != last_pct {
                last_pct = p;
                let _ = progress.send(Progress::Transcribing { pct: p.clamp(0, 100) as u8 });
            }
        },
        &|| cancel.load(Ordering::Relaxed),
    )
    .map_err(|e| e.to_string())?;

    let _ = progress.send(Progress::Rendering);
    let blocks = reflow::reflow(&transcript.cues);
    let meta = render::DocumentMeta {
        title: input
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
        duration: audio.duration,
        backend: transcript.backend.to_string(),
    };

    let docx_path = output::unique_path(&input.with_extension("docx"));
    std::fs::write(&docx_path, render::docx::render_docx(&blocks, &meta).map_err(|e| e.to_string())?)
        .map_err(|e| format!("could not write {}: {e}", docx_path.display()))?;

    let pdf_path = output::unique_path(&input.with_extension("pdf"));
    let fonts = font_dir(app)?;
    std::fs::write(
        &pdf_path,
        render::pdf::render_pdf(&blocks, &meta, &fonts).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("could not write {}: {e}", pdf_path.display()))?;

    Ok(Outcome {
        docx: docx_path.display().to_string(),
        pdf: pdf_path.display().to_string(),
        backend: meta.backend,
        duration_secs: audio.duration,
        paragraphs: blocks
            .iter()
            .filter(|b| matches!(b, reflow::Block::Para { .. }))
            .count(),
    })
}

#[tauri::command]
pub fn cancel_job(jobs: State<'_, Jobs>, job_id: String) {
    jobs.cancel(&job_id);
}
```

Errors cross the IPC boundary as `String` because everything returned from a Tauri command must implement `serde::Serialize`, and `transcriba-core`'s error types deliberately carry user-facing `Display` text. Using `e.to_string()` preserves exactly the messages Plan 1 wrote for humans — naming the actual codec, pointing at `TRANSCRIBA_MODEL_PATH`. Do not replace them with generic text.

`transcribe_file` is `async` so Tauri runs it off the UI thread, but `run` is synchronous and CPU-bound. **Verify whether Tauri v2 runs async commands on a runtime where a multi-minute blocking call is acceptable.** If it blocks other IPC, wrap `run` in `tauri::async_runtime::spawn_blocking` and await it. Report which you did and why.

- [ ] **Step 5: Register everything in `main.rs`**

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod jobs;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(jobs::Jobs::default())
        .invoke_handler(tauri::generate_handler![
            commands::transcribe_file,
            commands::cancel_job
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Transcriba app");
}
```

- [ ] **Step 6: Verify and commit**

Run: `cargo test --package transcriba-app`, then the full workspace suite, fmt and clippy.

```bash
git add app/src-tauri/src Cargo.lock
git commit -m "feat(app): transcribe command with streamed progress and cancellation"
```

---

### Task 5: The frontend

**Files:**
- Create: `app/src/api.ts`
- Modify: `app/src/main.ts`, `app/src/styles.css`, `app/index.html`

**Interfaces:**
- Consumes: the `transcribe_file` and `cancel_job` commands, `Progress`, `Outcome`
- Produces: a working UI

- [ ] **Step 1: Write the typed Tauri boundary**

`app/src/api.ts`:

```ts
import { invoke, Channel } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { open } from "@tauri-apps/plugin-dialog";

export type Progress =
  | { phase: "preparing"; pct: number }
  | { phase: "decoding" }
  | { phase: "transcribing"; pct: number }
  | { phase: "rendering" };

export interface Outcome {
  docx: string;
  pdf: string;
  backend: string;
  durationSecs: number;
  paragraphs: number;
}

const AUDIO_EXTENSIONS = ["mp3", "m4a", "mp4", "wav", "flac", "ogg", "opus", "mpeg", "mpga", "aac"];

export function looksLikeAudio(path: string): boolean {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return AUDIO_EXTENSIONS.includes(ext);
}

export async function pickFile(): Promise<string | null> {
  const picked = await open({
    multiple: false,
    directory: false,
    filters: [{ name: "Áudio", extensions: AUDIO_EXTENSIONS }],
  });
  return typeof picked === "string" ? picked : null;
}

/**
 * OS file drops never reach DOM drag events in Tauri — the native window layer
 * intercepts them first, and `dragDropEnabled` actively disables DOM drag/drop.
 * This webview event is the only way to receive dropped paths.
 */
export async function onFilesDropped(handler: (paths: string[]) => void): Promise<void> {
  await getCurrentWebviewWindow().onDragDropEvent((event) => {
    const payload = event.payload as { type: string; paths?: string[] };
    if (payload.type === "drop" && payload.paths?.length) {
      handler(payload.paths);
    }
  });
}

export async function transcribe(
  path: string,
  language: string,
  jobId: string,
  onProgress: (p: Progress) => void,
): Promise<Outcome> {
  const channel = new Channel<Progress>();
  channel.onmessage = onProgress;
  return invoke<Outcome>("transcribe_file", { path, language, jobId, progress: channel });
}

export async function cancel(jobId: string): Promise<void> {
  await invoke("cancel_job", { jobId });
}
```

**Verify `onDragDropEvent`'s payload shape against the v2 API docs before trusting the `{ type, paths }` destructuring above** — the discriminant may be `"drop"`, `"dropped"`, or nested differently, and getting it wrong produces a drop zone that silently ignores files. This is the single most likely thing in this task to be wrong. Report what the real shape is.

- [ ] **Step 2: Write the UI wiring**

`app/src/main.ts`:

```ts
import { cancel, looksLikeAudio, onFilesDropped, pickFile, transcribe, type Outcome, type Progress } from "./api";

const el = <T extends HTMLElement>(sel: string): T => document.querySelector<T>(sel)!;
const drop = el<HTMLDivElement>("#drop");
const pick = el<HTMLButtonElement>("#pick");
const lang = el<HTMLSelectElement>("#lang");
const status = el<HTMLElement>("#status");
const phase = el<HTMLParagraphElement>("#phase");
const bar = el<HTMLProgressElement>("#bar");
const cancelBtn = el<HTMLButtonElement>("#cancel");
const result = el<HTMLElement>("#result");

const LANGUAGES: [string, string][] = [
  ["pt", "Português"],
  ["auto", "Detetar automaticamente"],
  ["en", "English"],
  ["es", "Español"],
  ["fr", "Français"],
  ["de", "Deutsch"],
  ["it", "Italiano"],
];

for (const [code, label] of LANGUAGES) {
  const opt = document.createElement("option");
  opt.value = code;
  opt.textContent = label;
  lang.append(opt);
}
lang.value = "pt";

let running: string | null = null;

function describe(p: Progress): { text: string; value: number } {
  switch (p.phase) {
    case "preparing":
      return { text: `A preparar o modelo de voz… ${p.pct}%`, value: Math.min(5, p.pct / 20) };
    case "decoding":
      return { text: "A ler o áudio…", value: 5 };
    case "transcribing":
      return { text: `A transcrever… ${p.pct}%`, value: 5 + (p.pct * 90) / 100 };
    case "rendering":
      return { text: "A criar os documentos…", value: 95 };
  }
}

function showError(message: string) {
  status.hidden = true;
  result.hidden = false;
  result.className = "result error";
  result.textContent = message;
}

function showDone(o: Outcome) {
  status.hidden = true;
  result.hidden = false;
  result.className = "result ok";
  const mins = Math.round(o.durationSecs / 60);
  result.innerHTML = `
    <p><strong>Concluído.</strong> ${mins} minutos de áudio, ${o.paragraphs} parágrafos.</p>
    <p>Transcrito com ${o.backend}.</p>
    <p class="paths">${o.docx}<br />${o.pdf}</p>`;
}

async function start(path: string) {
  if (running) return;
  if (!looksLikeAudio(path)) {
    showError("Esse ficheiro não parece ser áudio. Escolha um ficheiro de áudio.");
    return;
  }
  running = `job-${Date.now()}`;
  drop.classList.add("busy");
  result.hidden = true;
  status.hidden = false;
  bar.value = 0;
  phase.textContent = "A começar…";

  try {
    const outcome = await transcribe(path, lang.value, running, (p) => {
      const { text, value } = describe(p);
      phase.textContent = text;
      bar.value = value;
    });
    showDone(outcome);
  } catch (e) {
    showError(String(e));
  } finally {
    running = null;
    drop.classList.remove("busy");
  }
}

pick.addEventListener("click", async () => {
  const path = await pickFile();
  if (path) await start(path);
});

cancelBtn.addEventListener("click", async () => {
  if (running) await cancel(running);
});

await onFilesDropped((paths) => {
  if (paths[0]) void start(paths[0]);
});
```

Note `showError(String(e))` surfaces the library's own message verbatim, since the command returns those strings as its error type. A user with an Opus file on a build without Opus support sees the codec named, not "something went wrong".

- [ ] **Step 3: Verify the frontend compiles**

Run: `cd app && npm run build`
Expected: no TypeScript errors, `dist/` produced.

- [ ] **Step 4: Verify by hand, honestly**

Run: `cd app && npx tauri dev`

Check, using the 5-minute slice at `/tmp/meeting5.mp3` if it still exists, or create one with
`ffmpeg -y -i <any long audio> -t 300 -c copy /tmp/meeting5.mp3`:

1. Dropping the file onto the window starts a job — **this is the step most likely to fail**, per the payload-shape warning in Step 1.
2. The file picker button also starts a job.
3. The progress bar advances and the phase text changes through preparing/decoding/transcribing/rendering.
4. On completion, the backend line reads "Transcrito com CPU (N threads)" and both output paths are shown.
5. `.docx` and `.pdf` exist beside the input and open correctly.
6. Cancel during transcription returns the app to a usable state with a cancellation message, not a crash.
7. Dropping a non-audio file (a `.txt`) shows the "not audio" message without starting a job.

Record which of these you actually observed. If you cannot open a GUI window in this environment, say so plainly and report which checks are therefore unverified — do not describe UI behaviour you did not see.

- [ ] **Step 5: Commit**

```bash
git add app/src app/index.html
git commit -m "feat(app): drop zone, language picker, progress and results UI"
```

---

### Task 6: Opportunistic GPU with honest backend reporting

The spec calls the backend display "the substance of" this feature, because whisper.cpp has three documented silent-fallback paths. A GPU build that quietly runs on CPU is the failure mode being designed against.

**Files:**
- Modify: `crates/core/Cargo.toml`, `crates/core/src/transcribe.rs`

**Interfaces:**
- Consumes: `Backend`, `Transcript` from Task 1
- Produces: `transcribe` returning a `Backend` reflecting what actually ran; a `vulkan` cargo feature on `transcriba-core`

- [ ] **Step 1: Add the feature, off by default**

In `crates/core/Cargo.toml`:

```toml
[features]
default = []
# Opportunistic Vulkan. Chosen over CUDA because the target machines are office
# laptops with Intel integrated graphics: one Vulkan build covers Intel, AMD and
# NVIDIA, whereas CUDA needs its toolkit at build time and only helps rare
# NVIDIA machines. Expected to be unusable on Windows MSVC static builds
# (whisper.cpp silently fails to register the backend there) — see the spec.
vulkan = ["whisper-rs/vulkan"]
```

- [ ] **Step 2: Determine how to detect the real backend, and report what you find**

This is a research step, not a coding step, and its outcome decides the rest of the task.

`whisper-rs` exposes hooks into whisper.cpp's log output (the crate advertises `log` and `tracing` backends). whisper.cpp prints `whisper_backend_init_gpu: no GPU found` when it falls back, and names the device when it succeeds. Investigate, in this order:

1. Whether `whisper-rs` 0.16 offers any direct query for the active backend or device name.
2. If not, whether its log hook can be installed and captured around the `WhisperContext` construction, so the GPU init line can be read.

Run `cargo doc --no-deps --open --package whisper-rs` and read the crate root for the logging API.

**Report which mechanism exists before writing detection code.** If neither works, the honest fallback is to report `Backend::Cpu` whenever the `vulkan` feature is off, and — when it is on — report GPU only if a positive signal was observed, never by assumption. A wrong "GPU" label is worse than no label, because it sends someone chasing a speedup they never had.

- [ ] **Step 3: Implement detection**

Set `Transcript.backend` from the mechanism found in Step 2. Structure it so the CPU path needs no detection at all: with the `vulkan` feature disabled, `Backend::Cpu { threads }` is correct by construction and no log parsing should run.

Add a unit test asserting that with default features, a successful transcription reports `Backend::Cpu` with the thread count from `Options` — a regression guard that a future GPU change cannot silently mislabel CPU runs.

- [ ] **Step 4: Measure whether Vulkan is worth enabling here**

```bash
cargo build --release --package transcriba-cli --features transcriba-core/vulkan
```

If it builds, run the 5-minute slice both ways and record wall-clock and the reported backend:

```bash
TRANSCRIBA_MODEL_PATH=$HOME/.local/share/whisper-models/ggml-large-v3-turbo-q5_0.bin \
  ./target/release/transcriba /tmp/meeting5.mp3 --lang pt
```

Expectation to hold you to: on integrated graphics the GPU shares memory bandwidth with the CPU, so a realistic gain is 1.5–2×, not 10×. **If the measured difference is under ~20%, say so and recommend leaving the feature off by default** — a build-time complication that buys nothing is worth rejecting.

Do not run the full 58-minute file. Use the 5-minute slice; that is enough to compare.

- [ ] **Step 5: Verify and commit**

Full workspace suite with default features, then fmt and clippy. Also confirm `cargo clippy --package transcriba-core --features vulkan` is clean if the feature builds at all.

```bash
git add crates/core/Cargo.toml crates/core/src/transcribe.rs Cargo.lock
git commit -m "feat(core): opportunistic vulkan behind a feature, with real backend reporting"
```

---

### Task 7: Bundle the app for Linux

**Files:**
- Modify: `app/src-tauri/tauri.conf.json`
- Create: `README.md`

**Interfaces:** none — this produces artifacts.

- [ ] **Step 1: Build the AppImage**

```bash
cd app && npx tauri build --bundles appimage
```

Expected: an AppImage under `app/src-tauri/target/release/bundle/appimage/`. Report its exact path and size.

If the build fails on a missing system library, report the missing package rather than working around it — Linux Tauri prerequisites are already installed on this machine (`webkit2gtk-4.1`, `libsoup-3.0`, `javascriptcoregtk-4.1` all verified present), so a failure here means something new.

- [ ] **Step 2: Verify the bundled fonts resolve at runtime**

This is the acceptance criterion that matters. `render_pdf` needs the four Liberation Serif files, and Task 4 resolves them via `BaseDirectory::Resource`. In a dev build that path may differ from a bundled one, so the AppImage must be tested directly:

```bash
cp /tmp/meeting5.mp3 /tmp/bundle-test.mp3
./app/src-tauri/target/release/bundle/appimage/*.AppImage
```

Transcribe `/tmp/bundle-test.mp3` through the GUI and confirm `/tmp/bundle-test.pdf` is produced. **A PDF failure here means the resource path is wrong** — the most likely bug in this task, and one a dev-mode run cannot catch.

If you cannot run a GUI in this environment, report that Step 2 is unverified and say so explicitly. Do not mark the task done claiming a bundle you never launched works.

- [ ] **Step 3: Write the README**

`README.md` at the repo root. Cover, concisely:

- what Transcriba does, in two sentences
- **for users:** install the AppImage or the Windows installer, drop a file, get a `.docx` and `.pdf` beside it; the model downloads once (~574MB) on first run; a run takes roughly 25 minutes per hour of audio on a typical laptop
- the Windows SmartScreen warning and how to get past it, since installers are unsigned and this will otherwise stop non-technical colleagues
- `TRANSCRIBA_MODEL_PATH` as the escape hatch when a corporate network blocks HuggingFace
- **for developers:** build prerequisites (Rust, cmake, libclang, node, and the Linux webkit2gtk packages), `cargo test --workspace`, `cd app && npx tauri dev`
- that the bare `transcriba` CLI binary is not relocatable if it still resolves fonts from the source tree, and that the app is the supported artifact
- a pointer to `docs/superpowers/plan-2-carry-forward.md` and the spec

- [ ] **Step 4: Commit**

```bash
git add README.md app/src-tauri/tauri.conf.json
git commit -m "build(app): linux AppImage bundle and user documentation"
```

---

### Task 8: Windows CI and installer

**Files:**
- Modify: `.github/workflows/ci.yml`
- Create: `.github/workflows/release.yml`

**Interfaces:** none.

- [ ] **Step 1: Make CI a two-platform matrix**

Replace `.github/workflows/ci.yml` with this. Note the step order — system packages before fmt/clippy/test — which Plan 1 fixed a real bug to establish; libopus is no longer installed because Task 2 vendors it.

```yaml
name: ci
on: [push, pull_request]

jobs:
  test:
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, windows-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt
      - uses: Swatinem/rust-cache@v2
      - uses: actions/setup-node@v4
        with:
          node-version: 22
          cache: npm
          cache-dependency-path: app/package-lock.json

      - name: Install Linux system dependencies
        if: matrix.os == 'ubuntu-latest'
        run: |
          sudo apt-get update
          sudo apt-get install -y ffmpeg cmake libclang-dev \
            libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev

      - name: Verify Windows build tools
        if: matrix.os == 'windows-latest'
        run: |
          cmake --version
          if (-not $env:LIBCLANG_PATH) { echo "LIBCLANG_PATH unset; bindgen may need it" }
        shell: pwsh

      - name: Fetch tiny model (Linux)
        if: matrix.os == 'ubuntu-latest'
        run: |
          mkdir -p tests/fixtures
          curl -fsL -o tests/fixtures/ggml-tiny.bin \
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin?download=true"

      - name: Fetch tiny model (Windows)
        if: matrix.os == 'windows-latest'
        run: |
          New-Item -ItemType Directory -Force -Path tests/fixtures | Out-Null
          Invoke-WebRequest -Uri "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin?download=true" `
            -OutFile tests/fixtures/ggml-tiny.bin
        shell: pwsh

      - name: Build the frontend
        working-directory: app
        run: |
          npm ci
          npm run build

      - run: cargo fmt --all -- --check
      - run: cargo clippy --workspace --all-targets --locked -- -D warnings
      - run: cargo test --workspace --locked
        env:
          TRANSCRIBA_TEST_MODEL: ${{ github.workspace }}/tests/fixtures/ggml-tiny.bin
```

`Invoke-WebRequest` throws on a non-success status, so the Windows fetch cannot silently write an HTML error page as the "model" — the same failure `curl -fsL` guards against on Linux. `fail-fast: false` so a Windows-only failure still reports the Linux result.

The frontend build must precede the Rust steps: `cargo clippy --all-targets` compiles the app crate, whose `tauri::generate_context!` reads `tauri.conf.json` and expects `frontendDist` to exist.

**Verify `cache-dependency-path` matches reality** — it requires `app/package-lock.json` to be committed. If Task 3 did not commit a lockfile, either commit it or drop the `cache`/`cache-dependency-path` lines, and use `npm install` rather than `npm ci`.

- [ ] **Step 2: Add a release workflow**

`.github/workflows/release.yml`:

```yaml
# Builds installers on a v* tag and attaches them to a GitHub release.
#
# Artifacts are UNSIGNED, a deliberate choice: a code-signing certificate costs
# €200-400/year and this is an internal tool for a handful of colleagues.
# Windows users will see "Windows protected your PC" and must click More info ->
# Run anyway. The README documents this; do not remove that note.
name: release
on:
  push:
    tags: ["v*"]

permissions:
  contents: write

jobs:
  bundle:
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: ubuntu-latest
            bundles: appimage
          - os: windows-latest
            bundles: nsis
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - uses: actions/setup-node@v4
        with:
          node-version: 22

      - name: Install Linux system dependencies
        if: matrix.os == 'ubuntu-latest'
        run: |
          sudo apt-get update
          sudo apt-get install -y cmake libclang-dev \
            libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev

      - name: Build the installer
        working-directory: app
        run: |
          npm install
          npx tauri build --bundles ${{ matrix.bundles }}

      - name: Attach artifacts to the release
        uses: softprops/action-gh-release@v2
        with:
          files: |
            app/src-tauri/target/release/bundle/appimage/*.AppImage
            app/src-tauri/target/release/bundle/nsis/*.exe
          fail_on_unmatched_files: false
          draft: true
```

`fail_on_unmatched_files: false` because each platform produces only its own bundle type, so the other glob legitimately matches nothing. `draft: true` so you review before anyone downloads.

- [ ] **Step 3: Verify what you can, and be explicit about what you cannot**

You cannot run GitHub Actions locally. Do this instead:

- Validate both workflow files are well-formed YAML: `python3 -c "import yaml,sys; [yaml.safe_load(open(f)) for f in sys.argv[1:]]" .github/workflows/*.yml`
- Re-read each step against the failure modes named above and confirm ordering.
- Report clearly that CI is unverified until it runs on a real push, and list precisely what you expect could fail first on Windows.

Do not claim CI passes. It has never run.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows
git commit -m "ci: build and test on windows and linux, add a release workflow"
```

---

## Phase boundary

Tasks 1–7 produce a working, installable Linux app. Task 8 adds Windows. If work stops after Task 7, the result is still useful — it just isn't yet on your colleagues' most likely OS.

## Deliberately out of scope

- Code signing. The human partner chose unsigned installers over a €200–400/year certificate.
- Auto-updates. Tauri's updater needs hosting and signing keys; revisit once the tool proves itself.
- Diarization, summaries, raw `.txt`/`.srt` output, and any cloud transcription — all explicit spec non-goals.
- The two upstream per-call leaks in `whisper-rs` (`set_progress_callback_safe`'s boxed closure, `set_language`'s `CString`). Negligible per run; if a user transcribes dozens of files without restarting, revisit. Recorded in the carry-forward doc.
- Making `download()` resumable. The spec promises it and the code does not implement it; that gap is recorded in the carry-forward doc and is not made worse by this plan.
