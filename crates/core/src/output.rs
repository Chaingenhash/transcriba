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
    let name_with_suffix = |suffix: &str| {
        if ext.is_empty() {
            format!("{stem}-{suffix}")
        } else {
            format!("{stem}-{suffix}.{ext}")
        }
    };
    for n in 2..1000 {
        let candidate = parent.join(name_with_suffix(&n.to_string()));
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(name_with_suffix(&std::process::id().to_string()))
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

    #[test]
    fn extension_less_paths_get_no_trailing_dot() {
        let dir = std::env::temp_dir().join("transcriba-output-tests-c");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("meeting");
        std::fs::write(&target, b"x").unwrap();
        let next = unique_path(&target);
        assert_eq!(next.file_name().unwrap(), "meeting-2");
        std::fs::remove_file(&target).ok();
    }
}
