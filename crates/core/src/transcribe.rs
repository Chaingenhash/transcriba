//! Runs whisper over decoded audio and returns timestamped cues.

use crate::decode::Audio;
use crate::reflow::Cue;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

#[derive(Debug, Clone)]
pub struct Options {
    pub language: String,
    pub threads: usize,
    pub model_path: PathBuf,
}

/// Marker returned when a caller-requested cancellation is honoured.
#[derive(Debug)]
pub struct Cancelled;

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

/// How often the polling loop checks `should_cancel` and drains progress while
/// the whisper worker thread runs. Small enough to feel responsive, large
/// enough not to burn a core spinning.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// RMS amplitude below which audio is treated as having no signal at all.
/// Well under any real recording's noise floor, so it only ever fires on
/// literal digital silence.
const SILENCE_RMS_THRESHOLD: f64 = 1e-4;

fn is_digital_silence(samples: &[f32]) -> bool {
    if samples.is_empty() {
        return true;
    }
    let sum_sq: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    (sum_sq / samples.len() as f64).sqrt() < SILENCE_RMS_THRESHOLD
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

    let ctx =
        WhisperContext::new_with_params(&opts.model_path, WhisperContextParameters::default())
            .map_err(|e| TranscribeError::Model(e.to_string()))?;

    let mut state = ctx
        .create_state()
        .map_err(|e| TranscribeError::Model(e.to_string()))?;

    // whisper.cpp does not reliably flag this itself: on all-zero PCM, ggml-tiny
    // was observed hallucinating full sentences while reporting
    // `no_speech_probability()` as low as 0.00002 — i.e. near-certain there
    // *was* speech. A direct energy check catches genuine digital silence
    // (this synthetic case, or a dropped/muted input) before wasting a full
    // decode pass on it; it does not attempt to catch merely quiet speech,
    // which is a much harder, out-of-scope problem.
    if is_digital_silence(&audio.samples) {
        return Err(TranscribeError::NoSpeech);
    }

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(opts.threads as i32);
    params.set_language(Some(&opts.language));
    params.set_print_progress(false);
    params.set_print_special(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    // whisper.cpp defaults this to false. Without it, silence and music beds
    // reliably get hallucinated as bracketed captions like "[Music]" — the
    // model's own no_speech_probability does not catch this (observed as low
    // as 0.00002 on pure digital silence with ggml-tiny) — which would make
    // TranscribeError::NoSpeech unreachable for genuinely silent input.
    params.set_suppress_nst(true);

    // `set_progress_callback_safe`/`set_abort_callback_safe` require `'static`
    // closures, but `on_progress`/`should_cancel` are borrowed trait objects
    // scoped to this call. Bridge the gap with `Arc`s the whisper-side
    // callbacks only read/write, while a polling loop on this thread — which
    // still owns the original references — relays their state across. This
    // is also the shape a future GUI needs anyway, since progress there must
    // cross a thread boundary to reach a webview.
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let progress_value = Arc::new(AtomicI32::new(0));

    {
        let cancel_flag = Arc::clone(&cancel_flag);
        params.set_abort_callback_safe(move || cancel_flag.load(Ordering::Relaxed));
    }
    {
        let progress_value = Arc::clone(&progress_value);
        params.set_progress_callback_safe(move |p: i32| {
            progress_value.store(p, Ordering::Relaxed);
        });
    }

    let run_result = std::thread::scope(|scope| {
        let handle = scope.spawn(|| state.full(params, &audio.samples));

        let mut last_reported = -1;
        while !handle.is_finished() {
            if should_cancel() {
                cancel_flag.store(true, Ordering::Relaxed);
            }
            let current = progress_value.load(Ordering::Relaxed);
            if current != last_reported {
                on_progress(current);
                last_reported = current;
            }
            std::thread::sleep(POLL_INTERVAL);
        }

        let final_progress = progress_value.load(Ordering::Relaxed);
        if final_progress != last_reported {
            on_progress(final_progress);
        }

        handle.join().expect("whisper worker thread panicked")
    });

    run_result.map_err(|e| TranscribeError::Run(e.to_string()))?;

    if cancel_flag.load(Ordering::Relaxed) {
        return Err(TranscribeError::Cancelled);
    }

    let mut cues = Vec::new();
    for segment in state.as_iter() {
        let text = segment
            .to_str_lossy()
            .unwrap_or_default()
            .trim()
            .to_string();
        if text.is_empty() {
            continue;
        }
        // whisper reports segment timestamps in centiseconds (hundredths of a second).
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
        let Some(opts) = tiny_model_opts() else {
            return;
        };
        let err = transcribe(&silence(2.0), &opts, &mut |_| {}, &|| true);
        assert!(matches!(err, Err(TranscribeError::Cancelled)));
    }

    #[test]
    fn silence_reports_no_speech() {
        let Some(opts) = tiny_model_opts() else {
            return;
        };
        let err = transcribe(&silence(3.0), &opts, &mut |_| {}, &|| false);
        assert!(matches!(err, Err(TranscribeError::NoSpeech)));
    }
}
