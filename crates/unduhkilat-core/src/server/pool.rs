//! Kept-open connections to saved servers for queue transfers.
//!
//! Copying a folder of thousands of small files used to open two fresh SSH/FTP logins per file
//! (handshake, authentication, SFTP start), which took far longer than the data itself and made
//! servers drop the flood of logins (sshd `MaxStartups`). Transfers now lease a connection from
//! this pool and give it back when they finish, so a batch logs in a handful of times in total.
//!
//! Two rules keep it safe:
//!  * At most [`PER_SERVER`] connections to one server exist at once; extra transfers wait for a
//!    free slot instead of failing.
//!  * A transfer that needs several connections (a copy) reserves all its slots in one ordered
//!    step, so two copies in opposite directions cannot each hold one slot and wait forever.

use crate::error::{AppError, ErrorCode, Result};
use crate::server::fs::RemoteFs;
use crate::server::service::ServerService;
use crate::server::ssh::Interact;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

/// Most connections kept to one saved server at the same time. Well below the default sshd
/// `MaxStartups` (10) so even the first burst of logins is accepted.
pub const PER_SERVER: usize = 6;
/// An unused connection is closed after this long.
pub const IDLE_TTL: Duration = Duration::from_secs(60);
/// A connection idle longer than this is checked with one cheap request before it is reused.
const CHECK_AFTER: Duration = Duration::from_secs(2);
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

struct Slot {
    permits: Arc<Semaphore>,
    idle: Vec<(Box<dyn RemoteFs>, Instant)>,
}

impl Slot {
    fn new() -> Self {
        Slot { permits: Arc::new(Semaphore::new(PER_SERVER)), idle: Vec::new() }
    }
}

#[derive(Default)]
pub struct ConnPool {
    slots: Mutex<HashMap<String, Slot>>,
    reaper: AtomicBool,
}

/// One leased connection. Give it back with [`Lease::keep`] when it is known to be healthy, or
/// [`Lease::discard`] after a connection-level failure. Dropping it closes the connection.
pub struct Lease {
    fs: Option<Box<dyn RemoteFs>>,
    server_id: String,
    _permit: OwnedSemaphorePermit,
    pool: Arc<ConnPool>,
}

impl Lease {
    pub fn fs(&mut self) -> &mut dyn RemoteFs {
        self.fs.as_mut().expect("lease holds a connection until it is returned").as_mut()
    }

    /// Return the connection to the pool for the next transfer.
    pub fn keep(mut self) {
        if let Some(fs) = self.fs.take() {
            self.pool.put_back(&self.server_id, fs);
        }
    }

    /// Close the connection; the next transfer opens a fresh one.
    pub async fn discard(mut self) {
        if let Some(fs) = self.fs.take() {
            fs.close().await;
        }
    }
}

/// Whether an error leaves the connection itself usable (the file was the problem, not the link).
pub fn connection_survives(e: &AppError) -> bool {
    matches!(
        e.code,
        ErrorCode::ErrNameCollision
            | ErrorCode::ErrNotFound
            | ErrorCode::ErrFileNotFound
            | ErrorCode::ErrInvalidArgument
            | ErrorCode::ErrInvalidPath
            | ErrorCode::ErrPathUnwritable
            | ErrorCode::ErrPermissionDenied
            | ErrorCode::ErrStorageFull
            | ErrorCode::ErrStorageWriteFailed
    )
}

impl ConnPool {
    pub fn new() -> Arc<Self> {
        Arc::new(ConnPool::default())
    }

    fn permits(&self, id: &str) -> Arc<Semaphore> {
        self.slots.lock().unwrap().entry(id.to_string()).or_insert_with(Slot::new).permits.clone()
    }

