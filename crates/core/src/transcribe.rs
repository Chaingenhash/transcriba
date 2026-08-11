//! Runs whisper over decoded audio and returns timestamped cues.

use crate::decode::Audio;
use crate::reflow::Cue;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Test-only proof that `abort_if_cancelled` actually got called by whisper.cpp, not just
/// that the contract it implements (cancel_flag true -> `Cancelled`) holds. Without this,
/// if the raw callback wiring silently stopped firing (e.g. a future whisper-rs upgrade
/// changing the FFI shape), `full()` would simply run to completion, `cancel_flag` would
/// still read `true` from the polling loop having set it, and `transcribe` would still
/// return `Cancelled` for the wrong reason — the test guarding this would pass regardless.
/// Does not exist in release builds: it is `#[cfg(test)]`, not merely inert in one.
#[cfg(test)]
static ABORT_CALLBACK_CALLS: AtomicI32 = AtomicI32::new(0);

#[derive(Debug, Clone)]
pub struct Options {
    pub language: String,
    pub threads: usize,
    pub model_path: PathBuf,
}

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

    // Deliberately NOT `set_abort_callback_safe`: whisper-rs 0.16.0 has a
    // type-erasure bug there. It instantiates its generic trampoline as
    // `trampoline::<F>` — the caller's own closure type — over data that was
    // actually boxed as `Box<dyn FnMut() -> bool>` (whisper_params.rs:645).
    // Contrast with `set_progress_callback_safe` just above it in the same
    // file, which correctly hardcodes `trampoline::<Box<dyn FnMut(i32)>>`
    // (whisper_params.rs:597) to match what it actually stores. In practice
    // this means the abort callback silently never fires: verified
    // empirically that a run taking 1.29s to finish still returned `Ok(())`
    // from `full()`, despite `cancel_flag` having already been `true` for
    // the entire 1.29s (set at the 48-microsecond mark). Using the raw,
    // unsafe callback API instead sidesteps the bug entirely: there is no
    // closure to type-erase, just a raw pointer to our own `AtomicBool`,
    // read back as the exact same type on both ends.
    unsafe extern "C" fn abort_if_cancelled(user_data: *mut std::ffi::c_void) -> bool {
        // SAFETY: `user_data` is set immediately below from `Arc::as_ptr` on
        // `cancel_flag`, which is held alive on this function's stack for
        // the entire duration of the `thread::scope` block below, which is
        // the only place this callback can be invoked (synchronously, on
        // the worker thread, inside `state.full`).
        #[cfg(test)]
        ABORT_CALLBACK_CALLS.fetch_add(1, Ordering::Relaxed);
        unsafe { &*(user_data as *const AtomicBool) }.load(Ordering::Relaxed)
    }
    // SAFETY: `abort_if_cancelled` matches `WhisperAbortCallback`'s
    // signature exactly, and the user data pointer it receives is read back
    // as the same `AtomicBool` type it was cast from.
    unsafe {
        params.set_abort_callback(Some(abort_if_cancelled));
        params.set_abort_callback_user_data(Arc::as_ptr(&cancel_flag) as *mut std::ffi::c_void);
    }
    {
        let progress_value = Arc::clone(&progress_value);
        params.set_progress_callback_safe(move |p: i32| {
            progress_value.store(p, Ordering::Relaxed);
        });
    }

    let run_result: Result<(), TranscribeError> = std::thread::scope(|scope| {
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

        match handle.join() {
            Ok(full_result) => full_result.map_err(|e| TranscribeError::Run(e.to_string())),
            // A panic across the FFI boundary (OOM, a malformed model, etc.)
            // must not take down the caller's thread; surface it as a typed
            // error instead.
            Err(_) => Err(TranscribeError::Run(
                "the transcription worker thread panicked".to_string(),
            )),
        }
    });

    // Checked before `run_result`: when the abort callback returns `true`
    // mid-run, whisper.cpp's whisper_encode_internal/whisper_decode_internal
    // return `false`, which makes `whisper_full_with_state` return a nonzero
    // code (-6/-8/-9) that whisper-rs's `full()` does not special-case, so it
    // surfaces as `Err(WhisperError::GenericError(_))` — indistinguishable
    // from a genuine failure unless cancellation is checked first. A cancelled
    // run is not a `Run` failure even though `full()` returned an `Err`.
    if cancel_flag.load(Ordering::Relaxed) {
        return Err(TranscribeError::Cancelled);
    }
    run_result?;

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

    /// An audible tone, well above the digital-silence threshold, so it
    /// reaches the real decode path instead of being short-circuited.
    fn tone(seconds: f64, amplitude: f32) -> Audio {
        let rate = crate::decode::TARGET_RATE as f64;
        let n = (seconds * rate) as usize;
        let samples = (0..n)
            .map(|i| {
                amplitude * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / rate).sin() as f32
            })
            .collect();
        Audio {
            samples,
            duration: seconds,
        }
    }

    /// Deterministic noise, loud enough to clear the silence gate. A pure
    /// tone gets bailed out of almost immediately by whisper's own
    /// low-information heuristics ("skip entire chunk"), finishing before a
    /// cancellation request has any chance to land mid-decode; noise keeps
    /// the decoder busy across many tokens for long enough that a
    /// mid-run `should_cancel` reliably arrives while `full()` is still
    /// running, not after it has already returned.
    fn noise(seconds: f64, amplitude: f32) -> Audio {
        let n = (seconds * crate::decode::TARGET_RATE as f64) as usize;
        let mut state: u32 = 0x2545_F491;
        let samples = (0..n)
            .map(|_| {
                // xorshift32
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let unit = (state as f32) / (u32::MAX as f32); // [0, 1]
                amplitude * (unit * 2.0 - 1.0)
            })
            .collect();
        Audio {
            samples,
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
    fn cancellation_mid_run_returns_cancelled() {
        let Some(opts) = tiny_model_opts() else {
            return;
        };
        // `should_cancel` must answer `false` on the very first call (the
        // top-of-function guard, before the worker starts) and `true` on
        // every call after — deterministically, not by racing a timer
        // against however long this particular decode happens to take.
        // The second call happens the instant the polling loop first runs,
        // immediately after the worker thread is spawned and while `full()`
        // is still genuinely in flight (noise input, unlike a pure tone,
        // keeps ggml-tiny decoding across multiple temperature retries for
        // over a second, giving ample room for the abort to land before
        // natural completion).
        let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let call_count_reader = Arc::clone(&call_count);
        let should_cancel = move || call_count_reader.fetch_add(1, Ordering::Relaxed) > 0;

        // Reset first so a stale count left behind by another test in this process
        // (tests share the same `static`) can't make the assertion below pass for free.
        ABORT_CALLBACK_CALLS.store(0, Ordering::Relaxed);
        let err = transcribe(&noise(5.0, 0.1), &opts, &mut |_| {}, &should_cancel);
        assert!(matches!(err, Err(TranscribeError::Cancelled)));
        assert!(
            ABORT_CALLBACK_CALLS.load(Ordering::Relaxed) > 0,
            "abort_if_cancelled was never actually invoked by whisper.cpp \
— cancellation returned Cancelled for the wrong reason"
        );
    }

    #[test]
    fn is_digital_silence_does_not_flag_quiet_real_signal() {
        // RMS is roughly amplitude/sqrt(2) =~ 7e-3: comfortably above the
        // 1e-4 threshold, but still clearly a quiet signal, not silence.
        let quiet = tone(0.5, 0.01);
        assert!(!is_digital_silence(&quiet.samples));
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
