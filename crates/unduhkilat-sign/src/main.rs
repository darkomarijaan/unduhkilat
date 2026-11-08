//! Release signing tool (FR-060). The private key never enters the repository.
//!
//!   unduhkilat-sign keygen <key-file>        create a key pair; prints the public key
//!   unduhkilat-sign sign <key-file> <file> [--expect-public <base64>]
//!                                            write <file>.sig (base64 Ed25519 signature), check it
//!                                            verifies, and (with --expect-public) that the key is
//!                                            the one the in-app updater trusts
//!   unduhkilat-sign verify <public-base64> <file>   check <file>.sig against a public key
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
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
        ["sign", key, file, rest @ ..] => {
            let expect = match rest {
                [] => None,
                ["--expect-public", p] => Some(*p),
                _ => die("usage: unduhkilat-sign sign <key-file> <file> [--expect-public <base64>]"),
            };
            let data = std::fs::read(file).unwrap_or_else(|e| die(&format!("cannot read {file}: {e}")));
            let key = load(key);
            let public = B64.encode(key.verifying_key().to_bytes());
            if let Some(p) = expect {
                if p != public {
                    die(&format!("this key's public key {public} is not the one the updater trusts ({p}); nothing signed"));
                }
            }
            let sig = key.sign(&data);
            key.verifying_key().verify(&data, &sig).unwrap_or_else(|_| die("signature does not verify"));
            std::fs::write(format!("{file}.sig"), B64.encode(sig.to_bytes())).unwrap_or_else(|e| die(&format!("cannot write signature: {e}")));
            println!("wrote {file}.sig (verified, public key {public})");
        }
        ["verify", public, file] => {
            let pk: [u8; 32] = B64.decode(public).ok().and_then(|b| b.try_into().ok()).unwrap_or_else(|| die("public key must be 32 base64 bytes"));
            let pk = VerifyingKey::from_bytes(&pk).unwrap_or_else(|_| die("invalid public key"));
            let data = std::fs::read(file).unwrap_or_else(|e| die(&format!("cannot read {file}: {e}")));
            let text = std::fs::read_to_string(format!("{file}.sig")).unwrap_or_else(|e| die(&format!("cannot read {file}.sig: {e}")));
            let sig: [u8; 64] = B64.decode(text.trim()).ok().and_then(|b| b.try_into().ok()).unwrap_or_else(|| die("signature must be 64 base64 bytes"));
            pk.verify(&data, &Signature::from_bytes(&sig)).unwrap_or_else(|_| die("INVALID signature"));
            println!("valid signature");
        }
        _ => die("usage: unduhkilat-sign keygen <key-file> | sign <key-file> <file> [--expect-public <b64>] | verify <public-b64> <file>"),
    }
}