    /// Connections to `ids` (a server may appear twice, e.g. a copy within one server), in the same
    /// order. `Ok(None)` when `cancel` fired while waiting for a free slot or connecting.
    pub async fn lease(self: &Arc<Self>, svc: &ServerService, interact: Arc<dyn Interact>, ids: &[&str], cancel: &CancellationToken) -> Result<Option<Vec<Lease>>> {
        // Reserve every slot first, servers in a fixed order and all of one server's slots at once.
        let mut wanted: BTreeMap<&str, u32> = BTreeMap::new();
        for id in ids {
            *wanted.entry(id).or_default() += 1;
        }
        let mut held: HashMap<&str, OwnedSemaphorePermit> = HashMap::new();
        for (id, n) in &wanted {
            let sem = self.permits(id);
            let permit = tokio::select! {
                _ = cancel.cancelled() => return Ok(None),
                p = sem.acquire_many_owned(*n) => p.map_err(|_| AppError::internal("antrean koneksi server ditutup"))?,
            };
            held.insert(id, permit);
        }
        let mut leases: Vec<Lease> = Vec::with_capacity(ids.len());
        for id in ids {
            let all = held.get_mut(id).expect("reserved above");
            let permit = if all.num_permits() > 1 { all.split(1).expect("more than one permit held") } else { held.remove(id).expect("reserved above") };
            let fs = match self.connection(svc, interact.clone(), id, cancel).await {
                Ok(Some(fs)) => fs,
                Ok(None) => {
                    for l in leases {
                        l.keep();
                    }
                    return Ok(None);
                }
                Err(e) => {
                    for l in leases {
                        l.keep();
                    }
                    return Err(e);
                }
            };
            leases.push(Lease { fs: Some(fs), server_id: id.to_string(), _permit: permit, pool: self.clone() });
        }
        Ok(Some(leases))
    }

    /// A kept connection that still answers, or a new one.
    async fn connection(&self, svc: &ServerService, interact: Arc<dyn Interact>, id: &str, cancel: &CancellationToken) -> Result<Option<Box<dyn RemoteFs>>> {
        while let Some((mut fs, since)) = self.take_idle(id) {
            if !fs.is_alive() {
                continue;
            }
            if since.elapsed() < CHECK_AFTER {
                return Ok(Some(fs));
            }
            match tokio::time::timeout(CHECK_TIMEOUT, fs.home()).await {
                Ok(Ok(_)) => return Ok(Some(fs)),
                // The server dropped it meanwhile: close quietly and try the next one.
                _ => drop(fs),
            }
        }
        if cancel.is_cancelled() {
            return Ok(None);
        }
        let opened = tokio::select! {
            _ = cancel.cancelled() => return Ok(None),
            fs = svc.open_fs(id, interact, cancel.clone()) => fs?,
        };
        Ok(Some(opened))
    }

    fn take_idle(&self, id: &str) -> Option<(Box<dyn RemoteFs>, Instant)> {
        let mut slots = self.slots.lock().unwrap();
        let slot = slots.get_mut(id)?;
        // Expired ones are dropped here; the most recently used is the likeliest to be alive.
        slot.idle.retain(|(_, t)| t.elapsed() < IDLE_TTL);
        slot.idle.pop()
    }

    fn put_back(self: &Arc<Self>, id: &str, fs: Box<dyn RemoteFs>) {
        self.slots.lock().unwrap().entry(id.to_string()).or_insert_with(Slot::new).idle.push((fs, Instant::now()));
        self.start_reaper();
    }

    /// Closes connections nobody used for [`IDLE_TTL`]; stops once nothing is kept.
    fn start_reaper(self: &Arc<Self>) {
        if self.reaper.swap(true, Ordering::SeqCst) {
            return;
        }
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            self.reaper.store(false, Ordering::SeqCst);
            return;
        };
        let pool = Arc::downgrade(self);
        rt.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                let Some(pool) = pool.upgrade() else { return };
                let expired: Vec<Box<dyn RemoteFs>> = {
                    let mut slots = pool.slots.lock().unwrap();
                    let mut out = Vec::new();
                    for slot in slots.values_mut() {
                        let (old, fresh): (Vec<_>, Vec<_>) = slot.idle.drain(..).partition(|(_, t)| t.elapsed() >= IDLE_TTL);
                        slot.idle = fresh;
                        out.extend(old.into_iter().map(|(fs, _)| fs));
                    }
                    out
                };
                for fs in expired {
                    fs.close().await;
                }
                let empty = pool.slots.lock().unwrap().values().all(|s| s.idle.is_empty());
                if empty {
                    pool.reaper.store(false, Ordering::SeqCst);
                    // A connection may have been returned between the check and the store.
                    if pool.slots.lock().unwrap().values().all(|s| s.idle.is_empty()) || pool.reaper.swap(true, Ordering::SeqCst) {
                        return;
                    }
                }
            }
        });
    }

    /// Close every kept connection (e.g. after a server's settings changed).
    pub async fn clear(&self) {
        let all: Vec<Box<dyn RemoteFs>> = {
            let mut slots = self.slots.lock().unwrap();
            slots.values_mut().flat_map(|s| s.idle.drain(..).map(|(fs, _)| fs)).collect()
        };
        for fs in all {
            fs.close().await;
        }
    }

    /// Connections currently kept open and unused, for tests and diagnostics.
    pub fn idle_count(&self) -> usize {
        self.slots.lock().unwrap().values().map(|s| s.idle.len()).sum()
    }
}
