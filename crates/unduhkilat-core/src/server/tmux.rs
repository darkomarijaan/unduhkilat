//! tmux di server tujuan: memeriksa sesi yang sedang aktif dan menyusun perintah untuk menyambung
//! ke sesi itu atau membuat yang baru. Semua nama lewat [`crate::server::exec::quote`], tidak pernah
//! ditempel mentah ke perintah.

use crate::server::exec;
use crate::server::ssh::SshSession;
use std::time::Duration;

/// Cara sebuah tab terminal SSH membuka shell-nya.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ShellLaunch {
    /// Shell login biasa; hilang kalau koneksi putus.
    #[default]
    Direct,
    /// Sambung ke sesi tmux yang sudah ada. `take_over` melepas klien lain dari sesi itu (`attach -d`).
    TmuxAttach { name: String, take_over: bool },
    /// Buat sesi tmux baru (atau sambung bila namanya sudah ada, supaya sambung ulang aman diulang).
    TmuxNew { name: String },
}

impl ShellLaunch {
    /// Perintah yang dijalankan di server, atau `None` untuk shell biasa.
    pub fn command(&self) -> Option<String> {
        let inner = match self {
            ShellLaunch::Direct => return None,
            ShellLaunch::TmuxAttach { name, take_over } => {
                format!("tmux attach-session{} -t {}", if *take_over { " -d" } else { "" }, exec::quote(&format!("={name}")))
            }
            ShellLaunch::TmuxNew { name } => format!("tmux new-session -A -s {}", exec::quote(name)),
        };
        Some(login_shell(&inner))
    }

    /// Nama sesi tmux, bila ada.
    pub fn session_name(&self) -> Option<&str> {
        match self {
            ShellLaunch::Direct => None,
            ShellLaunch::TmuxAttach { name, .. } | ShellLaunch::TmuxNew { name } => Some(name),
        }
    }

    /// Label singkat untuk judul tab: `tmux:api`, atau `None` untuk shell biasa.
    pub fn tab_label(&self) -> Option<String> {
        self.session_name().map(|n| format!("tmux:{n}"))
    }

    /// Cara membuka shell saat tab menyambung ulang: ke sesi yang sama dan tanpa melepas klien lain,
    /// jadi "ambil alih" hanya berlaku pada sambungan pertama. `TmuxNew` memakai `-A`, sehingga
    /// sesi yang sudah dibuat sambungan pertama dipasang kembali, bukan dibuat dua kali.
    pub fn for_reconnect(&self) -> ShellLaunch {
        match self {
            ShellLaunch::TmuxNew { name } => ShellLaunch::TmuxNew { name: name.clone() },
            ShellLaunch::TmuxAttach { name, .. } => ShellLaunch::TmuxAttach { name: name.clone(), take_over: false },
            ShellLaunch::Direct => ShellLaunch::Direct,
        }
    }
}

/// Kanal `exec` tidak memuat profil login, jadi `tmux` di `/usr/local/bin` atau `~/bin` bisa tidak
/// ditemukan. Jalankan lewat shell login supaya `PATH` sama dengan saat pengguna masuk sendiri.
fn login_shell(inner: &str) -> String {
    format!("sh -lc {}", exec::quote(inner))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxSession {
    pub name: String,
    pub windows: u32,
    /// Jumlah klien yang sedang tersambung ke sesi ini.
    pub attached: u32,
    /// Waktu aktivitas terakhir (detik sejak epoch).
    pub activity: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxProbe {
    /// `tmux` tidak ada di server.
    Missing,
    /// Daftar sesi (kosong bila belum ada server tmux yang berjalan).
    Sessions(Vec<TmuxSession>),
    /// Pengecekan gagal (waktu habis, perintah ditolak, dan sebagainya).
    Failed(String),
}

const NO_TMUX: &str = "@notmux";

/// Nama sesi yang boleh dibuat: tmux mengganti titik dan titik dua, dan spasi merusak perintah.
pub fn validate_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("Nama wajib diisi.");
    }
    if name.chars().count() > 64 {
        return Err("Nama terlalu panjang (maksimal 64 karakter).");
    }
    if name.chars().any(|c| c.is_whitespace() || c == '.' || c == ':' || c.is_control()) {
        return Err("Nama tidak boleh memuat spasi, titik, atau titik dua.");
    }
    Ok(())
}

/// Keluaran `tmux list-sessions -F '#{session_windows} #{session_attached} #{session_activity} #{session_name}'`.
/// Nama di bagian akhir supaya boleh memuat karakter apa pun. Baris lain (mis. teks dari profil login)
/// diabaikan.
pub fn parse_sessions(out: &str) -> Vec<TmuxSession> {
    let mut v: Vec<TmuxSession> = out
        .lines()
        .filter_map(|line| {
            let mut p = line.trim_end_matches('\r').splitn(4, ' ');
            let windows = p.next()?.parse().ok()?;
            let attached = p.next()?.parse().ok()?;
            let activity = p.next()?.parse().ok()?;
            let name = p.next()?.to_string();
            (!name.is_empty()).then_some(TmuxSession { name, windows, attached, activity })
        })
        .collect();
    v.sort_by(|a, b| b.activity.cmp(&a.activity).then_with(|| a.name.cmp(&b.name)));
    v
}

