//! Info sistem server untuk bilah di bawah terminal SSH (seperti bilah MobaXterm): CPU, memori,
//! koneksi TCP per port layanan, proses, uptime, pengguna yang masuk, dan isi disk `/`.
//!
//! Dibaca lewat kanal `exec` pada sambungan SSH yang sudah dibuka pengguna untuk terminal (tidak ada
//! sambungan baru). Perintahnya satu skrip tetap tanpa masukan pengguna. Server yang bukan Linux (tanpa
//! `/proc`) atau yang menolak `exec` membuat pemantauan berhenti diam-diam: bilahnya tidak tampil.

use crate::server::exec;
use crate::server::ssh::SshSession;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Skrip yang dijalankan di server. Setiap bagian diawali penanda `@nama`; bagian yang gagal kosong.
pub const SCRIPT: &str = "export LC_ALL=C; \
echo @stat; head -n1 /proc/stat; \
echo @mem; grep -E '^(MemTotal|MemAvailable):' /proc/meminfo; \
echo @load; cat /proc/loadavg; \
echo @up; cat /proc/uptime; \
echo @user; id -un; \
echo @who; who 2>/dev/null | awk '{print $1}'; \
echo @df; df -P / 2>/dev/null | awk 'NR==2{print $5}'; \
echo @tcp; cat /proc/net/tcp /proc/net/tcp6 2>/dev/null | awk '$4==\"01\"||$4==\"0A\"{print $2, $4}'";

/// Selang antar-pembacaan.
pub const INTERVAL: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerStats {
    /// Persen CPU terpakai sejak pembacaan sebelumnya; `None` pada pembacaan pertama.
    pub cpu_pct: Option<f32>,
    pub mem_used_kb: u64,
    pub mem_total_kb: u64,
    /// Koneksi TCP tersambung per port yang didengarkan server, terbanyak dulu: `(port, jumlah)`.
    pub conns: Vec<(u16, u32)>,
    pub procs_running: u32,
    pub procs_total: u32,
    pub uptime_secs: u64,
    pub user: String,
    /// Berapa sesi login milik `user` (menurut `who`).
    pub user_sessions: u32,
    /// Persen terisi dari sistem berkas `/`.
    pub disk_root_pct: Option<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StatsState {
    /// Belum ada pembacaan.
    Waiting,
    Ready(ServerStats),
    /// Server tidak mendukung (bukan Linux, atau `exec` ditolak). Bilah tidak ditampilkan.
    Unsupported,
}

/// Penghitung CPU dari baris `cpu` di `/proc/stat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuSample {
    pub busy: u64,
    pub total: u64,
}

fn sections(out: &str) -> BTreeMap<&str, Vec<&str>> {
    let mut map: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut cur: Option<&str> = None;
    for line in out.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(name) = line.strip_prefix('@') {
            cur = Some(name.trim());
            map.entry(name.trim()).or_default();
        } else if let Some(c) = cur {
            if !line.trim().is_empty() {
                map.entry(c).or_default().push(line);
            }
        }
    }
    map
}

fn cpu_sample(line: &str) -> Option<CpuSample> {
    let mut it = line.split_whitespace();
    if it.next()? != "cpu" {
        return None;
    }
    // user nice system idle iowait irq softirq steal (guest sudah termasuk dalam user).
    let v: Vec<u64> = it.take(8).filter_map(|x| x.parse().ok()).collect();
    if v.len() < 4 {
        return None;
    }
    let total: u64 = v.iter().sum();
    let idle = v[3] + v.get(4).copied().unwrap_or(0);
    Some(CpuSample { busy: total.saturating_sub(idle), total })
}

/// Persen CPU antara dua sampel, atau `None` bila penghitung tidak maju (atau mundur setelah reboot).
pub fn cpu_pct(prev: CpuSample, cur: CpuSample) -> Option<f32> {
    let dt = cur.total.checked_sub(prev.total)?;
    if dt == 0 {
        return None;
    }
    let db = cur.busy.saturating_sub(prev.busy).min(dt);
    Some(db as f32 * 100.0 / dt as f32)
}

