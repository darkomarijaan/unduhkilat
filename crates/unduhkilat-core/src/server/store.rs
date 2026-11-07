//! Persistence of saved servers and their jump chains. All functions take a plain
//! `&Connection` so callers can use them inside `Db::read` / `Db::write`.

use crate::error::{AppError, ErrorCode, Result};
use crate::repo::now;
use crate::server::model::{AuthMethod, NewServer, Protocol, Server};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashSet;

/// Longest jump chain (spec 8: at most 5 hops).
pub const MAX_HOPS: usize = 5;

struct Clean {
    name: String,
    host: String,
    port: u16,
    username: String,
    key_path: Option<String>,
}

fn normalize_host(raw: &str) -> Result<String> {
    let t = raw.trim();
    let t = t.strip_prefix('[').and_then(|x| x.strip_suffix(']')).unwrap_or(t);
    if t.is_empty() || t.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(AppError::invalid("Alamat server kosong atau mengandung spasi."));
    }
    Ok(t.to_lowercase())
}

fn clean(n: &NewServer) -> Result<Clean> {
    let name = n.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::invalid("Nama server tidak boleh kosong."));
    }
    let username = n.username.trim().to_string();
    if username.is_empty() {
        return Err(AppError::invalid("Nama pengguna tidak boleh kosong."));
    }
    let port = match n.port {
        Some(0) => return Err(AppError::invalid("Port harus antara 1 dan 65535.")),
        Some(p) => p,
        None => n.protocol.default_port(),
    };
    if !n.protocol.supports_jump() && n.auth != AuthMethod::Password {
        return Err(AppError::invalid("FTP dan FTPS hanya mendukung masuk dengan password."));
    }
    let key_path = match (n.auth, n.key_path.as_deref().map(str::trim)) {
        (AuthMethod::Key, Some(k)) if !k.is_empty() => Some(k.to_string()),
        (AuthMethod::Key, _) => return Err(AppError::invalid("Masuk dengan kunci pribadi membutuhkan berkas kunci.")),
        _ => None,
    };
    Ok(Clean { name, host: normalize_host(&n.host)?, port, username, key_path })
}

fn name_taken(conn: &Connection, name: &str, except: Option<&str>) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM servers WHERE name = ?1 COLLATE NOCASE AND id != ?2",
        params![name, except.unwrap_or("")],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn check_hops(conn: &Connection, own_id: Option<&str>, protocol: Protocol, hops: &[String]) -> Result<()> {
    if hops.is_empty() {
        return Ok(());
    }
    if !protocol.supports_jump() {
        return Err(AppError::invalid("FTP dan FTPS tidak mendukung jump host (hanya SSH dan SFTP)."));
    }
    if hops.len() > MAX_HOPS {
        return Err(AppError::new(ErrorCode::ErrTooManyHops, format!("Maksimal {MAX_HOPS} lompatan.")));
    }
    let mut seen = HashSet::new();
    for id in hops {
        let hop = get_server(conn, id)?;
        if own_id == Some(id.as_str()) {
            return Err(AppError::invalid(format!("{} tidak boleh menjadi jump host bagi dirinya sendiri.", hop.name)));
        }
        if !seen.insert(id.clone()) {
            return Err(AppError::invalid(format!("{} muncul lebih dari sekali dalam rute jump.", hop.name)));
        }
        if hop.protocol != Protocol::Ssh {
            return Err(AppError::invalid(format!("{} bukan server SSH, jadi tidak bisa menjadi jump host.", hop.name)));
        }
    }
    Ok(())
}

/// Names of the servers that use `id` as a jump host.
fn dependents(conn: &Connection, id: &str) -> Result<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT DISTINCT s.name FROM server_hops h JOIN servers s ON s.id = h.server_id WHERE h.hop_id = ?1 ORDER BY s.name COLLATE NOCASE",
    )?;
    let rows = st.query_map([id], |r| r.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn write_hops(conn: &Connection, id: &str, hops: &[String]) -> Result<()> {
    conn.execute("DELETE FROM server_hops WHERE server_id = ?1", [id])?;
    for (pos, hop) in hops.iter().enumerate() {
        conn.execute("INSERT INTO server_hops (server_id, position, hop_id) VALUES (?1, ?2, ?3)", params![id, pos as i64, hop])?;
    }
    Ok(())
}

pub fn insert_server(conn: &Connection, n: &NewServer) -> Result<Server> {
    let c = clean(n)?;
    if name_taken(conn, &c.name, None)? {
        return Err(AppError::new(ErrorCode::ErrAlreadyExists, format!("Nama server \"{}\" sudah dipakai.", c.name)));
    }
    check_hops(conn, None, n.protocol, &n.hops)?;
    let id = uuid::Uuid::new_v4().to_string();
    let ts = now();
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO servers (id, name, protocol, host, port, username, auth, key_path, save_secret, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?10)",
        params![id, c.name, n.protocol.as_str(), c.host, c.port as i64, c.username, n.auth.as_str(), c.key_path, n.save_secret as i64, ts],
    )?;
    write_hops(&tx, &id, &n.hops)?;
    tx.commit()?;
    get_server(conn, &id)
}

