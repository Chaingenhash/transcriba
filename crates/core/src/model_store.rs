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
    resolve_cached(&cache_dir()?.join(MODEL_FILENAME))
}

/// The cache-path half of [`resolve`], split out so its not-found/corrupt/IO-error
/// branching can be exercised directly against an arbitrary path in tests, without
/// needing to control the real OS cache directory.
fn resolve_cached(cached: &Path) -> Result<Option<PathBuf>, ModelError> {
    // Checked separately from the `verify` match below: `ModelError::Io(String)` has
    // already discarded the underlying `io::ErrorKind`, so "not found" can no longer be
    // told apart from a genuine permissions or I/O problem once it's wrapped in that
    // variant. Ruling out "not found" here first means any `Io` error `verify` returns
    // past this point is real and must propagate, rather than being treated as "must
    // download" — which would otherwise re-download 574MB on every launch and then fail
    // to rename over a file it was never actually permitted to touch, with nothing
    // pointing at the real cause.
    if !cached.exists() {
        return Ok(None);
    }
    match verify(cached) {
        Ok(()) => Ok(Some(cached.to_path_buf())),
        // A corrupt cached file is deleted so the next run re-downloads cleanly.
        Err(ModelError::Invalid(_)) => {
            std::fs::remove_file(cached).ok();
            Ok(None)
        }
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

    #[test]
    fn resolve_cached_reports_none_when_nothing_is_cached() {
        let dir = std::env::temp_dir().join("transcriba-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("does-not-exist.bin");
        std::fs::remove_file(&path).ok();
        assert!(matches!(resolve_cached(&path), Ok(None)));
    }

    #[test]
    fn resolve_cached_still_deletes_and_redownloads_on_corruption() {
        let mut bytes = vec![0u8; MIN_MODEL_BYTES as usize + 1];
        bytes[..4].copy_from_slice(b"junk");
        let path = temp_file("resolve-corrupt.bin", &bytes);
        assert!(matches!(resolve_cached(&path), Ok(None)));
        assert!(!path.exists(), "corrupt cached file should be deleted");
    }

    // Regression test for the finding that `resolve()` was collapsing every IO error
    // (permissions, EIO, ...) to "must download", which on a real permissions problem
    // means every launch re-downloads the 574MB model and then fails to rename over a
    // file it was never allowed to touch, forever, with nothing pointing at the real
    // cause. A permission-denied file is used to produce a genuine IO error: the file
    // is a sparse file (created via `set_len`, so this doesn't actually write ~500MB to
    // disk) sized past `MIN_MODEL_BYTES` so `verify` reaches the point of trying to open
    // and read it, then chmod'd to be unreadable so that open fails.
    #[cfg(unix)]
    #[test]
    fn resolve_cached_propagates_genuine_io_errors_instead_of_redownloading() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_file("resolve-permission-denied.bin", b"");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MIN_MODEL_BYTES + 1).unwrap();
        drop(file);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

        // Root (and some CI/container setups) ignores file permission bits entirely, in
        // which case the scenario this test relies on can't be constructed at all; skip
        // rather than fail on an assumption that doesn't hold on this machine.
        let root_can_read_anyway = std::fs::File::open(&path).is_ok();

        if !root_can_read_anyway {
            assert!(
                matches!(resolve_cached(&path), Err(ModelError::Io(_))),
                "a genuine IO error must propagate, not be reported as 'must download'"
            );
        }

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::remove_file(&path).ok();
    }

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
        assert!(
            !called,
            "must not report download progress when the model already exists"
        );
        std::fs::remove_file(path).ok();
    }
}
