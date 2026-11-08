//! Running a command on a saved SSH server without a terminal (an `exec` channel), used by the
//! fast folder copy. Commands are built from fixed templates; every path is passed through
//! [`quote`], never pasted in raw.

use crate::error::{AppError, ErrorCode, Result};
use crate::server::ssh::SshSession;
use russh::client::Msg;
use russh::{Channel, ChannelMsg};
use std::time::Duration;

/// POSIX shell single-quoting: `it's` becomes `'it'\''s'`. Safe for any byte string without NUL.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Start `cmd` on the server. The caller reads and writes the returned channel.
pub async fn open(session: &SshSession, cmd: &str) -> Result<Channel<Msg>> {
    let ch = session.handle().channel_open_session().await.map_err(|_| AppError::new(ErrorCode::ErrConnectionDropped, "Server menolak membuka kanal perintah."))?;
    ch.exec(true, cmd.as_bytes()).await.map_err(|_| AppError::new(ErrorCode::ErrRemoteFs, "Server menolak menjalankan perintah."))?;
    Ok(ch)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// `None` when the server closed the channel without reporting a status (e.g. exec refused).
    pub status: Option<u32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Run `cmd` to completion and collect its output (capped, for short commands only).
pub async fn run(session: &SshSession, cmd: &str, timeout: Duration) -> Result<Output> {
    const CAP: usize = 64 * 1024;
    let mut ch = open(session, cmd).await?;
    let mut out = Output { status: None, stdout: Vec::new(), stderr: Vec::new() };
    let work = async {
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { data } if out.stdout.len() < CAP => out.stdout.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, .. } if out.stderr.len() < CAP => out.stderr.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => out.status = Some(exit_status),
                ChannelMsg::Failure => break,
                ChannelMsg::Close => break,
                _ => {}
            }
        }
    };
    tokio::time::timeout(timeout, work).await.map_err(|_| AppError::new(ErrorCode::ErrConnectionDropped, "Server tidak menyelesaikan perintah (kehabisan waktu)."))?;
    let _ = ch.close().await;
    Ok(out)
}

/// The last few lines of a command's error output, for a message the user can act on.
pub fn stderr_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let tail = &lines[lines.len().saturating_sub(3)..];
    let s = tail.join(" | ");
    if s.chars().count() > 300 {
        s.chars().take(300).collect::<String>() + "..."
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_survives_quotes_spaces_and_shell_characters() {
        assert_eq!(quote("/home/eko/Foto 2024"), "'/home/eko/Foto 2024'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote("$(rm -rf /); `x` *"), "'$(rm -rf /); `x` *'");
    }

    #[test]
    fn stderr_tail_keeps_the_last_lines() {
        assert_eq!(stderr_tail(b"a\n\nb\nc\nd\n"), "b | c | d");
        assert_eq!(stderr_tail(b""), "");
    }
}
