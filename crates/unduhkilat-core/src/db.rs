//! SQLite persistence: one dedicated writer thread (single-writer actor) plus a
//! small pool of read-only connections (ARCHITECTURE.md §6.3, DATABASE.md §1.3).

use crate::error::{AppError, ErrorCode, Result};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};

const SCHEMA: &str = include_str!("schema.sql");
const SCHEMA_VERSION: i64 = 3;

type Job = Box<dyn FnOnce(&mut Connection) + Send + 'static>;

struct Inner {
    path: PathBuf,
    tx: Mutex<mpsc::Sender<Job>>,
    readers: Mutex<Vec<Connection>>,
}

/// Cheaply cloneable handle. The writer thread exits when the last handle drops.
#[derive(Clone)]
pub struct Db {
    inner: Arc<Inner>,
}

fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    Ok(())
}

fn open_writer(path: &Path) -> Result<Connection> {
    let init = |e: rusqlite::Error| AppError::new(ErrorCode::ErrStorageInitFailed, e.to_string());
    let mut conn = Connection::open(path).map_err(init)?;
    configure(&conn).map_err(init)?;
    let mode: String = conn
        .pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))
        .map_err(init)?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(AppError::new(ErrorCode::ErrStorageInitFailed, format!("WAL unavailable, got {mode}")));
    }
    // DATABASE.md §5.1 leaves NORMAL vs FULL as an owner decision; the documented
    // baseline (and TASK-02 acceptance) is NORMAL.
    conn.pragma_update(None, "synchronous", "NORMAL").map_err(init)?;
    migrate(&mut conn).map_err(init)?;
    Ok(conn)
}

fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version < SCHEMA_VERSION {
        let tx = conn.transaction()?;
        tx.execute_batch(SCHEMA)?;
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
    }
    Ok(())
}

