//! The single entry point the UI uses for servers: saved entries, secrets, host keys, and
//! "test connection". Thin on purpose: it only wires `store`, `secrets`, `hostkeys`, and `ssh`.

use crate::db::Db;
use crate::engine::proxy::ProxyConfig;
use crate::error::Result;
use crate::server::hostkeys;
use crate::server::model::{NewServer, Protocol, Server};
use crate::server::secrets::{self, NewServerSecrets, SecretKind};
use crate::server::ssh::{connect, ConnectOptions, Interact};
use crate::server::store;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// A shell opened by [`ServerService::open_terminal`].
pub struct OpenedTerminal {
    pub io: crate::term::source::SourceIo,
    pub hops: Vec<String>,
    pub stats: Option<tokio::sync::watch::Receiver<crate::server::stats::StatsState>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestReport {
    pub rtt_ms: u64,
    /// Names of every server traversed, in order, ending with the target.
    pub hop_names: Vec<String>,
}

#[derive(Clone)]
pub struct ServerService {
    db: Db,
    /// Current proxy settings, read at connect time so a change in Settings applies immediately.
    proxy: Arc<dyn Fn() -> ProxyConfig + Send + Sync>,
    /// Kept transfer connections, closed whenever a server entry or a pinned key changes so the
    /// next transfer logs in with the new settings.
    pool: Option<Arc<crate::server::pool::ConnPool>>,
}

impl ServerService {
    pub fn new(db: Db, proxy: Arc<dyn Fn() -> ProxyConfig + Send + Sync>) -> Self {
        ServerService { db, proxy, pool: None }
    }

    pub fn with_pool(mut self, pool: Arc<crate::server::pool::ConnPool>) -> Self {
        self.pool = Some(pool);
        self
    }

    async fn drop_kept_connections(&self) {
        if let Some(p) = &self.pool {
            p.clear().await;
        }
    }

    pub async fn list(&self) -> Result<Vec<Server>> {
        self.db.read(store::list_servers)
    }

    /// Create (`id = None`) or update a server. Secrets are written only when `save_secret` is on;
    /// turning it off wipes what was saved. A secret left `None` keeps the saved one.
    pub async fn save(&self, id: Option<String>, server: NewServer, secrets_in: NewServerSecrets) -> Result<Server> {
        self.drop_kept_connections().await;
        self.db
            .write(move |c| {
                let saved = match &id {
                    None => store::insert_server(c, &server)?,
                    Some(id) => store::update_server(c, id, &server)?,
                };
                if saved.save_secret {
                    if let Some(p) = &secrets_in.password {
                        secrets::store_secret(c, &saved.id, SecretKind::Password, p.expose())?;
                    }
                    if let Some(p) = &secrets_in.passphrase {
                        secrets::store_secret(c, &saved.id, SecretKind::Passphrase, p.expose())?;
                    }
                } else {
                    secrets::delete_secrets(c, &saved.id)?;
                }
                Ok(saved)
            })
            .await
    }

    /// Refused with `ErrServerInUse` while another server uses it as a jump host. Its saved secrets
    /// go with it (foreign-key cascade), and only if the delete is allowed.
    pub async fn delete(&self, id: &str) -> Result<()> {
        self.drop_kept_connections().await;
        let id = id.to_string();
        self.db.write(move |c| store::delete_server(c, &id)).await
    }

    /// Connect through the whole chain and open an interactive shell on the target. Returns the
    /// shell's I/O, the names of every server traversed (ending with the target), and, with `stats`,
    /// the server's system info read over the same connection.
    pub async fn open_terminal(&self, id: &str, interact: Arc<dyn Interact>, cancel: CancellationToken, cols: u16, rows: u16, stats: bool) -> Result<OpenedTerminal> {
        let opts = ConnectOptions { proxy: (self.proxy)(), ..Default::default() };
        let session = connect(&self.db, id, interact, opts, cancel).await?;
        let target_id = session.target.id.clone();
        let hop_names = self.db.read(|c| store::chain_for(c, &target_id))?.into_iter().map(|s| s.name).collect();
        let (io, stats) = crate::server::shell::open_shell_with_stats(session, cols, rows, stats).await?;
        Ok(OpenedTerminal { io, hops: hop_names, stats })
    }

    /// Connect through the whole chain, report, and disconnect. Nothing else is done on the server.
    /// Connect and open the file system of a saved server: SFTP over SSH for SSH and SFTP entries
    /// (through the whole jump chain), FTP or FTPS for the others. Nothing is contacted until this
    /// is called.
    pub async fn open_fs(&self, id: &str, interact: Arc<dyn Interact>, cancel: CancellationToken) -> Result<Box<dyn crate::server::fs::RemoteFs>> {
        let server = self.db.read({
            let id = id.to_string();
            move |c| store::get_server(c, &id)
        })?;
        match server.protocol {
            Protocol::Ssh | Protocol::Sftp => {
                let opts = ConnectOptions { proxy: (self.proxy)(), ..Default::default() };
                let session = connect(&self.db, id, interact, opts, cancel).await?;
                crate::server::fs::sftp::open_sftp(session).await
            }
            Protocol::Ftp | Protocol::Ftps => crate::server::fs::ftp::open_ftp(&self.db, &server, interact, cancel).await,
        }
    }

    pub async fn test_connection(&self, id: &str, interact: Arc<dyn Interact>, cancel: CancellationToken) -> Result<TestReport> {
        let opts = ConnectOptions { proxy: (self.proxy)(), ..Default::default() };
        let session = connect(&self.db, id, interact, opts, cancel).await?;
        let rtt_ms = session.rtt.as_millis() as u64;
        let target_id = session.target.id.clone();
        session.close().await;
        let hop_names = self.db.read(|c| store::chain_for(c, &target_id))?.into_iter().map(|s| s.name).collect();
        Ok(TestReport { rtt_ms, hop_names })
    }

    /// Forget the pinned host keys of `host:port` so the next connection asks again (explicit user action).
    pub async fn forget_host_key(&self, host: &str, port: u16) -> Result<()> {
        self.drop_kept_connections().await;
        let host = host.to_string();
        self.db.write(move |c| hostkeys::forget(c, &host, port)).await
    }
}
