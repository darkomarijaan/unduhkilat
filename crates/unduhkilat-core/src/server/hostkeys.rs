//! Pinned host keys (like `known_hosts`). A key that changes is never accepted silently.

use crate::error::{AppError, ErrorCode, Result};
use crate::repo::now;
use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyStatus {
    /// Never seen for this host, port, and key type: ask the user.
    Unknown,
    Trusted,
    /// Pinned to a different key: block (possible interception).
    Changed { known: String },
}

/// `SHA256:` + base64 (no padding) of the SHA-256 of the key blob, the same text `ssh-keygen -lf` prints.
pub fn fingerprint_sha256(key_blob: &[u8]) -> String {
    format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(key_blob)))
}

fn known(conn: &Connection, host: &str, port: u16, key_type: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT fingerprint FROM host_keys WHERE host = ?1 AND port = ?2 AND key_type = ?3",
            params![host.to_lowercase(), port as i64, key_type],
            |r| r.get(0),
        )
        .optional()?)
}

pub fn check(conn: &Connection, host: &str, port: u16, key_type: &str, fingerprint: &str) -> Result<HostKeyStatus> {
    Ok(match known(conn, host, port, key_type)? {
        None => HostKeyStatus::Unknown,
        Some(k) if k == fingerprint => HostKeyStatus::Trusted,
        Some(k) => HostKeyStatus::Changed { known: k },
    })
}

/// Pin a key the user just confirmed. Never replaces a different pinned key.
pub fn trust(conn: &Connection, host: &str, port: u16, key_type: &str, fingerprint: &str) -> Result<()> {
    match known(conn, host, port, key_type)? {
        Some(k) if k == fingerprint => Ok(()),
        Some(_) => Err(AppError::new(ErrorCode::ErrHostKeyChanged, format!("Kunci host {host}:{port} sudah tersimpan dengan sidik jari lain; gunakan penggantian eksplisit."))),
        None => {
            conn.execute(
                "INSERT INTO host_keys (host, port, key_type, fingerprint, trusted_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![host.to_lowercase(), port as i64, key_type, fingerprint, now()],
            )?;
            Ok(())
        }
    }
}

/// Explicit user action: accept a new key for a host that changed it.
pub fn replace(conn: &Connection, host: &str, port: u16, key_type: &str, fingerprint: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO host_keys (host, port, key_type, fingerprint, trusted_at) VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(host, port, key_type) DO UPDATE SET fingerprint = excluded.fingerprint, trusted_at = excluded.trusted_at",
        params![host.to_lowercase(), port as i64, key_type, fingerprint, now()],
    )?;
    Ok(())
}

/// Drop every pinned key of a host and port (so the next connection asks again).
pub fn forget(conn: &Connection, host: &str, port: u16) -> Result<()> {
    conn.execute("DELETE FROM host_keys WHERE host = ?1 AND port = ?2", params![host.to_lowercase(), port as i64])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;
    use rusqlite::Connection;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../schema.sql")).unwrap();
        c
    }

    #[test]
    fn fingerprint_matches_known_vector() {
        // SHA-256("abc") in the same shape `ssh-keygen -lf` prints: base64 without padding.
        assert_eq!(fingerprint_sha256(b"abc"), "SHA256:ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0");
    }

    #[test]
    fn unknown_then_trusted_then_changed() {
        let c = conn();
        assert_eq!(check(&c, "example.com", 22, "ssh-ed25519", "SHA256:aaa").unwrap(), HostKeyStatus::Unknown);
        trust(&c, "example.com", 22, "ssh-ed25519", "SHA256:aaa").unwrap();
        assert_eq!(check(&c, "example.com", 22, "ssh-ed25519", "SHA256:aaa").unwrap(), HostKeyStatus::Trusted);
        assert_eq!(
            check(&c, "example.com", 22, "ssh-ed25519", "SHA256:bbb").unwrap(),
            HostKeyStatus::Changed { known: "SHA256:aaa".into() }
        );
    }

    #[test]
    fn trust_never_overwrites_a_different_key() {
        let c = conn();
        trust(&c, "h", 22, "ssh-ed25519", "SHA256:aaa").unwrap();
        trust(&c, "h", 22, "ssh-ed25519", "SHA256:aaa").expect("trusting the same key again is fine");
        let e = trust(&c, "h", 22, "ssh-ed25519", "SHA256:bbb").unwrap_err();
        assert_eq!(e.code, ErrorCode::ErrHostKeyChanged);
        assert_eq!(check(&c, "h", 22, "ssh-ed25519", "SHA256:aaa").unwrap(), HostKeyStatus::Trusted, "the old key is still pinned");
    }

    #[test]
    fn replace_updates_only_after_an_explicit_call() {
        let c = conn();
        trust(&c, "h", 22, "ssh-ed25519", "SHA256:aaa").unwrap();
        replace(&c, "h", 22, "ssh-ed25519", "SHA256:bbb").unwrap();
        assert_eq!(check(&c, "h", 22, "ssh-ed25519", "SHA256:bbb").unwrap(), HostKeyStatus::Trusted);
        assert_eq!(check(&c, "h", 22, "ssh-ed25519", "SHA256:aaa").unwrap(), HostKeyStatus::Changed { known: "SHA256:bbb".into() });
    }

    #[test]
    fn host_is_case_insensitive_and_port_specific() {
        let c = conn();
        trust(&c, "Example.COM", 22, "ssh-ed25519", "SHA256:aaa").unwrap();
        assert_eq!(check(&c, "example.com", 22, "ssh-ed25519", "SHA256:aaa").unwrap(), HostKeyStatus::Trusted);
        assert_eq!(check(&c, "example.com", 2222, "ssh-ed25519", "SHA256:aaa").unwrap(), HostKeyStatus::Unknown, "another port is another server");
        assert_eq!(check(&c, "example.com", 22, "ssh-rsa", "SHA256:zzz").unwrap(), HostKeyStatus::Unknown, "another key type has not been pinned yet");
    }

    #[test]
    fn forget_removes_every_key_of_a_host_port() {
        let c = conn();
        trust(&c, "h", 22, "ssh-ed25519", "SHA256:aaa").unwrap();
        trust(&c, "h", 22, "ssh-rsa", "SHA256:bbb").unwrap();
        trust(&c, "h", 2222, "ssh-ed25519", "SHA256:ccc").unwrap();
        forget(&c, "H", 22).unwrap();
        assert_eq!(check(&c, "h", 22, "ssh-ed25519", "SHA256:aaa").unwrap(), HostKeyStatus::Unknown);
        assert_eq!(check(&c, "h", 22, "ssh-rsa", "SHA256:bbb").unwrap(), HostKeyStatus::Unknown);
        assert_eq!(check(&c, "h", 2222, "ssh-ed25519", "SHA256:ccc").unwrap(), HostKeyStatus::Trusted);
    }
}
