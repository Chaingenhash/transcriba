//! Decodes any supported audio file to the 16kHz mono f32 whisper requires.
//!
//! Replaces the `ffmpeg -ar 16000 -ac 1 -c:a pcm_s16le` step of the manual pipeline.

use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{Fft, FixedSync, Resampler};
use std::path::Path;
use std::sync::OnceLock;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

pub const TARGET_RATE: u32 = 16_000;

/// The codec registry symphonia's own `default::get_codecs()` returns is fixed at build time
/// from Cargo feature flags and cannot be mutated, so Opus support — provided by
/// `symphonia-adapter-libopus` rather than a native symphonia codec — needs a registry of our
/// own. Built once and reused, since constructing a registry per decode call would be wasteful.
///
/// `symphonia-adapter-libopus` is configured with `default-features = false` in Cargo.toml
/// (see the comment there), which makes the libopus registered here link dynamically against
/// whatever `libopus` is present on this machine rather than a statically-bundled copy.
fn codecs() -> &'static CodecRegistry {
    static REGISTRY: OnceLock<CodecRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut registry);
        registry.register_audio_decoder::<symphonia_adapter_libopus::OpusDecoder>();
        registry
    })
}

#[derive(Debug)]
pub struct Audio {
    pub samples: Vec<f32>,
    pub duration: f64,
}

#[derive(Debug)]
pub enum DecodeError {
    Open(String),
    UnsupportedCodec(String),
    /// The container/format itself wasn't recognised at all (no codec to name — probing never
    /// got far enough to find one). Distinct from `UnsupportedCodec`, where the container is
    /// understood but the codec inside it isn't.
    UnsupportedFormat(String),
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
            DecodeError::UnsupportedFormat(m) => write!(
                f,
                "the file's format could not be recognized ({m}). \
Convert it to MP3 and try again."
            ),
            DecodeError::Empty => write!(f, "the file contains no audio"),
            DecodeError::Decode(m) => write!(f, "the audio could not be decoded: {m}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Returns a human-readable name for `codec`, preferring the codec registry's own short name
/// (available when the codec is registered but decoder construction still failed for some
/// other reason) and falling back to the raw codec identifier when the codec isn't registered
/// at all — which is the common case for a genuinely unsupported codec.
fn codec_name(codec: symphonia::core::codecs::audio::AudioCodecId) -> String {
    match codecs().get_audio_decoder(codec) {
        Some(registered) => registered.codec.info.short_name.to_string(),
        None => format!("{codec:?}"),
    }
}

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
        .map_err(|e| DecodeError::UnsupportedFormat(e.to_string()))?;

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

    let mut decoder = codecs()
        .make_audio_decoder(&audio_params, &AudioDecoderOptions::default())
        .map_err(|_| DecodeError::UnsupportedCodec(codec_name(audio_params.codec)))?;

    // Downmixed mono samples, averaged across channels rather than picking one.
    let mut mono = Vec::new();
    // Scratch buffer reused across packets to avoid reallocating every iteration.
    let mut interleaved = Vec::new();
    // The container's own declared rate/channel count is advisory only — some
    // containers omit it. The first decoded buffer's spec is authoritative (it's
    // what the decoder itself actually produced), so channel count and sample
    // rate are read from there rather than trusted from `audio_params` up front.
    // Guessing either one silently corrupts the output: a wrong rate skips
    // resampling that was actually needed, and a wrong channel count makes
    // `chunks(channels)` slice interleaved stereo as if it were twice as much
    // mono audio, both while still reporting success.
    let mut source_rate: Option<u32> = None;
    let mut channels: Option<usize> = None;
    let mut decoded_packets = 0u64;
    let mut failed_packets = 0u64;

    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            // `next_packet` returns `Ok(None)` at end of stream in symphonia 0.6 (not an
            // IO error as in older versions) — this is normal termination, not a failure.
            Ok(None) => break,
            // `ResetRequired` means the track list changed and there is *more* audio to
            // decode, not that the stream ended — treating it like end-of-stream would
            // silently truncate the transcript with no indication anything was lost.
            // Re-creating the decoder and continuing is a larger change; fail loudly instead.
            Err(SymphoniaError::ResetRequired) => {
                return Err(DecodeError::Decode(
                    "the file's stream structure changed partway through decoding, \
which isn't supported"
                        .to_string(),
                ));
            }
            Err(e) => return Err(DecodeError::Decode(e.to_string())),
        };
        if packet.track_id != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(audio_buf) => {
                decoded_packets += 1;
                // Only the first buffer's spec is consulted: the stream is assumed to
                // have a single, consistent rate/channel layout throughout (a change
                // partway through would surface as `ResetRequired`, handled above).
                if source_rate.is_none() {
                    let rate = audio_buf.spec().rate();
                    if rate == 0 {
                        return Err(DecodeError::Decode(
                            "the decoded stream did not declare a sample rate".to_string(),
                        ));
                    }
                    source_rate = Some(rate);
                }
                if channels.is_none() {
                    let count = audio_buf.spec().channels().count();
                    if count == 0 {
                        return Err(DecodeError::Decode(
                            "the decoded stream did not declare a channel count".to_string(),
                        ));
                    }
                    channels = Some(count);
                }
                let channels = channels.expect("just populated above if it was None");

                let frames = audio_buf.frames();
                interleaved.resize(frames * channels, 0.0f32);
                audio_buf.copy_to_slice_interleaved(interleaved.as_mut_slice());
                for frame in interleaved.chunks(channels) {
                    mono.push(frame.iter().sum::<f32>() / channels as f32);
                }
            }
            // A single malformed or truncated packet shouldn't abort the whole decode,
            // but a stream that's mostly failing packets shouldn't quietly report a
            // fraction of the audio as a complete success either (checked below).
            Err(SymphoniaError::DecodeError(_)) => {
                failed_packets += 1;
                continue;
            }
            Err(e) => return Err(DecodeError::Decode(e.to_string())),
        }
    }

    let total_packets = decoded_packets + failed_packets;
    if total_packets > 0 && failed_packets * 10 > total_packets {
        return Err(DecodeError::Decode(format!(
            "{failed_packets} of {total_packets} packets failed to decode"
        )));
    }

    if mono.is_empty() {
        return Err(DecodeError::Empty);
    }

    let source_rate = source_rate.ok_or_else(|| {
        DecodeError::Decode("the decoded stream did not declare a sample rate".to_string())
    })?;

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
    fn decodes_opus() {
        let audio = decode(&fixture("tone.opus")).expect("decodes opus");
        assert!(
            (audio.duration - 2.0).abs() < 0.2,
            "duration was {}",
            audio.duration
        );
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

    #[test]
    fn unsupported_codec_message_names_the_codec_without_library_noise() {
        let message = DecodeError::UnsupportedCodec("AAC".to_string()).to_string();
        assert!(
            message.contains("AAC"),
            "message should name the codec: {message}"
        );
        assert!(
            !message.contains("core (codec)"),
            "message should not leak raw library error text: {message}"
        );
    }
}
