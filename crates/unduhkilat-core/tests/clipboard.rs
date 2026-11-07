//! FR-040 against the real Windows clipboard. Saves and restores the user's text.
#![cfg(windows)]

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::time::Duration;

fn ps(script: &str) -> String {
    let o = Command::new("powershell").args(["-NoProfile", "-STA", "-Command", script]).output().unwrap();
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn set_clip(text: &str) {
    ps(&format!("Set-Clipboard -Value '{}'", text.replace('\'', "''")));
}

#[test]
fn listener_proposes_only_lone_urls_and_honours_the_toggle() {
    let saved = ps("[Console]::OutputEncoding=[Text.Encoding]::UTF8; Get-Clipboard -Raw");
    let (tx, rx) = channel();
    let enabled = Arc::new(AtomicBool::new(true));
    let woke = Arc::new(AtomicBool::new(false));
    let w2 = woke.clone();
    unduhkilat_core::clipboard::start(tx, enabled.clone(), Arc::new(move || w2.store(true, Ordering::SeqCst)));
    std::thread::sleep(Duration::from_millis(500)); // let the window attach

    set_clip("some prose, https://example.com/not-alone.zip inside");
    assert!(rx.recv_timeout(Duration::from_millis(1500)).is_err(), "prose must be ignored");

    set_clip("https://example.com/files/a.zip");
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "https://example.com/files/a.zip");
    assert!(woke.load(Ordering::SeqCst), "the window must be woken for the prompt");

    set_clip("https://example.com/files/a.zip");
    assert!(rx.recv_timeout(Duration::from_millis(1500)).is_err(), "same link twice in a row prompts once");

    enabled.store(false, Ordering::Relaxed);
    set_clip("https://example.com/files/b.zip");
    assert!(rx.recv_timeout(Duration::from_millis(1500)).is_err(), "disabled means silent");

    enabled.store(true, Ordering::Relaxed);
    set_clip("ftp://ftp.example.com/pub/c.iso");
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "ftp://ftp.example.com/pub/c.iso");

    set_clip(saved.trim_end_matches(['\r', '\n']));
}
