//! Release signing tool (FR-060). The private key never enters the repository.
//!
//!   unduhkilat-sign keygen <key-file>        create a key pair; prints the public key
//!   unduhkilat-sign sign <key-file> <file>   write <file>.sig (base64 Ed25519 signature)
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::{Signer, SigningKey};
use std::process::exit;

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    exit(1);
}

fn load(path: &str) -> SigningKey {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| die(&format!("cannot read key {path}: {e}")));
    let bytes = B64.decode(text.trim()).unwrap_or_else(|_| die("key file is not base64"));
    let arr: [u8; 32] = bytes.try_into().unwrap_or_else(|_| die("key must be 32 bytes"));
    SigningKey::from_bytes(&arr)
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["keygen", path] => {
            if std::path::Path::new(path).exists() {
                die("key file already exists; refusing to overwrite");
            }
            let key = SigningKey::generate(&mut rand_core::OsRng);
            std::fs::write(path, B64.encode(key.to_bytes())).unwrap_or_else(|e| die(&format!("cannot write key: {e}")));
            println!("private key saved to {path} (keep it secret and back it up)");
            println!("public key: {}", B64.encode(key.verifying_key().to_bytes()));
        }
        ["sign", key, file] => {
            let data = std::fs::read(file).unwrap_or_else(|e| die(&format!("cannot read {file}: {e}")));
            let sig = load(key).sign(&data);
            std::fs::write(format!("{file}.sig"), B64.encode(sig.to_bytes())).unwrap_or_else(|e| die(&format!("cannot write signature: {e}")));
            println!("wrote {file}.sig");
        }
        _ => die("usage: unduhkilat-sign keygen <key-file> | sign <key-file> <file>"),
    }
}
