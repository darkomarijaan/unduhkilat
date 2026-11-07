//! Signed in-place updates of the portable bundle (FR-060, DEPLOYMENT.md §8.1).
//!
//! 1. `check` asks GitHub for the newest release (pre-releases included).
//! 2. `download` fetches the zip and its `.sig`, and refuses to go on unless the
//!    Ed25519 signature verifies against the public key built into this binary.
//! 3. `stage` unpacks the verified zip; `spawn_apply` starts a copy of the NEW exe as a
//!    helper that waits for this process to exit, swaps the files (keeping `.bak`
//!    copies), starts the new version and rolls back if it dies right away.
//!
//! Anything that fails leaves the installed files untouched or restored.

use crate::error::{AppError, ErrorCode, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Public half of the release signing key (private half lives only on the publisher's machine).
pub const PUBLIC_KEY_B64: &str = "GKf5Qxn7KInpvO/Msh7Fty6AapLKT7ANaSJVA351lxk=";
const RELEASES_URL: &str = "https://api.github.com/repos/darkomarijaan/unduhkilat/releases?per_page=20";
const DOWNLOAD_PREFIX: &str = "https://github.com/darkomarijaan/unduhkilat/releases/download/";
/// Hard cap on what we are willing to download as an update.
const MAX_ZIP_BYTES: u64 = 200 * 1024 * 1024;

const APP_EXE: &str = "UnduhKilat.exe";
const HOST_EXE: &str = "unduhkilat-host.exe";

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: String,
    pub zip_url: String,
    pub sig_url: String,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// `v0.1.3` / `0.1.3` -> (0, 1, 3). Anything else is not a version we understand.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let mut it = s.trim().trim_start_matches('v').split('.');
    let v = (it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?);
    it.next().is_none().then_some(v)
}

fn net(e: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::ErrConnectionDropped, format!("update check failed: {e}"))
}

/// The newest release newer than `current`, if it carries a zip and a signature.
pub async fn check(client: &reqwest::Client, current: &str) -> Result<Option<UpdateInfo>> {
    let cur = parse_version(current).ok_or_else(|| AppError::internal("own version is not x.y.z"))?;
    let text = client
        .get(RELEASES_URL)
        .header("User-Agent", "UnduhKilat-updater")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(net)?
        .error_for_status()
        .map_err(net)?
        .text()
        .await
        .map_err(net)?;
    let releases: Vec<GhRelease> = serde_json::from_str(&text).map_err(net)?;
    Ok(pick_update(releases, cur))
}

fn pick_update(releases: Vec<GhRelease>, cur: (u64, u64, u64)) -> Option<UpdateInfo> {
    releases
        .into_iter()
        .filter(|r| !r.draft)
        .filter_map(|r| parse_version(&r.tag_name).map(|v| (v, r)))
        .filter(|(v, _)| *v > cur)
        .filter_map(|(v, r)| {
            let zip = r.assets.iter().find(|a| a.name.ends_with("-x64.zip"))?;
            let sig = r.assets.iter().find(|a| a.name == format!("{}.sig", zip.name))?;
            let info = UpdateInfo {
                version: r.tag_name.trim_start_matches('v').to_string(),
                notes: r.body.clone().unwrap_or_default(),
                zip_url: zip.browser_download_url.clone(),
                sig_url: sig.browser_download_url.clone(),
            };
            Some((v, info))
        })
        .max_by_key(|(v, _)| *v)
        .map(|(_, u)| u)
}

/// Verify `sig_b64` over `data` with the built-in public key.
pub fn verify(data: &[u8], sig_b64: &str) -> Result<()> {
    verify_with(PUBLIC_KEY_B64, data, sig_b64)
}

fn verify_with(key_b64: &str, data: &[u8], sig_b64: &str) -> Result<()> {
    let bad = |m: &str| AppError::new(ErrorCode::ErrPackageSignatureInvalid, format!("Package verification failed: {m}. The package has been discarded."));
    let key: [u8; 32] = B64.decode(key_b64).ok().and_then(|b| b.try_into().ok()).ok_or_else(|| bad("bad public key"))?;
    let key = VerifyingKey::from_bytes(&key).map_err(|_| bad("bad public key"))?;
    let sig: [u8; 64] = B64.decode(sig_b64.trim()).ok().and_then(|b| b.try_into().ok()).ok_or_else(|| bad("malformed signature"))?;
    key.verify(data, &Signature::from_bytes(&sig)).map_err(|_| bad("invalid cryptographic signature"))
}

