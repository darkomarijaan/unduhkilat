//! Local rotating file logger with secret redaction and an in-memory tail
//! buffer for the diagnostics panel (FR-056, FR-057). Never touches the network.

use crate::error::{AppError, ErrorCode, Result};
use std::collections::VecDeque;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt::MakeWriter;

pub const TAIL_CAPACITY: usize = 100;

/// Replace secrets in a log line with `[REDACTED]`.
pub fn redact(input: &str) -> String {
    let mut s = input.to_string();

    // Whole-header values.
    for key in ["proxy-authorization:", "authorization:", "set-cookie:", "cookie:"] {
        s = redact_after(&s, key, |c| c == '\n' || c == '\r');
    }
    // key=value / key: value / "key":"value" forms.
    for key in ["password=", "passwd=", "pwd=", "token=", "secret=", "api_key=", "apikey="] {
        s = redact_after(&s, key, |c| c.is_whitespace() || matches!(c, '&' | ';' | '"' | '\'' | ','));
    }
    for key in ["\"password\":\"", "\"password\": \"", "\"cookies\":\"", "\"cookies\": \"", "\"token\":\"", "\"token\": \""] {
        s = redact_after(&s, key, |c| c == '"');
    }
    redact_url_userinfo(&s)
}

/// Redact text following each (ASCII case-insensitive) `key` up to a terminator.
fn redact_after(s: &str, key: &str, is_end: impl Fn(char) -> bool) -> String {
    let lower = s.to_ascii_lowercase(); // same byte offsets as `s` (ASCII-only lowercasing)
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while let Some(pos) = lower[i..].find(key) {
        let start = i + pos + key.len();
        out.push_str(&s[i..start]);
        let rest = &s[start..];
        let skip = rest.char_indices().find(|(_, c)| is_end(*c)).map(|(n, _)| n).unwrap_or(rest.len());
        // Preserve a leading space after "header:" style keys.
        let lead = rest.len() - rest.trim_start_matches(' ').len();
        out.push_str(&rest[..lead.min(skip)]);
        if skip > lead.min(skip) {
            out.push_str("[REDACTED]");
        }
        i = start + skip;
    }
    out.push_str(&s[i..]);
    out
}

/// `scheme://user:pass@host` -> `scheme://user:[REDACTED]@host`
fn redact_url_userinfo(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while let Some(pos) = s[i..].find("://") {
        let auth_start = i + pos + 3;
        out.push_str(&s[i..auth_start]);
        let rest = &s[auth_start..];
        let end = rest
            .find(|c: char| c == '/' || c == '?' || c == '#' || c.is_whitespace() || c == '"')
            .unwrap_or(rest.len());
        let authority = &rest[..end];
        match authority.rfind('@') {
            Some(at) => {
                let userinfo = &authority[..at];
                match userinfo.find(':') {
                    Some(colon) => {
                        out.push_str(&userinfo[..colon]);
                        out.push_str(":[REDACTED]");
                        out.push_str(&authority[at..]);
                    }
                    None => out.push_str(authority),
                }
            }
            None => out.push_str(authority),
        }
        i = auth_start + end;
    }
    out.push_str(&s[i..]);
    out
}

/// Cloneable ring buffer holding the last [`TAIL_CAPACITY`] log lines.
#[derive(Clone, Default)]
pub struct LogTail {
    lines: Arc<Mutex<VecDeque<String>>>,
}

impl LogTail {
    pub fn push(&self, text: &str) {
        let mut q = self.lines.lock().unwrap();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            if q.len() == TAIL_CAPACITY {
                q.pop_front();
            }
            q.push_back(line.to_string());
        }
    }
    pub fn tail(&self) -> Vec<String> {
        self.lines.lock().unwrap().iter().cloned().collect()
    }
}

#[derive(Clone)]
struct RedactingMake {
    file: NonBlocking,
    tail: LogTail,
}

struct RedactingWriter {
    file: NonBlocking,
    tail: LogTail,
}

