// Resolved dependency versions (from `cargo add --dry-run`):
// - symphonia v0.6.0
// - rubato v4.0.0 (provides Fft<f32> API)
// - whisper-rs v0.16.0
// - docx-rs v0.4.22
// - genpdf v0.2.0

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod decode;
pub mod reflow;
pub mod render;

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_not_empty() {
        assert!(!super::VERSION.is_empty());
    }
}
