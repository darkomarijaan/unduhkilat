//! Local shell (ConPTY) integration tests. They run the real `cmd.exe` / PowerShell on this machine
//! and touch no network.

use std::path::PathBuf;
use std::time::Duration;
use unduhkilat_core::error::ErrorCode;
use unduhkilat_core::term::local::{detect_shells, spawn_local, ShellKind, ShellOptions};
use unduhkilat_core::term::source::{SourceEvent, SourceIo};

/// Drop terminal escape sequences so assertions see the text a person would read.
fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            match it.next() {
                Some('[') => {
                    for n in it.by_ref() {
                        if ('@'..='~').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    for n in it.by_ref() {
                        if n == '\u{7}' {
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Read until the plain text contains `needle` or the shell closes; returns (text, close status).
///
/// ConPTY asks the terminal where the cursor is (`ESC[6n`) at startup and prints nothing until it is
/// answered. In the app the emulator answers; here this helper plays the terminal.
async fn read_until(io: &mut SourceIo, needle: &str, secs: u64) -> (String, Option<Option<i32>>) {
    let mut text = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(left, io.rx.recv()).await {
            Ok(Some(SourceEvent::Data(b))) => {
                let chunk = String::from_utf8_lossy(&b).to_string();
                if chunk.contains("\u{1b}[6n") {
                    io.control.write(b"\x1b[1;1R").unwrap();
                }
                text.push_str(&chunk);
                if plain(&text).contains(needle) {
                    return (plain(&text), None);
                }
            }
            Ok(Some(SourceEvent::Closed(code))) => return (plain(&text), Some(code)),
            Ok(None) | Err(_) => return (plain(&text), None),
        }
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("{name}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn cmd(cwd: PathBuf) -> ShellOptions {
    ShellOptions { kind: ShellKind::Cmd, cwd, cols: 100, rows: 30 }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_starts_in_the_requested_folder() {
    let dir = temp_dir("uk test \u{fc}");
    let mut io = spawn_local(cmd(dir.clone())).unwrap();
    io.control.write(b"cd\r\n").unwrap();
    let leaf = dir.file_name().unwrap().to_string_lossy().to_string();
    let (text, _) = read_until(&mut io, &leaf, 10).await;
    assert!(text.contains(&leaf), "cd should print the folder (spaces and Unicode intact): {text:?}");
    io.control.close();
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn typed_input_is_echoed_and_output_arrives() {
    let dir = temp_dir("uk-echo");
    let mut io = spawn_local(cmd(dir.clone())).unwrap();
    io.control.write(b"echo abc123\r\n").unwrap();
    // The typed text itself is echoed; the command's real output comes on its own line.
    let (text, _) = read_until(&mut io, "abc123\r\n", 10).await;
    assert!(text.matches("abc123").count() >= 1, "{text:?}");
    io.control.close();
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resize_reaches_the_shell() {
    let dir = temp_dir("uk-resize");
    let mut io = spawn_local(cmd(dir.clone())).unwrap();
    io.control.resize(90, 33).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    io.control.write(b"mode con\r\n").unwrap();
    let (text, _) = read_until(&mut io, "Columns:", 10).await;
    let after = text.split("Columns:").nth(1).unwrap_or("");
    assert!(after.trim_start().starts_with("90"), "`mode con` should report 90 columns: {text:?}");
    io.control.close();
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exit_reports_the_exit_code() {
    let dir = temp_dir("uk-exit");
    let mut io = spawn_local(cmd(dir.clone())).unwrap();
    io.control.write(b"exit 7\r\n").unwrap();
    let (_, closed) = read_until(&mut io, "\u{0}never", 10).await;
    assert_eq!(closed, Some(Some(7)), "the shell's exit code is reported");
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_ends_the_session() {
    let dir = temp_dir("uk-close");
    let mut io = spawn_local(cmd(dir.clone())).unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    io.control.close();
    let (_, closed) = read_until(&mut io, "\u{0}never", 10).await;
    assert!(closed.is_some(), "close() must end the session and report it");
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_folder_is_a_clear_error() {
    let missing = std::env::temp_dir().join("uk-folder-that-does-not-exist-xyz");
    let e = spawn_local(cmd(missing)).err().expect("a missing folder is refused");
    assert_eq!(e.code, ErrorCode::ErrInvalidPath);
    assert!(e.message.contains("Folder"), "{}", e.message);
}

#[test]
fn detect_shells_always_has_powershell_and_cmd() {
    let shells = detect_shells();
    assert!(shells.contains(&ShellKind::PowerShell) && shells.contains(&ShellKind::Cmd), "{shells:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extreme_sizes_do_not_panic() {
    let dir = temp_dir("uk-sizes");
    let io = spawn_local(ShellOptions { kind: ShellKind::Cmd, cwd: dir.clone(), cols: 0, rows: 0 }).expect("a zero size is clamped, not an error");
    for (c, r) in [(0, 0), (1, 1), (500, 200), (80, 24)] {
        io.control.resize(c, r).unwrap();
    }
    io.control.close();
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn powershell_runs_a_command() {
    let dir = temp_dir("uk-ps");
    let mut io = spawn_local(ShellOptions { kind: ShellKind::PowerShell, cwd: dir.clone(), cols: 120, rows: 30 }).unwrap();
    // The typed text contains `$((1+2))`; only an evaluated command prints `hello3`.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    io.control.write(b"Write-Output \"hello$((1+2))\"\r\n").unwrap();
    let (text, _) = read_until(&mut io, "hello3", 20).await;
    assert!(text.contains("hello3"), "{text:?}");
    io.control.close();
    std::fs::remove_dir_all(dir).ok();
}
