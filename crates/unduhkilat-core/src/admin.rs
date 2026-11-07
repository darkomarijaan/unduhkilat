//! Elevated maintenance operations (FR-035, FR-062). Every entry point that changes
//! machine state checks the Windows token first and fails with `ERR_PERMISSION_DENIED`
//! otherwise; none of them ever touches user download folders.

use crate::error::{AppError, ErrorCode, Result};
use crate::native::{self, Browser, Status, Target};
use crate::paths::AppPaths;
use serde::Serialize;
use std::path::Path;

/// Is the current process token elevated (UAC "Run as administrator")?
#[cfg(windows)]
pub fn is_elevated() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    // SAFETY: standard token query; the handle is closed before returning.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elev = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0u32;
        let ok = GetTokenInformation(token, TokenElevation, &mut elev as *mut _ as *mut _, std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut len);
        CloseHandle(token);
        ok != 0 && elev.TokenIsElevated != 0
    }
}

#[cfg(not(windows))]
pub fn is_elevated() -> bool {
    false
}

pub fn require_elevation() -> Result<()> {
    if is_elevated() {
        Ok(())
    } else {
        Err(AppError::new(ErrorCode::ErrPermissionDenied, "this command needs an elevated (Administrator) prompt"))
    }
}

/// FR-035: register the native host for every user of the machine (HKLM). The manifest
/// lives next to the binaries, so an in-place bundle update keeps the registration valid.
pub fn register_host_machine_wide(host_exe: &Path) -> Result<Vec<(Browser, Status)>> {
    require_elevation()?;
    register_host_in(&Target { machine: true, ..Default::default() }, host_exe)
}

/// Shared by the elevated command and the tests (which use a throwaway HKCU prefix).
pub fn register_host_in(t: &Target, host_exe: &Path) -> Result<Vec<(Browser, Status)>> {
    let dir = host_exe.parent().ok_or_else(|| AppError::new(ErrorCode::ErrNativeHostNotFound, "host path has no directory"))?;
    native::ensure_registered(t, dir, host_exe)
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ResetReport {
    pub database_purged: bool,
    pub logs_purged: bool,
    pub files_removed: usize,
    pub downloaded_files_touched: usize,
}

/// FR-062: purge `unduhkilat.db` (+ WAL/SHM) and the log directory contents.
/// Needs elevation and an explicit `confirm`.
pub fn reset_data(paths: &AppPaths, confirm: bool) -> Result<ResetReport> {
    require_elevation()?;
    if !confirm {
        return Err(AppError::invalid("confirm_reset must be true; nothing was deleted"));
    }
    perform_reset(paths)
}

/// A file may stay locked for a moment after its last handle closes (database writer
/// thread winding down, antivirus scan): retry briefly instead of failing the reset.
fn remove_with_retry(p: &Path) -> std::io::Result<()> {
    let mut last = None;
    for _ in 0..20 {
        match std::fs::remove_file(p) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(e),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
    Err(last.expect("loop ran"))
}

/// The deletion itself, with no privilege check (tests drive it directly).
/// Only the database trio and files directly inside the logs directory are removed.
pub fn perform_reset(paths: &AppPaths) -> Result<ResetReport> {
    let mut report = ResetReport::default();
    let db = paths.db_path();
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut p = db.clone().into_os_string();
        p.push(suffix);
        let p = std::path::PathBuf::from(p);
        match remove_with_retry(&p) {
            Ok(()) => {
                report.files_removed += 1;
                if suffix.is_empty() {
                    report.database_purged = true;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if suffix.is_empty() {
                    report.database_purged = true; // already absent
                }
            }
            Err(e) => return Err(AppError::new(ErrorCode::ErrPathUnwritable, format!("cannot delete {}: {e}", p.display()))),
        }
    }
    if let Ok(rd) = std::fs::read_dir(&paths.logs_dir) {
        for entry in rd.flatten() {
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                remove_with_retry(&entry.path()).map_err(|e| AppError::new(ErrorCode::ErrPathUnwritable, format!("cannot delete {}: {e}", entry.path().display())))?;
                report.files_removed += 1;
            }
        }
    }
    std::fs::create_dir_all(&paths.logs_dir).map_err(|e| AppError::new(ErrorCode::ErrStorageFull, e.to_string()))?;
    std::fs::create_dir_all(&paths.state_dir).map_err(|e| AppError::new(ErrorCode::ErrStorageFull, e.to_string()))?;
    report.logs_purged = true;
    Ok(report)
}