async fn fetch(client: &reqwest::Client, url: &str, max: u64) -> Result<Vec<u8>> {
    if !url.starts_with(DOWNLOAD_PREFIX) {
        return Err(AppError::new(ErrorCode::ErrInvalidUrlScheme, "update URL is not from the official release page"));
    }
    let resp = client.get(url).header("User-Agent", "UnduhKilat-updater").send().await.map_err(net)?.error_for_status().map_err(net)?;
    if resp.content_length().is_some_and(|n| n > max) {
        return Err(AppError::internal("update is larger than allowed"));
    }
    let bytes = resp.bytes().await.map_err(net)?;
    if bytes.len() as u64 > max {
        return Err(AppError::internal("update is larger than allowed"));
    }
    Ok(bytes.to_vec())
}

/// Download and verify. Returns the zip bytes only if the signature is valid.
pub async fn download(client: &reqwest::Client, info: &UpdateInfo) -> Result<Vec<u8>> {
    let sig = fetch(client, &info.sig_url, 4096).await?;
    let zip = fetch(client, &info.zip_url, MAX_ZIP_BYTES).await?;
    verify(&zip, &String::from_utf8_lossy(&sig))?;
    Ok(zip)
}

/// Unpack a verified zip into `staging` (replaced if present). `data/` is never taken.
pub fn stage(zip_bytes: &[u8], staging: &Path) -> Result<()> {
    let _ = std::fs::remove_dir_all(staging);
    std::fs::create_dir_all(staging)?;
    let bad = |e: zip::result::ZipError| AppError::internal(format!("bad update archive: {e}"));
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(bad)?;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(bad)?;
        // `enclosed_name` rejects `..` and absolute paths (zip-slip).
        let Some(rel) = f.enclosed_name() else { return Err(AppError::internal("update archive has an unsafe path")) };
        if rel.components().next().is_some_and(|c| c.as_os_str().eq_ignore_ascii_case("data")) {
            continue;
        }
        let out = staging.join(&rel);
        if f.is_dir() {
            std::fs::create_dir_all(&out)?;
        } else {
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p)?;
            }
            let mut w = std::fs::File::create(&out)?;
            std::io::copy(&mut f, &mut w)?;
        }
    }
    for need in [APP_EXE, HOST_EXE] {
        if !staging.join(need).is_file() {
            return Err(AppError::internal(format!("update archive is missing {need}")));
        }
    }
    Ok(())
}

pub fn staging_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("update-staging")
}

/// Start the helper (a copy of the new exe) and return; the caller must exit right after.
#[cfg(windows)]
pub fn spawn_apply(staging: &Path, app_dir: &Path) -> Result<()> {
    let helper = std::env::temp_dir().join(format!("UnduhKilat-updater-{}.exe", std::process::id()));
    std::fs::copy(staging.join(APP_EXE), &helper)?;
    std::process::Command::new(&helper)
        .arg("--apply-update")
        .arg(std::process::id().to_string())
        .arg(staging)
        .arg(app_dir)
        .spawn()
        .map_err(|e| AppError::internal(format!("cannot start the updater: {e}")))?;
    Ok(())
}

#[cfg(not(windows))]
pub fn spawn_apply(_: &Path, _: &Path) -> Result<()> {
    Err(AppError::internal("updates are Windows-only (DEC-005)"))
}

#[cfg(windows)]
fn wait_for_exit(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    // SAFETY: plain Win32 calls; the handle is closed after the wait.
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !h.is_null() {
            WaitForSingleObject(h, 30_000);
            CloseHandle(h);
        }
    }
}

#[cfg(not(windows))]
fn wait_for_exit(_: u32) {}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let dest = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &dest)?;
        } else {
            std::fs::copy(e.path(), dest)?;
        }
    }
    Ok(())
}

