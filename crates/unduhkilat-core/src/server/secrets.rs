//! Encrypted server secrets (password, key passphrase). DPAPI binds them to the Windows account.

use crate::dpapi::{dpapi_decrypt, dpapi_encrypt};
use crate::error::{AppError, ErrorCode, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::fmt;
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretKind {
    Password,
    Passphrase,
}

impl SecretKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SecretKind::Password => "password",
            SecretKind::Passphrase => "passphrase",
        }
    }
}

/// A secret held in memory. Wiped on drop; `Debug` never shows the value and there is no `Display`.
pub struct SecretString(Zeroizing<String>);

impl SecretString {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<&str> for SecretString {
    fn from(s: &str) -> Self {
        SecretString(Zeroizing::new(s.to_string()))
    }
}

impl From<String> for SecretString {
    fn from(s: String) -> Self {
        SecretString(Zeroizing::new(s))
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(***)")
    }
}

/// Secrets typed into the server dialog, handed to the service layer for saving.
#[derive(Debug, Default)]
pub struct NewServerSecrets {
    pub password: Option<SecretString>,
    pub passphrase: Option<SecretString>,
}

pub fn store_secret(conn: &Connection, server_id: &str, kind: SecretKind, secret: &str) -> Result<()> {
    let cipher = dpapi_encrypt(secret)?;
    conn.execute(
        "INSERT INTO server_secrets (server_id, kind, ciphertext) VALUES (?1, ?2, ?3) ON CONFLICT(server_id, kind) DO UPDATE SET ciphertext = excluded.ciphertext",
        params![server_id, kind.as_str(), cipher],
    )?;
    Ok(())
}

/// `Ok(None)` when nothing is stored. A stored secret that cannot be decrypted (typically the data
/// folder was moved to another Windows account) is `ErrCredentialUnreadable`; the message never
/// includes the stored blob.
pub fn load_secret(conn: &Connection, server_id: &str, kind: SecretKind) -> Result<Option<Zeroizing<String>>> {
    let cipher: Option<String> = conn
        .query_row("SELECT ciphertext FROM server_secrets WHERE server_id = ?1 AND kind = ?2", params![server_id, kind.as_str()], |r| r.get(0))
        .optional()?;
    match cipher {
        None => Ok(None),
        Some(c) => dpapi_decrypt(&c).map(Some).map_err(|_| {
            AppError::new(ErrorCode::ErrCredentialUnreadable, "Kredensial ini tersimpan di akun Windows lain dan tidak bisa dibaca; masukkan ulang.")
        }),
    }
}

pub fn delete_secrets(conn: &Connection, server_id: &str) -> Result<()> {
    conn.execute("DELETE FROM server_secrets WHERE server_id = ?1", [server_id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;
    use crate::server::model::{AuthMethod, NewServer, Protocol};
    use crate::server::store::insert_server;
    use rusqlite::Connection;

    fn conn_with_server() -> (Connection, String) {
        let c = Connection::open_in_memory().unwrap();
        c.pragma_update(None, "foreign_keys", "ON").unwrap();
        c.execute_batch(include_str!("../schema.sql")).unwrap();
        let s = insert_server(
            &c,
            &NewServer { name: "web".into(), protocol: Protocol::Ssh, host: "h".into(), port: None, username: "u".into(), auth: AuthMethod::Password, key_path: None, save_secret: true, hops: vec![] },
        )
        .unwrap();
        (c, s.id)
    }

    #[test]
    fn secret_round_trip() {
        let (c, id) = conn_with_server();
        store_secret(&c, &id, SecretKind::Password, "p@ss w0rd \u{fc}").unwrap();
        let got = load_secret(&c, &id, SecretKind::Password).unwrap().expect("stored");
        assert_eq!(got.as_str(), "p@ss w0rd \u{fc}");
    }

    #[test]
    fn store_overwrites_and_kinds_are_separate() {
        let (c, id) = conn_with_server();
        store_secret(&c, &id, SecretKind::Password, "one").unwrap();
        store_secret(&c, &id, SecretKind::Password, "two").unwrap();
        store_secret(&c, &id, SecretKind::Passphrase, "phrase").unwrap();
        assert_eq!(load_secret(&c, &id, SecretKind::Password).unwrap().unwrap().as_str(), "two");
        assert_eq!(load_secret(&c, &id, SecretKind::Passphrase).unwrap().unwrap().as_str(), "phrase");
    }

    #[test]
    fn ciphertext_in_db_is_not_plaintext() {
        let (c, id) = conn_with_server();
        store_secret(&c, &id, SecretKind::Password, "s3cr3t-XYZ").unwrap();
        let raw: String = c.query_row("SELECT ciphertext FROM server_secrets WHERE server_id=?1", [&id], |r| r.get(0)).unwrap();
        assert!(!raw.contains("s3cr3t-XYZ"));
        assert!(!raw.is_empty());
    }

    #[test]
    fn missing_secret_is_none() {
        let (c, id) = conn_with_server();
        assert!(load_secret(&c, &id, SecretKind::Password).unwrap().is_none());
    }

    #[test]
    fn unreadable_ciphertext_gives_credential_unreadable_with_message() {
        let (c, id) = conn_with_server();
        c.execute("INSERT INTO server_secrets(server_id,kind,ciphertext) VALUES(?1,'password','AAECAwQFBgcICQ==')", [&id]).unwrap();
        let e = load_secret(&c, &id, SecretKind::Password).unwrap_err();
        assert_eq!(e.code, ErrorCode::ErrCredentialUnreadable);
        assert!(e.message.contains("masukkan ulang"), "message tells the user to re-enter: {}", e.message);
        assert!(!e.message.contains("AAECAwQ"), "message never echoes the stored blob");
    }

    #[test]
    fn debug_output_never_contains_the_secret() {
        let secrets = NewServerSecrets { password: Some(SecretString::from("s3cr3t-XYZ")), passphrase: Some(SecretString::from("frasa-QQQ")) };
        let shown = format!("{secrets:?}");
        assert!(!shown.contains("s3cr3t-XYZ") && !shown.contains("frasa-QQQ"), "Debug leaked: {shown}");
        assert!(shown.contains("***"));
    }

    #[test]
    fn delete_secrets_removes_rows() {
        let (c, id) = conn_with_server();
        store_secret(&c, &id, SecretKind::Password, "a").unwrap();
        store_secret(&c, &id, SecretKind::Passphrase, "b").unwrap();
        delete_secrets(&c, &id).unwrap();
        assert!(load_secret(&c, &id, SecretKind::Password).unwrap().is_none());
        assert!(load_secret(&c, &id, SecretKind::Passphrase).unwrap().is_none());
    }
}
