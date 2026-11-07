//! Native messaging host manifest + per-user registry registration
//! (FR-033/034, DEC-006/007, TASK-38).
//!
//! The registry value under each browser key is the absolute path of the host
//! *manifest JSON*; the manifest's `path` points at `unduhkilat-host.exe`.
//! Registering the exe path directly would leave the browser unable to connect.

use crate::error::{AppError, ErrorCode, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

pub const HOST_NAME: &str = "com.unduhkilat.host";
/// Derived from the public key pinned in `extension/manifest.json`. The host rejects
/// any other caller (`ERR_UNAUTHORIZED`).
pub const EXTENSION_ID: &str = "bcjegfdifgnbcgkcdfliiilobgbelbae";

pub fn allowed_origin() -> String {
    format!("chrome-extension://{EXTENSION_ID}/")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Browser {
    Chrome,
    Edge,
    Brave,
}

pub const ALL_BROWSERS: [Browser; 3] = [Browser::Chrome, Browser::Edge, Browser::Brave];

impl Browser {
    pub fn name(&self) -> &'static str {
        match self {
            Browser::Chrome => "Google Chrome",
            Browser::Edge => "Microsoft Edge",
            Browser::Brave => "Brave",
        }
    }

    /// Key under HKCU (or HKLM) that holds `NativeMessagingHosts`.
    fn subkey(&self) -> &'static str {
        match self {
            Browser::Chrome => r"Software\Google\Chrome\NativeMessagingHosts",
            Browser::Edge => r"Software\Microsoft\Edge\NativeMessagingHosts",
            Browser::Brave => r"Software\BraveSoftware\Brave-Browser\NativeMessagingHosts",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Status {
    Registered,
    Missing,
    /// Registry points somewhere else (portable folder moved) or the manifest is wrong.
    Stale,
}

/// Where registrations live. `prefix` lets tests use a throwaway subtree instead of
/// the real browser keys; production uses an empty prefix.
#[derive(Debug, Clone, Default)]
pub struct Target {
    pub prefix: String,
    /// Register under HKLM (all users, needs elevation, FR-035) instead of HKCU.
    pub machine: bool,
}

impl Target {
    fn key_path(&self, b: Browser) -> String {
        format!(r"{}{}\{}", self.prefix, b.subkey(), HOST_NAME)
    }
}

pub fn manifest_json(host_exe: &Path) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "name": HOST_NAME,
        "description": "UnduhKilat native messaging host",
        "path": host_exe.to_string_lossy(),
        "type": "stdio",
        "allowed_origins": [allowed_origin()],
    }))
    .expect("manifest serializes")
}

/// Write (or repair) the manifest next to the app's data. Returns its path.
pub fn write_manifest(dir: &Path, host_exe: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{HOST_NAME}.json"));
    let want = manifest_json(host_exe);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(want.as_str()) {
        std::fs::write(&path, want)?;
    }
    Ok(path)
}

#[cfg(windows)]
mod reg {
    use super::*;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;

    fn hive(t: &Target) -> RegKey {
        RegKey::predef(if t.machine { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER })
    }

    pub fn read(t: &Target, b: Browser) -> Option<String> {
        hive(t).open_subkey_with_flags(t.key_path(b), KEY_READ).ok()?.get_value::<String, _>("").ok()
    }

    pub fn write(t: &Target, b: Browser, manifest: &Path) -> Result<()> {
        let (key, _) = hive(t)
            .create_subkey(t.key_path(b))
            .map_err(|e| AppError::new(ErrorCode::ErrRegistryWriteFailed, format!("{}: {e}", b.name())))?;
        key.set_value("", &manifest.to_string_lossy().as_ref())
            .map_err(|e| AppError::new(ErrorCode::ErrRegistryWriteFailed, format!("{}: {e}", b.name())))
    }

    pub fn delete(t: &Target, b: Browser) {
        let _ = hive(t).delete_subkey_all(t.key_path(b));
    }

