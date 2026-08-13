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
/// `rename_all` only renames the variants; `rename_all_fields` is what reaches
/// the fields inside them. Without the latter, `Decoded` ships `duration_secs`
/// to a frontend that reads `durationSecs`.
#[derive(Serialize, Clone)]
#[serde(
    tag = "phase",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Progress {
    Preparing {
        pct: u8,
    },
    Decoding,
    /// Emitted once the audio's length is known, before transcription starts.
    ///
    /// The UI uses this to lay out the sections the finished document will have
    /// — `reflow` emits a heading every 600s of audio — so the wait shows the
    /// shape of the document being written rather than a bare percentage.
    Decoded {
        duration_secs: f64,
    },
    Transcribing {
        pct: u8,
    },
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

/// The frontend needs to tell "the user pressed Cancel" apart from every other
/// failure: cancellation isn't an error and must not be shown as one (red,
/// "Erro: ..."). A bare `String` error can't carry that distinction across IPC,
/// so commands return this tagged shape instead. `kind` serializes as
/// `"cancelled" | "failed"` so `main.ts` can discriminate without parsing text;
/// `message` is unused for `Cancelled` on the frontend but kept for logging/debug.
///
/// This only tags cancellation vs. everything else. Typed errors for the rest of
/// the library's failure modes (so the UI could show Portuguese messages instead
/// of whisper's/decode's English ones) are Plan 3 work.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ErrorKind {
    Cancelled,
    Failed,
}

impl CommandError {
    fn failed(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Failed,
            message: message.into(),
        }
    }

    fn cancelled() -> Self {
        Self {
            kind: ErrorKind::Cancelled,
            message: transcribe::TranscribeError::Cancelled.to_string(),
        }
    }
}

impl From<transcribe::TranscribeError> for CommandError {
    fn from(e: transcribe::TranscribeError) -> Self {
        match e {
            transcribe::TranscribeError::Cancelled => CommandError::cancelled(),
            other => CommandError::failed(other.to_string()),
        }
    }
}

// This literal "assets/fonts" must match how `tauri.conf.json`'s `bundle.resources`
// places the files, which is why that config uses the *map* form
// (`{ "../../assets/fonts/*": "assets/fonts" }`) rather than a plain list. The list form
// derives each resource's destination from its own source path, turning every ".."
// component into a literal "_up_" folder — so a two-levels-up source would land at
// "_up_/_up_/assets/fonts" inside the bundle, not "assets/fonts". `resolve()` here has no
// such source path to mangle; it only maps ".." components *in the string passed to it*
// (there are none), so it always looks in "$RESOURCE/assets/fonts" verbatim. Reverting
// `tauri.conf.json` to the list form silently breaks PDF rendering in bundled builds only
// — `tauri dev` reads fonts from the source tree directly and would not catch it.
fn font_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .resolve("assets/fonts", BaseDirectory::Resource)
        .map_err(|e| format!("could not locate the bundled fonts: {e}"))
}

// `run` is synchronous and CPU-bound for up to ~25 minutes. `transcribe_file`
// is `async` so Tauri dispatches it via `respond_async_serialized`, which
// spawns it onto the shared tokio runtime `tauri::async_runtime` owns (a
// default multi-threaded runtime, worker count = number of CPUs — see
// `tauri::async_runtime::default_runtime`, which calls `TokioRuntime::new()`).
// That runtime is not dedicated to this command: `tauri-plugin-dialog`'s
// `open`/`save`/`message` commands are themselves `async fn`, so they are
// spawned onto the very same worker pool. Running `run` inline here would
// occupy one worker thread for the whole transcription without ever
// yielding, which is exactly what tokio's docs warn against for multi-thread
// runtimes: a blocked worker cannot make progress on any other task queued
// on it, including those unrelated async commands. `spawn_blocking` moves
// the call onto tokio's separate, growable blocking-thread pool, keeping
// every async worker free for other IPC.
#[tauri::command]
pub async fn transcribe_file(
    app: AppHandle,
    jobs: State<'_, Jobs>,
    path: String,
    language: String,
    job_id: String,
    progress: Channel<Progress>,
) -> Result<Outcome, CommandError> {
    let cancel = jobs.flag(&job_id);
    let result = tauri::async_runtime::spawn_blocking(move || {
        run(&app, &path, &language, &cancel, &progress)
    })
    .await
    .unwrap_or_else(|e| {
        Err(CommandError::failed(format!(
            "the transcription task panicked: {e}"
        )))
    });
    jobs.finish(&job_id);
    result
}

