//! JSON command / response / event envelopes (API.md §2.2–2.5) and the
//! Chromium native-messaging length-prefixed framing (API.md §2.1).

use crate::error::{AppError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{Read, Write};

/// Chromium limits a single message from the host to the browser to 1 MB.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Request {
    pub id: String,
    pub command: String,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum Response {
    Ok { id: String, data: Value },
    Error { id: String, error: ErrorBody },
}

impl Response {
    pub fn ok(id: impl Into<String>, data: impl Serialize) -> Self {
        let id = id.into();
        match serde_json::to_value(data) {
            Ok(data) => Response::Ok { id, data },
            Err(e) => Response::error(id, &AppError::from(e)),
        }
    }
    pub fn error(id: impl Into<String>, e: &AppError) -> Self {
        Response::Error {
            id: id.into(),
            error: ErrorBody { code: e.code, message: e.message.clone(), details: e.details.clone() },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    #[serde(rename = "type")]
    pub kind: String, // always "event"
    pub event: String,
    pub payload: Value,
}

impl Event {
    pub fn new(event: impl Into<String>, payload: impl Serialize) -> Self {
        Self {
            kind: "event".into(),
            event: event.into(),
            payload: serde_json::to_value(payload).unwrap_or(Value::Null),
        }
    }
}

/// Write one 32-bit little-endian length-prefixed UTF-8 JSON frame.
pub fn write_frame<W: Write, T: Serialize>(w: &mut W, msg: &T) -> Result<(), AppError> {
    let body = serde_json::to_vec(msg)?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(AppError::new(ErrorCode::ErrPayloadCorrupt, "frame exceeds 1 MB"));
    }
    w.write_all(&(body.len() as u32).to_le_bytes())?;
    w.write_all(&body)?;
    w.flush()?;
    Ok(())
}

/// Read one frame. `Ok(None)` on clean EOF before a new frame starts.
pub fn read_frame<R: Read, T: for<'de> Deserialize<'de>>(r: &mut R) -> Result<Option<T>, AppError> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let n = u32::from_le_bytes(len) as usize;
    // We never accept more than 1 MB, regardless of what the prefix claims.
    if n > MAX_FRAME_BYTES {
        return Err(AppError::new(ErrorCode::ErrPayloadCorrupt, "frame exceeds 1 MB"));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)?;
    Ok(Some(serde_json::from_slice(&buf)?))
}

/// Async variants for the app side of the local pipe.
pub async fn write_frame_async<W, T>(w: &mut W, msg: &T) -> Result<(), AppError>
where
    W: tokio::io::AsyncWrite + Unpin,
    T: Serialize,
{
    use tokio::io::AsyncWriteExt;
    let body = serde_json::to_vec(msg)?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(AppError::new(ErrorCode::ErrPayloadCorrupt, "frame exceeds 1 MB"));
    }
    let mut buf = Vec::with_capacity(4 + body.len());
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
    buf.extend_from_slice(&body);
    w.write_all(&buf).await?;
    w.flush().await?;
    Ok(())
}

pub async fn read_frame_async<R, T>(r: &mut R) -> Result<Option<T>, AppError>
where
    R: tokio::io::AsyncRead + Unpin,
    T: for<'de> Deserialize<'de>,
{
    use tokio::io::AsyncReadExt;
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let n = u32::from_le_bytes(len) as usize;
    if n > MAX_FRAME_BYTES {
        return Err(AppError::new(ErrorCode::ErrPayloadCorrupt, "frame exceeds 1 MB"));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).await?;
    Ok(Some(serde_json::from_slice(&buf)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_envelope_matches_spec() {
        let e = AppError::new(ErrorCode::ErrRangeNotSupported, "no ranges");
        let v = serde_json::to_value(Response::error("req_1", &e)).unwrap();
        assert_eq!(v["status"], "error");
        assert_eq!(v["id"], "req_1");
        assert_eq!(v["error"]["code"], "ERR_RANGE_NOT_SUPPORTED");
    }

    #[test]
    fn ok_envelope_matches_spec() {
        let v = serde_json::to_value(Response::ok("req_2", serde_json::json!({"state":"queued"}))).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["data"]["state"], "queued");
    }

    #[test]
    fn serde_name_equals_as_str() {
        let v = serde_json::to_value(ErrorCode::ErrChecksumSizeMismatch).unwrap();
        assert_eq!(v, "ERR_CHECKSUM_SIZE_MISMATCH");
        assert_eq!(ErrorCode::ErrChecksumSizeMismatch.as_str(), "ERR_CHECKSUM_SIZE_MISMATCH");
    }

    #[test]
    fn frame_roundtrip_and_prefix_is_little_endian() {
        let req = Request { id: "req_3".into(), command: "download.list".into(), payload: serde_json::json!({}) };
        let mut buf = Vec::new();
        write_frame(&mut buf, &req).unwrap();
        let len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
        assert_eq!(len, buf.len() - 4);
        let back: Request = read_frame(&mut buf.as_slice()).unwrap().unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn oversized_frame_rejected() {
        let mut data = ((MAX_FRAME_BYTES + 1) as u32).to_le_bytes().to_vec();
        data.extend_from_slice(b"{}");
        let r: Result<Option<Request>, _> = read_frame(&mut data.as_slice());
        assert_eq!(r.unwrap_err().code, ErrorCode::ErrPayloadCorrupt);
    }

    #[test]
    fn clean_eof_is_none() {
        let r: Option<Request> = read_frame(&mut [].as_slice()).unwrap();
        assert!(r.is_none());
    }
}
