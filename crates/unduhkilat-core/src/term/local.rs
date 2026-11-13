//! Local shells through the Windows pseudo console (ConPTY). No SSH, no network.

use crate::error::{AppError, ErrorCode, Result};
use crate::term::source::{SourceControl, SourceEvent, SourceIo};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShellKind {
    PowerShell,
    Cmd,
    GitBash,
    Wsl,
}

impl ShellKind {
    /// Value stored in the settings table.
    pub fn as_setting(&self) -> &'static str {
        match self {
            ShellKind::PowerShell => "powershell",
            ShellKind::Cmd => "cmd",
            ShellKind::GitBash => "gitbash",
            ShellKind::Wsl => "wsl",
        }
    }

    /// PowerShell when the setting is missing or unknown.
    pub fn from_setting(value: Option<&str>) -> ShellKind {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("cmd") => ShellKind::Cmd,
            Some("gitbash") => ShellKind::GitBash,
            Some("wsl") => ShellKind::Wsl,
            _ => ShellKind::PowerShell,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            ShellKind::PowerShell => "PowerShell",
            ShellKind::Cmd => "CMD",
            ShellKind::GitBash => "Git Bash",
            ShellKind::Wsl => "WSL",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShellOptions {
    pub kind: ShellKind,
    /// Starting folder. Must exist.
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
}

fn git_bash_path() -> Option<PathBuf> {
    let candidates = [
        std::env::var_os("ProgramFiles").map(|p| PathBuf::from(p).join("Git").join("bin").join("bash.exe")),
        std::env::var_os("ProgramFiles(x86)").map(|p| PathBuf::from(p).join("Git").join("bin").join("bash.exe")),
        std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Programs").join("Git").join("bin").join("bash.exe")),
    ];
    candidates.into_iter().flatten().find(|p| p.is_file())
}

fn wsl_path() -> Option<PathBuf> {
    std::env::var_os("SystemRoot").map(|p| PathBuf::from(p).join("System32").join("wsl.exe")).filter(|p| p.is_file())
}

/// Shells that can be opened on this computer. PowerShell and CMD always; Git Bash and WSL when found.
pub fn detect_shells() -> Vec<ShellKind> {
    let mut v = vec![ShellKind::PowerShell, ShellKind::Cmd];
    if git_bash_path().is_some() {
        v.push(ShellKind::GitBash);
    }
    if wsl_path().is_some() {
        v.push(ShellKind::Wsl);
    }
    v
}

// Shells report their folder with OSC 9;9 before each prompt, so the app can show where the shell
// is and reopen it there. Nothing in the user's profile is changed.

/// CMD prompt: the folder report, then the user's own prompt (or the default `$P$G`).
pub fn cmd_prompt(user: Option<&str>) -> String {
    let user = user.map(str::trim).filter(|p| !p.is_empty()).unwrap_or("$P$G");
    if user.contains("]9;9;") {
        user.to_string()
    } else {
        format!("$e]9;9;$P$e\\{user}")
    }
}

/// Wraps whatever prompt PowerShell ends up with (after the profile ran) so it reports the folder
/// first. Only file-system locations are reported (not `HKLM:` and the like).
const PS_REPORT_FOLDER: &str = r#"$global:__ukPrompt = $function:prompt
function global:prompt {
  $p = if ($global:__ukPrompt) { & $global:__ukPrompt } else { "PS $($executionContext.SessionState.Path.CurrentLocation)> " }
  $l = $executionContext.SessionState.Path.CurrentLocation
  if ($l.Provider.Name -eq 'FileSystem') { "$([char]27)]9;9;$($l.ProviderPath)$([char]27)\" + $p } else { $p }
}"#;

/// `-EncodedCommand` wants Base64 of UTF-16LE, which avoids every quoting problem.
fn encode_ps(script: &str) -> String {
    use base64::Engine;
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn command_for(kind: ShellKind, cwd: &Path) -> Result<CommandBuilder> {
    let mut cmd = match kind {
        ShellKind::PowerShell => {
            let mut c = CommandBuilder::new("powershell.exe");
            c.args(["-NoLogo", "-NoExit", "-EncodedCommand"]);
            c.arg(encode_ps(PS_REPORT_FOLDER));
            c.cwd(cwd);
            c
        }
        ShellKind::Cmd => {
            let mut c = CommandBuilder::new("cmd.exe");
            c.env("PROMPT", cmd_prompt(std::env::var("PROMPT").ok().as_deref()));
            c.cwd(cwd);
            c
        }
        ShellKind::GitBash => {
            let bash = git_bash_path().ok_or_else(|| AppError::new(ErrorCode::ErrFileNotFound, "Git Bash tidak ditemukan di komputer ini."))?;
            let mut c = CommandBuilder::new(bash);
            c.args(["--login", "-i"]);
            c.env("PROMPT_COMMAND", r#"printf '\e]9;9;%s\e\\' "$(cygpath -w "$PWD")""#);
            c.cwd(cwd);
            c
        }
        ShellKind::Wsl => {
            let wsl = wsl_path().ok_or_else(|| AppError::new(ErrorCode::ErrFileNotFound, "WSL tidak ditemukan di komputer ini."))?;
            let mut c = CommandBuilder::new(wsl);
            c.arg("--cd");
            c.arg(cwd);
            c
        }
    };
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    Ok(cmd)
}

type SharedChild = Mutex<Box<dyn Child + Send + Sync>>;

/// Every shell started by this process, so quitting the app can stop them all (`process::exit` does
/// not run destructors).
fn live() -> &'static Mutex<Vec<Weak<SharedChild>>> {
    static LIVE: OnceLock<Mutex<Vec<Weak<SharedChild>>>> = OnceLock::new();
    LIVE.get_or_init(|| Mutex::new(Vec::new()))
}

/// Stop every local shell this process started. Call before the app exits.
pub fn kill_all_local() {
    let all: Vec<Weak<SharedChild>> = live().lock().unwrap().drain(..).collect();
    for w in all {
        if let Some(c) = w.upgrade() {
            let _ = c.lock().unwrap().kill();
        }
    }
}

struct LocalControl {
    writer: Mutex<Box<dyn Write + Send>>,
    master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    child: Arc<SharedChild>,
}

fn gone() -> AppError {
    AppError::new(ErrorCode::ErrConnectionDropped, "Terminal sudah tertutup.")
}

impl SourceControl for LocalControl {
    fn write(&self, bytes: &[u8]) -> Result<()> {
        let mut w = self.writer.lock().unwrap();
        w.write_all(bytes).and_then(|_| w.flush()).map_err(|_| gone())
    }

    fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        match self.master.lock().unwrap().as_ref() {
            Some(m) => m.resize(PtySize { rows: rows.max(1), cols: cols.max(1), pixel_width: 0, pixel_height: 0 }).map_err(|_| gone()),
            None => Err(gone()),
        }
    }

    fn close(&self) {
        let _ = self.child.lock().unwrap().kill();
    }
}

impl Drop for LocalControl {
    fn drop(&mut self) {
        self.close();
    }
}

/// Start a shell in `opts.cwd` and return its byte stream and controls. Dropping the returned control
/// (or calling `close`) stops the shell.
pub fn spawn_local(opts: ShellOptions) -> Result<SourceIo> {
    if !opts.cwd.is_dir() {
        return Err(AppError::new(ErrorCode::ErrInvalidPath, format!("Folder tidak ditemukan: {}", opts.cwd.display())));
    }
    let size = PtySize { rows: opts.rows.max(1), cols: opts.cols.max(1), pixel_width: 0, pixel_height: 0 };
    let start_failed = |e: &dyn std::fmt::Display| AppError::internal(format!("Tidak bisa menjalankan terminal (butuh Windows 10 versi 1809 atau lebih baru): {e}"));
    let pair = native_pty_system().openpty(size).map_err(|e| start_failed(&e))?;
    let child = pair.slave.spawn_command(command_for(opts.kind, &opts.cwd)?).map_err(|e| start_failed(&e))?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().map_err(|e| start_failed(&e))?;
    let writer = pair.master.take_writer().map_err(|e| start_failed(&e))?;

    let master = Arc::new(Mutex::new(Some(pair.master)));
    let child = Arc::new(Mutex::new(child));
    {
        let mut l = live().lock().unwrap();
        l.retain(|w| w.strong_count() > 0);
        l.push(Arc::downgrade(&child));
    }
    let exit_code: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    // Output: ends when the pseudo console closes.
    let (tx_r, code_r) = (tx.clone(), exit_code.clone());
    std::thread::spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx_r.send(SourceEvent::Data(buf[..n].to_vec())).is_err() {
                        return;
                    }
                }
            }
        }
        // The watcher publishes the exit code just before it closes the console; give it a moment.
        let until = Instant::now() + Duration::from_secs(2);
        let mut code = *code_r.lock().unwrap();
        while code.is_none() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
            code = *code_r.lock().unwrap();
        }
        let _ = tx_r.send(SourceEvent::Closed(code));
    });

    // Exit watcher: ConPTY keeps the output pipe open after the shell exits, so closing it is our job.
    let (child_w, master_w, code_w) = (child.clone(), master.clone(), exit_code);
    std::thread::spawn(move || loop {
        let status = child_w.lock().unwrap().try_wait();
        match status {
            Ok(Some(st)) => {
                *code_w.lock().unwrap() = Some(st.exit_code() as i32);
                std::thread::sleep(Duration::from_millis(200)); // let the last output drain
                master_w.lock().unwrap().take();
                return;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => {
                master_w.lock().unwrap().take();
                return;
            }
        }
    });

    Ok(SourceIo { rx, control: Arc::new(LocalControl { writer: Mutex::new(writer), master, child }) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_setting_round_trips_and_falls_back_to_powershell() {
        for k in [ShellKind::PowerShell, ShellKind::Cmd, ShellKind::GitBash, ShellKind::Wsl] {
            assert_eq!(ShellKind::from_setting(Some(k.as_setting())), k);
        }
        assert_eq!(ShellKind::from_setting(None), ShellKind::PowerShell, "not set");
        assert_eq!(ShellKind::from_setting(Some("")), ShellKind::PowerShell);
        assert_eq!(ShellKind::from_setting(Some("fish")), ShellKind::PowerShell, "unknown value");
        assert_eq!(ShellKind::from_setting(Some("CMD")), ShellKind::Cmd, "case does not matter");
    }

    #[test]
    fn cmd_prompt_reports_the_folder_and_keeps_the_users_prompt() {
        assert_eq!(cmd_prompt(None), "$e]9;9;$P$e\\$P$G");
        assert_eq!(cmd_prompt(Some("  ")), "$e]9;9;$P$e\\$P$G", "blank means default");
        assert_eq!(cmd_prompt(Some("[$T] $P$G")), "$e]9;9;$P$e\\[$T] $P$G");
        let already = "$e]9;9;$P$e\\$P$G";
        assert_eq!(cmd_prompt(Some(already)), already, "not added twice");
    }

    #[test]
    fn powershell_script_is_utf16_base64() {
        use base64::Engine;
        let raw = base64::engine::general_purpose::STANDARD.decode(encode_ps("ab")).unwrap();
        assert_eq!(raw, vec![b'a', 0, b'b', 0]);
    }

    #[test]
    fn labels_are_human_readable() {
        assert_eq!(ShellKind::PowerShell.label(), "PowerShell");
        assert_eq!(ShellKind::GitBash.label(), "Git Bash");
    }
}
