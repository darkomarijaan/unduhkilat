//! Canonical error codes (API.md §2.8) and the crate-wide error type.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Canonical machine-readable error codes. Serialized as `ERR_UPPER_SNAKE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    ErrStorageInitFailed,
    ErrRegistryWriteFailed,
    ErrNativeHostNotFound,
    ErrNativeHostTimeout,
    ErrPayloadCorrupt,
    ErrInvalidUrlScheme,
    ErrMediaParseFailed,
    ErrPathUnwritable,
    ErrNameCollision,
    ErrHttpAuthRequired,
    ErrRangeNotSupported,
    ErrRangeNotSatisfiable,
    ErrStorageFull,
    ErrChecksumSizeMismatch,
    ErrConnectionDropped,
    ErrFileNotFound,
    ErrInvalidPath,
    ErrCrawlFailed,
    ErrExportFailed,
    ErrPackageSignatureInvalid,
    ErrUnauthorized,
    ErrPermissionDenied,
    ErrInvalidArgument,
    ErrNotFound,
    ErrAlreadyExists,
    ErrInternal,
    /// Referenced by ARCHITECTURE.md §7.4 but absent from the API.md catalogue.
    ErrStorageWriteFailed,
    /// Referenced by ARCHITECTURE.md §7.4 (DPAPI decryption failure); absent from API.md.
    ErrAuthChallengeFailed,
    /// Server (SSH/FTPS) host key or certificate not seen before and not confirmed by the user.
    ErrHostKeyUnknown,
    /// A pinned host key changed: the connection is blocked (possible interception).
    ErrHostKeyChanged,
    ErrSshAuthFailed,
    /// A jump host in a chain could not be reached or authenticated; the message names it.
    ErrJumpFailed,
    /// A stored credential cannot be decrypted (e.g. data folder moved to another Windows account).
    ErrCredentialUnreadable,
    ErrTooManyHops,
    /// The server is referenced by another (as a jump host) and cannot be deleted.
    ErrServerInUse,
    /// A remote file operation (list, rename, delete, read, write) failed on the server.
    ErrRemoteFs,
    /// The file is larger than the preview limit.
    ErrPreviewTooLarge,
    /// The file type or content cannot be previewed.
    ErrPreviewUnsupported,
    /// Unduhan browser ini termasuk jenis berkas yang dikecualikan pengguna; browser yang mengunduh.
    ErrCaptureSkipped,
}