    /// Test helper: remove a whole throwaway prefix.
    pub fn delete_prefix(prefix: &str) {
        let root = prefix.trim_end_matches('\\');
        let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(root);
    }
}

#[cfg(not(windows))]
mod reg {
    use super::*;
    pub fn read(_: &Target, _: Browser) -> Option<String> {
        None
    }
    pub fn write(_: &Target, _: Browser, _: &Path) -> Result<()> {
        Err(AppError::new(ErrorCode::ErrRegistryWriteFailed, "registry is Windows-only (DEC-005)"))
    }
    pub fn delete(_: &Target, _: Browser) {}
    pub fn delete_prefix(_: &str) {}
}

pub fn status(t: &Target, manifest: &Path, host_exe: &Path) -> Vec<(Browser, Status)> {
    let manifest_ok = std::fs::read_to_string(manifest).ok().as_deref() == Some(manifest_json(host_exe).as_str());
    ALL_BROWSERS
        .iter()
        .map(|&b| {
            let st = match reg::read(t, b) {
                None => Status::Missing,
                Some(v) if manifest_ok && Path::new(&v) == manifest => Status::Registered,
                Some(_) => Status::Stale,
            };
            (b, st)
        })
        .collect()
}

/// Idempotent: writes the manifest and registers every browser, repairing stale entries
/// (the portable folder may have moved). No elevation needed (HKCU only).
pub fn ensure_registered(t: &Target, data_dir: &Path, host_exe: &Path) -> Result<Vec<(Browser, Status)>> {
    if !host_exe.exists() {
        return Err(AppError::new(ErrorCode::ErrNativeHostNotFound, format!("{} is missing", host_exe.display())));
    }
    let manifest = write_manifest(data_dir, host_exe)?;
    for b in ALL_BROWSERS {
        if reg::read(t, b).as_deref().map(Path::new) != Some(manifest.as_path()) {
            reg::write(t, b, &manifest)?;
        }
    }
    Ok(status(t, &manifest, host_exe))
}

pub fn unregister(t: &Target) {
    for b in ALL_BROWSERS {
        reg::delete(t, b);
    }
}

#[doc(hidden)]
pub fn __delete_test_prefix(prefix: &str) {
    reg::delete_prefix(prefix);
}

