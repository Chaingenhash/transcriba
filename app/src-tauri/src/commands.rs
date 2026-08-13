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
) -> Result<Outcome, String> {
    let cancel = jobs.flag(&job_id);
    let result = tauri::async_runtime::spawn_blocking(move || {
        run(&app, &path, &language, &cancel, &progress)
    })
    .await
    .unwrap_or_else(|e| Err(format!("the transcription task panicked: {e}")));
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
        let pct = total.map_or(0, |t| (done * 100 / t.max(1)).min(100) as u8);
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
                let _ = progress.send(Progress::Transcribing {
                    pct: p.clamp(0, 100) as u8,
                });
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

    let paths = output::unique_path_set(input, &["docx", "pdf"]);
    let (docx_path, pdf_path) = (paths[0].clone(), paths[1].clone());
    std::fs::write(
        &docx_path,
        render::docx::render_docx(&blocks, &meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("could not write {}: {e}", docx_path.display()))?;

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