impl ErrorCode {
    pub fn as_str(&self) -> &'static str {
        use ErrorCode::*;
        match self {
            ErrStorageInitFailed => "ERR_STORAGE_INIT_FAILED",
            ErrRegistryWriteFailed => "ERR_REGISTRY_WRITE_FAILED",
            ErrNativeHostNotFound => "ERR_NATIVE_HOST_NOT_FOUND",
            ErrNativeHostTimeout => "ERR_NATIVE_HOST_TIMEOUT",
            ErrPayloadCorrupt => "ERR_PAYLOAD_CORRUPT",
            ErrInvalidUrlScheme => "ERR_INVALID_URL_SCHEME",
            ErrMediaParseFailed => "ERR_MEDIA_PARSE_FAILED",
            ErrPathUnwritable => "ERR_PATH_UNWRITABLE",
            ErrNameCollision => "ERR_NAME_COLLISION",
            ErrHttpAuthRequired => "ERR_HTTP_AUTH_REQUIRED",
            ErrRangeNotSupported => "ERR_RANGE_NOT_SUPPORTED",
            ErrRangeNotSatisfiable => "ERR_RANGE_NOT_SATISFIABLE",
            ErrStorageFull => "ERR_STORAGE_FULL",
            ErrChecksumSizeMismatch => "ERR_CHECKSUM_SIZE_MISMATCH",
            ErrConnectionDropped => "ERR_CONNECTION_DROPPED",
            ErrFileNotFound => "ERR_FILE_NOT_FOUND",
            ErrInvalidPath => "ERR_INVALID_PATH",
            ErrCrawlFailed => "ERR_CRAWL_FAILED",
            ErrExportFailed => "ERR_EXPORT_FAILED",
            ErrPackageSignatureInvalid => "ERR_PACKAGE_SIGNATURE_INVALID",
            ErrUnauthorized => "ERR_UNAUTHORIZED",
            ErrPermissionDenied => "ERR_PERMISSION_DENIED",
            ErrInvalidArgument => "ERR_INVALID_ARGUMENT",
            ErrNotFound => "ERR_NOT_FOUND",
            ErrAlreadyExists => "ERR_ALREADY_EXISTS",
            ErrInternal => "ERR_INTERNAL",
            ErrStorageWriteFailed => "ERR_STORAGE_WRITE_FAILED",
            ErrAuthChallengeFailed => "ERR_AUTH_CHALLENGE_FAILED",
            ErrHostKeyUnknown => "ERR_HOST_KEY_UNKNOWN",
            ErrHostKeyChanged => "ERR_HOST_KEY_CHANGED",
            ErrSshAuthFailed => "ERR_SSH_AUTH_FAILED",
            ErrJumpFailed => "ERR_JUMP_FAILED",
            ErrCredentialUnreadable => "ERR_CREDENTIAL_UNREADABLE",
            ErrTooManyHops => "ERR_TOO_MANY_HOPS",
            ErrServerInUse => "ERR_SERVER_IN_USE",
            ErrRemoteFs => "ERR_REMOTE_FS",
            ErrPreviewTooLarge => "ERR_PREVIEW_TOO_LARGE",
            ErrPreviewUnsupported => "ERR_PREVIEW_UNSUPPORTED",
            ErrCaptureSkipped => "ERR_CAPTURE_SKIPPED",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{code}: {message}")]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
    pub details: Option<serde_json::Value>,
}

pub type Result<T> = std::result::Result<T, AppError>;

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), details: None }
    }
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
    pub fn not_found(what: &str, id: &str) -> Self {
        Self::new(ErrorCode::ErrNotFound, format!("{what} {id} does not exist"))
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ErrInvalidArgument, message)
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ErrInternal, message)
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        if let rusqlite::Error::SqliteFailure(f, _) = &e {
            if f.code == rusqlite::ErrorCode::DiskFull {
                return Self::new(ErrorCode::ErrStorageFull, e.to_string());
            }
            if f.code == rusqlite::ErrorCode::ConstraintViolation {
                return Self::new(ErrorCode::ErrAlreadyExists, e.to_string());
            }
        }
        Self::new(ErrorCode::ErrInternal, format!("sqlite: {e}"))
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        // ERROR_DISK_FULL (112) / ERROR_HANDLE_DISK_FULL (39) on Windows, ENOSPC (28) elsewhere.
        let full = matches!(e.raw_os_error(), Some(112) | Some(39) | Some(28));
        let code = if full {
            ErrorCode::ErrStorageFull
        } else if e.kind() == std::io::ErrorKind::NotFound {
            ErrorCode::ErrFileNotFound
        } else {
            ErrorCode::ErrStorageWriteFailed
        };
        Self::new(code, e.to_string())
    }
}

impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        Self::new(ErrorCode::ErrConnectionDropped, e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        Self::new(ErrorCode::ErrPayloadCorrupt, e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::ErrorCode::*;

    #[test]
    fn new_server_codes_serialize_screaming_snake() {
        let cases = [
            (ErrHostKeyUnknown, "ERR_HOST_KEY_UNKNOWN"),
            (ErrHostKeyChanged, "ERR_HOST_KEY_CHANGED"),
            (ErrSshAuthFailed, "ERR_SSH_AUTH_FAILED"),
            (ErrJumpFailed, "ERR_JUMP_FAILED"),
            (ErrCredentialUnreadable, "ERR_CREDENTIAL_UNREADABLE"),
            (ErrTooManyHops, "ERR_TOO_MANY_HOPS"),
            (ErrServerInUse, "ERR_SERVER_IN_USE"),
            (ErrRemoteFs, "ERR_REMOTE_FS"),
            (ErrPreviewTooLarge, "ERR_PREVIEW_TOO_LARGE"),
            (ErrPreviewUnsupported, "ERR_PREVIEW_UNSUPPORTED"),
        ];
        for (code, text) in cases {
            assert_eq!(serde_json::to_string(&code).unwrap(), format!("\"{text}\""));
            assert_eq!(code.as_str(), text);
        }
    }
}
