//! Locates, verifies and downloads the whisper model.
//!
//! A truncated download that passes as valid is the most annoying possible
//! failure, so verification is strict and failure deletes the file.

use std::io::Read;
use std::path::{Path, PathBuf};

pub const MODEL_FILENAME: &str = "ggml-large-v3-turbo-q5_0.bin";
pub const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin?download=true";
/// The real file is ~574MB. Anything materially smaller is truncated.
pub const MIN_MODEL_BYTES: u64 = 500_000_000;

#[derive(Debug)]
pub enum ModelError {
    Io(String),
    Network(String),
    Invalid(String),
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelError::Io(m) => write!(f, "could not read or write the model file: {m}"),
            ModelError::Network(m) => write!(
                f,
                "could not download the speech model: {m}. Check your internet connection, \
or set TRANSCRIBA_MODEL_PATH to a copy of {MODEL_FILENAME}."
            ),
            ModelError::Invalid(m) => write!(f, "the model file is not valid: {m}"),
        }
    }
}

impl std::error::Error for ModelError {}

pub fn cache_dir() -> Result<PathBuf, ModelError> {
    let base = dirs::data_local_dir()
        .ok_or_else(|| ModelError::Io("no local data directory for this user".into()))?;
    Ok(base.join("transcriba").join("models"))
}

/// Checks size and the ggml magic bytes. ggml files begin with the little-endian
/// bytes of "ggml", which read as `lmgg`.
pub fn verify(path: &Path) -> Result<(), ModelError> {
    let meta = std::fs::metadata(path).map_err(|e| ModelError::Io(e.to_string()))?;
    if meta.len() < MIN_MODEL_BYTES {
        return Err(ModelError::Invalid(format!(
            "expected at least {MIN_MODEL_BYTES} bytes, found {}",
            meta.len()
        )));
    }
    let mut magic = [0u8; 4];
    std::fs::File::open(path)
        .map_err(|e| ModelError::Io(e.to_string()))?
        .read_exact(&mut magic)
        .map_err(|e| ModelError::Io(e.to_string()))?;
    if &magic != b"lmgg" {
        return Err(ModelError::Invalid("missing ggml magic bytes".into()));
    }
    Ok(())
}

/// Returns the model path if one is already available, or None if it must be downloaded.
pub fn resolve() -> Result<Option<PathBuf>, ModelError> {
    if let Some(override_path) = std::env::var_os("TRANSCRIBA_MODEL_PATH") {
        let path = PathBuf::from(override_path);
        verify(&path)?;
        return Ok(Some(path));
    }
    let cached = cache_dir()?.join(MODEL_FILENAME);
    match verify(&cached) {
        Ok(()) => Ok(Some(cached)),
        // A corrupt cached file is deleted so the next run re-downloads cleanly.
        Err(ModelError::Invalid(_)) => {
            std::fs::remove_file(&cached).ok();
            Ok(None)
        }
        Err(ModelError::Io(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Downloads the model to `dest`, streaming to a `.part` file so an interrupted
/// transfer is never mistaken for a valid model. `on_progress` receives
/// `(bytes_written_so_far, total_bytes_if_known)` as the transfer proceeds.
pub fn download(
    dest: &Path,
    on_progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<(), ModelError> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ModelError::Io(e.to_string()))?;
    }
    let response = ureq::get(MODEL_URL)
        .call()
        .map_err(|e| ModelError::Network(e.to_string()))?;
    let total: Option<u64> = response.body().content_length();

    // Download to a .part file so an interrupted transfer is never mistaken for a model.
    let part = dest.with_extension("part");
    let mut file = std::fs::File::create(&part).map_err(|e| ModelError::Io(e.to_string()))?;
    let mut reader = response.into_body().into_reader();
    let mut buf = vec![0u8; 1 << 16];
    let mut written = 0u64;

    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| ModelError::Network(e.to_string()))?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n])
            .map_err(|e| ModelError::Io(e.to_string()))?;
        written += n as u64;
        on_progress(written, total);
    }
    drop(file);

    if let Err(e) = verify(&part) {
        std::fs::remove_file(&part).ok();
        return Err(e);
    }
    std::fs::rename(&part, dest).map_err(|e| ModelError::Io(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("transcriba-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(bytes).unwrap();
        path
    }

    #[test]
    fn rejects_a_file_that_is_too_small() {
        let path = temp_file("small.bin", b"ggml and nothing else");
        assert!(matches!(verify(&path), Err(ModelError::Invalid(_))));
    }

    #[test]
    fn rejects_a_file_without_ggml_magic() {
        let mut bytes = vec![0u8; MIN_MODEL_BYTES as usize + 1];
        bytes[..4].copy_from_slice(b"junk");
        let path = temp_file("nomagic.bin", &bytes);
        assert!(matches!(verify(&path), Err(ModelError::Invalid(_))));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn accepts_a_file_with_ggml_magic_and_sufficient_size() {
        let mut bytes = vec![0u8; MIN_MODEL_BYTES as usize + 1];
        bytes[..4].copy_from_slice(b"lmgg");
        let path = temp_file("good.bin", &bytes);
        assert!(verify(&path).is_ok());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn missing_file_is_reported_as_io_error() {
        assert!(matches!(
            verify(std::path::Path::new("/nonexistent")),
            Err(ModelError::Io(_))
        ));
    }

    #[test]
    fn env_override_takes_precedence_when_valid() {
        let mut bytes = vec![0u8; MIN_MODEL_BYTES as usize + 1];
        bytes[..4].copy_from_slice(b"lmgg");
        let path = temp_file("override.bin", &bytes);
        std::env::set_var("TRANSCRIBA_MODEL_PATH", &path);
        assert_eq!(resolve().unwrap(), Some(path.clone()));
        std::env::remove_var("TRANSCRIBA_MODEL_PATH");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn cache_dir_ends_with_expected_path() {
        let dir = cache_dir().unwrap();
        assert!(dir.ends_with("transcriba/models"));
    }
}