/// Helper entry point: swap the files, start the new version, roll back if it dies.
/// Never touches `data/`. Returns a message for the caller to show on failure.
pub fn run_apply(old_pid: u32, staging: &Path, app_dir: &Path) -> std::result::Result<(), String> {
    wait_for_exit(old_pid);
    let bak = |n: &str| app_dir.join(format!("{n}.bak"));
    let swap = || -> std::io::Result<()> {
        for n in [APP_EXE, HOST_EXE] {
            let _ = std::fs::remove_file(bak(n));
            if app_dir.join(n).exists() {
                std::fs::rename(app_dir.join(n), bak(n))?;
            }
            std::fs::copy(staging.join(n), app_dir.join(n))?;
        }
        let ext = staging.join("extension");
        if ext.is_dir() {
            let dest = app_dir.join("extension");
            let _ = std::fs::remove_dir_all(&dest);
            copy_dir(&ext, &dest)?;
        }
        if staging.join("BACA-SAYA.txt").is_file() {
            std::fs::copy(staging.join("BACA-SAYA.txt"), app_dir.join("BACA-SAYA.txt"))?;
        }
        Ok(())
    };
    let restore = || {
        for n in [APP_EXE, HOST_EXE] {
            if bak(n).exists() {
                let _ = std::fs::remove_file(app_dir.join(n));
                let _ = std::fs::rename(bak(n), app_dir.join(n));
            }
        }
    };
    if let Err(e) = swap() {
        restore();
        let _ = std::process::Command::new(app_dir.join(APP_EXE)).spawn();
        return Err(format!("pembaruan gagal dan dibatalkan: {e}"));
    }
    let mut child = std::process::Command::new(app_dir.join(APP_EXE)).spawn().map_err(|e| format!("tidak bisa membuka versi baru: {e}"))?;
    std::thread::sleep(std::time::Duration::from_secs(8));
    if matches!(child.try_wait(), Ok(Some(_))) {
        // The new version died on launch: put the old one back (DEPLOYMENT.md §8.1).
        restore();
        let _ = std::process::Command::new(app_dir.join(APP_EXE)).spawn();
        return Err("versi baru langsung berhenti; versi lama dipulihkan".into());
    }
    let _ = std::fs::remove_dir_all(staging);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }
    fn pubkey(k: &SigningKey) -> String {
        B64.encode(k.verifying_key().to_bytes())
    }

    #[test]
    fn versions_parse_and_compare() {
        assert_eq!(parse_version("v0.1.10"), Some((0, 1, 10)));
        assert!(parse_version("0.1.10") > parse_version("0.1.9"));
        assert_eq!(parse_version("0.1"), None);
        assert_eq!(parse_version("1.2.3-beta"), None);
    }

    #[test]
    fn valid_signature_passes_and_tampering_is_rejected() {
        let k = key();
        let sig = B64.encode(k.sign(b"zip bytes").to_bytes());
        assert!(verify_with(&pubkey(&k), b"zip bytes", &sig).is_ok());
        let e = verify_with(&pubkey(&k), b"zip bytez", &sig).unwrap_err();
        assert_eq!(e.code, ErrorCode::ErrPackageSignatureInvalid);
        let other = SigningKey::from_bytes(&[9u8; 32]);
        assert_eq!(verify_with(&pubkey(&other), b"zip bytes", &sig).unwrap_err().code, ErrorCode::ErrPackageSignatureInvalid);
        assert_eq!(verify_with(&pubkey(&k), b"zip bytes", "not-base64!!").unwrap_err().code, ErrorCode::ErrPackageSignatureInvalid);
    }

    fn rel(tag: &str, with_sig: bool, draft: bool) -> GhRelease {
        let zip = format!("UnduhKilat-portable-{}-x64.zip", tag.trim_start_matches('v'));
        let mut assets = vec![GhAsset { name: zip.clone(), browser_download_url: format!("{DOWNLOAD_PREFIX}{tag}/{zip}") }];
        if with_sig {
            assets.push(GhAsset { name: format!("{zip}.sig"), browser_download_url: format!("{DOWNLOAD_PREFIX}{tag}/{zip}.sig") });
        }
        GhRelease { tag_name: tag.into(), draft, body: None, assets }
    }

    #[test]
    fn picks_newest_signed_non_draft_release() {
        let got = pick_update(vec![rel("v0.1.2", true, false), rel("v0.1.4", false, false), rel("v0.1.3", true, false), rel("v0.2.0", true, true)], (0, 1, 2));
        assert_eq!(got.unwrap().version, "0.1.3", "unsigned and draft releases are skipped");
        assert!(pick_update(vec![rel("v0.1.2", true, false)], (0, 1, 2)).is_none());
    }

    #[test]
    fn staging_rejects_zip_slip_and_skips_data() {
        use std::io::Write;
        let mk = |names: &[&str]| {
            let mut buf = Vec::new();
            {
                let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
                for n in names {
                    w.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
                    w.write_all(b"x").unwrap();
                }
                w.finish().unwrap();
            }
            buf
        };
        let dir = std::env::temp_dir().join(format!("uk-stage-{}", uuid::Uuid::new_v4().simple()));
        assert!(stage(&mk(&["../evil.txt", APP_EXE, HOST_EXE]), &dir).is_err());
        stage(&mk(&[APP_EXE, HOST_EXE, "data/unduhkilat.db", "extension/a.js"]), &dir).unwrap();
        assert!(dir.join("extension/a.js").is_file());
        assert!(!dir.join("data").exists(), "user data is never part of an update");
        assert!(stage(&mk(&[APP_EXE]), &dir).is_err(), "an archive without the host exe is incomplete");
        let _ = std::fs::remove_dir_all(dir);
    }
}