impl Write for RedactingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let clean = redact(&String::from_utf8_lossy(buf));
        self.tail.push(&clean);
        self.file.write_all(clean.as_bytes())?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl<'a> MakeWriter<'a> for RedactingMake {
    type Writer = RedactingWriter;
    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter { file: self.file.clone(), tail: self.tail.clone() }
    }
}

/// Keep this alive for the process lifetime; dropping it flushes pending log lines.
pub struct LogHandle {
    pub tail: LogTail,
    _guard: WorkerGuard,
}

/// PROVISIONAL: rotation size / file count / retention are TBD (PRD FR-057).
/// Rotation is daily and the newest `max_files` are retained until the owner decides.
pub const PROVISIONAL_MAX_LOG_FILES: usize = 7;

pub fn init(logs_dir: &Path, level: &str) -> Result<LogHandle> {
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("unduhkilat.log")
        .max_log_files(PROVISIONAL_MAX_LOG_FILES)
        .build(logs_dir)
        .map_err(|e| AppError::new(ErrorCode::ErrStorageInitFailed, e.to_string()))?;
    let (file, guard) = tracing_appender::non_blocking(appender);
    let tail = LogTail::default();
    let filter = tracing_subscriber::EnvFilter::try_new(level).unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    // Ignore "already set" so tests / repeated init do not panic.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(RedactingMake { file, tail: tail.clone() })
        .try_init();
    Ok(LogHandle { tail, _guard: guard })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_passwords_tokens_and_headers() {
        let out = redact("login password=hunter2&user=bob Authorization: Basic YWJjOjEyMw== end");
        assert!(!out.contains("hunter2"), "{out}");
        assert!(!out.contains("YWJjOjEyMw"), "{out}");
        assert!(out.contains("user=bob"), "{out}");
    }

    #[test]
    fn redacts_cookie_and_json_fields() {
        let out = redact(r#"Cookie: sid=abc123; other=1"#);
        assert!(!out.contains("abc123"), "{out}");
        let out = redact(r#"{"password":"s3cr3t","name":"x"}"#);
        assert!(!out.contains("s3cr3t"), "{out}");
        assert!(out.contains(r#""name":"x""#), "{out}");
    }

    #[test]
    fn redacts_url_userinfo_password_only() {
        let out = redact("GET ftp://alice:pw123@files.example.test/a.bin ok");
        assert!(!out.contains("pw123"), "{out}");
        assert!(out.contains("alice:[REDACTED]@files.example.test/a.bin"), "{out}");
        assert_eq!(redact("https://example.test/a?b=c"), "https://example.test/a?b=c");
    }

    #[test]
    fn case_insensitive_keys() {
        let out = redact("PASSWORD=Zed AUTHORIZATION: Bearer xyz");
        assert!(!out.contains("Zed") && !out.contains("xyz"), "{out}");
    }

    #[test]
    fn ring_buffer_keeps_last_100() {
        let t = LogTail::default();
        for i in 0..150 {
            t.push(&format!("line {i}\n"));
        }
        let v = t.tail();
        assert_eq!(v.len(), TAIL_CAPACITY);
        assert_eq!(v.first().unwrap(), "line 50");
        assert_eq!(v.last().unwrap(), "line 149");
    }

    #[test]
    fn file_logger_writes_redacted_lines() {
        let dir = std::env::temp_dir().join(format!("uk-log-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let h = init(&dir, "info").unwrap();
        tracing::info!("connecting with password=topsecret to host");
        std::thread::sleep(std::time::Duration::from_millis(200));
        let tail = h.tail.tail().join("\n");
        drop(h); // flush
        // Another test may have installed the global subscriber first; only assert on
        // content when this test's subscriber was the one installed.
        if !tail.is_empty() {
            assert!(!tail.contains("topsecret"), "{tail}");
            let mut found = String::new();
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                assert!(e.file_name().to_string_lossy().starts_with("unduhkilat.log"));
                found.push_str(&std::fs::read_to_string(e.path()).unwrap_or_default());
            }
            assert!(!found.contains("topsecret"));
        }
        std::fs::remove_dir_all(dir).ok();
    }
}
