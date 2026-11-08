//! Remote file systems behind one interface: SFTP, FTP and FTPS (design: docs/superpowers/specs/2026-11-07-server-client-design.md).

use crate::error::Result;
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

/// One item of a remote directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    /// Seconds since the Unix epoch, when the server reports it.
    pub modified: Option<i64>,
    pub is_link: bool,
    /// Neither a regular file, a folder nor a link: a socket, FIFO or device. It has no content to
    /// copy (opening a FIFO can even block), so folder transfers skip it.
    pub is_special: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Modified,
    /// By file extension (the "Jenis" column).
    Type,
}

pub type RemoteReader = Box<dyn AsyncRead + Send + Unpin>;
pub type RemoteWriter = Box<dyn AsyncWrite + Send + Unpin>;

#[async_trait]
pub trait RemoteFs: Send {
    async fn list(&mut self, dir: &str) -> Result<Vec<Entry>>;
    async fn stat(&mut self, path: &str) -> Result<Entry>;
    async fn mkdir(&mut self, path: &str) -> Result<()>;
    async fn rename(&mut self, from: &str, to: &str) -> Result<()>;
    async fn remove(&mut self, path: &str, is_dir: bool) -> Result<()>;
    /// Up to `len` bytes starting at `offset` (fewer at the end of the file).
    async fn read_at(&mut self, path: &str, offset: u64, len: usize) -> Result<Vec<u8>>;
    /// Stream a file from `offset`.
    async fn open_read(&mut self, path: &str, offset: u64) -> Result<RemoteReader>;
    /// Write a file. `offset == 0 && !append` truncates; `append` continues at the end (resume).
    async fn open_write(&mut self, path: &str, offset: u64, append: bool) -> Result<RemoteWriter>;
    /// Home or starting directory of the login.
    async fn home(&mut self) -> Result<String>;
    /// False when the connection is known to be gone (checked without contacting the server).
    fn is_alive(&self) -> bool {
        true
    }
    /// The SSH connection underneath, when there is one (SFTP), for running commands.
    fn ssh(&self) -> Option<&crate::server::ssh::SshSession> {
        None
    }
    async fn close(self: Box<Self>);
}

fn is_reserved_device(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit() && stem.as_bytes()[3] != b'0')
}

/// A file name from a server made safe to create inside a local folder: no separators, no drive
/// letters, no device names, and short. `None` when nothing safe can be made of it.
pub fn safe_local_name(remote_name: &str) -> Option<String> {
    // `\u{5c}` is the backslash; control characters (including NUL) are refused too.
    if remote_name.chars().any(|c| matches!(c, '/' | '\u{5c}' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()) {
        return None;
    }
    let trimmed = remote_name.trim_end_matches(['.', ' ']);
    if trimmed.trim().is_empty() || trimmed == "." || trimmed == ".." {
        return None;
    }
    let mut name: String = trimmed.chars().take(200).collect();
    if is_reserved_device(&name) {
        name.insert(0, '_');
    }
    Some(name)
}

/// Folders first, then by `key`; ties by name (case-insensitive).
/// The lower-case extension of a file name; none for `README` and for dot files like `.bashrc`.
pub fn extension_of(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_lowercase(),
        _ => String::new(),
    }
}

pub fn sort_entries(v: &mut [Entry], key: SortKey, ascending: bool) {
    v.sort_by(|a, b| {
        let primary = match key {
            SortKey::Name => std::cmp::Ordering::Equal,
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Modified => a.modified.cmp(&b.modified),
            SortKey::Type => extension_of(&a.name).cmp(&extension_of(&b.name)),
        };
        let primary = if ascending { primary } else { primary.reverse() };
        let by_name = a.name.to_lowercase().cmp(&b.name.to_lowercase());
        // Name sorting honours the direction itself; other keys fall back to ascending names.
        let by_name = if key == SortKey::Name && !ascending { by_name.reverse() } else { by_name };
        b.is_dir.cmp(&a.is_dir).then(primary).then(by_name)
    });
}

