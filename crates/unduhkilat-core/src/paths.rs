//! Data-directory resolution (DEC-007): portable marker selects `./data`,
//! otherwise `%LOCALAPPDATA%\UnduhKilat`.

use std::path::{Path, PathBuf};

pub const PORTABLE_MARKER: &str = "unduhkilat-portable.marker";

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub data_dir: PathBuf,
    pub state_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub portable: bool,
}

impl AppPaths {
    pub fn db_path(&self) -> PathBuf {
        self.state_dir.join("unduhkilat.db")
    }

    /// Resolve using explicit inputs (testable). `local_app_data` is `%LOCALAPPDATA%`.
    pub fn resolve(exe_dir: &Path, local_app_data: Option<PathBuf>) -> Self {
        let portable = exe_dir.join(PORTABLE_MARKER).exists();
        let data_dir = if portable {
            exe_dir.join("data")
        } else {
            local_app_data.unwrap_or_else(|| exe_dir.join("data")).join("UnduhKilat")
        };
        Self { state_dir: data_dir.join("state"), logs_dir: data_dir.join("logs"), data_dir, portable }
    }

    /// Stable per-data-directory key (FNV-1a 64) used for the single-instance mutex and the
    /// IPC pipe name, so portable copies in different folders never collide. Deterministic
    /// across processes (unlike `DefaultHasher`), which the host and app both rely on.
    pub fn instance_key(&self) -> String {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in self.data_dir.to_string_lossy().to_lowercase().bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        format!("{h:016x}")
    }

    /// `UNDUHKILAT_DATA_DIR` overrides the data directory (used by tests and for
    /// running several isolated instances). Not part of the end-user surface.
    pub fn from_current_exe() -> std::io::Result<Self> {
        if let Some(dir) = std::env::var_os("UNDUHKILAT_DATA_DIR").map(PathBuf::from) {
            return Ok(Self { state_dir: dir.join("state"), logs_dir: dir.join("logs"), data_dir: dir, portable: true });
        }
        let exe = std::env::current_exe()?;
        let dir = exe.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        Ok(Self::resolve(&dir, std::env::var_os("LOCALAPPDATA").map(PathBuf::from)))
    }

    pub fn create_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.state_dir)?;
        std::fs::create_dir_all(&self.logs_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_selects_local_data_dir() {
        let tmp = std::env::temp_dir().join(format!("uk-paths-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join(PORTABLE_MARKER), b"").unwrap();
        let p = AppPaths::resolve(&tmp, Some(PathBuf::from("C:/ignored")));
        assert!(p.portable);
        assert_eq!(p.db_path(), tmp.join("data").join("state").join("unduhkilat.db"));
        assert_eq!(p.logs_dir, tmp.join("data").join("logs"));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn instance_key_is_stable_and_distinguishes_folders() {
        let mk = |d: &str| AppPaths { data_dir: PathBuf::from(d), state_dir: PathBuf::new(), logs_dir: PathBuf::new(), portable: true };
        assert_eq!(mk("C:/A/data").instance_key(), mk("c:/a/DATA").instance_key(), "case-insensitive");
        assert_ne!(mk("C:/A/data").instance_key(), mk("C:/B/data").instance_key());
        assert_eq!(mk("C:/A/data").instance_key().len(), 16);
        // Known FNV-1a vector so a future change cannot silently break host<->app agreement.
        assert_eq!(mk("").instance_key(), "cbf29ce484222325");
    }

    #[test]
    fn without_marker_falls_back_to_local_app_data() {
        let tmp = std::env::temp_dir().join(format!("uk-paths-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let p = AppPaths::resolve(&tmp, Some(PathBuf::from("C:/Users/x/AppData/Local")));
        assert!(!p.portable);
        assert_eq!(p.data_dir, PathBuf::from("C:/Users/x/AppData/Local").join("UnduhKilat"));
        std::fs::remove_dir_all(&tmp).ok();
    }
}