impl Db {
    /// Open (creating if needed) the database, run migrations, start the writer actor.
    pub fn open(path: impl AsRef<Path>) -> Result<Db> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AppError::new(ErrorCode::ErrStorageInitFailed, e.to_string()))?;
        }
        let (tx, rx) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let wpath = path.clone();
        std::thread::Builder::new()
            .name("unduhkilat-db-writer".into())
            .spawn(move || {
                let mut conn = match open_writer(&wpath) {
                    Ok(c) => {
                        let _ = ready_tx.send(Ok(()));
                        c
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                while let Ok(job) = rx.recv() {
                    job(&mut conn);
                }
            })
            .map_err(|e| AppError::new(ErrorCode::ErrStorageInitFailed, e.to_string()))?;
        ready_rx
            .recv()
            .map_err(|_| AppError::new(ErrorCode::ErrStorageInitFailed, "writer thread died"))??;
        Ok(Db { inner: Arc::new(Inner { path, tx: Mutex::new(tx), readers: Mutex::new(Vec::new()) }) })
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    fn submit<T, F>(&self, f: F) -> Result<mpsc::Receiver<Result<T>>>
    where
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let (rtx, rrx) = mpsc::channel();
        let job: Job = Box::new(move |conn| {
            let _ = rtx.send(f(conn));
        });
        self.inner
            .tx
            .lock()
            .unwrap()
            .send(job)
            .map_err(|_| AppError::internal("database writer is not running"))?;
        Ok(rrx)
    }

    /// Run `f` on the single writer connection and await the result.
    /// `f` runs outside any transaction; open one with `conn.transaction()` when needed.
    pub async fn write<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let rrx = self.submit(f)?;
        tokio::task::spawn_blocking(move || {
            rrx.recv().map_err(|_| AppError::internal("database writer dropped the job"))?
        })
        .await
        .map_err(|e| AppError::internal(e.to_string()))?
    }

    /// Blocking variant for non-async callers (startup, tests, UI thread).
    pub fn write_blocking<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        self.submit(f)?
            .recv()
            .map_err(|_| AppError::internal("database writer dropped the job"))?
    }

    /// Run a query on a pooled read-only connection (WAL lets this proceed beside the writer).
    pub fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = match self.inner.readers.lock().unwrap().pop() {
            Some(c) => c,
            None => {
                let c = Connection::open_with_flags(
                    &self.inner.path,
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?;
                configure(&c)?;
                c
            }
        };
        let out = f(&conn);
        let mut pool = self.inner.readers.lock().unwrap();
        if pool.len() < 8 {
            pool.push(conn);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (Db, PathBuf) {
        let dir = std::env::temp_dir().join(format!("uk-db-{}", uuid::Uuid::new_v4()));
        let db = Db::open(dir.join("unduhkilat.db")).unwrap();
        (db, dir)
    }

    #[test]
    fn open_sets_wal_and_synchronous_normal_and_creates_all_tables() {
        let (db, dir) = temp_db();
        let (mode, sync, fk) = db
            .write_blocking(|c| {
                let m: String = c.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
                let s: i64 = c.pragma_query_value(None, "synchronous", |r| r.get(0))?;
                let f: i64 = c.pragma_query_value(None, "foreign_keys", |r| r.get(0))?;
                Ok((m, s, f))
            })
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        assert_eq!(sync, 1, "synchronous NORMAL == 1");
        assert_eq!(fk, 1);
        let tables: Vec<String> = db
            .read(|c| {
                let mut st = c.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")?;
                let rows = st.query_map([], |r| r.get::<_, String>(0))?.collect::<std::result::Result<_, _>>()?;
                Ok(rows)
            })
            .unwrap();
        for t in ["downloads", "segments", "categories", "queue_items", "credentials", "schedules", "settings"] {
            assert!(tables.iter().any(|x| x == t), "missing table {t}");
        }
        drop(db);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn reopen_is_idempotent() {
        let (db, dir) = temp_db();
        let path = db.path().to_path_buf();
        drop(db);
        let db2 = Db::open(&path).unwrap();
        let v: i64 = db2.read(|c| Ok(c.pragma_query_value(None, "user_version", |r| r.get(0))?)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        drop(db2);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn reader_sees_writer_commits() {
        let (db, dir) = temp_db();
        db.write_blocking(|c| {
            c.execute("INSERT INTO settings(key,value,updated_at) VALUES('k','v','t')", [])?;
            Ok(())
        })
        .unwrap();
        let v: String = db.read(|c| Ok(c.query_row("SELECT value FROM settings WHERE key='k'", [], |r| r.get(0))?)).unwrap();
        assert_eq!(v, "v");
        drop(db);
        std::fs::remove_dir_all(dir).ok();
    }

    /// The v2 schema is the current schema text minus the server section (marker comment), so this
    /// test cannot drift from what v2 databases really contain.
    fn v2_schema() -> &'static str {
        SCHEMA.split("-- 0003_servers").next().unwrap()
    }

    #[test]
    fn v2_database_migrates_to_v3_without_losing_data() {
        assert!(SCHEMA.contains("-- 0003_servers"), "schema must carry the 0003_servers section marker");
        let dir = std::env::temp_dir().join(format!("uk-db-v2-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("unduhkilat.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(v2_schema()).unwrap();
            conn.execute("INSERT INTO settings(key,value,updated_at) VALUES('old','kept','t')", []).unwrap();
            conn.execute(
                "INSERT INTO credentials(id,host,username,password_encrypted,port,protocol,created_at,updated_at) VALUES('c1','h','u','x',21,'ftp','t','t')",
                [],
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 2).unwrap();
        }
        let db = Db::open(&path).unwrap();
        let v: i64 = db.read(|c| Ok(c.pragma_query_value(None, "user_version", |r| r.get(0))?)).unwrap();
        assert_eq!(v, 3);
        let kept: String = db.read(|c| Ok(c.query_row("SELECT value FROM settings WHERE key='old'", [], |r| r.get(0))?)).unwrap();
        assert_eq!(kept, "kept");
        let creds: i64 = db.read(|c| Ok(c.query_row("SELECT COUNT(*) FROM credentials", [], |r| r.get(0))?)).unwrap();
        assert_eq!(creds, 1);
        for t in ["servers", "server_hops", "server_secrets", "host_keys"] {
            let n: i64 = db
                .read(|c| Ok(c.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1", [t], |r| r.get(0))?))
                .unwrap();
            assert_eq!(n, 1, "missing table {t}");
        }
        drop(db);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn server_tables_enforce_constraints() {
        let (db, dir) = temp_db();
        let add = |c: &Connection, id: &str, name: &str, port: i64| {
            c.execute(
                "INSERT INTO servers(id,name,protocol,host,port,username,auth,created_at,updated_at) VALUES(?1,?2,'ssh','h',?3,'u','password','t','t')",
                rusqlite::params![id, name, port],
            )
        };
        db.write_blocking(move |c| {
            add(c, "a", "Alpha", 22).unwrap();
            assert!(add(c, "a2", "alpha", 22).is_err(), "names are unique ignoring case");
            assert!(add(c, "z0", "Zero", 0).is_err(), "port 0 rejected");
            assert!(add(c, "z1", "Big", 65536).is_err(), "port 65536 rejected");
            add(c, "b", "Bravo", 22).unwrap();
            c.execute("INSERT INTO server_hops(server_id,position,hop_id) VALUES('a',0,'b')", []).unwrap();
            c.execute("INSERT INTO server_secrets(server_id,kind,ciphertext) VALUES('a','password','x')", []).unwrap();
            assert!(c.execute("DELETE FROM servers WHERE id='b'", []).is_err(), "a server used as a hop cannot be deleted");
            c.execute("DELETE FROM servers WHERE id='a'", []).unwrap();
            let hops: i64 = c.query_row("SELECT COUNT(*) FROM server_hops", [], |r| r.get(0)).unwrap();
            let secrets: i64 = c.query_row("SELECT COUNT(*) FROM server_secrets", [], |r| r.get(0)).unwrap();
            assert_eq!((hops, secrets), (0, 0), "deleting a server removes its hops and secrets");
            c.execute("DELETE FROM servers WHERE id='b'", []).unwrap();
            Ok(())
        })
        .unwrap();
        drop(db);
        std::fs::remove_dir_all(dir).ok();
    }
}
