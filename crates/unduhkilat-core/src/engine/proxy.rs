//! Proxy configuration (FR-015, TASK-11): direct, system, or manual host/port with
//! optional credentials. The password lives in SQLite only as a DPAPI blob.

use crate::dpapi::{dpapi_decrypt, dpapi_encrypt};
use crate::error::{AppError, ErrorCode, Result};
use crate::repo;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use rusqlite::Connection;
use zeroize::Zeroizing;

pub mod keys {
    pub const MODE: &str = "proxy_mode";
    pub const HOST: &str = "proxy_host";
    pub const PORT: &str = "proxy_port";
    pub const USERNAME: &str = "proxy_username";
    pub const PASSWORD: &str = "proxy_password_encrypted";
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyMode {
    Direct,
    /// Windows system proxy (DATABASE.md default).
    System,
    Manual,
}

impl ProxyMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProxyMode::Direct => "direct",
            ProxyMode::System => "system",
            ProxyMode::Manual => "manual",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "direct" => ProxyMode::Direct,
            "system" => ProxyMode::System,
            "manual" => ProxyMode::Manual,
            _ => return None,
        })
    }
}

/// An HTTP proxy usable for `CONNECT` tunnels (FTP).
#[derive(Clone)]
pub struct HttpProxy {
    pub host: String,
    pub port: u16,
    pub auth: Option<(String, Zeroizing<String>)>,
}

impl HttpProxy {
    pub fn basic_header(&self) -> Option<String> {
        self.auth.as_ref().map(|(u, p)| format!("Basic {}", STANDARD.encode(format!("{u}:{}", &**p))))
    }
}

impl std::fmt::Debug for HttpProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpProxy").field("host", &self.host).field("port", &self.port).field("auth", &self.auth.as_ref().map(|(u, _)| format!("{u}:[REDACTED]"))).finish()
    }
}

#[derive(Clone)]
pub struct ProxyConfig {
    pub mode: ProxyMode,
    /// May carry a scheme: `socks5://host`. Default scheme is `http`.
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<Zeroizing<String>>,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self { mode: ProxyMode::System, host: String::new(), port: 0, username: None, password: None }
    }
}

impl std::fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyConfig").field("mode", &self.mode).field("host", &self.host).field("port", &self.port).field("username", &self.username).field("password", &self.password.as_ref().map(|_| "[REDACTED]")).finish()
    }
}

/// `socks5://h` -> ("socks5", "h"); `h` -> ("http", "h").
pub fn split_scheme(host: &str) -> (&str, &str) {
    match host.split_once("://") {
        Some((s, h)) => (s, h),
        None => ("http", host),
    }
}

impl ProxyConfig {
    pub fn validate(&self) -> Result<()> {
        if self.mode != ProxyMode::Manual {
            return Ok(());
        }
        let (scheme, host) = split_scheme(&self.host);
        if host.trim().is_empty() || host.contains(char::is_whitespace) || host.contains('/') {
            return Err(AppError::invalid("proxy host is empty or malformed"));
        }
        if self.port == 0 {
            return Err(AppError::invalid("proxy port must be 1-65535"));
        }
        if !matches!(scheme, "http" | "https" | "socks5" | "socks5h") {
            return Err(AppError::invalid(format!("unsupported proxy scheme {scheme}")));
        }
        Ok(())
    }

    pub fn load(conn: &Connection) -> Result<Self> {
        let get = |k: &str| repo::get_setting(conn, k);
        let mode = get(keys::MODE)?.as_deref().and_then(ProxyMode::parse).unwrap_or(ProxyMode::System);
        let password = match get(keys::PASSWORD)?.filter(|s| !s.is_empty()) {
            Some(enc) => Some(Zeroizing::new((*dpapi_decrypt(&enc)?).clone())),
            None => None,
        };
        Ok(Self {
            mode,
            host: get(keys::HOST)?.unwrap_or_default(),
            port: get(keys::PORT)?.and_then(|p| p.parse().ok()).unwrap_or(0),
            username: get(keys::USERNAME)?.filter(|s| !s.is_empty()),
            password,
        })
    }

    /// Persist. The password is DPAPI-encrypted before it touches SQLite.
    pub fn save(&self, conn: &Connection) -> Result<()> {
        self.validate()?;
        repo::set_setting(conn, keys::MODE, self.mode.as_str())?;
        repo::set_setting(conn, keys::HOST, &self.host)?;
        repo::set_setting(conn, keys::PORT, &self.port.to_string())?;
        repo::set_setting(conn, keys::USERNAME, self.username.as_deref().unwrap_or(""))?;
        let enc = match &self.password {
            Some(p) if !p.is_empty() => dpapi_encrypt(p)?,
            _ => String::new(),
        };
        repo::set_setting(conn, keys::PASSWORD, &enc)
    }

    /// Apply to a reqwest client builder (HTTP/HTTPS downloads and probes).
    pub fn apply(&self, b: reqwest::ClientBuilder) -> Result<reqwest::ClientBuilder> {
        self.validate()?;
        Ok(match self.mode {
            ProxyMode::Direct => b.no_proxy(),
            ProxyMode::System => b, // reqwest reads the Windows system proxy by default
            ProxyMode::Manual => {
                let (scheme, host) = split_scheme(&self.host);
                let mut p = reqwest::Proxy::all(format!("{scheme}://{host}:{}", self.port)).map_err(|e| AppError::invalid(format!("proxy: {e}")))?;
                if let Some(u) = &self.username {
                    p = p.basic_auth(u, self.password.as_ref().map(|s| s.as_str()).unwrap_or(""));
                }
                b.proxy(p)
            }
        })
    }