pub fn update_server(conn: &Connection, id: &str, n: &NewServer) -> Result<Server> {
    get_server(conn, id)?;
    let c = clean(n)?;
    if name_taken(conn, &c.name, Some(id))? {
        return Err(AppError::new(ErrorCode::ErrAlreadyExists, format!("Nama server \"{}\" sudah dipakai.", c.name)));
    }
    check_hops(conn, Some(id), n.protocol, &n.hops)?;
    if n.protocol != Protocol::Ssh {
        // Other servers route through this one: it must stay an SSH server.
        let deps = dependents(conn, id)?;
        if !deps.is_empty() {
            return Err(AppError::new(ErrorCode::ErrServerInUse, format!("Server ini dipakai sebagai jump host oleh: {}, jadi harus tetap SSH.", deps.join(", "))));
        }
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE servers SET name=?2, protocol=?3, host=?4, port=?5, username=?6, auth=?7, key_path=?8, save_secret=?9, updated_at=?10 WHERE id=?1",
        params![id, c.name, n.protocol.as_str(), c.host, c.port as i64, c.username, n.auth.as_str(), c.key_path, n.save_secret as i64, now()],
    )?;
    write_hops(&tx, id, &n.hops)?;
    tx.commit()?;
    get_server(conn, id)
}

pub fn delete_server(conn: &Connection, id: &str) -> Result<()> {
    get_server(conn, id)?;
    let deps = dependents(conn, id)?;
    if !deps.is_empty() {
        return Err(AppError::new(ErrorCode::ErrServerInUse, format!("Server ini dipakai sebagai jump host oleh: {}.", deps.join(", "))));
    }
    conn.execute("DELETE FROM servers WHERE id = ?1", [id])?;
    Ok(())
}

pub fn get_server(conn: &Connection, id: &str) -> Result<Server> {
    let row = conn
        .query_row(
            "SELECT id, name, protocol, host, port, username, auth, key_path, save_secret, created_at, updated_at FROM servers WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, i64>(8)?,
                    r.get::<_, String>(9)?,
                    r.get::<_, String>(10)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| AppError::not_found("server", id))?;
    let mut st = conn.prepare("SELECT hop_id FROM server_hops WHERE server_id = ?1 ORDER BY position")?;
    let hops = st.query_map([id], |r| r.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Server {
        id: row.0,
        name: row.1,
        protocol: Protocol::parse(&row.2)?,
        host: row.3,
        port: row.4 as u16,
        username: row.5,
        auth: AuthMethod::parse(&row.6)?,
        key_path: row.7,
        save_secret: row.8 != 0,
        hops,
        created_at: row.9,
        updated_at: row.10,
    })
}