fn run(
    app: &AppHandle,
    path: &str,
    language: &str,
    cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    progress: &Channel<Progress>,
) -> Result<Outcome, CommandError> {
    use std::sync::atomic::Ordering;
    let input = Path::new(path);

    let mut last = u8::MAX;
    let model_path = model_store::ensure_available(&mut |done, total| {
        let pct = total.map_or(0, |t| (done * 100 / t.max(1)).min(100) as u8);
        if pct != last {
            last = pct;
            let _ = progress.send(Progress::Preparing { pct });
        }
    })
    .map_err(|e| CommandError::failed(e.to_string()))?;

    // `ensure_available` and `decode` never consult `cancel` — they're calls into
    // library code whose signatures this fix does not change (see Plan 3 for
    // threading a cancellation callback into them). Checking the flag at each phase
    // boundary is the minimum viable fix: on first run the Cancel button spends the
    // entire 574MB download and decode doing nothing otherwise, which is worse than
    // an unresponsive button — it's a button that looks like it works but doesn't.
    if cancel.load(Ordering::Relaxed) {
        return Err(CommandError::cancelled());
    }

    let _ = progress.send(Progress::Decoding);
    let audio = decode::decode(input).map_err(|e| CommandError::failed(e.to_string()))?;

    if cancel.load(Ordering::Relaxed) {
        return Err(CommandError::cancelled());
    }

    let _ = progress.send(Progress::Decoded {
        duration_secs: audio.duration,
    });

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
                let _ = progress.send(Progress::Transcribing {
                    pct: p.clamp(0, 100) as u8,
                });
            }
        },
        &|| cancel.load(Ordering::Relaxed),
    )?;

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

    let paths = output::unique_path_set(input, &["docx", "pdf"]);
    let (docx_path, pdf_path) = (paths[0].clone(), paths[1].clone());
    std::fs::write(
        &docx_path,
        render::docx::render_docx(&blocks, &meta)
            .map_err(|e| CommandError::failed(e.to_string()))?,
    )
    .map_err(|e| CommandError::failed(format!("could not write {}: {e}", docx_path.display())))?;

    let fonts = font_dir(app).map_err(CommandError::failed)?;
    std::fs::write(
        &pdf_path,
        render::pdf::render_pdf(&blocks, &meta, &fonts)
            .map_err(|e| CommandError::failed(e.to_string()))?,
    )
    .map_err(|e| CommandError::failed(format!("could not write {}: {e}", pdf_path.display())))?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// On an enum, `rename_all` only renames the *variants*; struct-variant
    /// fields keep their Rust names unless `rename_all_fields` is set. Getting
    /// that wrong sends `duration_secs` while `Progress` in `app/src/api.ts`
    /// reads `durationSecs`, and the wait screen renders `NaN min`.
    #[test]
    fn decoded_progress_carries_a_camel_case_duration() {
        assert_eq!(
            serde_json::to_value(Progress::Decoded {
                duration_secs: 90.0
            })
            .unwrap(),
            json!({ "phase": "decoded", "durationSecs": 90.0 }),
        );
    }

    #[test]
    fn preparing_progress_keeps_its_tag_and_percentage() {
        assert_eq!(
            serde_json::to_value(Progress::Preparing { pct: 42 }).unwrap(),
            json!({ "phase": "preparing", "pct": 42 }),
        );
    }

    #[test]
    fn outcome_fields_reach_the_frontend_in_camel_case() {
        assert_eq!(
            serde_json::to_value(Outcome {
                docx: "a.docx".into(),
                pdf: "a.pdf".into(),
                backend: "cpu".into(),
                duration_secs: 90.0,
                paragraphs: 3,
            })
            .unwrap(),
            json!({
                "docx": "a.docx",
                "pdf": "a.pdf",
                "backend": "cpu",
                "durationSecs": 90.0,
                "paragraphs": 3,
            }),
        );
    }
}
