//! A running terminal: a byte source feeding an emulator, with replies, clipboard requests, and
//! chunked pasting. The UI only ever borrows the emulator briefly (`with_engine`).

use crate::term::engine::{TermEngine, TermEvent};
use crate::term::paste::plan_paste;
use crate::term::source::{SourceControl, SourceEvent, SourceIo};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Most bytes the emulator processes while holding its lock. Bigger inputs are fed in slices so the UI
/// can take the lock in between.
const SLICE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Running,
    /// The program or connection ended, with its exit status when known.
    Ended(Option<i32>),
}

struct Inner {
    engine: Mutex<TermEngine>,
    /// Held for the whole of one paste so pastes are sent one after another.
    paste_gate: tokio::sync::Mutex<()>,
    control: Arc<dyn SourceControl>,
    status: Mutex<SessionStatus>,
    title: Mutex<Option<String>>,
    /// Folder the shell last reported (OSC 9;9 / OSC 7).
    cwd: Mutex<Option<String>>,
    clipboard: Mutex<Vec<String>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

#[derive(Clone)]
pub struct TermSession {
    inner: Arc<Inner>,
    rt: tokio::runtime::Handle,
}

impl TermSession {
    /// Start reading from `io`. `wake` is called (from a background task) whenever there is something new
    /// to draw, so the UI can request a repaint.
    pub fn start(io: SourceIo, cols: u16, rows: u16, scrollback: usize, wake: Arc<dyn Fn() + Send + Sync>, rt: &tokio::runtime::Handle) -> TermSession {
        let SourceIo { mut rx, control } = io;
        let inner = Arc::new(Inner {
            engine: Mutex::new(TermEngine::new(cols, rows, scrollback)),
            paste_gate: tokio::sync::Mutex::new(()),
            control,
            status: Mutex::new(SessionStatus::Running),
            title: Mutex::new(None),
            cwd: Mutex::new(None),
            clipboard: Mutex::new(Vec::new()),
            wake,
        });
        let task = inner.clone();
        rt.spawn(async move {
            while let Some(ev) = rx.recv().await {
                match ev {
                    SourceEvent::Data(bytes) => {
                        for slice in bytes.chunks(SLICE_BYTES) {
                            let events = task.engine.lock().unwrap().feed(slice);
                            for e in events {
                                match e {
                                    TermEvent::PtyWrite(reply) => {
                                        let _ = task.control.write(&reply);
                                    }
                                    TermEvent::ClipboardStore(text) => task.clipboard.lock().unwrap().push(text),
                                    TermEvent::Title(t) => *task.title.lock().unwrap() = t,
                                    TermEvent::Cwd(dir) => *task.cwd.lock().unwrap() = Some(dir),
                                    TermEvent::Bell => {}
                                }
                            }
                            (task.wake)();
                            tokio::task::yield_now().await; // let the UI and other tasks have the lock
                        }
                    }
                    SourceEvent::Closed(code) => {
                        *task.status.lock().unwrap() = SessionStatus::Ended(code);
                        (task.wake)();
                        return;
                    }
                }
            }
            // The source went away without saying so.
            *task.status.lock().unwrap() = SessionStatus::Ended(None);
            (task.wake)();
        });
        TermSession { inner, rt: rt.clone() }
    }

    /// Borrow the emulator. Keep it short: the background task needs the same lock.
    pub fn with_engine<R>(&self, f: impl FnOnce(&mut TermEngine) -> R) -> R {
        f(&mut self.inner.engine.lock().unwrap())
    }

    /// Send input to the program (keys, mouse reports, focus events).
    pub fn send(&self, bytes: Vec<u8>) {
        let _ = self.inner.control.write(&bytes);
    }

    /// Whether pasting `text` should first ask the user (several lines, program without bracketed paste).
    pub fn paste_needs_confirm(&self, text: &str) -> bool {
        let bracketed = self.with_engine(|e| e.modes().bracketed_paste);
        plan_paste(text, bracketed).needs_confirm
    }

