//! SFTP client over an authenticated SSH connection (the `sftp` subsystem of a session channel).

use super::{join_remote, Entry, RemoteFs, RemoteReader, RemoteWriter};
use crate::error::{AppError, ErrorCode, Result};
use crate::server::ssh::SshSession;
use async_trait::async_trait;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use std::io::SeekFrom;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// Give up on a server that stops answering instead of hanging the UI.
const REPLY_TIMEOUT_SECS: u64 = 30;

pub struct SftpFs {
    sftp: SftpSession,
    /// Keeps the SSH connection (and every jump hop) alive for as long as this file system lives.
    session: SshSession,
}

fn map_err(e: SftpError) -> AppError {
    match &e {
        SftpError::Status(s) if s.status_code == StatusCode::NoSuchFile => AppError::new(ErrorCode::ErrNotFound, "Berkas atau folder tidak ditemukan di server."),
        SftpError::Status(s) if s.status_code == StatusCode::PermissionDenied => AppError::new(ErrorCode::ErrRemoteFs, "Akses ditolak oleh server."),
        SftpError::Status(s) => AppError::new(ErrorCode::ErrRemoteFs, format!("Server menolak operasi: {}", s.error_message)),
        SftpError::Timeout => AppError::new(ErrorCode::ErrConnectionDropped, "Server tidak menjawab. Sambungan mungkin terputus."),
        other => AppError::new(ErrorCode::ErrConnectionDropped, format!("Sambungan ke server bermasalah: {other}")),
    }
}

fn to_entry(dir: &str, name: String, a: &FileAttributes, is_link: bool) -> Entry {
    Entry { path: join_remote(dir, &name), name, is_dir: a.is_dir(), size: a.len(), modified: a.mtime.map(|t| t as i64), is_link }
}

/// Start the SFTP subsystem on `session`. The returned file system owns the connection.
pub async fn open_sftp(session: SshSession) -> Result<Box<dyn RemoteFs>> {
    let channel = session.handle().channel_open_session().await.map_err(|_| AppError::new(ErrorCode::ErrConnectionDropped, "Server menolak membuka kanal SFTP."))?;
    channel.request_subsystem(true, "sftp").await.map_err(|_| AppError::new(ErrorCode::ErrRemoteFs, "Server ini tidak menyediakan SFTP."))?;
    let sftp = SftpSession::new(channel.into_stream()).await.map_err(map_err)?;
    sftp.set_timeout(REPLY_TIMEOUT_SECS);
    Ok(Box::new(SftpFs { sftp, session }))
}

#[async_trait]
impl RemoteFs for SftpFs {
    async fn list(&mut self, dir: &str) -> Result<Vec<Entry>> {
        let mut out = Vec::new();
        for e in self.sftp.read_dir(dir).await.map_err(map_err)? {
            let name = e.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let attrs = e.metadata();
            let is_link = attrs.is_symlink();
            let mut entry = to_entry(dir, name, &attrs, is_link);
            if is_link {
                // Follow the link so a link to a folder opens like a folder.
                if let Ok(target) = self.sftp.metadata(entry.path.clone()).await {
                    entry.is_dir = target.is_dir();
                    entry.size = target.len();
                }
            }
            out.push(entry);
        }
        Ok(out)
    }

    async fn stat(&mut self, path: &str) -> Result<Entry> {
        let a = self.sftp.metadata(path).await.map_err(map_err)?;
        let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or(path).to_string();
        Ok(Entry { name, path: path.to_string(), is_dir: a.is_dir(), size: a.len(), modified: a.mtime.map(|t| t as i64), is_link: false })
    }

    async fn mkdir(&mut self, path: &str) -> Result<()> {
        self.sftp.create_dir(path).await.map_err(map_err)
    }

    async fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        self.sftp.rename(from, to).await.map_err(map_err)
    }

    async fn remove(&mut self, path: &str, is_dir: bool) -> Result<()> {
        if is_dir {
            self.sftp.remove_dir(path).await.map_err(map_err)
        } else {
            self.sftp.remove_file(path).await.map_err(map_err)
        }
    }

    async fn read_at(&mut self, path: &str, offset: u64, len: usize) -> Result<Vec<u8>> {
        let mut f = self.sftp.open(path).await.map_err(map_err)?;
        f.seek(SeekFrom::Start(offset)).await.map_err(|e| AppError::new(ErrorCode::ErrRemoteFs, format!("Gagal membaca dari server: {e}")))?;
        let mut buf = vec![0u8; len];
        let mut got = 0;
        while got < len {
            let n = tokio::time::timeout(Duration::from_secs(REPLY_TIMEOUT_SECS), f.read(&mut buf[got..]))
                .await
                .map_err(|_| AppError::new(ErrorCode::ErrConnectionDropped, "Server tidak menjawab."))?
                .map_err(|e| AppError::new(ErrorCode::ErrRemoteFs, format!("Gagal membaca dari server: {e}")))?;
            if n == 0 {
                break;
            }
            got += n;
        }
        buf.truncate(got);
        Ok(buf)
    }

    async fn open_read(&mut self, path: &str, offset: u64) -> Result<RemoteReader> {
        let mut f = self.sftp.open(path).await.map_err(map_err)?;
        if offset > 0 {
            f.seek(SeekFrom::Start(offset)).await.map_err(|e| AppError::new(ErrorCode::ErrRemoteFs, format!("Gagal membaca dari server: {e}")))?;
        }
        Ok(Box::new(f))
    }

    async fn open_write(&mut self, path: &str, offset: u64, append: bool) -> Result<RemoteWriter> {
        let mut flags = OpenFlags::WRITE | OpenFlags::CREATE;
        if offset == 0 && !append {
            flags |= OpenFlags::TRUNCATE;
        }
        let mut f = self.sftp.open_with_flags(path, flags).await.map_err(map_err)?;
        if offset > 0 {
            f.seek(SeekFrom::Start(offset)).await.map_err(|e| AppError::new(ErrorCode::ErrRemoteFs, format!("Gagal menulis ke server: {e}")))?;
        } else if append {
            let end = f.metadata().await.map_err(map_err)?.len();
            f.seek(SeekFrom::Start(end)).await.map_err(|e| AppError::new(ErrorCode::ErrRemoteFs, format!("Gagal menulis ke server: {e}")))?;
        }
        Ok(Box::new(f))
    }

    async fn home(&mut self) -> Result<String> {
        self.sftp.canonicalize(".").await.map_err(map_err)
    }

    async fn close(self: Box<Self>) {
        let _ = self.sftp.close().await;
        self.session.close().await;
    }
}
