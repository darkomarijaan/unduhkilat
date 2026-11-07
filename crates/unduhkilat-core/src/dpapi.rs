//! Windows DPAPI credential vault (DEC-005, FR-014, FR-058).
//!
//! Secrets are encrypted with `CryptProtectData` (bound to the current Windows
//! user) and stored base64-encoded. Plaintext buffers are zeroized after use.

use crate::error::{AppError, ErrorCode, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use zeroize::Zeroizing;

/// Fixed application-specific entropy so a blob from another app cannot be replayed here.
const ENTROPY: &[u8] = b"UnduhKilat/credential-vault/v1";

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
    }

    /// Copies the output blob into owned memory, wipes and frees the system buffer.
    unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        // Zero the system-allocated buffer before releasing it (it may hold plaintext).
        std::ptr::write_bytes(out.pbData, 0, out.cbData as usize);
        LocalFree(out.pbData as _);
        v
    }

    pub fn protect(plain: &[u8]) -> Result<Vec<u8>> {
        let input = blob(plain);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        // SAFETY: all pointers reference live slices for the duration of the call.
        let ok = unsafe {
            CryptProtectData(&input, std::ptr::null(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        if ok == 0 {
            return Err(AppError::new(ErrorCode::ErrInternal, "CryptProtectData failed"));
        }
        Ok(unsafe { take(out) })
    }

    pub fn unprotect(cipher: &[u8]) -> Result<Vec<u8>> {
        let input = blob(cipher);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        // SAFETY: as above.
        let ok = unsafe {
            CryptUnprotectData(&input, std::ptr::null_mut(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        if ok == 0 {
            return Err(AppError::new(ErrorCode::ErrAuthChallengeFailed, "CryptUnprotectData failed"));
        }
        Ok(unsafe { take(out) })
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    pub fn protect(_: &[u8]) -> Result<Vec<u8>> {
        Err(AppError::internal("DPAPI is only available on Windows (DEC-005)"))
    }
    pub fn unprotect(_: &[u8]) -> Result<Vec<u8>> {
        Err(AppError::internal("DPAPI is only available on Windows (DEC-005)"))
    }
}

/// Encrypt `plaintext` and return base64 ciphertext suitable for `password_encrypted`.
pub fn dpapi_encrypt(plaintext: &str) -> Result<String> {
    let buf = Zeroizing::new(plaintext.as_bytes().to_vec());
    Ok(STANDARD.encode(imp::protect(&buf)?))
}

/// Decrypt base64 ciphertext. The returned buffer wipes itself on drop.
pub fn dpapi_decrypt(ciphertext_b64: &str) -> Result<Zeroizing<String>> {
    let raw = STANDARD
        .decode(ciphertext_b64)
        .map_err(|e| AppError::new(ErrorCode::ErrPayloadCorrupt, e.to_string()))?;
    let plain = Zeroizing::new(imp::unprotect(&raw)?);
    String::from_utf8(plain.to_vec())
        .map(Zeroizing::new)
        .map_err(|e| AppError::new(ErrorCode::ErrPayloadCorrupt, e.to_string()))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_ciphertext_hides_plaintext() {
        let secret = "p@ssw0rd-Üñî-123";
        let enc = dpapi_encrypt(secret).unwrap();
        assert!(!enc.contains("p@ssw0rd"));
        let raw = STANDARD.decode(&enc).unwrap();
        assert!(!raw.windows(8).any(|w| w == b"p@ssw0rd"));
        assert_eq!(&*dpapi_decrypt(&enc).unwrap(), secret);
    }

    #[test]
    fn same_plaintext_gives_different_ciphertext_each_time() {
        assert_ne!(dpapi_encrypt("same").unwrap(), dpapi_encrypt("same").unwrap());
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let enc = dpapi_encrypt("x").unwrap();
        let mut raw = STANDARD.decode(enc).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0xFF;
        assert!(dpapi_decrypt(&STANDARD.encode(raw)).is_err());
    }
}
