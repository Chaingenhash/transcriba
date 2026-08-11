//! Chooses output paths that never overwrite an existing file.

use std::path::{Path, PathBuf};

pub fn unique_path(preferred: &Path) -> PathBuf {
    if !preferred.exists() {
        return preferred.to_path_buf();
    }
    let stem = preferred
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = preferred
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = preferred.parent().unwrap_or(Path::new("."));
    for n in 2..1000 {
        let candidate = parent.join(format!("{stem}-{n}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!("{stem}-{}.{ext}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_the_preferred_path_when_free() {
        let dir = std::env::temp_dir().join("transcriba-output-tests-a");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("meeting.docx");
        std::fs::remove_file(&target).ok();
        assert_eq!(unique_path(&target), target);
    }

    #[test]
    fn adds_a_numeric_suffix_when_the_file_exists() {
        let dir = std::env::temp_dir().join("transcriba-output-tests-b");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("meeting.docx");
        std::fs::write(&target, b"x").unwrap();
        let next = unique_path(&target);
        assert_eq!(next.file_name().unwrap(), "meeting-2.docx");
        std::fs::remove_file(&target).ok();
    }
}
