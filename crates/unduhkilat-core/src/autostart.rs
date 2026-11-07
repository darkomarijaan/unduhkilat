//! "Start with Windows": a per-user `Run` entry (no elevation, no service, no scheduled task).
//!
//! The entry launches `UnduhKilat.exe --background`, which starts hidden in the tray.
//! The registry value is the single source of truth for the setting, so the checkbox can
//! never disagree with what Windows will really do.

use crate::error::{AppError, ErrorCode, Result};
use std::path::Path;

pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const VALUE_NAME: &str = "UnduhKilat";
/// Flag the autostart entry passes; the app then starts hidden in the tray.
pub const BACKGROUND_FLAG: &str = "--background";

pub fn command_for(exe: &Path) -> String {
    format!("\"{}\" {BACKGROUND_FLAG}", exe.display())
}

#[cfg(windows)]
mod imp {
    use super::*;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};
    use winreg::RegKey;

    fn werr(e: std::io::Error) -> AppError {
        AppError::new(ErrorCode::ErrRegistryWriteFailed, format!("autostart: {e}"))
    }

    pub fn current(key: &str) -> Option<String> {
        RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(key, KEY_READ).ok()?.get_value::<String, _>(VALUE_NAME).ok()
    }

    pub fn set(key: &str, enabled: bool, exe: &Path) -> Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if enabled {
            let (k, _) = hkcu.create_subkey(key).map_err(werr)?;
            k.set_value(VALUE_NAME, &command_for(exe)).map_err(werr)
        } else {
            match hkcu.open_subkey_with_flags(key, KEY_SET_VALUE) {
                Ok(k) => match k.delete_value(VALUE_NAME) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(werr(e)),
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(werr(e)),
            }
        }
    }

    #[allow(dead_code)] // used by the tests
    pub fn delete_key(key: &str) {
        let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(key);
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    pub fn current(_: &str) -> Option<String> {
        None
    }
    pub fn set(_: &str, _: bool, _: &Path) -> Result<()> {
        Err(AppError::new(ErrorCode::ErrRegistryWriteFailed, "registry is Windows-only (DEC-005)"))
    }
    pub fn delete_key(_: &str) {}
}

/// Is the app set to start with Windows?
pub fn is_enabled() -> bool {
    imp::current(RUN_KEY).is_some()
}

pub fn set_enabled(enabled: bool, exe: &Path) -> Result<()> {
    imp::set(RUN_KEY, enabled, exe)
}

/// A portable folder that was moved leaves a stale path behind: repoint it, but only if the
/// user had the option on (never turns it on by itself).
pub fn refresh_if_stale(exe: &Path) {
    refresh_at(RUN_KEY, exe)
}

fn refresh_at(key: &str, exe: &Path) {
    if let Some(cur) = imp::current(key) {
        if cur != command_for(exe) {
            let _ = imp::set(key, true, exe);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn enable_disable_and_repair_a_moved_folder() {
        let key = r"Software\UnduhKilatTest\Run";
        imp::delete_key(r"Software\UnduhKilatTest");
        let (a, b) = (Path::new(r"C:\A B\UnduhKilat.exe"), Path::new(r"D:\Moved\UnduhKilat.exe"));
        assert!(imp::current(key).is_none());
        refresh_at(key, a);
        assert!(imp::current(key).is_none(), "repair must never switch the option on by itself");
        imp::set(key, true, a).unwrap();
        assert_eq!(imp::current(key).unwrap(), "\"C:\\A B\\UnduhKilat.exe\" --background", "path with spaces is quoted");
        refresh_at(key, b);
        assert_eq!(imp::current(key).unwrap(), command_for(b));
        imp::set(key, false, b).unwrap();
        assert!(imp::current(key).is_none());
        imp::set(key, false, b).unwrap(); // turning off twice is fine
        imp::delete_key(r"Software\UnduhKilatTest");
    }
}
