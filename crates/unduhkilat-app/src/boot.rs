//! Startup sequence shared by the GUI and the CLI, plus toast and exit flows.

use std::path::PathBuf;
use std::time::Duration;
use tokio::runtime::Handle;
use unduhkilat_core::config::{keys, EngineConfig};
use unduhkilat_core::db::Db;
use unduhkilat_core::engine::manager::Manager;
use unduhkilat_core::engine::EngineEvent;
use unduhkilat_core::logging::{self, LogHandle};
use unduhkilat_core::models::DownloadState;
use unduhkilat_core::paths::AppPaths;
use unduhkilat_core::{category, repo};

pub struct Boot {
    pub paths: AppPaths,
    pub manager: Manager,
    pub log: LogHandle,
}

pub async fn boot() -> Result<Boot, Box<dyn std::error::Error>> {
    let paths = AppPaths::from_current_exe()?;
    paths.create_dirs()?;
    let log = logging::init(&paths.logs_dir, "info,wgpu_hal=off,wgpu_core=warn,naga=warn,egui_wgpu=warn")?;
    tracing::info!(portable = paths.portable, data = %paths.data_dir.display(), "UnduhKilat starting");
    let db = Db::open(paths.db_path())?;
    let root = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| paths.data_dir.clone()).join("Downloads");
    category::seed_defaults(&db, &root)?;
    let manager = Manager::start(db, EngineConfig::default()).await?;
    Ok(Boot { paths, manager, log })
}

/// Windows toast on completion (DEC-004, FR-054). Independent of window visibility.
pub fn spawn_toasts(rt: &Handle, manager: &Manager) {
    let manager = manager.clone();
    let mut rx = manager.subscribe();
    rt.spawn(async move {
        while let Ok(ev) = rx.recv().await {
            let EngineEvent::StateChanged { id, state: DownloadState::Completed, .. } = ev else { continue };
            let enabled = manager
                .db()
                .read(|c| repo::get_setting(c, keys::COMPLETION_TOAST))
                .ok()
                .flatten()
                .map(|v| v != "0")
                .unwrap_or(true);
            if !enabled {
                continue;
            }
            if let Ok(d) = manager.get(&id) {
                let text = format!("{} berhasil diunduh dan diverifikasi integritasnya.", d.file_name);
                let path = d.target_path.clone();
                let _ = tokio::task::spawn_blocking(move || show_toast(&text, &path)).await;
            }
        }
    });
}

#[cfg(windows)]
fn show_toast(text: &str, path: &str) {
    use winrt_notification::{Duration as D, Toast};
    if let Err(e) = Toast::new(Toast::POWERSHELL_APP_ID).title("Unduhan Selesai").text1(text).text2(path).duration(D::Short).show() {
        tracing::warn!(error = %e, "toast failed");
    }
}

#[cfg(not(windows))]
fn show_toast(_: &str, _: &str) {}

/// Explicit exit (FR-053): tell the user transfers will stop, checkpoint, terminate.
/// Blocking; call from a non-runtime thread.
pub fn exit_flow(rt: Handle, manager: Manager) {
    let running = manager.running_count();
    if running > 0 {
        let answer = rfd::MessageDialog::new()
            .set_title("Keluar dari UnduhKilat")
            .set_description(format!("{running} unduhan sedang berjalan dan akan dijeda. Anda dapat melanjutkannya saat aplikasi dibuka lagi. Keluar sekarang?"))
            .set_buttons(rfd::MessageButtons::YesNo)
            .show();
        if answer != rfd::MessageDialogResult::Yes {
            return;
        }
    }
    unduhkilat_core::term::local::kill_all_local();
    if let Err(e) = rt.block_on(manager.shutdown(Duration::from_secs(15))) {
        tracing::error!(error = %e, "shutdown did not finish cleanly");
    }
    std::process::exit(0);
}

/// Holds the OS mutex for the process lifetime. A second instance over the same data
/// directory would run crash-normalization and flip the first instance's running
/// downloads to `paused` in the shared database.
pub struct SingleInstance(#[allow(dead_code)] isize);

/// `None` means another instance already owns this data directory.
#[cfg(windows)]
pub fn single_instance(paths: &AppPaths) -> Option<SingleInstance> {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    // Same key as the IPC pipe name, so "one app per data folder" holds for both.
    let name: Vec<u16> = format!("Local\\UnduhKilat.{}", paths.instance_key()).encode_utf16().chain(Some(0)).collect();
    // SAFETY: `name` is NUL-terminated and outlives the call.
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if handle.is_null() || already {
        return None;
    }
    Some(SingleInstance(handle as isize))
}

#[cfg(not(windows))]
pub fn single_instance(_: &AppPaths) -> Option<SingleInstance> {
    Some(SingleInstance(0))
}
