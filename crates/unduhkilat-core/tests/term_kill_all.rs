//! `kill_all_local` stops every shell in the process, so this test lives in its own file (its own
//! process) and cannot interfere with the other shell tests.

use std::time::Duration;
use unduhkilat_core::term::local::{kill_all_local, spawn_local, ShellKind, ShellOptions};
use unduhkilat_core::term::source::{SourceEvent, SourceIo};

async fn wait_closed(io: &mut SourceIo) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(left, io.rx.recv()).await {
            Ok(Some(SourceEvent::Closed(_))) => return true,
            Ok(Some(SourceEvent::Data(b))) => {
                // ConPTY waits for the terminal to answer its cursor-position query.
                if String::from_utf8_lossy(&b).contains("\u{1b}[6n") {
                    let _ = io.control.write(b"\x1b[1;1R");
                }
            }
            _ => return false,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_all_stops_every_local_shell() {
    let dir = std::env::temp_dir();
    let opts = ShellOptions { kind: ShellKind::Cmd, cwd: dir, cols: 100, rows: 30 };
    let mut a = spawn_local(opts.clone()).unwrap();
    let mut b = spawn_local(opts).unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    kill_all_local();
    assert!(wait_closed(&mut a).await && wait_closed(&mut b).await, "both shells end when the app quits");
}
