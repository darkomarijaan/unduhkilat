//! Manual-test aid: an SFTP server on loopback serving sample files, plus a seeded portable data
//! folder for the app. Usage: `fixture_server <app-exe-folder>`. Never touches the network.

#[path = "../tests/common/ssh_server.rs"]
mod ssh_server;

use ssh_server::{test_keypair, TestCred, TestSsh, TestSshConfig};
use std::io::Write;
use unduhkilat_core::db::Db;
use unduhkilat_core::server::hostkeys;
use unduhkilat_core::server::model::{AuthMethod, NewServer, Protocol};
use unduhkilat_core::server::store;

fn zip_of(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (n, b) in parts {
        w.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(b.as_bytes()).unwrap();
    }
    w.finish().unwrap().into_inner()
}

#[tokio::main]
async fn main() {
    let app_dir = std::path::PathBuf::from(std::env::args().nth(1).expect("app exe folder"));
    let root = std::env::temp_dir().join("uk-fixture-root");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Dokumen")).unwrap();
    std::fs::write(root.join("halo.txt"), "baris satu\nbaris dua\nbaris tiga\n".repeat(200)).unwrap();
    std::fs::write(root.join("catatan.md"), "# Judul Catatan\n\nIsi **tebal** dan `kode`.\n\n- satu\n- dua\n\n| A | B |\n|---|---|\n| 1 | 2 |\n").unwrap();
    let img = image::RgbaImage::from_fn(240, 140, |x, y| image::Rgba([(x % 255) as u8, (y * 2 % 255) as u8, 160, 255]));
    img.save(root.join("gambar.png")).unwrap();
    std::fs::write(root.join("laporan.docx"), zip_of(&[("word/document.xml", r#"<w:document xmlns:w="w"><w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Laporan Bulanan</w:t></w:r></w:p><w:p><w:r><w:t>Isi laporan di sini.</w:t></w:r></w:p></w:body></w:document>"#)])).unwrap();
    std::fs::write(root.join("Dokumen").join("data.bin"), vec![0u8; 5000]).unwrap();

    let (private, public) = test_keypair(None);
    let key_path = app_dir.join("fixture_key");
    std::fs::write(&key_path, private).unwrap();
    let cfg = TestSshConfig { users: vec![("eko".into(), TestCred::Key(public.clone()))], allow_forwarding: false, accept_delay: None, shell: false };
    let ssh = TestSsh::start_with_sftp(cfg, root).await;
    // A second server (its own folder) to copy between.
    let root_b = std::env::temp_dir().join("uk-fixture-root-b");
    let _ = std::fs::remove_dir_all(&root_b);
    std::fs::create_dir_all(root_b.join("Masuk")).unwrap();
    std::fs::write(root_b.join("sudah-ada.txt"), "isi lama di server B\n").unwrap();
    let cfg_b = TestSshConfig { users: vec![("eko".into(), TestCred::Key(public))], allow_forwarding: false, accept_delay: None, shell: false };
    let ssh_b = TestSsh::start_with_sftp(cfg_b, root_b).await;

    let state = app_dir.join("data").join("state");
    std::fs::create_dir_all(&state).unwrap();
    let db = Db::open(state.join("unduhkilat.db")).unwrap();
    let port = ssh.addr.port();
    let n = NewServer { name: "Fixture SFTP".into(), protocol: Protocol::Sftp, host: "127.0.0.1".into(), port: Some(port), username: "eko".into(), auth: AuthMethod::Key, key_path: Some(key_path.to_string_lossy().into_owned()), save_secret: false, hops: vec![] };
    db.write_blocking(move |c| store::insert_server(c, &n)).unwrap();
    let fp = ssh.host_key_fingerprint();
    db.write_blocking(move |c| hostkeys::trust(c, "127.0.0.1", port, "ssh-ed25519", &fp)).unwrap();
    let port_b = ssh_b.addr.port();
    let nb = NewServer { name: "Fixture B".into(), protocol: Protocol::Sftp, host: "127.0.0.1".into(), port: Some(port_b), username: "eko".into(), auth: AuthMethod::Key, key_path: Some(key_path.to_string_lossy().into_owned()), save_secret: false, hops: vec![] };
    db.write_blocking(move |c| store::insert_server(c, &nb)).unwrap();
    let fpb = ssh_b.host_key_fingerprint();
    db.write_blocking(move |c| hostkeys::trust(c, "127.0.0.1", port_b, "ssh-ed25519", &fpb)).unwrap();
    println!("READY port={port}");
    std::io::stdout().flush().unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
}
