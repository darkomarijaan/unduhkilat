//! UnduhKilat desktop entry point.
//!
//!   UnduhKilat.exe                      GUI
//!   UnduhKilat.exe cli <url>...         headless engine driver
//!   UnduhKilat.exe register-host        (re)register the native messaging host for this user
//!   UnduhKilat.exe unregister-host      remove those registry entries
#![cfg_attr(not(test), windows_subsystem = "windows")]

mod app;
mod boot;
mod cli;
mod format;
mod theme;
mod tray;

use eframe::egui;
use std::sync::Arc;
use unduhkilat_core::bridge::Bridge;
use unduhkilat_core::native;
use unduhkilat_core::paths::AppPaths;

fn attach_console() {
    // The GUI subsystem has no console; attach the parent's so `cli` can print.
    #[cfg(windows)]
    unsafe {
        unsafe extern "system" {
            fn AttachConsole(pid: u32) -> i32;
        }
        AttachConsole(u32::MAX); // ATTACH_PARENT_PROCESS
    }
}

/// `register-host` / `unregister-host`: no GUI, no single-instance guard, exit code 0/1.
fn host_registration_command(cmd: &str) -> ! {
    attach_console();
    let paths = AppPaths::from_current_exe().expect("data directory");
    let code = match cmd {
        "register-host" => match app::register_host(&paths) {
            Ok(st) => {
                for (b, s) in st {
                    println!("{}: {s:?}", b.name());
                }
                0
            }
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        _ => {
            native::unregister(&native::Target::default());
            let _ = std::fs::remove_file(paths.data_dir.join(format!("{}.json", native::HOST_NAME)));
            println!("native messaging host unregistered");
            0
        }
    };
    std::process::exit(code);
}

/// `--admin-*` maintenance commands (FR-035, FR-062). Elevation is checked by the core
/// before anything changes; exit code 0 = done, 5 = not elevated, 2 = app running, 1 = other.
fn admin_command(cmd: &str, rest: &[String]) -> ! {
    use unduhkilat_core::admin;
    use unduhkilat_core::error::ErrorCode;
    attach_console();
    let fail = |e: unduhkilat_core::AppError| -> ! {
        eprintln!("{}: {}", e.code, e.message);
        std::process::exit(if e.code == ErrorCode::ErrPermissionDenied { 5 } else { 1 });
    };
    let paths = AppPaths::from_current_exe().unwrap_or_else(|e| fail(unduhkilat_core::AppError::internal(e.to_string())));
    match cmd {
        "--admin-register-host" => {
            let exe = std::env::current_exe().unwrap_or_else(|e| fail(unduhkilat_core::AppError::internal(e.to_string())));
            match admin::register_host_machine_wide(&native::host_exe_next_to(&exe)) {
                Ok(st) => {
                    for (b, s) in st {
                        println!("HKLM {}: {s:?}", b.name());
                    }
                    std::process::exit(0);
                }
                Err(e) => fail(e),
            }
        }
        "--admin-reset-data" => {
            // Permission first, so an unelevated caller learns nothing about the app state.
            if let Err(e) = admin::require_elevation() {
                fail(e);
            }
            // The database is open while the app runs; wiping it underneath would corrupt state.
            let Some(_guard) = boot::single_instance(&paths) else {
                eprintln!("UnduhKilat is running for this data folder; exit it first.");
                std::process::exit(2);
            };
            let confirm = rest.iter().any(|a| a == "--confirm-reset");
            match admin::reset_data(&paths, confirm) {
                Ok(r) => {
                    println!("database_purged: {}
logs_purged: {}
files_removed: {}
downloaded_files_touched: {}", r.database_purged, r.logs_purged, r.files_removed, r.downloaded_files_touched);
                    std::process::exit(0);
                }
                Err(e) => fail(e),
            }
        }
        "--admin-upgrade-bundle" => {
            eprintln!("--admin-upgrade-bundle is not available yet: it needs the update signing key (TASK-47/49).");
            std::process::exit(3);
        }
        other => {
            eprintln!("unknown command {other}; use --admin-register-host, --admin-reset-data --confirm-reset or --admin-upgrade-bundle");
            std::process::exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Update helper (FR-060): a copy of the new exe that swaps files after the old process exits.
    if args.first().map(String::as_str) == Some("--apply-update") && args.len() == 4 {
        let pid: u32 = args[1].parse().unwrap_or(0);
        let code = match unduhkilat_core::update::run_apply(pid, std::path::Path::new(&args[2]), std::path::Path::new(&args[3])) {
            Ok(()) => 0,
            Err(msg) => {
                rfd::MessageDialog::new().set_title("UnduhKilat").set_description(msg).set_level(rfd::MessageLevel::Error).show();
                1
            }
        };
        std::process::exit(code);
    }
    if let Some(cmd) = args.first().filter(|a| a.starts_with("--admin-")) {
        admin_command(cmd, &args[1..]);
    }
    if let Some(cmd @ ("register-host" | "unregister-host")) = args.first().map(String::as_str) {
        host_registration_command(cmd);
    }
    // A clicked magnet/link: hand it to the running instance and exit (no second window).
    if let Some(link) = args.iter().find(|a| unduhkilat_core::protocol::is_link_arg(a)) {
        if args.first().map(String::as_str) != Some("cli") {
            if let Ok(paths) = AppPaths::from_current_exe() {
                if unduhkilat_core::protocol::forward_link(&paths, link).is_ok() {
                    return;
                }
            }
        }
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");

    let is_cli = args.first().map(String::as_str) == Some("cli");
    let _instance = match AppPaths::from_current_exe().ok().map(|p| boot::single_instance(&p)) {
        Some(None) if args.iter().any(|a| a == unduhkilat_core::autostart::BACKGROUND_FLAG) => {
            // Autostart while the app is already open: nothing to say, nothing to do.
            std::process::exit(0);
        }
        Some(None) => {
            let msg = "UnduhKilat sudah berjalan untuk folder data ini. Buka dari ikon tray di dekat jam.";
            if is_cli {
                attach_console();
                eprintln!("{msg}");
            } else {
                rfd::MessageDialog::new().set_title("UnduhKilat").set_description(msg).show();
            }
            std::process::exit(2);
        }
        other => other.flatten(),
    };

    if is_cli {
        attach_console();
        if let Err(e) = rt.block_on(cli::run(args[1..].to_vec())) {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        return;
    }

    let b = match rt.block_on(boot::boot()) {
        Ok(b) => b,
        Err(e) => {
            rfd::MessageDialog::new().set_title("UnduhKilat").set_description(format!("Gagal memulai: {e}")).set_level(rfd::MessageLevel::Error).show();
            std::process::exit(1);
        }
    };
    boot::spawn_toasts(rt.handle(), &b.manager);
    // DEC-008: torrents that were still seeding when the app last closed (opens a port only if any).
    {
        let m = b.manager.clone();
        rt.spawn(async move {
            // Items still queued when the app last closed start now (nothing else would trigger a dispatch).
            if let Err(e) = m.dispatch().await {
                tracing::warn!(error = %e, "could not start queued downloads");
            }
            m.restore_seeding().await;
        });
    }

    // FR-034: register (or repair after a folder move) the native host for this user. No elevation.
    // UNDUHKILAT_NO_REGISTER: tests and demos must not repoint the real browser registration.
    if std::env::var_os("UNDUHKILAT_NO_REGISTER").is_none() {
        match app::register_host(&b.paths) {
            Ok(st) => tracing::info!(?st, "native messaging host registration verified"),
            Err(e) => tracing::warn!(error = %e, "native messaging host registration failed; browser capture is unavailable"),
        }
        // A moved portable folder would leave the "start with Windows" entry pointing nowhere.
        if let Ok(exe) = std::env::current_exe() {
            unduhkilat_core::autostart::refresh_if_stale(&exe);
        }
        // Lets UnduhKilat be chosen as the handler for magnet: links (Settings > Default apps).
        if let Ok(exe) = std::env::current_exe() {
            if let Err(e) = unduhkilat_core::protocol::register(&exe) {
                tracing::warn!(error = %e, "magnet handler registration failed");
            }
        }
    }

    let start_hidden = args.iter().any(|a| a == unduhkilat_core::autostart::BACKGROUND_FLAG);
    let first_url = args.into_iter().find(|a| unduhkilat_core::protocol::is_link_arg(a));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("UnduhKilat - Manajer Unduhan Berkecepatan Tinggi [Portabel]")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([800.0, 600.0])
            // The orange header band is the title bar (DESIGN.md); app.rs draws the controls.
            .with_decorations(false)
            .with_icon(tray::window_icon()),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    let (handle, manager, paths, tail) = (rt.handle().clone(), b.manager.clone(), b.paths.clone(), b.log.tail.clone());
    let _keep_log = b.log;
    let result = eframe::run_native(
        "UnduhKilat",
        options,
        Box::new(move |cc| {
            let tray = match tray::create(cc.egui_ctx.clone(), handle.clone(), manager.clone()) {
                Ok(t) => Some(t),
                Err(e) => {
                    tracing::warn!(error = %e, "tray unavailable; close will exit the app");
                    None
                }
            };
            // Browser bridge: a proposal pops the window forward so the user can confirm (FR-039).
            let (ptx, prx) = std::sync::mpsc::channel();
            let wake_ctx = cc.egui_ctx.clone();
            let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || tray::show_window(&wake_ctx));
            let bridge = match Bridge::start(&handle, manager.clone(), &paths, ptx, wake) {
                Ok(b) => Some(b),
                Err(e) => {
                    tracing::warn!(error = %e, "browser bridge unavailable");
                    None
                }
            };
            let prx = bridge.as_ref().map(|_| prx);
            let mut app = app::App::new(cc.egui_ctx.clone(), handle, manager, paths, tail, tray, bridge, prx);
            if let Some(u) = &first_url {
                app.open_add_dialog(u);
            }
            app.start_hidden = start_hidden;
            Ok(Box::new(app))
        }),
    );
    // Window closed for real (no tray): checkpoint before exiting (FR-053).
    let _ = rt.block_on(b.manager.shutdown(std::time::Duration::from_secs(15)));
    if let Err(e) = result {
        eprintln!("ui error: {e}");
    }
}
