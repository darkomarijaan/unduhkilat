//! Saved server entries (SSH, SFTP, FTP, FTPS). Secrets are never part of these types:
//! they live encrypted in `server_secrets` (see `secrets.rs`).

use crate::error::{AppError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    Ssh,
    Sftp,
    Ftp,
    Ftps,
}

impl Protocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Protocol::Ssh => "ssh",
            Protocol::Sftp => "sftp",
            Protocol::Ftp => "ftp",
            Protocol::Ftps => "ftps",
        }
    }

    pub fn parse(s: &str) -> Result<Protocol> {
        match s {
            "ssh" => Ok(Protocol::Ssh),
            "sftp" => Ok(Protocol::Sftp),
            "ftp" => Ok(Protocol::Ftp),
            "ftps" => Ok(Protocol::Ftps),
            other => Err(AppError::invalid(format!("protokol tidak dikenal: {other}"))),
        }
    }

    pub fn default_port(&self) -> u16 {
        match self {
            Protocol::Ssh | Protocol::Sftp => 22,
            Protocol::Ftp | Protocol::Ftps => 21,
        }
    }

    /// SSH and SFTP ride an SSH transport, so they can go through jump hosts; FTP/FTPS cannot.
    pub fn supports_jump(&self) -> bool {
        matches!(self, Protocol::Ssh | Protocol::Sftp)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuthMethod {
    Password,
    Key,
    Agent,
}

impl AuthMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthMethod::Password => "password",
            AuthMethod::Key => "key",
            AuthMethod::Agent => "agent",
        }
    }

    pub fn parse(s: &str) -> Result<AuthMethod> {
        match s {
            "password" => Ok(AuthMethod::Password),
            "key" => Ok(AuthMethod::Key),
            "agent" => Ok(AuthMethod::Agent),
            other => Err(AppError::invalid(format!("cara masuk tidak dikenal: {other}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: AuthMethod,
    pub key_path: Option<String>,
    pub save_secret: bool,
    /// Ids of the jump hosts in order, the first one reached first. The route is explicit:
    /// a hop's own `hops` are not consulted.
    pub hops: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewServer {
    pub name: String,
    pub protocol: Protocol,
    pub host: String,
    /// `None` takes the protocol's default port.
    pub port: Option<u16>,
    pub username: String,
    pub auth: AuthMethod,
    pub key_path: Option<String>,
    pub save_secret: bool,
    pub hops: Vec<String>,
}
