//! Saved folders for local terminals: open a shell in a folder you use often with one click.
//! Stored as JSON in the settings table (`keys::TERMINAL_SAVED_FOLDERS`).

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedFolder {
    /// Shown in the menu.
    pub name: String,
    pub path: String,
    /// Shell for this folder (`ShellKind::as_setting`); `None` uses the default shell from Settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
}

/// Most folders kept; the menu has to stay usable.
pub const MAX_SAVED: usize = 50;

/// Read the stored list. A missing or damaged value is an empty list, never an error.
pub fn parse(json: Option<&str>) -> Vec<SavedFolder> {
    let mut v: Vec<SavedFolder> = json.and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default();
    v.retain(|f| !f.path.trim().is_empty());
    v.truncate(MAX_SAVED);
    v
}

pub fn to_json(list: &[SavedFolder]) -> String {
    serde_json::to_string(list).unwrap_or_else(|_| "[]".into())
}

/// Name for a newly saved folder: its last part, or the whole path for a drive root (`D:\`).
pub fn default_name(path: &str) -> String {
    Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).filter(|n| !n.is_empty()).unwrap_or_else(|| path.to_string())
}

/// Windows paths: case does not matter, nor does a trailing backslash.
pub fn same_path(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        let t = s.trim().replace('/', "\\");
        let t = if t.len() > 3 { t.trim_end_matches('\\').to_string() } else { t };
        t.to_lowercase()
    };
    norm(a) == norm(b)
}

pub fn position(list: &[SavedFolder], path: &str) -> Option<usize> {
    list.iter().position(|f| same_path(&f.path, path))
}

/// Save `path` (named after its last part). Returns false when it was already saved or the list is full.
pub fn add(list: &mut Vec<SavedFolder>, path: &str) -> bool {
    if path.trim().is_empty() || position(list, path).is_some() || list.len() >= MAX_SAVED {
        return false;
    }
    list.push(SavedFolder { name: default_name(path), path: path.to_string(), shell: None });
    true
}

/// Remove `path`. Returns whether it was saved.
pub fn remove(list: &mut Vec<SavedFolder>, path: &str) -> bool {
    match position(list, path) {
        Some(i) => {
            list.remove(i);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_damaged_values() {
        let list = vec![
            SavedFolder { name: "Rust Projects".into(), path: r"D:\Rust Projects".into(), shell: None },
            SavedFolder { name: "Zendo".into(), path: r"C:\DeepSeek-Harness\Zendo_Rust".into(), shell: Some("powershell".into()) },
        ];
        assert_eq!(parse(Some(&to_json(&list))), list);
        assert!(!to_json(&list[..1]).contains("shell"), "no shell key when the default is used");
        assert!(parse(None).is_empty());
        assert!(parse(Some("not json")).is_empty(), "damaged setting is an empty list");
        assert!(parse(Some(r#"[{"name":"x","path":"  "}]"#)).is_empty(), "entries without a path are dropped");
    }

    #[test]
    fn default_names() {
        assert_eq!(default_name(r"D:\Rust Projects"), "Rust Projects");
        assert_eq!(default_name(r"C:\DeepSeek-Harness\Zendo_Rust\"), "Zendo_Rust");
        assert_eq!(default_name(r"D:\"), r"D:\", "a drive root keeps its path");
    }

    #[test]
    fn add_and_remove_ignore_case_and_trailing_backslash() {
        let mut list = Vec::new();
        assert!(add(&mut list, r"D:\Rust Projects"));
        assert!(!add(&mut list, r"d:\rust projects\"), "same folder");
        assert_eq!(list.len(), 1);
        assert!(position(&list, r"D:/Rust Projects").is_some());
        assert!(same_path(r"D:\", r"d:\"), "a drive root keeps its backslash");
        assert!(!same_path(r"D:\a", r"D:\ab"));
        assert!(remove(&mut list, r"D:\RUST PROJECTS\"));
        assert!(!remove(&mut list, r"D:\Rust Projects"));
        assert!(list.is_empty());
    }

    #[test]
    fn list_is_capped() {
        let mut list = Vec::new();
        for i in 0..MAX_SAVED {
            assert!(add(&mut list, &format!(r"C:\f{i}")));
        }
        assert!(!add(&mut list, r"C:\one-more"));
        let many: Vec<SavedFolder> = (0..MAX_SAVED + 5).map(|i| SavedFolder { name: i.to_string(), path: format!(r"C:\f{i}"), shell: None }).collect();
        assert_eq!(parse(Some(&to_json(&many))).len(), MAX_SAVED);
    }
}