    /// The tunnel to use for FTP (control and data connections), if any.
    pub fn ftp_proxy(&self) -> Result<Option<HttpProxy>> {
        self.validate()?;
        match self.mode {
            ProxyMode::Direct => Ok(None),
            ProxyMode::System => Ok(system_ftp_proxy()),
            ProxyMode::Manual => {
                let (scheme, host) = split_scheme(&self.host);
                if !matches!(scheme, "http" | "https") {
                    return Err(AppError::new(ErrorCode::ErrInvalidArgument, "FTP through a SOCKS proxy is not supported yet; use an HTTP proxy or direct mode"));
                }
                let auth = self.username.as_ref().map(|u| (u.clone(), self.password.clone().unwrap_or_else(|| Zeroizing::new(String::new()))));
                Ok(Some(HttpProxy { host: host.to_string(), port: self.port, auth }))
            }
        }
    }
}

/// Pick the proxy for FTP out of a Windows `ProxyServer` string:
/// either `host:port` or `http=h:p;https=h:p;ftp=h:p`.
pub fn parse_proxy_server(s: &str) -> Option<(String, u16)> {
    let pick = |entry: &str| -> Option<(String, u16)> {
        let hp = entry.rsplit('=').next()?.trim().trim_start_matches("http://").trim_start_matches("https://");
        let (h, p) = hp.rsplit_once(':')?;
        Some((h.to_string(), p.parse().ok()?))
    };
    if !s.contains('=') {
        return pick(s);
    }
    let entries: Vec<&str> = s.split(';').collect();
    for pref in ["ftp=", "http=", "https="] {
        if let Some(e) = entries.iter().find(|e| e.trim().to_ascii_lowercase().starts_with(pref)) {
            return pick(e);
        }
    }
    None
}

#[cfg(windows)]
fn system_ftp_proxy() -> Option<HttpProxy> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    use winreg::RegKey;
    let k = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings", KEY_READ).ok()?;
    if k.get_value::<u32, _>("ProxyEnable").ok()? != 1 {
        return None;
    }
    let (host, port) = parse_proxy_server(&k.get_value::<String, _>("ProxyServer").ok()?)?;
    Some(HttpProxy { host, port, auth: None })
}

#[cfg(not(windows))]
fn system_ftp_proxy() -> Option<HttpProxy> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn manual(host: &str, port: u16) -> ProxyConfig {
        ProxyConfig { mode: ProxyMode::Manual, host: host.into(), port, username: Some("u".into()), password: Some(Zeroizing::new("pw-secret".into())) }
    }

    #[test]
    fn validation() {
        assert!(manual("proxy.test", 8080).validate().is_ok());
        assert!(manual("socks5://proxy.test", 1080).validate().is_ok());
        assert!(manual("", 8080).validate().is_err());
        assert!(manual("bad host", 8080).validate().is_err());
        assert!(manual("proxy.test", 0).validate().is_err());
        assert!(manual("ftp://proxy.test", 21).validate().is_err());
        assert!(ProxyConfig { mode: ProxyMode::Direct, ..Default::default() }.validate().is_ok());
    }

    #[test]
    fn windows_proxy_server_strings() {
        assert_eq!(parse_proxy_server("proxy.corp:3128"), Some(("proxy.corp".into(), 3128)));
        assert_eq!(parse_proxy_server("http=a:1;https=b:2;ftp=c:3"), Some(("c".into(), 3)));
        assert_eq!(parse_proxy_server("http=a:1;https=b:2"), Some(("a".into(), 1)));
        assert_eq!(parse_proxy_server("garbage"), None);
    }

    #[test]
    fn debug_never_prints_secrets() {
        let c = manual("proxy.test", 8080);
        assert!(!format!("{c:?}").contains("pw-secret"));
        let p = c.ftp_proxy().unwrap().unwrap();
        assert!(!format!("{p:?}").contains("pw-secret"));
    }

    #[test]
    fn ftp_proxy_by_mode() {
        assert!(ProxyConfig { mode: ProxyMode::Direct, ..Default::default() }.ftp_proxy().unwrap().is_none());
        let p = manual("proxy.test", 8080).ftp_proxy().unwrap().unwrap();
        assert_eq!((p.host.as_str(), p.port), ("proxy.test", 8080));
        assert!(p.basic_header().unwrap().starts_with("Basic "));
        assert!(manual("socks5://proxy.test", 1080).ftp_proxy().is_err(), "SOCKS for FTP is rejected, not silently ignored");
    }

    #[cfg(windows)]
    #[test]
    fn roundtrip_persists_and_password_is_encrypted_at_rest() {
        let dir = std::env::temp_dir().join(format!("uk-proxy-{}", uuid::Uuid::new_v4()));
        let db = Db::open(dir.join("t.db")).unwrap();
        db.write_blocking(|c| manual("proxy.test", 8080).save(c)).unwrap();
        let back = db.read(ProxyConfig::load).unwrap();
        assert_eq!(back.mode, ProxyMode::Manual);
        assert_eq!((back.host.as_str(), back.port), ("proxy.test", 8080));
        assert_eq!(back.username.as_deref(), Some("u"));
        assert_eq!(back.password.as_deref().map(String::as_str), Some("pw-secret"));
        let raw: String = db.read(|c| Ok(c.query_row("SELECT value FROM settings WHERE key='proxy_password_encrypted'", [], |r| r.get(0))?)).unwrap();
        assert!(!raw.contains("pw-secret") && !raw.is_empty());
        drop(db);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn default_mode_is_system() {
        let dir = std::env::temp_dir().join(format!("uk-proxy-{}", uuid::Uuid::new_v4()));
        let db = Db::open(dir.join("t.db")).unwrap();
        assert_eq!(db.read(ProxyConfig::load).unwrap().mode, ProxyMode::System);
        drop(db);
        std::fs::remove_dir_all(dir).ok();
    }
}
