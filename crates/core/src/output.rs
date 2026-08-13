//! Chooses output paths that never overwrite an existing file.

use std::path::{Path, PathBuf};

/// Builds a candidate file name from a stem, a numeric suffix, and an
/// extension, without emitting a trailing dot when `ext` is empty.
fn suffixed_name(stem: &str, suffix: &str, ext: &str) -> String {
    if ext.is_empty() {
        format!("{stem}-{suffix}")
    } else {
        format!("{stem}-{suffix}.{ext}")
    }
}

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
        let candidate = parent.join(suffixed_name(&stem, &n.to_string(), &ext));
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(suffixed_name(&stem, &std::process::id().to_string(), &ext))
}

/// Chooses output paths for a set of extensions that share one suffix, so a
/// transcription's documents stay a matched pair even when only one of them
/// already exists on disk. `stem_path` supplies the base name and directory
/// (any extension it carries is replaced per-entry, mirroring
/// `Path::with_extension`); `extensions` lists the extensions in the order
/// the returned `Vec` should follow.
///
/// The shared suffix is the first one — starting unsuffixed — at which none
/// of the extensions collide with an existing file, so the pair never ends
/// up mismatched (e.g. `meeting.docx` alongside `meeting-2.pdf`).
pub fn unique_path_set(stem_path: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    let preferred: Vec<PathBuf> = extensions
        .iter()
        .map(|ext| stem_path.with_extension(ext))
        .collect();
    if preferred.iter().all(|p| !p.exists()) {
        return preferred;
    }

    let stem = stem_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = stem_path.parent().unwrap_or(Path::new("."));
    let candidates_for = |suffix: &str| -> Vec<PathBuf> {
        extensions
            .iter()
            .map(|ext| parent.join(suffixed_name(&stem, suffix, ext)))
            .collect()
    };

    for n in 2..1000 {
        let candidates = candidates_for(&n.to_string());
        if candidates.iter().all(|p| !p.exists()) {
            return candidates;
        }
    }
    candidates_for(&std::process::id().to_string())
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

    #[test]
    fn unique_path_set_returns_unsuffixed_names_when_nothing_exists() {
        let dir = std::env::temp_dir().join("transcriba-output-tests-set-a");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::remove_file(dir.join("meeting.docx")).ok();
        std::fs::remove_file(dir.join("meeting.pdf")).ok();
        let stem = dir.join("meeting.wav");

        let paths = unique_path_set(&stem, &["docx", "pdf"]);

        assert_eq!(paths[0].file_name().unwrap(), "meeting.docx");
        assert_eq!(paths[1].file_name().unwrap(), "meeting.pdf");
    }

    #[test]
    fn unique_path_set_suffixes_both_when_only_one_extension_collides() {
        let dir = std::env::temp_dir().join("transcriba-output-tests-set-b");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::remove_file(dir.join("meeting.pdf")).ok();
        std::fs::write(dir.join("meeting.docx"), b"x").unwrap();
        let stem = dir.join("meeting.wav");

        let paths = unique_path_set(&stem, &["docx", "pdf"]);

        assert_eq!(paths[0].file_name().unwrap(), "meeting-2.docx");
        assert_eq!(paths[1].file_name().unwrap(), "meeting-2.pdf");

        std::fs::remove_file(dir.join("meeting.docx")).ok();
    }

    #[test]
    fn unique_path_set_suffixes_both_when_both_extensions_collide() {
        let dir = std::env::temp_dir().join("transcriba-output-tests-set-c");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("meeting.docx"), b"x").unwrap();
        std::fs::write(dir.join("meeting.pdf"), b"x").unwrap();
        let stem = dir.join("meeting.wav");

        let paths = unique_path_set(&stem, &["docx", "pdf"]);

        assert_eq!(paths[0].file_name().unwrap(), "meeting-2.docx");
        assert_eq!(paths[1].file_name().unwrap(), "meeting-2.pdf");

        std::fs::remove_file(dir.join("meeting.docx")).ok();
        std::fs::remove_file(dir.join("meeting.pdf")).ok();
    }

    #[test]
    fn unique_path_set_advances_past_a_suffix_free_for_only_one_extension() {
        let dir = std::env::temp_dir().join("transcriba-output-tests-set-d");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("meeting.docx"), b"x").unwrap();
        std::fs::write(dir.join("meeting-2.pdf"), b"x").unwrap();
        std::fs::remove_file(dir.join("meeting-2.docx")).ok();
        std::fs::remove_file(dir.join("meeting.pdf")).ok();
        let stem = dir.join("meeting.wav");

        let paths = unique_path_set(&stem, &["docx", "pdf"]);

        assert_eq!(paths[0].file_name().unwrap(), "meeting-3.docx");
        assert_eq!(paths[1].file_name().unwrap(), "meeting-3.pdf");

        std::fs::remove_file(dir.join("meeting.docx")).ok();
        std::fs::remove_file(dir.join("meeting-2.pdf")).ok();
    }
}
