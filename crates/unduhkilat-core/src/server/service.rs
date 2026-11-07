//! The single entry point the UI uses for servers: saved entries, secrets, host keys, and
//! "test connection". Thin on purpose: it only wires `store`, `secrets`, `hostkeys`, and `ssh`.

use crate::db::Db;
use crate::engine::proxy::ProxyConfig;
use crate::error::Result;
use crate::server::hostkeys;
use crate::server::model::{NewServer, Server};
use crate::server::secrets::{self, NewServerSecrets, SecretKind};
use crate::server::ssh::{connect, ConnectOptions, Interact};
use crate::server::store;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

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
}

impl ServerService {
    pub fn new(db: Db, proxy: Arc<dyn Fn() -> ProxyConfig + Send + Sync>) -> Self {
        ServerService { db, proxy }
    }

    pub async fn list(&self) -> Result<Vec<Server>> {
        self.db.read(store::list_servers)
    }

    /// Create (`id = None`) or update a server. Secrets are written only when `save_secret` is on;
    /// turning it off wipes what was saved. A secret left `None` keeps the saved one.
    pub async fn save(&self, id: Option<String>, server: NewServer, secrets_in: NewServerSecrets) -> Result<Server> {
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
        let id = id.to_string();
        self.db.write(move |c| store::delete_server(c, &id)).await
    }

    /// Connect through the whole chain, report, and disconnect. Nothing else is done on the server.
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
        let host = host.to_string();
        self.db.write(move |c| hostkeys::forget(c, &host, port)).await
    }
}