/// Path of `unduhkilat-host.exe` next to the running executable.
pub fn host_exe_next_to(app_exe: &Path) -> PathBuf {
    app_exe.with_file_name("unduhkilat-host.exe")
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    struct Sandbox {
        prefix: String,
        dir: PathBuf,
        host: PathBuf,
    }
    impl Sandbox {
        fn new() -> Self {
            let id = uuid::Uuid::new_v4();
            let dir = std::env::temp_dir().join(format!("uk-native-{id}"));
            std::fs::create_dir_all(&dir).unwrap();
            let host = dir.join("unduhkilat-host.exe");
            std::fs::write(&host, b"stub").unwrap();
            Sandbox { prefix: format!(r"Software\UnduhKilatTest-{id}\"), dir, host }
        }
        fn target(&self) -> Target {
            Target { prefix: self.prefix.clone(), machine: false }
        }
    }
    impl Drop for Sandbox {
        fn drop(&mut self) {
            reg::delete_prefix(&self.prefix);
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    #[test]
    fn manifest_has_required_fields_and_pinned_origin() {
        let v: serde_json::Value = serde_json::from_str(&manifest_json(Path::new(r"C:\x\unduhkilat-host.exe"))).unwrap();
        assert_eq!(v["name"], HOST_NAME);
        assert_eq!(v["type"], "stdio");
        assert_eq!(v["path"], r"C:\x\unduhkilat-host.exe");
        assert_eq!(v["allowed_origins"][0], format!("chrome-extension://{EXTENSION_ID}/"));
        assert_eq!(v["allowed_origins"].as_array().unwrap().len(), 1, "no wildcard origins");
    }

    #[test]
    fn registers_all_three_browsers_pointing_at_the_manifest_not_the_exe() {
        let sb = Sandbox::new();
        let st = ensure_registered(&sb.target(), &sb.dir, &sb.host).unwrap();
        assert_eq!(st.len(), 3);
        assert!(st.iter().all(|(_, s)| *s == Status::Registered));
        let manifest = sb.dir.join("com.unduhkilat.host.json");
        for b in ALL_BROWSERS {
            let v = reg::read(&sb.target(), b).unwrap();
            assert_eq!(Path::new(&v), manifest, "{b:?} must point at the manifest JSON");
            assert!(v.ends_with(".json"));
        }
    }

    #[test]
    fn missing_before_registration_and_idempotent_after() {
        let sb = Sandbox::new();
        let manifest = sb.dir.join("com.unduhkilat.host.json");
        assert!(status(&sb.target(), &manifest, &sb.host).iter().all(|(_, s)| *s == Status::Missing));
        ensure_registered(&sb.target(), &sb.dir, &sb.host).unwrap();
        let before = std::fs::read_to_string(&manifest).unwrap();
        ensure_registered(&sb.target(), &sb.dir, &sb.host).unwrap();
        assert_eq!(before, std::fs::read_to_string(&manifest).unwrap());
    }

    #[test]
    fn moved_portable_folder_is_detected_and_repaired_without_elevation() {
        let a = Sandbox::new();
        ensure_registered(&a.target(), &a.dir, &a.host).unwrap();

        // The folder moves: same registry, new location.
        let new_dir = std::env::temp_dir().join(format!("uk-native-moved-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&new_dir).unwrap();
        let new_host = new_dir.join("unduhkilat-host.exe");
        std::fs::write(&new_host, b"stub").unwrap();
        let new_manifest = new_dir.join("com.unduhkilat.host.json");

        let stale = status(&a.target(), &new_manifest, &new_host);
        assert!(stale.iter().all(|(_, s)| *s == Status::Stale), "{stale:?}");

        let fixed = ensure_registered(&a.target(), &new_dir, &new_host).unwrap();
        assert!(fixed.iter().all(|(_, s)| *s == Status::Registered));
        let m: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&new_manifest).unwrap()).unwrap();
        assert_eq!(m["path"], new_host.to_string_lossy().as_ref());
        std::fs::remove_dir_all(new_dir).ok();
    }

    #[test]
    fn tampered_manifest_is_reported_stale_then_repaired() {
        let sb = Sandbox::new();
        ensure_registered(&sb.target(), &sb.dir, &sb.host).unwrap();
        let manifest = sb.dir.join("com.unduhkilat.host.json");
        std::fs::write(&manifest, r#"{"name":"com.unduhkilat.host","path":"C:\\evil.exe","allowed_origins":["chrome-extension://*/"]}"#).unwrap();
        assert!(status(&sb.target(), &manifest, &sb.host).iter().all(|(_, s)| *s == Status::Stale));
        ensure_registered(&sb.target(), &sb.dir, &sb.host).unwrap();
        assert!(std::fs::read_to_string(&manifest).unwrap().contains("unduhkilat-host.exe"));
    }

    #[test]
    fn unregister_removes_every_key() {
        let sb = Sandbox::new();
        ensure_registered(&sb.target(), &sb.dir, &sb.host).unwrap();
        unregister(&sb.target());
        let manifest = sb.dir.join("com.unduhkilat.host.json");
        assert!(status(&sb.target(), &manifest, &sb.host).iter().all(|(_, s)| *s == Status::Missing));
    }

    #[test]
    fn missing_host_binary_is_a_clear_error() {
        let sb = Sandbox::new();
        let err = ensure_registered(&sb.target(), &sb.dir, &sb.dir.join("nope.exe")).unwrap_err();
        assert_eq!(err.code, ErrorCode::ErrNativeHostNotFound);
    }
}
