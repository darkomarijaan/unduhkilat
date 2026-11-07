//! `magnet:` link handler registration (per user, no elevation) and hand-off of a
//! clicked link to the running instance.
//!
//! Windows keeps the user's *chosen* default (UserChoice) out of reach of programs, so
//! registering only makes UnduhKilat selectable under Settings > Default apps; it does
//! not take over from an app the user already chose (e.g. qBittorrent).

use crate::error::{AppError, ErrorCode, Result};
use crate::paths::AppPaths;
use std::path::Path;

pub const PROG_ID: &str = "UnduhKilat.Magnet";
const APP_KEY: &str = r"Software\UnduhKilat";
const CAPS_KEY: &str = r"Software\UnduhKilat\Capabilities";

/// A link the OS handed to us as a command-line argument.
pub fn is_link_arg(a: &str) -> bool {
    let l = a.trim().to_ascii_lowercase();
    l.starts_with("magnet:") || l.starts_with("http://") || l.starts_with("https://") || l.starts_with("ftp://")
}

#[cfg(windows)]
pub fn register(exe: &Path) -> Result<()> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let werr = |e: std::io::Error| AppError::new(ErrorCode::ErrRegistryWriteFailed, format!("magnet handler: {e}"));
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let exe = exe.to_string_lossy().to_string();
    let command = format!("\"{exe}\" \"%1\"");

    for class in [r"Software\Classes\UnduhKilat.Magnet", r"Software\Classes\magnet"] {
        let (k, _) = hkcu.create_subkey(class).map_err(werr)?;
        k.set_value("", &"URL:Magnet Link").map_err(werr)?;
        k.set_value("URL Protocol", &"").map_err(werr)?;
        let (icon, _) = k.create_subkey("DefaultIcon").map_err(werr)?;
        icon.set_value("", &format!("\"{exe}\",0")).map_err(werr)?;
        let (cmd, _) = k.create_subkey(r"shell\open\command").map_err(werr)?;
        cmd.set_value("", &command).map_err(werr)?;
    }

    // Makes "UnduhKilat" appear in Settings > Default apps for the magnet protocol.
    let (caps, _) = hkcu.create_subkey(CAPS_KEY).map_err(werr)?;
    caps.set_value("ApplicationName", &"UnduhKilat").map_err(werr)?;
    caps.set_value("ApplicationDescription", &"Manajer unduhan berkecepatan tinggi").map_err(werr)?;
    let (assoc, _) = caps.create_subkey("URLAssociations").map_err(werr)?;
    assoc.set_value("magnet", &PROG_ID).map_err(werr)?;
    let (reg, _) = hkcu.create_subkey(r"Software\RegisteredApplications").map_err(werr)?;
    reg.set_value("UnduhKilat", &CAPS_KEY).map_err(werr)?;
    let _ = APP_KEY;
    Ok(())
}

#[cfg(not(windows))]
pub fn register(_: &Path) -> Result<()> {
    Err(AppError::new(ErrorCode::ErrRegistryWriteFailed, "registry is Windows-only (DEC-005)"))
}

/// Send `url` to the instance that owns this data folder. It opens the confirmation
/// dialog there; nothing starts without the user's click.
#[cfg(windows)]
pub fn forward_link(paths: &AppPaths, url: &str) -> Result<()> {
    use crate::bridge::{pipe_name, token_path};
    use crate::ipc::{read_frame, write_frame, Request, Response};
    use serde_json::json;
    use std::fs::OpenOptions;

    let token = std::fs::read_to_string(token_path(&paths.state_dir)).map_err(|_| AppError::internal("no running instance"))?;
    let mut pipe = OpenOptions::new().read(true).write(true).open(pipe_name(paths)).map_err(|_| AppError::internal("no running instance"))?;
    let mut call = |id: &str, command: &str, payload: serde_json::Value| -> Result<()> {
        write_frame(&mut pipe, &Request { id: id.into(), command: command.into(), payload })?;
        match read_frame::<_, Response>(&mut pipe)? {
            Some(Response::Ok { .. }) => Ok(()),
            Some(Response::Error { error, .. }) => Err(AppError::new(error.code, error.message)),
            None => Err(AppError::internal("instance closed the connection")),
        }
    };
    call("h", "host.hello", json!({ "token": token.trim() }))?;
    call("l", "app.open_link", json!({ "url": url.trim() }))
}

#[cfg(not(windows))]
pub fn forward_link(_: &AppPaths, _: &str) -> Result<()> {
    Err(AppError::internal("no running instance"))
}