pub fn join_remote(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

pub fn parent_remote(path: &str) -> String {
    let t = path.trim_end_matches('/');
    match t.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(i) => t[..i].to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, dir: bool, size: u64) -> Entry {
        Entry { name: name.into(), path: format!("/x/{name}"), is_dir: dir, size, modified: None, is_link: false, is_special: false }
    }

    #[test]
    fn safe_local_name_rejects_traversal() {
        for bad in ["", ".", "..", "a/b", "a\\b", "a\0b", "C:evil", "  "] {
            assert!(safe_local_name(bad).is_none(), "{bad:?} must be refused");
        }
        assert_eq!(safe_local_name("laporan 2026.pdf").as_deref(), Some("laporan 2026.pdf"));
        assert_eq!(safe_local_name("\u{4F60}\u{597D}.txt").as_deref(), Some("\u{4F60}\u{597D}.txt"));
        let long = "\u{4F60}".repeat(300);
        let cut = safe_local_name(&long).unwrap();
        assert!(cut.chars().count() <= 200 && cut.chars().all(|c| c == '\u{4F60}'), "cut on a character boundary");
        assert_eq!(safe_local_name("con.txt").as_deref(), Some("_con.txt"), "reserved Windows device names are prefixed");
        assert_eq!(safe_local_name("a.txt. ").as_deref(), Some("a.txt"), "trailing dots and spaces are dropped on Windows");
    }

    #[test]
    fn sort_puts_folders_first_and_is_case_insensitive() {
        let mut v = vec![e("b.txt", false, 5), e("Zeta", true, 0), e("a.txt", false, 9), e("alpha", true, 0)];
        sort_entries(&mut v, SortKey::Name, true);
        assert_eq!(v.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["alpha", "Zeta", "a.txt", "b.txt"]);
        sort_entries(&mut v, SortKey::Size, false);
        assert_eq!(v.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["alpha", "Zeta", "a.txt", "b.txt"].map(|s| s), "folders stay first; larger files first");
    }

    #[test]
    fn sort_by_type_groups_extensions_and_keeps_folders_first() {
        let mut v = vec![e("b.zip", false, 1), e("a.PDF", false, 1), e("Dir", true, 0), e("c.pdf", false, 1), e("README", false, 1)];
        sort_entries(&mut v, SortKey::Type, true);
        assert_eq!(v.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["Dir", "README", "a.PDF", "c.pdf", "b.zip"], "no extension first, then by extension, then by name");
        sort_entries(&mut v, SortKey::Type, false);
        assert_eq!(v.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["Dir", "b.zip", "a.PDF", "c.pdf", "README"]);
    }

    #[test]
    fn join_remote_handles_root_and_trailing_slash() {
        assert_eq!(join_remote("/", "a"), "/a");
        assert_eq!(join_remote("/home/x/", "a b"), "/home/x/a b");
        assert_eq!(join_remote("", "a"), "a");
        assert_eq!(parent_remote("/home/x/a"), "/home/x");
        assert_eq!(parent_remote("/a"), "/");
        assert_eq!(parent_remote("/"), "/");
    }
}

pub mod sftp;
pub mod ftp;
pub mod walk;

// ---- pasting a picture into a remote terminal (Alt+V) -------------------------------------------

/// Hidden folder inside the login's own home directory that receives pasted pictures (a shared
/// /tmp folder could be read or planted by other users).
pub const PASTE_SUBDIR: &str = ".unduhkilat-paste";

/// A unique, shell-safe file name for a pasted picture (`img-<milliseconds>-<nonce hex>.png`).
pub fn temp_image_name(millis: u128, nonce: u64) -> String {
    format!("img-{millis}-{nonce:016x}.png")
}

/// Encode RGBA pixels (as the clipboard gives them) as a PNG.
pub fn encode_png_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    use crate::error::{AppError, ErrorCode};
    let img = image::RgbaImage::from_raw(width, height, rgba.to_vec()).ok_or_else(|| AppError::new(ErrorCode::ErrInvalidArgument, "Data gambar di papan klip tidak lengkap."))?;
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).map_err(|e| AppError::internal(format!("gagal mengodekan PNG: {e}")))?;
    Ok(out)
}

#[cfg(test)]
mod paste_tests {
    use super::*;

    #[test]
    fn temp_image_name_is_unique_and_safe() {
        let a = temp_image_name(1_700_000_000_123, 0xdead_beef);
        assert_eq!(a, "img-1700000000123-00000000deadbeef.png");
        assert_ne!(a, temp_image_name(1_700_000_000_123, 1), "the nonce separates two pastes in the same millisecond");
        assert!(a.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.'), "nothing a shell would treat specially");
        assert_eq!(join_remote(&join_remote("/home/eko", PASTE_SUBDIR), &a), format!("/home/eko/.unduhkilat-paste/{a}"));
    }

    #[test]
    fn rgba_roundtrips_through_png() {
        let rgba: Vec<u8> = (0..4 * 3 * 2).map(|i| (i * 7) as u8).collect();
        let png = encode_png_rgba(3, 2, &rgba).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let back = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!((back.width(), back.height()), (3, 2));
        assert_eq!(back.into_raw(), rgba);
        assert!(encode_png_rgba(3, 2, &rgba[..10]).is_err(), "a short buffer is an error, not a panic");
    }
}
