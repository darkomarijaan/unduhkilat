//! Aturan sambung ulang otomatis tab terminal SSH: kapan menyambung ulang, galat mana yang
//! layak dicoba lagi, dan jeda antar percobaan.

use std::time::Duration;

use crate::error::AppError;

/// Jeda sebelum percobaan ke-`attempt` (mulai 1).
/// Berlipat dua dari 1 detik, paling lama 30 detik: cepat pulih dari gangguan singkat
/// tanpa membanjiri server saat jaringan mati lama.
pub fn delay(attempt: u32) -> Duration {
    let exp = attempt.saturating_sub(1).min(5);
    Duration::from_secs((1u64 << exp).min(30))
}

/// Shell SSH berakhir: sambung ulang otomatis?
/// Ada exit-status = shell berhenti dengan sendirinya (mis. `exit`); tanpa itu = sambungan putus.
pub fn after_end(exit: Option<i32>) -> bool {
    exit.is_none()
}

/// Percobaan sambung gagal: layak dicoba lagi?
/// Hanya galat jaringan. Login ditolak atau kunci host berubah tidak diulang (bisa mengunci
/// akun lewat fail2ban); galat lompatan juga tidak, karena bisa berupa login ditolak di hop.
pub fn should_retry(err: &AppError) -> bool {
    err.code == crate::error::ErrorCode::ErrConnectionDropped && err.message != crate::server::ssh::CANCELLED
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    #[test]
    fn delay_doubles_then_caps_at_thirty_seconds() {
        let secs: Vec<u64> = (1..=8).map(|n| delay(n).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 30, 30, 30]);
        assert_eq!(delay(0), Duration::from_secs(1));
        assert_eq!(delay(u32::MAX), Duration::from_secs(30));
    }

    #[test]
    fn only_a_drop_without_exit_status_reconnects() {
        assert!(after_end(None), "sambungan putus: tidak ada exit-status");
        assert!(!after_end(Some(0)), "pengguna mengetik exit");
        assert!(!after_end(Some(130)));
    }

    #[test]
    fn only_network_failures_are_retried() {
        let e = |c, m: &str| AppError::new(c, m.to_string());
        assert!(should_retry(&e(ErrorCode::ErrConnectionDropped, "Tidak bisa menghubungi \"x\"")));
        assert!(should_retry(&e(ErrorCode::ErrConnectionDropped, "Server \"x\" tidak menjawab dalam 15 detik.")));
        assert!(!should_retry(&e(ErrorCode::ErrConnectionDropped, crate::server::ssh::CANCELLED)), "dibatalkan pengguna");
        for c in [ErrorCode::ErrSshAuthFailed, ErrorCode::ErrHostKeyChanged, ErrorCode::ErrHostKeyUnknown, ErrorCode::ErrJumpFailed, ErrorCode::ErrCredentialUnreadable] {
            assert!(!should_retry(&e(c, "x")), "{c:?}");
        }
    }
}