/// Port dari alamat `/proc/net/tcp` seperti `0100007F:0CEA` atau `...:01BB`.
fn hex_port(addr: &str) -> Option<u16> {
    u16::from_str_radix(addr.rsplit(':').next()?, 16).ok()
}

/// Koneksi tersambung (`01`) per port yang didengarkan (`0A`), terbanyak dulu, lalu port terkecil.
fn connections(lines: &[&str]) -> Vec<(u16, u32)> {
    let mut listening = BTreeSet::new();
    let mut established: BTreeMap<u16, u32> = BTreeMap::new();
    for l in lines {
        // `alamat_lokal status` (keluaran awk); kolom di antaranya, bila ada, diabaikan.
        let mut it = l.split_whitespace();
        let (Some(addr), Some(st)) = (it.next(), it.next_back()) else { continue };
        let Some(port) = hex_port(addr) else { continue };
        match st {
            "0A" => {
                listening.insert(port);
            }
            "01" => *established.entry(port).or_default() += 1,
            _ => {}
        }
    }
    let mut v: Vec<(u16, u32)> = established.into_iter().filter(|(p, _)| listening.contains(p)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

fn meminfo_kb(lines: &[&str], key: &str) -> Option<u64> {
    lines.iter().find_map(|l| l.strip_prefix(key)?.trim_start_matches(':').split_whitespace().next()?.parse().ok())
}

/// Hasil skrip menjadi angka. `None` bila keluarannya bukan dari Linux (tidak ada baris `cpu`
/// atau `MemTotal`), artinya server tidak didukung.
pub fn parse(out: &str) -> Option<(ServerStats, CpuSample)> {
    let s = sections(out);
    let get = |k: &str| s.get(k).map(Vec::as_slice).unwrap_or(&[]);
    let cpu = cpu_sample(get("stat").first()?)?;
    let mem = get("mem");
    let mem_total_kb = meminfo_kb(mem, "MemTotal")?;
    let mem_avail = meminfo_kb(mem, "MemAvailable").unwrap_or(mem_total_kb);
    let (procs_running, procs_total) = get("load")
        .first()
        .and_then(|l| l.split_whitespace().nth(3))
        .and_then(|rt| rt.split_once('/'))
        .and_then(|(r, t)| Some((r.parse().ok()?, t.parse().ok()?)))
        .unwrap_or((0, 0));
    let uptime_secs = get("up").first().and_then(|l| l.split_whitespace().next()).and_then(|x| x.parse::<f64>().ok()).map(|x| x as u64).unwrap_or(0);
    let user = get("user").first().map(|u| u.trim().to_string()).unwrap_or_default();
    let user_sessions = get("who").iter().filter(|w| w.trim() == user).count() as u32;
    let disk_root_pct = get("df").first().and_then(|l| l.trim().trim_end_matches('%').parse().ok());
    let stats = ServerStats {
        cpu_pct: None,
        mem_used_kb: mem_total_kb.saturating_sub(mem_avail),
        mem_total_kb,
        conns: connections(get("tcp")),
        procs_running,
        procs_total,
        uptime_secs,
        user,
        user_sessions,
        disk_root_pct,
    };
    Some((stats, cpu))
}

/// Baca info server berkala sampai `stop` dibatalkan. Pembacaan pertama yang gagal menandai server
/// tidak didukung dan menghentikan pemantauan; kegagalan sesudahnya dicoba lagi pada giliran berikut.
pub fn spawn_monitor(session: Arc<SshSession>, stop: CancellationToken) -> (watch::Receiver<StatsState>, tokio::task::JoinHandle<()>) {
    let (tx, rx) = watch::channel(StatsState::Waiting);
    let task = tokio::spawn(async move {
        let mut prev: Option<CpuSample> = None;
        let mut first = true;
        loop {
            let res = tokio::select! {
                _ = stop.cancelled() => break,
                r = exec::run(&session, SCRIPT, Duration::from_secs(8)) => r,
            };
            match res.ok().and_then(|o| parse(&String::from_utf8_lossy(&o.stdout))) {
                Some((mut stats, cpu)) => {
                    stats.cpu_pct = prev.and_then(|p| cpu_pct(p, cpu));
                    prev = Some(cpu);
                    if tx.send(StatsState::Ready(stats)).is_err() {
                        break;
                    }
                }
                None if first => {
                    let _ = tx.send(StatsState::Unsupported);
                    break;
                }
                None => {}
            }
            // Pembacaan kedua cepat agar angka CPU segera muncul.
            let wait = if first { Duration::from_secs(1) } else { INTERVAL };
            first = false;
            tokio::select! {
                _ = stop.cancelled() => break,
                _ = tokio::time::sleep(wait) => {}
            }
        }
    });
    (rx, task)
}

/// Lama hidup seperti MobaXterm: `16 days`, `3 h`, `12 min`.
pub fn format_uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs / 3_600 % 24, secs / 60 % 60);
    if d > 0 {
        format!("{d} hari")
    } else if h > 0 {
        format!("{h} jam {m} mnt")
    } else {
        format!("{m} mnt")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "@stat\ncpu  100 0 50 800 50 0 0 0 0 0\n@mem\nMemTotal:        7800000 kB\nMemAvailable:    4067000 kB\n@load\n0.12 0.20 0.30 2/1185 4242\n@up\n1382400.55 100.00\n@user\nekohadi\n@who\nekohadi\nroot\nekohadi\nekohadi\n@df\n77%\n@tcp\n00000000:09B5 00000000:0000 0A\n0100007F:09B5 0200007F:D431 01\n0100007F:09B5 0200007F:D432 01\n0100007F:09B5 0200007F:D433 01\n00000000:01BB 00000000:0000 0A\n0100007F:01BB 0200007F:D434 01\n0100007F:01BB 0200007F:D435 01\n0100007F:01BB 0200007F:D436 01\n0100007F:01BB 0200007F:D437 01\n0100007F:D431 0100007F:09B5 01\n";

    #[test]
    fn parses_a_linux_server() {
        let (s, cpu) = parse(SAMPLE).expect("linux output parses");
        assert_eq!(cpu, CpuSample { busy: 150, total: 1000 });
        assert_eq!((s.mem_total_kb, s.mem_used_kb), (7_800_000, 3_733_000));
        assert_eq!((s.procs_running, s.procs_total), (2, 1185));
        assert_eq!(s.uptime_secs, 1_382_400);
        assert_eq!((s.user.as_str(), s.user_sessions), ("ekohadi", 3));
        assert_eq!(s.disk_root_pct, Some(77));
        // 443 has 4, 2485 (0x09B5) has 3; the client side port 0xD431 is not a listening port.
        assert_eq!(s.conns, vec![(443, 4), (2485, 3)]);
        assert_eq!(s.cpu_pct, None);
    }

    #[test]
    fn non_linux_output_is_unsupported() {
        assert!(parse("").is_none());
        assert!(parse("@stat\n@mem\n@load\n").is_none(), "no /proc on BSD or Windows OpenSSH");
        assert!(parse("sh: perintah tidak dikenal\n").is_none());
    }

    #[test]
    fn missing_optional_parts_still_parse() {
        let out = "@stat\ncpu 1 2 3 4\n@mem\nMemTotal: 1000 kB\n@df\n@tcp\n";
        let (s, _) = parse(out).unwrap();
        assert_eq!((s.mem_used_kb, s.disk_root_pct, s.conns.len()), (0, None, 0));
    }

    #[test]
    fn cpu_percent_between_samples() {
        let a = CpuSample { busy: 100, total: 1000 };
        assert_eq!(cpu_pct(a, CpuSample { busy: 106, total: 1100 }), Some(6.0));
        assert_eq!(cpu_pct(a, a), None, "no time passed");
        assert_eq!(cpu_pct(a, CpuSample { busy: 1, total: 10 }), None, "counters reset (reboot)");
    }

    #[test]
    fn uptime_reads_like_mobaxterm() {
        assert_eq!(format_uptime(16 * 86_400 + 5), "16 hari");
        assert_eq!(format_uptime(3 * 3600 + 120), "3 jam 2 mnt");
        assert_eq!(format_uptime(59), "0 mnt");
    }
}
