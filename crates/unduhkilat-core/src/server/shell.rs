//! An interactive shell on a saved server (SSH session channel with a PTY) as a terminal byte source.

use crate::error::{AppError, ErrorCode, Result};
use crate::server::ssh::SshSession;
use crate::term::source::{SourceControl, SourceEvent, SourceIo};
use russh::ChannelMsg;
use std::sync::Arc;
use std::time::Duration;
use crate::server::stats::StatsState;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

enum Ctl {
    Write(Vec<u8>),
    Resize(u16, u16),
    Close,
}

struct ShellControl {
    tx: UnboundedSender<Ctl>,
}

fn gone() -> AppError {
    AppError::new(ErrorCode::ErrConnectionDropped, "Sesi terminal sudah tertutup.")
}

impl SourceControl for ShellControl {
    fn write(&self, bytes: &[u8]) -> Result<()> {
        self.tx.send(Ctl::Write(bytes.to_vec())).map_err(|_| gone())
    }

    fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.tx.send(Ctl::Resize(cols.max(1), rows.max(1))).map_err(|_| gone())
    }

    fn close(&self) {
        let _ = self.tx.send(Ctl::Close);
    }
}

impl Drop for ShellControl {
    fn drop(&mut self) {
        self.close();
    }
}

async fn expect_reply(ch: &mut russh::Channel<russh::client::Msg>, what: &str) -> Result<()> {
    let reply = async {
        loop {
            match ch.wait().await {
                Some(ChannelMsg::Success) => return Ok(()),
                Some(ChannelMsg::Failure) => {
                    return Err(AppError::new(ErrorCode::ErrPermissionDenied, format!("Server menolak membuka shell ({what} ditolak).")));
                }
                Some(_) => continue,
                None => return Err(gone()),
            }
        }
    };
    match tokio::time::timeout(Duration::from_secs(10), reply).await {
        Ok(r) => r,
        Err(_) => Err(AppError::new(ErrorCode::ErrConnectionDropped, format!("Server tidak menjawab permintaan {what} untuk shell."))),
    }
}

/// Open a shell with a `xterm-256color` PTY of the given size. Takes the session: it stays open as long
/// as the shell does and is closed with it.
pub async fn open_shell(session: SshSession, cols: u16, rows: u16) -> Result<SourceIo> {
    Ok(open_shell_with_stats(session, cols, rows, false).await?.0)
}

/// Like [`open_shell`]; with `stats`, also reads the server's system info over the same connection
/// (see [`crate::server::stats`]) until the shell ends.
pub async fn open_shell_with_stats(session: SshSession, cols: u16, rows: u16, stats: bool) -> Result<(SourceIo, Option<watch::Receiver<StatsState>>)> {
    let (cols, rows) = (cols.max(1), rows.max(1));
    let mut ch = session
        .handle()
        .channel_open_session()
        .await
        .map_err(|_| AppError::new(ErrorCode::ErrConnectionDropped, "Server menolak membuka kanal untuk shell."))?;
    ch.request_pty(true, "xterm-256color", cols as u32, rows as u32, 0, 0, &[]).await.map_err(|_| gone())?;
    expect_reply(&mut ch, "permintaan PTY").await?;
    ch.request_shell(true).await.map_err(|_| gone())?;
    expect_reply(&mut ch, "permintaan shell").await?;

    let session = Arc::new(session);
    let stop = CancellationToken::new();
    let (stats_rx, monitor) = if stats {
        let (rx, task) = crate::server::stats::spawn_monitor(session.clone(), stop.clone());
        (Some(rx), Some(task))
    } else {
        (None, None)
    };
    let (tx, rx) = unbounded_channel();
    let (ctl_tx, mut ctl_rx) = unbounded_channel::<Ctl>();
    tokio::spawn(async move {
        let mut exit: Option<i32> = None;
        loop {
            tokio::select! {
                msg = ch.wait() => match msg {
                    Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                        if tx.send(SourceEvent::Data(data.to_vec())).is_err() {
                            break;
                        }
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => exit = Some(exit_status as i32),
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                    _ => {}
                },
                ctl = ctl_rx.recv() => match ctl {
                    Some(Ctl::Write(bytes)) => {
                        if ch.data_bytes(bytes).await.is_err() {
                            break;
                        }
                    }
                    Some(Ctl::Resize(c, r)) => {
                        let _ = ch.window_change(c as u32, r as u32, 0, 0).await;
                    }
                    Some(Ctl::Close) | None => {
                        let _ = ch.close().await;
                        break;
                    }
                },
            }
        }
        // Refuse further input before announcing the end: a write after `Closed` must fail, not queue.
        ctl_rx.close();
        drop(ctl_rx);
        let _ = tx.send(SourceEvent::Closed(exit));
        stop.cancel();
        if let Some(t) = monitor {
            let _ = t.await;
        }
        if let Ok(session) = Arc::try_unwrap(session) {
            session.close().await;
        }
    });
    Ok((SourceIo { rx, control: Arc::new(ShellControl { tx: ctl_tx }) }, stats_rx))
}