/// Tidak ada server tmux yang berjalan: itu bukan galat, hanya daftar kosong.
fn is_no_server(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("no server running") || (t.contains("error connecting to") && t.contains("no such file or directory"))
}

/// Periksa tmux di server tujuan lewat kanal `exec` (tanpa terminal).
pub async fn probe(session: &SshSession) -> TmuxProbe {
    let script = format!(
        "command -v tmux >/dev/null 2>&1 || {{ echo {NO_TMUX}; exit 0; }}; tmux list-sessions -F '#{{session_windows}} #{{session_attached}} #{{session_activity}} #{{session_name}}' 2>&1"
    );
    let out = match exec::run(session, &login_shell(&script), Duration::from_secs(10)).await {
        Ok(o) => o,
        Err(e) => return TmuxProbe::Failed(e.message),
    };
    classify(&String::from_utf8_lossy(&out.stdout), out.status, &exec::stderr_tail(&out.stderr))
}

fn classify(stdout: &str, status: Option<u32>, stderr_tail: &str) -> TmuxProbe {
    if stdout.lines().any(|l| l.trim() == NO_TMUX) {
        return TmuxProbe::Missing;
    }
    if status == Some(0) {
        return TmuxProbe::Sessions(parse_sessions(stdout));
    }
    if is_no_server(stdout) {
        return TmuxProbe::Sessions(Vec::new());
    }
    let detail = if stderr_tail.is_empty() { stdout.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string() } else { stderr_tail.to_string() };
    TmuxProbe::Failed(if detail.is_empty() { "Server menolak perintah tmux.".to_string() } else { detail })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sessions_newest_first_and_ignores_noise() {
        let out = "Selamat datang di server\n3 1 1700000100 toko\n1 0 1700000900 api\n2 0 1700000050 nama dengan spasi\n";
        let v = parse_sessions(out);
        assert_eq!(v.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["api", "toko", "nama dengan spasi"]);
        assert_eq!(v[1], TmuxSession { name: "toko".into(), windows: 3, attached: 1, activity: 1700000100 });
    }

    #[test]
    fn validates_names() {
        assert!(validate_name("api-2_x").is_ok());
        for bad in ["", "a b", "a.b", "a:b", "a\tb"] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
        assert!(validate_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn builds_quoted_commands() {
        let attach = ShellLaunch::TmuxAttach { name: "api".into(), take_over: false }.command().unwrap();
        assert_eq!(attach, r#"sh -lc 'tmux attach-session -t '\''=api'\'''"#);
        let take = ShellLaunch::TmuxAttach { name: "api".into(), take_over: true }.command().unwrap();
        assert!(take.contains("attach-session -d -t"));
        let new = ShellLaunch::TmuxNew { name: "it's".into() }.command().unwrap();
        assert!(new.contains("new-session -A -s"));
        assert!(!new.contains("it's'"), "kutip tunggal di nama harus lolos: {new}");
        assert_eq!(ShellLaunch::Direct.command(), None);
    }

    #[test]
    fn reconnect_never_steals_the_session() {
        let l = ShellLaunch::TmuxAttach { name: "api".into(), take_over: true };
        assert_eq!(l.for_reconnect(), ShellLaunch::TmuxAttach { name: "api".into(), take_over: false });
        assert_eq!(ShellLaunch::TmuxNew { name: "x".into() }.for_reconnect(), ShellLaunch::TmuxNew { name: "x".into() });
        assert_eq!(ShellLaunch::TmuxNew { name: "x".into() }.tab_label().as_deref(), Some("tmux:x"));
    }

    #[test]
    fn classifies_probe_output() {
        assert_eq!(classify("@notmux\n", Some(0), ""), TmuxProbe::Missing);
        assert!(matches!(classify("1 0 5 a\n", Some(0), ""), TmuxProbe::Sessions(v) if v.len() == 1));
        assert_eq!(classify("no server running on /tmp/tmux-1000/default\n", Some(1), ""), TmuxProbe::Sessions(vec![]));
        assert_eq!(classify("error connecting to /tmp/tmux-1000/default (No such file or directory)\n", Some(1), ""), TmuxProbe::Sessions(vec![]));
        assert!(matches!(classify("error connecting to /tmp/x (Permission denied)\n", Some(1), ""), TmuxProbe::Failed(m) if m.contains("Permission denied")));
    }
}