    /// Paste `text` (already confirmed if needed). Returns at once; the text is sent in small pieces.
    pub fn paste(&self, text: String) {
        let bracketed = self.with_engine(|e| e.modes().bracketed_paste);
        let plan = plan_paste(&text, bracketed);
        let inner = self.inner.clone();
        self.rt.spawn(async move {
            // One paste at a time: a second one waits, so its chunks never land inside the first's
            // bracketed region (where its line breaks would run as commands).
            let _turn = inner.paste_gate.lock().await;
            let control = inner.control.clone();
            for chunk in plan.chunks {
                if control.write(&chunk).is_err() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        self.with_engine(|e| e.resize(cols, rows));
        let _ = self.inner.control.resize(cols, rows);
    }

    pub fn title(&self) -> Option<String> {
        self.inner.title.lock().unwrap().clone()
    }

    /// The folder the shell last reported, when it reports one (local shells started by `spawn_local`).
    pub fn cwd(&self) -> Option<String> {
        self.inner.cwd.lock().unwrap().clone()
    }

    /// Text programs asked to put in the clipboard (OSC 52). Returns each request once.
    pub fn take_clipboard(&self) -> Vec<String> {
        std::mem::take(&mut *self.inner.clipboard.lock().unwrap())
    }

    pub fn status(&self) -> SessionStatus {
        self.inner.status.lock().unwrap().clone()
    }

    /// Stop the program / close the connection. The status changes to `Ended` when the source confirms.
    pub fn close(&self) {
        self.inner.control.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result;
    use crate::term::source::{SourceControl, SourceEvent, SourceIo};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

    #[derive(Default)]
    struct FakeControl {
        writes: Mutex<Vec<u8>>,
        resizes: Mutex<Vec<(u16, u16)>>,
        closed: AtomicBool,
    }
    impl SourceControl for FakeControl {
        fn write(&self, bytes: &[u8]) -> Result<()> {
            self.writes.lock().unwrap().extend_from_slice(bytes);
            Ok(())
        }
        fn resize(&self, cols: u16, rows: u16) -> Result<()> {
            self.resizes.lock().unwrap().push((cols, rows));
            Ok(())
        }
        fn close(&self) {
            self.closed.store(true, Ordering::SeqCst);
        }
    }

    struct Rig {
        tx: UnboundedSender<SourceEvent>,
        control: Arc<FakeControl>,
        wakes: Arc<AtomicUsize>,
        session: TermSession,
    }

    fn rig(cols: u16, rows: u16, scrollback: usize) -> Rig {
        let (tx, rx) = unbounded_channel();
        let control = Arc::new(FakeControl::default());
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        let io = SourceIo { rx, control: control.clone() };
        let session = TermSession::start(io, cols, rows, scrollback, Arc::new(move || { w.fetch_add(1, Ordering::SeqCst); }), &tokio::runtime::Handle::current());
        Rig { tx, control, wakes, session }
    }

    async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
        for _ in 0..200 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("never happened: {what}");
    }

    fn row0(s: &TermSession) -> String {
        s.with_engine(|e| (0..e.size().0).map(|c| e.cell(0, c).ch).collect::<String>().trim_end().to_string())
    }

    #[tokio::test]
    async fn output_reaches_the_screen_and_wakes_the_ui() {
        let r = rig(80, 24, 100);
        r.tx.send(SourceEvent::Data(b"hi there".to_vec())).unwrap();
        eventually("text on screen", || row0(&r.session) == "hi there").await;
        assert!(r.wakes.load(Ordering::SeqCst) >= 1, "the UI is asked to repaint");
        assert_eq!(r.session.status(), SessionStatus::Running);
    }

    #[tokio::test]
    async fn emulator_replies_go_back_to_the_source() {
        let r = rig(80, 24, 100);
        r.tx.send(SourceEvent::Data(b"\x1b[c".to_vec())).unwrap();
        eventually("device attributes reply", || r.control.writes.lock().unwrap().starts_with(b"\x1b[?")).await;
        r.control.writes.lock().unwrap().clear();
        // ConPTY waits for this answer before it prints anything.
        r.tx.send(SourceEvent::Data(b"\x1b[6n".to_vec())).unwrap();
        eventually("cursor position reply", || r.control.writes.lock().unwrap().as_slice() == b"\x1b[1;1R").await;
    }

    #[tokio::test]
    async fn osc52_is_queued_for_the_clipboard() {
        let r = rig(80, 24, 100);
        r.tx.send(SourceEvent::Data(b"\x1b]52;c;SGVsbG8=\x07".to_vec())).unwrap();
        eventually("clipboard request", || !r.session.take_clipboard().is_empty()).await;
        assert!(r.session.take_clipboard().is_empty(), "taking drains the queue");
    }

    #[tokio::test]
    async fn title_is_tracked() {
        let r = rig(80, 24, 100);
        r.tx.send(SourceEvent::Data(b"\x1b]0;mijn titel\x07".to_vec())).unwrap();
        eventually("title", || r.session.title().as_deref() == Some("mijn titel")).await;
        r.tx.send(SourceEvent::Data(b"\x1b]9;9;D:\\Werk\x1b\\".to_vec())).unwrap();
        eventually("cwd", || r.session.cwd().as_deref() == Some(r"D:\Werk")).await;
    }

    #[tokio::test]
    async fn closed_source_sets_ended_status_with_exit_code() {
        let r = rig(80, 24, 100);
        r.tx.send(SourceEvent::Closed(Some(3))).unwrap();
        eventually("ended", || r.session.status() == SessionStatus::Ended(Some(3))).await;
    }

    #[tokio::test]
    async fn a_dropped_source_ends_the_session_too() {
        let r = rig(80, 24, 100);
        let Rig { tx, session, .. } = r;
        drop(tx);
        eventually("ended", || session.status() == SessionStatus::Ended(None)).await;
    }

    #[tokio::test]
    async fn paste_is_chunked_and_wrapped_when_bracketed() {
        let r = rig(80, 24, 100);
        r.tx.send(SourceEvent::Data(b"\x1b[?2004h".to_vec())).unwrap();
        eventually("bracketed mode on", || r.session.with_engine(|e| e.modes().bracketed_paste)).await;
        let text = "abcdefghij".repeat(100_000); // 1 MB
        r.session.paste(text.clone());
        eventually("all of it was sent", || r.control.writes.lock().unwrap().len() >= text.len() + 12).await;
        let sent = r.control.writes.lock().unwrap().clone();
        assert!(sent.starts_with(b"\x1b[200~") && sent.ends_with(b"\x1b[201~"));
        assert_eq!(&sent[6..sent.len() - 6], text.as_bytes());
    }

    #[tokio::test]
    async fn paste_confirmation_follows_the_program_s_mode() {
        let r = rig(80, 24, 100);
        assert!(r.session.paste_needs_confirm("a\nb"));
        assert!(!r.session.paste_needs_confirm("one line"));
        r.tx.send(SourceEvent::Data(b"\x1b[?2004h".to_vec())).unwrap();
        eventually("bracketed mode on", || r.session.with_engine(|e| e.modes().bracketed_paste)).await;
        assert!(!r.session.paste_needs_confirm("a\nb"), "bracketed paste is safe for the program");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ui_lock_is_never_held_long_during_a_flood() {
        let r = rig(80, 24, 2000);
        let block: Vec<u8> = (0..4000).flat_map(|i| format!("\x1b[3{}mline {i} lorem ipsum dolor sit amet\x1b[0m\r\n", i % 7 + 1).into_bytes()).collect();
        // A thread that behaves like the UI: it keeps borrowing the emulator and records the worst wait.
        let done = Arc::new(AtomicBool::new(false));
        let (d2, s2) = (done.clone(), r.session.clone());
        let sampler = std::thread::spawn(move || {
            let mut worst = Duration::ZERO;
            while !d2.load(Ordering::SeqCst) {
                let t = Instant::now();
                s2.with_engine(|e| {
                    let _ = e.cell(0, 0);
                });
                worst = worst.max(t.elapsed());
                std::thread::sleep(Duration::from_millis(2));
            }
            worst
        });
        let mut sent = 0usize;
        while sent < 50 * 1024 * 1024 {
            r.tx.send(SourceEvent::Data(block.clone())).unwrap();
            sent += block.len();
            tokio::task::yield_now().await;
        }
        r.tx.send(SourceEvent::Data(b"\x1b]0;DONE\x07".to_vec())).unwrap();
        for _ in 0..2400 {
            if r.session.title().as_deref() == Some("DONE") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        done.store(true, Ordering::SeqCst);
        let worst = sampler.join().unwrap();
        assert_eq!(r.session.title().as_deref(), Some("DONE"), "the emulator got through the whole flood");
        assert!(worst < Duration::from_millis(50), "the UI waited {worst:?} for the emulator lock");
        assert!(r.session.with_engine(|e| e.history_len()) <= 2000, "history stays bounded");
    }

    #[tokio::test]
    async fn resize_updates_engine_and_source() {
        let r = rig(80, 24, 100);
        r.session.resize(100, 30);
        assert_eq!(r.session.with_engine(|e| e.size()), (100, 30));
        r.session.resize(0, 0);
        assert_eq!(r.session.with_engine(|e| e.size()), (1, 1));
        assert_eq!(r.control.resizes.lock().unwrap().as_slice(), [(100, 30), (1, 1)]);
    }

    #[tokio::test]
    async fn close_stops_the_source() {
        let r = rig(80, 24, 100);
        r.session.close();
        assert!(r.control.closed.load(Ordering::SeqCst));
        r.tx.send(SourceEvent::Closed(None)).unwrap();
        eventually("ended", || r.session.status() == SessionStatus::Ended(None)).await;
    }

    #[tokio::test]
    async fn two_pastes_never_interleave() {
        let r = rig(80, 24, 100);
        r.session.paste("a".repeat(20_000));
        r.session.paste("b".repeat(20_000));
        eventually("both pastes sent", || r.control.writes.lock().unwrap().len() >= 40_000).await;
        let sent = r.control.writes.lock().unwrap().clone();
        let last_a = sent.iter().rposition(|&c| c == b'a').unwrap();
        let first_b = sent.iter().position(|&c| c == b'b').unwrap();
        assert!(last_a < first_b, "the second paste waits for the first (a ends at {last_a}, b starts at {first_b})");
    }
}
