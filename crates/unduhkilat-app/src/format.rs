//! Number formatting for the telemetry columns. Pure, unit-tested.

pub fn bytes(n: i64) -> String {
    if n < 0 {
        return "Tidak diketahui".into();
    }
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{n} B") } else { format!("{v:.1} {}", UNITS[u]) }
}

pub fn speed(bps: u64) -> String {
    if bps == 0 {
        return "0 KB/s".into();
    }
    if bps >= 1024 * 1024 { format!("{:.1} MB/s", bps as f64 / 1048576.0) } else { format!("{:.0} KB/s", bps as f64 / 1024.0) }
}

pub fn eta(remaining: i64, bps: u64) -> String {
    if remaining < 0 || bps == 0 {
        return "-".into();
    }
    let s = remaining as u64 / bps.max(1);
    match s {
        0..=59 => format!("{s} dtk"),
        60..=3599 => format!("{} mnt {} dtk", s / 60, s % 60),
        _ => format!("{} j {} mnt", s / 3600, (s % 3600) / 60),
    }
}

pub fn percent(done: i64, total: i64) -> f32 {
    if total <= 0 { 0.0 } else { (done as f64 / total as f64).clamp(0.0, 1.0) as f32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(bytes(-1), "Tidak diketahui");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(5 * 1024 * 1024 * 1024), "5.0 GB");
    }

    #[test]
    fn speeds_and_eta() {
        assert_eq!(speed(0), "0 KB/s");
        assert_eq!(speed(2048), "2 KB/s");
        assert_eq!(speed(3 * 1024 * 1024), "3.0 MB/s");
        assert_eq!(eta(100, 0), "-");
        assert_eq!(eta(-1, 10), "-");
        assert_eq!(eta(600, 10), "1 mnt 0 dtk");
        assert_eq!(eta(10, 5), "2 dtk");
        assert_eq!(eta(7200 * 10, 10), "2 j 0 mnt");
    }

    #[test]
    fn percent_is_clamped() {
        assert_eq!(percent(50, 100), 0.5);
        assert_eq!(percent(5, 0), 0.0);
        assert_eq!(percent(200, 100), 1.0);
    }
}
