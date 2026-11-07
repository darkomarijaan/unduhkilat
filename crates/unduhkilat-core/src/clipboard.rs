//! FR-040: clipboard URL monitoring through `AddClipboardFormatListener`.
//!
//! The listener never starts a transfer. It only hands a candidate URL to the UI,
//! which shows the ordinary Add URL dialog prefilled for explicit confirmation.
//! Only text that is, in its entirety, one http/https/ftp URL qualifies; prose that
//! merely contains a link is ignored so ordinary copying stays unobtrusive.

/// The URL if `text` is exactly one http/https/ftp URL.
pub fn extract_url(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || t.len() > 2048 || t.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let u = url::Url::parse(t).ok()?;
    if !matches!(u.scheme(), "http" | "https" | "ftp") || u.host_str().is_none_or(|h| h.is_empty()) {
        return None;
    }
    Some(t.to_string())
}

#[cfg(windows)]
pub use win::start;

#[cfg(windows)]
mod win {
    use super::extract_url;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::DataExchange::{AddClipboardFormatListener, CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW, TranslateMessage, HWND_MESSAGE, MSG, WM_CLIPBOARDUPDATE, WNDCLASSW,
    };

    const CF_UNICODETEXT: u32 = 13;
    /// More than any URL we accept; bounds the copy out of foreign memory.
    const MAX_CHARS: usize = 4096;

    struct State {
        tx: Sender<String>,
        enabled: Arc<AtomicBool>,
        wake: Arc<dyn Fn() + Send + Sync>,
        last: String,
    }

    static STATE: OnceLock<Mutex<State>> = OnceLock::new();

    fn read_text() -> Option<String> {
        // Another process may hold the clipboard for a moment after it changes.
        for _ in 0..8 {
            // SAFETY: plain Win32 clipboard calls; every handle is checked and released here.
            unsafe {
                if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
                    return None;
                }
                if OpenClipboard(std::ptr::null_mut()) != 0 {
                    let h = GetClipboardData(CF_UNICODETEXT);
                    let mut out = None;
                    if !h.is_null() {
                        let p = GlobalLock(h) as *const u16;
                        if !p.is_null() {
                            let mut n = 0;
                            while n < MAX_CHARS && *p.add(n) != 0 {
                                n += 1;
                            }
                            out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)));
                            GlobalUnlock(h);
                        }
                    }
                    CloseClipboard();
                    return out;
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        None
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_CLIPBOARDUPDATE {
            if let Some(st) = STATE.get() {
                let enabled = st.lock().map(|s| s.enabled.load(Ordering::Relaxed)).unwrap_or(false);
                if enabled {
                    if let Some(url) = read_text().as_deref().and_then(extract_url) {
                        if let Ok(mut s) = st.lock() {
                            // Copying the same link twice in a row is one prompt, not two.
                            if s.last != url {
                                s.last = url.clone();
                                if s.tx.send(url).is_ok() {
                                    (s.wake)();
                                }
                            }
                        }
                    }
                }
            }
            return 0;
        }
        // SAFETY: forwarding the unchanged arguments of this window procedure.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    /// Start the listener thread (call once). `enabled` can be flipped at any time.
    pub fn start(tx: Sender<String>, enabled: Arc<AtomicBool>, wake: Arc<dyn Fn() + Send + Sync>) {
        if STATE.set(Mutex::new(State { tx, enabled, wake, last: String::new() })).is_err() {
            return;
        }
        std::thread::Builder::new()
            .name("clipboard-listener".into())
            .spawn(|| unsafe {
                let class: Vec<u16> = "UnduhKilatClipboard\0".encode_utf16().collect();
                let hinst = GetModuleHandleW(std::ptr::null());
                let mut wc: WNDCLASSW = std::mem::zeroed();
                wc.lpfnWndProc = Some(wndproc);
                wc.hInstance = hinst;
                wc.lpszClassName = class.as_ptr();
                if RegisterClassW(&wc) == 0 {
                    tracing::warn!("clipboard monitor: cannot register window class");
                    return;
                }
                let hwnd = CreateWindowExW(0, class.as_ptr(), class.as_ptr(), 0, 0, 0, 0, 0, HWND_MESSAGE, std::ptr::null_mut(), hinst, std::ptr::null());
                if hwnd.is_null() || AddClipboardFormatListener(hwnd) == 0 {
                    tracing::warn!("clipboard monitor: cannot attach to the clipboard");
                    return;
                }
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            })
            .ok();
    }
}

#[cfg(test)]
mod tests {
    use super::extract_url;

    #[test]
    fn accepts_a_lone_url() {
        assert_eq!(extract_url("  https://example.com/a.zip \r\n").as_deref(), Some("https://example.com/a.zip"));
        assert!(extract_url("ftp://ftp.gnu.org/gnu/hello/hello-2.12.1.tar.gz").is_some());
        assert!(extract_url("http://127.0.0.1:8080/x?y=1#z").is_some());
    }

    #[test]
    fn rejects_everything_else() {
        for t in [
            "",
            "hello world https://example.com",
            "https://example.com and more",
            "javascript:alert(1)",
            "file:///C:/secret.txt",
            "magnet:?xt=urn:btih:abc",
            "https://",
            "example.com/file.zip",
            "https://a.com/x\nhttps://b.com/y",
        ] {
            assert_eq!(extract_url(t), None, "{t:?}");
        }
        assert_eq!(extract_url(&format!("https://a.com/{}", "x".repeat(3000))), None);
    }
}
