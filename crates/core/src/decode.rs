//! Decodes any supported audio file to the 16kHz mono f32 whisper requires.
//!
//! Replaces the `ffmpeg -ar 16000 -ac 1 -c:a pcm_s16le` step of the manual pipeline.

use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{Fft, FixedSync, Resampler};
use std::path::Path;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

pub const TARGET_RATE: u32 = 16_000;

#[derive(Debug)]
pub struct Audio {
    pub samples: Vec<f32>,
    pub duration: f64,
}

#[derive(Debug)]
pub enum DecodeError {
    Open(String),
    UnsupportedCodec(String),
    Empty,
    Decode(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Open(m) => write!(f, "could not open the file: {m}"),
            DecodeError::UnsupportedCodec(c) => write!(
                f,
                "this file uses {c} audio, which isn't supported yet. \
Convert it to MP3 and try again."
            ),
            DecodeError::Empty => write!(f, "the file contains no audio"),
            DecodeError::Decode(m) => write!(f, "the audio could not be decoded: {m}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Decodes `path` to mono `f32` samples at [`TARGET_RATE`].
pub fn decode(path: &Path) -> Result<Audio, DecodeError> {
    let file = std::fs::File::open(path).map_err(|e| DecodeError::Open(e.to_string()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| DecodeError::UnsupportedCodec(e.to_string()))?;

    let track = format
        .default_track(TrackType::Audio)
        .ok_or(DecodeError::Empty)?;
    let track_id = track.id;
    let audio_params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or(DecodeError::Empty)?
        .clone();
    let source_rate = audio_params.sample_rate.unwrap_or(TARGET_RATE);
    let channels = audio_params
        .channels
        .as_ref()
        .map_or(1, |c| c.count())
        .max(1);

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&audio_params, &AudioDecoderOptions::default())
        .map_err(|e| DecodeError::UnsupportedCodec(e.to_string()))?;

    // Downmixed mono samples, averaged across channels rather than picking one.
    let mut mono = Vec::new();
    // Scratch buffer reused across packets to avoid reallocating every iteration.
    let mut interleaved = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            // `next_packet` returns `Ok(None)` at end of stream in symphonia 0.6 (not an
            // IO error as in older versions) — this is normal termination, not a failure.
            Ok(None) => break,
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(DecodeError::Decode(e.to_string())),
        };
        if packet.track_id != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(audio_buf) => {
                let frames = audio_buf.samples_interleaved() / channels;
                interleaved.resize(frames * channels, 0.0f32);
                audio_buf.copy_to_slice_interleaved(interleaved.as_mut_slice());
                for frame in interleaved.chunks(channels) {
                    mono.push(frame.iter().sum::<f32>() / channels as f32);
                }
            }
            // A single malformed or truncated packet shouldn't abort the whole decode.
            Err(SymphoniaError::DecodeError(_)) | Err(SymphoniaError::IoError(_)) => continue,
            Err(e) => return Err(DecodeError::Decode(e.to_string())),
        }
    }

    if mono.is_empty() {
        return Err(DecodeError::Empty);
    }

    let samples = if source_rate == TARGET_RATE {
        mono
    } else {
        resample(&mono, source_rate)?
    };
    let duration = samples.len() as f64 / TARGET_RATE as f64;
    Ok(Audio { samples, duration })
}

/// Fixed-ratio resampling of one mono channel down (or up) to [`TARGET_RATE`].
fn resample(input: &[f32], source_rate: u32) -> Result<Vec<f32>, DecodeError> {
    const CHUNK: usize = 1024;

    let mut resampler = Fft::<f32>::new(
        source_rate as usize,
        TARGET_RATE as usize,
        CHUNK,
        1,
        FixedSync::Both,
    )
    .map_err(|e| DecodeError::Decode(e.to_string()))?;

    let input_data = vec![input.to_vec()];
    let adapter = SequentialSliceOfVecs::new(&input_data, 1, input.len())
        .map_err(|e| DecodeError::Decode(e.to_string()))?;

    let output = resampler
        .process_all(&adapter, input.len(), None)
        .map_err(|e| DecodeError::Decode(e.to_string()))?;

    Ok(output.take_data())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name)
    }

    #[test]
    fn decodes_mp3_to_sixteen_khz_mono() {
        let audio = decode(&fixture("tone.mp3")).expect("decodes");
        assert!(
            (audio.duration - 2.0).abs() < 0.2,
            "duration was {}",
            audio.duration
        );
        let expected = (2.0 * TARGET_RATE as f64) as usize;
        let delta = (audio.samples.len() as i64 - expected as i64).abs();
        assert!(
            delta < TARGET_RATE as i64 / 5,
            "got {} samples",
            audio.samples.len()
        );
    }

    #[test]
    fn decodes_wav() {
        let audio = decode(&fixture("tone.wav")).expect("decodes");
        assert!(!audio.samples.is_empty());
    }

    #[test]
    fn decodes_m4a() {
        let audio = decode(&fixture("tone.m4a")).expect("decodes");
        assert!(!audio.samples.is_empty());
    }

    #[test]
    fn samples_are_within_valid_range() {
        let audio = decode(&fixture("tone.wav")).unwrap();
        assert!(audio
            .samples
            .iter()
            .all(|s| s.is_finite() && s.abs() <= 1.01));
    }

    #[test]
    fn missing_file_reports_open_error() {
        assert!(matches!(
            decode(Path::new("/nonexistent.mp3")),
            Err(DecodeError::Open(_))
        ));
    }
}