pub fn list_servers(conn: &Connection) -> Result<Vec<Server>> {
    let mut st = conn.prepare("SELECT id FROM servers ORDER BY name COLLATE NOCASE")?;
    let ids = st.query_map([], |r| r.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
    ids.iter().map(|id| get_server(conn, id)).collect()
}

/// The servers to traverse to reach `id`, in order: its hops, then the server itself.
pub fn chain_for(conn: &Connection, id: &str) -> Result<Vec<Server>> {
    let target = get_server(conn, id)?;
    let mut chain = target.hops.iter().map(|h| get_server(conn, h)).collect::<Result<Vec<_>>>()?;
    chain.push(target);
    Ok(chain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;
    use crate::server::model::{AuthMethod, NewServer, Protocol};
    use rusqlite::Connection;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.pragma_update(None, "foreign_keys", "ON").unwrap();
        c.execute_batch(include_str!("../schema.sql")).unwrap();
        c
    }

    fn ssh(name: &str) -> NewServer {
        NewServer { name: name.into(), protocol: Protocol::Ssh, host: "10.0.1.20".into(), port: None, username: "deploy".into(), auth: AuthMethod::Password, key_path: None, save_secret: false, hops: vec![] }
    }

    fn code(r: crate::error::Result<impl std::fmt::Debug>) -> (ErrorCode, String) {
        let e = r.unwrap_err();
        (e.code, e.message)
    }

    #[test]
    fn insert_normalizes_name_host_and_default_port() {
        let c = conn();
        let s = insert_server(&c, &NewServer { name: "  Web Prod ".into(), host: "10.0.1.20 ".into(), ..ssh("x") }).unwrap();
        assert_eq!((s.name.as_str(), s.host.as_str(), s.port), ("Web Prod", "10.0.1.20", 22));
        let h = insert_server(&c, &NewServer { host: "EXAMPLE.com".into(), ..ssh("Host") }).unwrap();
        assert_eq!(h.host, "example.com");
        let f = insert_server(&c, &NewServer { protocol: Protocol::Ftp, ..ssh("Old FTP") }).unwrap();
        assert_eq!(f.port, 21);
        let explicit = insert_server(&c, &NewServer { port: Some(2222), ..ssh("Custom") }).unwrap();
        assert_eq!(explicit.port, 2222);
    }

    #[test]
    fn rejects_bad_names_hosts_ports_and_auth() {
        let c = conn();
        for name in ["", "   "] {
            assert_eq!(code(insert_server(&c, &ssh(name))).0, ErrorCode::ErrInvalidArgument, "name {name:?}");
        }
        for host in ["", "a b", "  "] {
            assert_eq!(code(insert_server(&c, &NewServer { host: host.into(), ..ssh("n") })).0, ErrorCode::ErrInvalidArgument, "host {host:?}");
        }
        assert_eq!(code(insert_server(&c, &NewServer { port: Some(0), ..ssh("n") })).0, ErrorCode::ErrInvalidArgument);
        assert_eq!(code(insert_server(&c, &NewServer { username: " ".into(), ..ssh("n") })).0, ErrorCode::ErrInvalidArgument);
        assert_eq!(code(insert_server(&c, &NewServer { auth: AuthMethod::Key, key_path: None, ..ssh("n") })).0, ErrorCode::ErrInvalidArgument, "key auth needs a key file");
        assert_eq!(code(insert_server(&c, &NewServer { protocol: Protocol::Ftp, auth: AuthMethod::Key, key_path: Some("k".into()), ..ssh("n") })).0, ErrorCode::ErrInvalidArgument, "FTP only takes a password");
        insert_server(&c, &NewServer { port: Some(u16::MAX), ..ssh("ok") }).expect("65535 is a valid port");
    }

    #[test]
    fn unicode_name_and_ipv6_host_round_trip() {
        let c = conn();
        let s = insert_server(&c, &NewServer { host: "[::1]".into(), ..ssh("服务器 ①") }).unwrap();
        assert_eq!(s.host, "::1", "brackets around an IPv6 literal are dropped");
        assert_eq!(get_server(&c, &s.id).unwrap(), s);
        assert_eq!(s.name, "服务器 ①");
    }

    #[test]
    fn duplicate_name_is_case_insensitive_error() {
        let c = conn();
        let a = insert_server(&c, &ssh("Web Prod")).unwrap();
        assert_eq!(code(insert_server(&c, &ssh("web prod"))).0, ErrorCode::ErrAlreadyExists);
        update_server(&c, &a.id, &NewServer { host: "10.9.9.9".into(), ..ssh("Web Prod") }).expect("keeping its own name is fine");
        let b = insert_server(&c, &ssh("Other")).unwrap();
        assert_eq!(code(update_server(&c, &b.id, &ssh("WEB PROD"))).0, ErrorCode::ErrAlreadyExists);
    }

    #[test]
    fn list_is_sorted_by_name_ignoring_case() {
        let c = conn();
        for n in ["bravo", "Alpha", "charlie"] {
            insert_server(&c, &ssh(n)).unwrap();
        }
        let names: Vec<String> = list_servers(&c).unwrap().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["Alpha", "bravo", "charlie"]);
    }

    #[test]
    fn hops_only_ssh_and_max_five() {
        let c = conn();
        let hops: Vec<String> = (1..=6).map(|i| insert_server(&c, &ssh(&format!("hop{i}"))).unwrap().id).collect();
        assert_eq!(code(insert_server(&c, &NewServer { hops: hops.clone(), ..ssh("six") })).0, ErrorCode::ErrTooManyHops);
        let five = insert_server(&c, &NewServer { hops: hops[..5].to_vec(), ..ssh("five") }).unwrap();
        assert_eq!(five.hops, hops[..5]);
        let ftp = insert_server(&c, &NewServer { protocol: Protocol::Ftp, ..ssh("ftp hop") }).unwrap();
        assert_eq!(code(insert_server(&c, &NewServer { hops: vec![ftp.id.clone()], ..ssh("via ftp") })).0, ErrorCode::ErrInvalidArgument, "only SSH servers can be jump hosts");
        assert_eq!(code(insert_server(&c, &NewServer { hops: vec!["nope".into()], ..ssh("ghost") })).0, ErrorCode::ErrNotFound);
    }

    #[test]
    fn ftp_server_with_hops_is_rejected() {
        let c = conn();
        let b = insert_server(&c, &ssh("B")).unwrap();
        let (code_, msg) = code(insert_server(&c, &NewServer { protocol: Protocol::Ftps, hops: vec![b.id], ..ssh("F") }));
        assert_eq!(code_, ErrorCode::ErrInvalidArgument);
        assert!(msg.contains("jump"), "message explains FTP cannot jump: {msg}");
    }

    #[test]
    fn self_hop_and_duplicate_hop_are_rejected_naming_the_server() {
        let c = conn();
        let a = insert_server(&c, &ssh("Alpha")).unwrap();
        let b = insert_server(&c, &ssh("Bravo")).unwrap();
        let (cd, msg) = code(update_server(&c, &a.id, &NewServer { hops: vec![a.id.clone()], ..ssh("Alpha") }));
        assert_eq!(cd, ErrorCode::ErrInvalidArgument);
        assert!(msg.contains("Alpha"), "message names the server: {msg}");
        let (cd, msg) = code(insert_server(&c, &NewServer { hops: vec![b.id.clone(), b.id.clone()], ..ssh("Via") }));
        assert_eq!(cd, ErrorCode::ErrInvalidArgument);
        assert!(msg.contains("Bravo"), "message names the repeated hop: {msg}");
    }

    #[test]
    fn chain_for_returns_hops_then_target_in_order() {
        let c = conn();
        let b = insert_server(&c, &ssh("B")).unwrap();
        let cc = insert_server(&c, &ssh("C")).unwrap();
        let a = insert_server(&c, &NewServer { hops: vec![cc.id.clone(), b.id.clone()], ..ssh("A") }).unwrap();
        let names: Vec<String> = chain_for(&c, &a.id).unwrap().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["C", "B", "A"]);
        assert_eq!(chain_for(&c, &b.id).unwrap().len(), 1, "a server without hops is just itself");
    }

    #[test]
    fn update_replaces_the_chain() {
        let c = conn();
        let b = insert_server(&c, &ssh("B")).unwrap();
        let d = insert_server(&c, &ssh("D")).unwrap();
        let a = insert_server(&c, &NewServer { hops: vec![b.id.clone()], ..ssh("A") }).unwrap();
        let a2 = update_server(&c, &a.id, &NewServer { hops: vec![d.id.clone()], ..ssh("A") }).unwrap();
        assert_eq!(a2.hops, vec![d.id]);
        delete_server(&c, &b.id).expect("B is no longer a hop of anything");
    }

    #[test]
    fn changing_a_server_used_as_hop_to_ftp_is_refused() {
        let c = conn();
        let b = insert_server(&c, &ssh("Bastion")).unwrap();
        insert_server(&c, &NewServer { hops: vec![b.id.clone()], ..ssh("Prod") }).unwrap();
        let (cd, msg) = code(update_server(&c, &b.id, &NewServer { protocol: Protocol::Ftp, ..ssh("Bastion") }));
        assert_eq!(cd, ErrorCode::ErrServerInUse);
        assert!(msg.contains("Prod"), "message names the dependent: {msg}");
    }

    #[test]
    fn delete_refused_when_used_as_hop_names_dependents() {
        let c = conn();
        let b = insert_server(&c, &ssh("Bastion")).unwrap();
        let a = insert_server(&c, &NewServer { hops: vec![b.id.clone()], ..ssh("Prod") }).unwrap();
        let (cd, msg) = code(delete_server(&c, &b.id));
        assert_eq!(cd, ErrorCode::ErrServerInUse);
        assert!(msg.contains("Prod"), "message names the dependent server: {msg}");
        delete_server(&c, &a.id).unwrap();
        delete_server(&c, &b.id).expect("unused now");
        assert_eq!(code(delete_server(&c, &b.id)).0, ErrorCode::ErrNotFound);
    }
}
