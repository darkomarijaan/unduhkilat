# Klien Server, Bagian 3: Terminal (lokal dan SSH) (Implementation Plan)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Tab terminal di dalam UnduhKilat dengan dua sumber: shell lokal Windows (ConPTY) yang dibuka dari folder, dan shell SSH di server tersimpan, dengan mouse, papan klip, dan syarat Claude Code dari spec.

**Architecture:** Inti (`unduhkilat-core::term`) tidak mengenal egui: pembungkus emulator `alacritty_terminal`, pengodean tombol/mouse/tempel, dua sumber byte (`LocalShell` lewat `portable-pty`, `ShellChannel` lewat kanal `russh`), dan `TermSession` yang menjalankan loop baca-umpan-balas. UI (`unduhkilat-app/src/terminal.rs`, anak modul `app` seperti `servers.rs`) menerjemahkan peristiwa egui menjadi masukan terminal, menggambar grid, dan mengelola tab.

**Tech Stack:** Rust, `alacritty_terminal` 0.26 (diputuskan di purwarupa), `portable-pty` 0.9 (pilihan pembuat), `russh` 0.65, egui/eframe 0.37, server SSH uji dalam-proses.

**Spec:** `docs/superpowers/specs/2026-11-07-server-client-design.md` (bagian 8a, 8b, 14, 5). Catatan purwarupa: `docs/superpowers/spikes/2026-11-07-terminal-egui.md`. Peta: `…-00-overview.md`.

## Global Constraints

- Windows 10/11 x64 saja; portabel; tanpa admin; ConPTY butuh Windows 10 1809+ (bila tidak ada: pesan jelas).
- **Terminal lokal tidak memakai SSH, server, atau jaringan** (permintaan Eko). Sumber SSH hanya untuk tab server.
- Mouse dan papan klip **wajib** (spec 8a): seret pilih, klik dua kali = kata, tiga kali = baris, Shift+klik perluas; roda menggulir riwayat; pelaporan mouse ke program (xterm 1000/1002/1003 + SGR 1006), **Shift** memaksa pilihan teks; `Ctrl+V`, `Shift+Insert`, klik kanan, `Ctrl+Shift+V` menempel; `Ctrl+C` menyalin **hanya bila ada teks terpilih**, jika tidak mengirim `^C`; `Ctrl+Shift+C` dan `Ctrl+Insert` selalu menyalin; menu klik kanan: Salin, Tempel, Pilih semua, Bersihkan layar.
- Syarat Claude Code (spec 8b): truecolor, layar alternatif, bracketed paste, `Shift+Enter`/`Alt+Enter`/`Alt+huruf`/`Esc`, protokol papan ketik kitty (`CSI u`), OSC 52 mengisi papan klip Windows, lebar karakter benar (CJK/emoji), ukuran mengikuti jendela, keluaran deras tanpa membekukan UI.
- Tempel 1 MB tidak boleh membuat UI macet lebih dari 200 ms (kirim berpotong-potong); riwayat gulir 10.000 baris.
- Tombol pintas aplikasi (mis. `Ctrl+N`, `Delete`) **tidak boleh** menelan ketikan saat terminal terfokus.
- Isi sesi terminal tidak pernah masuk log; kredensial tidak pernah masuk log atau galat.
- Semua teks UI dan galat berbahasa Indonesia; uji tidak menyentuh internet (server SSH uji di loopback).
- **Di luar rencana ini:** `Alt+V` tempel gambar (butuh SFTP dari Rencana 2; dikerjakan sesudahnya), pratinjau, penjelajah berkas jarak jauh. Sisi "Berkas" pada tab SSH menampilkan "Segera hadir".
- Satu commit per tugas, pesan commit berbahasa Indonesia; jangan push dan jangan rilis di rencana ini.

## Review Focus

1. Tempel 1 MB yang memuat penutup bracketed-paste `ESC[201~` (penyuntikan), akhir baris CRLF, dan karakter multibita di batas potongan: tidak ada penyuntikan, tidak ada karakter terpotong, UI tidak terkunci lebih dari 200 ms. (Task 4, 7)
2. Banjir keluaran: 50 MB, byte biner/UTF-8 rusak, satu baris tanpa akhir baris, jutaan urutan escape: UI tetap responsif dan memori terbatas (riwayat 10.000 baris; pemrosesan per kunci dibatasi). (Task 1, 7)
3. Ukuran jendela ekstrem (0x0, 1x1, 500x200) dan perubahan ukuran beruntun: tidak panik, ukuran PTY/kanal SSH ikut. (Task 1, 5, 6)
4. Anak proses berakhir sendiri (`exit`) atau sambungan SSH putus: tab menunjukkan "berakhir" dengan kode keluar, tanpa panik atau proses yatim; menutup tab dan menutup aplikasi menghentikan semua anak proses. (Task 5, 6, 9)
5. Masukan: `Ctrl+C` dengan/tanpa seleksi, modifikator tersangkut saat fokus pindah, input Unicode/IME multi-karakter, jalur kerja Windows dengan spasi dan Unicode untuk shell lokal. (Task 5, 8)

---

File yang dibuat/diubah:

- `crates/unduhkilat-core/Cargo.toml` (ubah): `alacritty_terminal = "0.26"`, `portable-pty = "0.9"`.
- `crates/unduhkilat-core/src/term/{mod.rs,engine.rs,keys.rs,mouse.rs,paste.rs,source.rs,local.rs,session.rs}` (baru).
- `crates/unduhkilat-core/src/server/shell.rs` (baru), `server/mod.rs` (ubah).
- `crates/unduhkilat-core/tests/common/ssh_server.rs` (ubah): sesi shell/PTY.
- `crates/unduhkilat-core/tests/{term_local.rs,term_ssh.rs}` (baru).
- `crates/unduhkilat-app/src/terminal.rs` (baru, anak modul `app`), `app.rs` (ubah), `servers.rs` (ubah: tombol Terminal aktif).

### Task 1: Pembungkus emulator `TermEngine`

**Files:** Create `crates/unduhkilat-core/src/term/{mod.rs,engine.rs}`; Modify `lib.rs` (`pub mod term;`), `Cargo.toml`. Test: modul `tests` di `engine.rs`.

**Interfaces:**
- Produces:
  - `struct Rgb(pub u8, pub u8, pub u8)`; `struct CellView { ch: char, fg: Rgb, bg: Rgb, bg_default: bool, bold, italic, underline, strike, dim, wide: bool, spacer: bool }` (warna sudah memperhitungkan *inverse*; `spacer` = sel pendamping karakter lebar).
  - `enum TermEvent { ClipboardStore(String), PtyWrite(Vec<u8>), Title(Option<String>), Bell }`.
  - `enum MouseMode { Off, Click, Drag, Motion }`; `struct TermModes { alt_screen, bracketed_paste, app_cursor, app_keypad, mouse: MouseMode, sgr_mouse, focus_events, kitty_disambiguate, alternate_scroll }` (semua `bool` kecuali `mouse`).
  - `enum SelectKind { Simple, Word, Line }`; `struct CursorView { row: u16, col: u16, shape: CursorShape }`; `enum CursorShape { Block, Underline, Bar }`.
  - `TermEngine::new(cols: u16, rows: u16, scrollback: usize) -> TermEngine`; `feed(&mut self, bytes: &[u8]) -> Vec<TermEvent>`; `resize(&mut self, cols: u16, rows: u16)` (menjepit ke minimal 1x1); `size() -> (u16, u16)`; `modes() -> TermModes`; `cell(&self, row: u16, col: u16) -> CellView` (memperhitungkan `display_offset`); `cursor(&self) -> Option<CursorView>` (None bila disembunyikan atau sedang menggulir); `scroll(&mut self, lines: i32)` (positif = ke riwayat); `scroll_to_bottom`; `display_offset() -> usize`; `history_len() -> usize`; `select_start(&mut self, kind: SelectKind, row: u16, col: u16)`; `select_update(&mut self, row: u16, col: u16)`; `select_clear`; `select_all`; `selected_text(&self) -> Option<String>`; `clear_screen_and_history`.
  - Konfigurasi bawaan: `kitty_keyboard: true`, riwayat sesuai argumen.

- [ ] **Step 1:** Tulis uji (nilai dari fixture purwarupa): `modes_follow_private_mode_sequences` (`?1049h`, `?2004h`, `?1000h`+`?1006h`, `?1004h`, `>1u` mengubah `modes()`; `?…l` membalikkannya); `colors_256_and_truecolor_resolve_to_rgb` (`ESC[38;5;208m` = xterm 208 `(255,135,0)`, `ESC[38;2;18;165;148m` = `(18,165,148)`, `ESC[7m` menukar fg/bg, indeks 0-15 mengikuti palet xterm standar); `wide_chars_use_two_cells` (`你`, `😀`: sel pertama `wide`, sel berikutnya `spacer`); `osc52_becomes_a_clipboard_event` (`ESC]52;c;SGVsbG8=\x07` memberi `ClipboardStore("Hello")`); `device_query_gets_a_reply` (`ESC[c` memberi `PtyWrite` berawal `ESC[?`); `scrollback_is_capped` (20.000 baris dengan riwayat 10.000: `history_len() == 10_000`); `scroll_and_display_offset` (setelah 100 baris, `scroll(10)` menaikkan `display_offset` menjadi 10 dan `cell` membaca baris lama; `scroll_to_bottom` mengembalikan 0); `resize_reflows_and_never_panics` (0x0, 1x1, 500x200, 50 perubahan ukuran beruntun); `selection_text_word_line_and_all` (klik dua kali pada "dunia" dalam `halo dunia` menyalin `dunia`; `Line` menyalin seluruh baris; `select_all` menyalin semua baris tampak); `hostile_input_is_bounded` (1 MB byte acak + satu baris 1 MB tanpa `\n` selesai tanpa panik dan `history_len() <= scrollback`).
- [ ] **Step 2:** Jalankan `cargo test -p unduhkilat-core --lib term::engine` (gagal: modul belum ada).
- [ ] **Step 3:** Implementasikan `TermEngine` di atas `alacritty_terminal::{Term, vte::ansi::Processor}` dengan `EventListener` yang mengumpulkan peristiwa ke antrean internal; palet xterm sendiri untuk `Color::Indexed/Named`.
- [ ] **Step 4:** Jalankan uji (lulus). Commit `feat(term): pembungkus emulator dengan mode, warna, seleksi, dan riwayat`.

### Task 2: Pengodean tombol

**Files:** Create `term/keys.rs`. Test: modul `tests` di `keys.rs`.

**Interfaces:**
- Consumes: `TermModes` (Task 1).
- Produces: `enum Key { Char(char), Enter, Tab, Backspace, Escape, Up, Down, Left, Right, Home, End, PageUp, PageDown, Insert, Delete, F(u8) }`; `struct Mods { ctrl: bool, alt: bool, shift: bool }`; `fn encode_key(key: Key, mods: Mods, modes: &TermModes) -> Vec<u8>`.

- [ ] **Step 1:** Tulis uji dengan byte persis: `plain_keys` (`Enter`→`"\r"`, `Tab`→`"\t"`, `Shift+Tab`→`"\x1b[Z"`, `Backspace`→`"\x7f"`, `Escape`→`"\x1b"`); `ctrl_letters` (`Ctrl+C`→`0x03`, `Ctrl+A`→`0x01`, `Ctrl+[`→`0x1b`, `Ctrl+Space`→`0x00`); `alt_prefixes_escape` (`Alt+x`→`"\x1bx"`, `Alt+Enter`→`"\x1b\r"`); `arrows_normal_and_application` (`Up`→`"\x1b[A"`; dengan `app_cursor`→`"\x1bOA"`; `Ctrl+Left`→`"\x1b[1;5D"`, `Shift+Up`→`"\x1b[1;2A"`, `Alt+Right`→`"\x1b[1;3C"`); `editing_keys` (`Home`→`"\x1b[H"`, `End`→`"\x1b[F"`, `PageUp`→`"\x1b[5~"`, `Delete`→`"\x1b[3~"`, `Insert`→`"\x1b[2~"`); `function_keys` (`F1`→`"\x1bOP"`, `F5`→`"\x1b[15~"`, `F12`→`"\x1b[24~"`, `Shift+F5`→`"\x1b[15;2~"`); `kitty_disambiguates` (dengan `kitty_disambiguate`: `Shift+Enter`→`"\x1b[13;2u"`, `Escape`→`"\x1b[27u"`, `Ctrl+C`→`"\x1b[99;5u"`, `Alt+x`→`"\x1b[120;3u"`; `Enter` tanpa modifikator tetap `"\r"`; teks biasa `Char('a')` tetap `"a"`; tanpa kitty `Shift+Enter`→`"\r"`); `unicode_text` (`Char('é')`, `Char('你')`, `Char('😀')` → UTF-8).
- [ ] **Step 2:** Jalankan `cargo test -p unduhkilat-core --lib term::keys` (gagal).
- [ ] **Step 3:** Implementasikan `encode_key`; modifikator CSI = `1 + shift + 2*alt + 4*ctrl`.
- [ ] **Step 4:** Jalankan uji (lulus). Commit `feat(term): pengodean tombol dengan dukungan kitty`.

### Task 3: Pengodean mouse

**Files:** Create `term/mouse.rs`. Test: modul `tests`.

**Interfaces:**
- Produces: `enum MouseButton { Left, Middle, Right }`; `enum MouseKind { Press(MouseButton), Release(MouseButton), Move { held: Option<MouseButton> }, WheelUp, WheelDown }`; `struct MouseEvent { kind: MouseKind, col: u16, row: u16 /* mulai 0 */, mods: Mods }`; `fn encode_mouse(ev: &MouseEvent, modes: &TermModes) -> Option<Vec<u8>>` (None bila mode tidak melaporkan peristiwa itu); `fn wheel_as_arrows(up: bool, lines: u32, modes: &TermModes) -> Option<Vec<u8>>` (layar alternatif tanpa mouse + `alternate_scroll`).

- [ ] **Step 1:** Uji: `sgr_press_release` (kiri di kolom 10, baris 5: `"\x1b[<0;11;6M"` dan lepas `"\x1b[<0;11;6m"`); `wheel_buttons` (`WheelUp`→`"\x1b[<64;1;1M"`, `WheelDown`→`…65…`); `modifiers_add_bits` (Shift +4, Alt +8, Ctrl +16); `drag_reports_motion_only_in_drag_or_motion_mode` (mode `Click`: `Move` → `None`; `Drag` dengan tombol ditahan → kode +32; `Motion` tanpa tombol → kode 35); `off_mode_reports_nothing`; `legacy_encoding_when_sgr_off` (`"\x1b[M" + (32+b, 32+x+1, 32+y+1)`, kolom > 222 → `None`); `alternate_scroll_arrows` (`wheel_as_arrows(true, 3, app_cursor)` → tiga `"\x1bOA"`).
- [ ] **Step 2:** Jalankan `cargo test -p unduhkilat-core --lib term::mouse` (gagal).
- [ ] **Step 3:** Implementasikan sesuai spesifikasi xterm (kode tombol: kiri 0, tengah 1, kanan 2, lepas 3 pada legacy; gerak +32; roda 64/65).
- [ ] **Step 4:** Jalankan uji (lulus). Commit `feat(term): pengodean mouse xterm dan SGR`.

### Task 4: Logika tempel

**Files:** Create `term/paste.rs`. Test: modul `tests`.

**Interfaces:**
- Produces: `struct PastePlan { chunks: Vec<Vec<u8>>, needs_confirm: bool }`; `fn plan_paste(text: &str, bracketed: bool) -> PastePlan`; `const CHUNK_BYTES: usize = 4096`.
- Aturan: akhir baris `\r\n` dan `\n` menjadi `\r`; semua `ESC` (0x1b) dibuang dari teks yang ditempel; bila `bracketed`, potongan pertama diawali `ESC[200~` dan potongan terakhir diakhiri `ESC[201~`; potongan tidak memotong karakter UTF-8; `needs_confirm` benar bila **tidak** bracketed dan teks memuat lebih dari satu baris.

- [ ] **Step 1:** Uji: `crlf_becomes_cr`; `escape_bytes_are_stripped_so_the_end_marker_cannot_be_injected` (teks `"a\x1b[201~rm -rf"` tidak memuat `0x1b` di luar penanda); `bracketed_wraps_first_and_last_chunk`; `chunks_never_split_utf8` (teks 10.000 × `"é你😀"` dipotong; setiap potongan valid UTF-8 dan gabungannya sama dengan masukan setelah normalisasi); `one_megabyte_is_chunked` (1 MB → jumlah potongan ≥ 256, tiap potongan ≤ `CHUNK_BYTES`+14); `multiline_without_bracketed_needs_confirm` (dan satu baris atau bracketed: tidak).
- [ ] **Step 2:** Jalankan `cargo test -p unduhkilat-core --lib term::paste` (gagal).
- [ ] **Step 3:** Implementasikan `plan_paste`.
- [ ] **Step 4:** Jalankan uji (lulus). Commit `feat(term): rencana tempel aman dan berpotong`.

### Task 5: Shell lokal (ConPTY)

**Files:** Create `term/source.rs`, `term/local.rs`; `tests/term_local.rs`.

**Interfaces:**
- Produces (`source.rs`): `enum SourceEvent { Data(Vec<u8>), Closed(Option<i32>) }`; `trait SourceControl: Send + Sync { fn write(&self, bytes: &[u8]) -> Result<()>; fn resize(&self, cols: u16, rows: u16) -> Result<()>; fn close(&self); }`; `struct SourceIo { pub rx: tokio::sync::mpsc::UnboundedReceiver<SourceEvent>, pub control: Arc<dyn SourceControl> }`.
- Produces (`local.rs`): `enum ShellKind { PowerShell, Cmd, GitBash, Wsl }`; `fn detect_shells() -> Vec<ShellKind>` (PowerShell dan Cmd selalu; Git Bash bila `bash.exe` Git ditemukan; WSL bila `wsl.exe` ada dan punya distribusi); `struct ShellOptions { kind: ShellKind, cwd: PathBuf, cols: u16, rows: u16 }`; `fn spawn_local(opts: ShellOptions) -> Result<SourceIo>`.
- Perilaku: PowerShell = `powershell.exe -NoLogo`; Cmd = `cmd.exe`; Git Bash = `bash.exe --login -i`; WSL = `wsl.exe --cd <cwd>`; cwd harus ada (galat `ErrPathUnwritable`/`ErrInvalidPath` berbahasa Indonesia bila tidak); env `TERM=xterm-256color`, `COLORTERM=truecolor`; pembacaan di thread sendiri; `Closed(Some(kode))` saat proses berakhir; `close()` menghentikan proses; `Drop` menghentikan proses.

- [ ] **Step 1:** Uji (di `tests/term_local.rs`, memakai `cmd.exe` asli): `shell_starts_in_the_requested_folder` (folder sementara berspasi dan Unicode `uk test ü`; tulis `cd\r\n`, keluaran memuat jalur itu); `typed_input_is_echoed_and_output_arrives` (`echo abc123\r\n` → keluaran memuat `abc123`); `resize_reaches_the_shell` (setelah `resize(100, 30)`, `mode con` melaporkan 100 kolom dan 30 baris); `exit_reports_the_exit_code` (`exit 7\r\n` → `Closed(Some(7))` dalam 5 detik); `close_kills_the_process` (proses hilang setelah `close()`); `missing_folder_is_a_clear_error`; `detect_shells_always_has_powershell_and_cmd`; `extreme_sizes_do_not_panic` (0x0 dan 1x1 pada `resize`).
- [ ] **Step 2:** Jalankan `cargo test -p unduhkilat-core --test term_local` (gagal).
- [ ] **Step 3:** Implementasikan dengan `portable_pty::{native_pty_system, PtySize, CommandBuilder}`: `openpty`, `spawn_command` dengan `cwd`, `try_clone_reader` di thread, `take_writer`, `resize`, `Child::kill/wait`.
- [ ] **Step 4:** Jalankan uji (lulus); jalankan sekali lagi untuk menyingkirkan kerapuhan waktu. Commit `feat(term): shell lokal lewat ConPTY dengan folder awal`.

### Task 6: Shell SSH

**Files:** Create `server/shell.rs`; Modify `server/mod.rs`, `tests/common/ssh_server.rs`; Test: `tests/term_ssh.rs`.

**Interfaces:**
- Consumes: `SshSession::handle() -> &Handle<HostKeyHandler>` (Rencana 1), `SourceIo` (Task 5).
- Produces: `pub async fn open_shell(session: &SshSession, cols: u16, rows: u16) -> Result<SourceIo>` (minta PTY `xterm-256color` dengan ukuran itu, lalu shell; `SourceControl::resize` mengirim `window_change`; EOF/tutup menghasilkan `Closed(exit_status)`).
- Harness (`TestSsh`): `TestSshConfig` mendapat `shell: bool` (bawaan `false`); bila `true`, server menerima `pty_request`, `shell_request`, memantulkan (`echo`) semua data masukan, mencatat ukuran PTY terakhir dan jumlah `window_change`, dan menutup kanal bila menerima `exit\n` dengan status 0; metode `TestSsh::pty_size() -> Option<(u16, u16)>`, `TestSsh::resize_count() -> usize`, `TestSsh::close_shells()` (memutus dari sisi server).

- [ ] **Step 1:** Uji (`tests/term_ssh.rs`, memakai `TestSsh` dengan `shell: true` dan `server::ssh::connect`): `shell_echoes_input` (kirim `halo\n`, data balik memuat `halo`); `pty_is_requested_with_the_initial_size` (`pty_size() == Some((120, 40))`); `resize_sends_window_change` (setelah `resize(80, 24)`: `pty_size() == Some((80, 24))` dan `resize_count() == 1`); `remote_close_ends_the_stream` (`close_shells()` → `Closed(_)` dalam 3 detik); `exit_status_is_reported` (`exit\n` → `Closed(Some(0))`); `closing_the_source_closes_the_channel`; `non_shell_server_gives_a_clear_error` (server tanpa `shell`: `open_shell` gagal dengan pesan berbahasa Indonesia, bukan menggantung).
- [ ] **Step 2:** Jalankan `cargo test -p unduhkilat-core --test term_ssh` (gagal).
- [ ] **Step 3:** Perluas harness (handler `pty_request`, `shell_request`, `window_change_request`, `data`, `channel_eof`) dan implementasikan `open_shell` dengan `Channel::request_pty/request_shell/window_change/wait` dan tugas yang meneruskan `ChannelMsg::Data/ExtendedData/Eof/ExitStatus`.
- [ ] **Step 4:** Jalankan uji (lulus) dan seluruh `cargo test -p unduhkilat-core --test servers` (tidak ada regresi). Commit `feat(term): shell SSH berbasis kanal PTY dan perluasan harness`.

### Task 7: Sesi terminal

**Files:** Create `term/session.rs`. Test: modul `tests` (sumber palsu).

**Interfaces:**
- Consumes: `SourceIo`, `TermEngine`, `plan_paste`, `encode_key`, `encode_mouse`.
- Produces: `struct TermSession` (`Clone`, berisi `Arc`); `TermSession::start(io: SourceIo, cols: u16, rows: u16, scrollback: usize, wake: Arc<dyn Fn() + Send + Sync>, rt: &tokio::runtime::Handle) -> TermSession`; `with_engine<R>(&self, f: impl FnOnce(&mut TermEngine) -> R) -> R`; `send(&self, bytes: Vec<u8>)`; `paste(&self, text: String)` (membaca `modes().bracketed_paste`, memakai `plan_paste`, mengirim potongan satu per satu dengan jeda `yield` antar potongan; mengembalikan segera); `resize(&self, cols: u16, rows: u16)`; `title(&self) -> Option<String>`; `take_clipboard(&self) -> Vec<String>`; `status(&self) -> SessionStatus` (`Running | Ended(Option<i32>)`); `close(&self)`.
- Perilaku: loop menerima `SourceEvent::Data`, mengunci mesin, memproses **paling banyak 64 KB per penguncian**, melepas kunci, memanggil `wake`; `TermEvent::PtyWrite` ditulis balik ke sumber; `ClipboardStore` masuk antrean `take_clipboard`; `Closed` mengubah status.

- [ ] **Step 1:** Uji (dengan `SourceIo` palsu dari pasangan kanal): `output_reaches_the_screen_and_wakes_the_ui`; `emulator_replies_go_back_to_the_source` (`ESC[c` masuk → sumber palsu menerima balasan `ESC[?…c`); `osc52_is_queued_for_the_clipboard`; `closed_source_sets_ended_status_with_exit_code`; `paste_is_chunked_and_wrapped_when_bracketed` (program mengaktifkan `?2004h`; tempel 1 MB → sumber menerima urutan potongan dengan `ESC[200~` di awal dan `ESC[201~` di akhir, isi sama); `ui_lock_is_never_held_long_during_a_flood` (kirim 50 MB berisi baris dan urutan escape dari sumber palsu sambil membaca `with_engine` berulang dari utas lain: setiap penguncian selesai < 50 ms dan `history_len() <= scrollback`); `resize_updates_engine_and_source` (kedua sisi menerima ukuran baru, ukuran ekstrem tidak panik); `close_stops_the_loop`.
- [ ] **Step 2:** Jalankan `cargo test -p unduhkilat-core --lib term::session` (gagal).
- [ ] **Step 3:** Implementasikan dengan satu tugas tokio per sesi; `Mutex<TermEngine>` dan antrean klip di `Arc`.
- [ ] **Step 4:** Jalankan uji (lulus) dan `cargo test --workspace`. Commit `feat(term): sesi terminal dengan pemrosesan terbatas dan tempel berpotong`.

### Task 8: Terjemahan masukan egui dan tampilan terminal

**Files:** Create `crates/unduhkilat-app/src/terminal.rs` (bagian terjemahan dan tampilan); Modify `app.rs` (`#[path = "terminal.rs"] mod terminal;`). Test: modul `tests` di `terminal.rs`.

**Interfaces:**
- Consumes: `encode_key`, `encode_mouse`, `TermSession`, `TermModes`.
- Produces: `enum TermInput { Bytes(Vec<u8>), Paste(String), Copy, PasteRequest, SelectAll }`; `fn translate_events(events: &[egui::Event], modes: &TermModes, has_selection: bool, ctrl_v_pastes: bool) -> Vec<TermInput>`; `struct TerminalView` dengan `fn show(&mut self, ui: &mut egui::Ui, session: &TermSession, font_size: f32) -> TerminalResponse` (menggambar grid, kursor, seleksi; menangani fokus, roda, klik, seret, menu klik kanan, perubahan ukuran sel→baris/kolom, dan mengirim masukan); `fn cell_metrics(ui: &egui::Ui, font_size: f32) -> (f32, f32)`.
- Aturan terjemahan: `Event::Text` dikirim sebagai UTF-8 (satu atau banyak karakter, termasuk emoji); `Event::Key` hanya diterjemahkan bila tombolnya bukan penghasil teks **atau** ada `ctrl`/`alt`; `Event::Paste(t)` → `TermInput::Paste` (egui memberi ini untuk `Ctrl+V`/`Shift+Insert`); `Event::Copy` → `Copy` bila ada seleksi, jika tidak `Bytes([0x03])`; `Ctrl+Shift+C` selalu `Copy`; `Ctrl+Insert` `Copy`; `Ctrl+Shift+V` `PasteRequest`; bila `ctrl_v_pastes == false`, `Ctrl+V` dikirim sebagai `0x16`.

- [ ] **Step 1:** Uji terjemahan dengan `egui::Event` buatan: `text_and_emoji_become_utf8_bytes`; `printable_key_events_are_not_sent_twice` (Key `A` + Text `a` → satu `"a"`); `ctrl_c_copies_only_with_a_selection` (tanpa seleksi → `Bytes([3])`); `ctrl_shift_c_always_copies`; `paste_event_becomes_paste`; `ctrl_v_can_be_given_to_the_program` (`ctrl_v_pastes == false` → `Bytes([0x16])`); `alt_letter_is_escape_prefixed`; `shift_enter_follows_kitty_mode` (kitty aktif → `"\x1b[13;2u"`, tidak → `"\r"`); `arrows_follow_application_cursor_mode`; `app_shortcuts_are_not_consumed` (fungsi memberi tahu lewat nilai kembali bahwa tombol dikonsumsi oleh terminal: `Ctrl+N` dan `Delete` ada dalam daftar `consumed_keys` saat terminal terfokus).
- [ ] **Step 2:** Jalankan `cargo test -p unduhkilat-app terminal::` (gagal).
- [ ] **Step 3:** Implementasikan `translate_events` dan `TerminalView::show`: satu `LayoutJob` per baris dengan rentang warna sama; latar per rentang; kursor; seleksi; sel lebar menempati dua sel; ukuran grid dari `cell_metrics` dan area yang tersedia; roda menggulir riwayat atau melaporkan ke program sesuai `modes`; **Shift** menahan pelaporan mouse; klik dua/tiga kali memilih kata/baris; teks yang disalin lewat `ctx.copy_text`; OSC 52 dari `take_clipboard` juga lewat `ctx.copy_text`; penempelan multibaris tanpa bracketed memunculkan dialog konfirmasi "Tempel N baris?".
- [ ] **Step 4:** Jalankan uji (lulus). Verifikasi tampilan di Task 9 (bergantung pada layar). Commit `feat(terminal): terjemahan masukan egui dan tampilan terminal`.

### Task 9: Layar Terminal, tab lokal, dan pengaturan

**Files:** Modify `crates/unduhkilat-app/src/terminal.rs`, `app.rs` (`Screen::Terminal`, entri sidebar "Terminal" dengan jumlah tab, pengaturan, kait `UNDUHKILAT_START_SCREEN=terminal`), `crates/unduhkilat-core/src/config.rs` (kunci pengaturan).

**Interfaces:**
- Produces: `keys::TERMINAL_SHELL`, `TERMINAL_START_DIR`, `TERMINAL_FONT_SIZE`, `TERMINAL_CTRL_V_PASTES`, `TERMINAL_LAYOUT` (`"a" | "b"`) di `config::keys`; `struct TerminalState { tabs: Vec<TermTab>, active: usize, ... }`; `struct TermTab { title: String, kind: TabKind /* Local | Ssh(server_id) */, session: Option<TermSession>, status: TabStatus /* Connecting | Running | Ended(Option<i32>) | Failed(String) */, view: TerminalView }`; `impl App { fn open_local_terminal(&mut self, cwd: PathBuf, kind: Option<ShellKind>); fn terminal_screen(&mut self, ui: &mut Ui); }`.
- Perilaku: deretan tab dengan lencana LOKAL/SSH dan tombol tutup; tombol "+" membuka menu (shell yang terdeteksi, folder Unduhan); sakelar `Terminal | Berkas` (tata letak A) dan pembagian berdampingan (tata letak B) sesuai pengaturan `terminal_layout` dengan tombol cepat; "Berkas" pada tab LOKAL menampilkan isi folder kerja (daftar sederhana dengan tombol "Buka di terminal"); pada tab SSH menampilkan "Segera hadir"; menutup tab yang sesi-nya berjalan meminta konfirmasi; tab yang berakhir menampilkan "Berakhir (kode N)" dengan tombol "Mulai ulang" dan "Tutup"; menutup aplikasi menutup semua sesi (anak proses berhenti); tombol pintas aplikasi tidak aktif saat terminal terfokus; Pengaturan mendapat tab "Terminal" (shell bawaan, folder awal, ukuran huruf, `Ctrl+V` menempel, tata letak).
- Tidak ada uji otomatis untuk gambar egui; verifikasi dengan peluncuran dan tangkapan layar (praktik proyek).

- [ ] **Step 1:** Tambahkan kunci pengaturan dan uji kecil di `config.rs`/`terminal.rs`: `layout_setting_parses_and_defaults_to_a` dan `shell_setting_falls_back_to_powershell_when_missing`.
- [ ] **Step 2:** Jalankan `cargo test -p unduhkilat-app terminal::` (gagal pada dua uji baru).
- [ ] **Step 3:** Implementasikan layar, tab, dialog konfirmasi tutup, pengaturan, dan tata letak A/B; jalankan dengan `UNDUHKILAT_START_SCREEN=terminal`.
- [ ] **Step 4:** Verifikasi manual dengan tangkapan layar pada jendela 1280x800 dan jendela pendek: tab lokal berjalan (`ls`, `claude --version` bila ada), mouse memilih teks, roda menggulir, `Ctrl+V` menempel, menutup tab menghentikan proses (`Get-Process` untuk memastikan tidak ada `powershell.exe` yatim). Catat hasil di pesan commit.
- [ ] **Step 5:** Jalankan uji (lulus) dan `cargo test --workspace`. Commit `feat(terminal): layar Terminal, tab lokal, tata letak A/B, dan pengaturan`.

### Task 10: Titik masuk: dari folder dan dari server

**Files:** Modify `crates/unduhkilat-app/src/app.rs` (kartu unduhan dan menu konteks), `servers.rs` (tombol Terminal aktif), `terminal.rs`.

**Interfaces:**
- Consumes: `open_local_terminal`, `ServerService`, `open_shell`, `UiInteract` (dialog kunci host dan kredensial dari Rencana 1).
- Produces: `Action::Terminal` pada kartu unduhan (membuka tab lokal di folder tujuan: `parent` untuk berkas tunggal, folder itu sendiri untuk torrent multi-file; bila folder tidak ada, pakai folder terdekat yang ada seperti `reveal_in_explorer`); pada kartu server, tombol "Terminal" untuk protokol SSH/SFTP membuka tab SSH: status `Connecting` ("Menghubungkan lewat <rute>…"), memakai `connect` dengan `UiInteract`, lalu `open_shell`; kegagalan jadi status `Failed(pesan)` dengan tombol "Coba lagi"; sambungan putus menampilkan "Sambungan terputus" dengan tombol **"Sambung ulang"** (tidak otomatis, spec 8); pembatalan saat `Connecting` memakai `CancellationToken`.

- [ ] **Step 1:** Uji: `terminal_folder_for_a_download_is_its_parent_or_itself` (fungsi murni `terminal_dir_for(target_path: &str) -> PathBuf`: berkas tunggal → induknya; direktori ada → direktori itu; tidak ada → leluhur terdekat yang ada); `ssh_tab_title_names_the_route` (judul tab `web-prod` dan keterangan `via bastion-b`).
- [ ] **Step 2:** Jalankan `cargo test -p unduhkilat-app terminal::` (gagal).
- [ ] **Step 3:** Implementasikan aksi, tombol, dan alur sambungan SSH (tanpa menyambung sebelum tombol ditekan).
- [ ] **Step 4:** Verifikasi manual: dari kartu unduhan "Hard Disk Sentinel…" membuka tab di folder yang benar; dari kartu server uji (fixture sementara seperti di Rencana 1, atau NAS nyata milik Eko bila Eko menjalankannya) membuka tab SSH, mengetik `echo halo`, memutus server lalu "Sambung ulang". Catat apa yang **tidak** sempat dilihat.
- [ ] **Step 5:** Jalankan uji (lulus) dan `cargo test --workspace`. Commit `feat(terminal): buka terminal dari folder unduhan dan dari server SSH`.

### Task 11: Verifikasi Claude Code dan kinerja

**Files:** Create `docs/superpowers/spikes/2026-11-07-terminal-verification.md` (catatan hasil); Modify `README.md`, `docs/16_DECISIONS.md` (DEC-012).

- [ ] **Step 1:** Ukur kinerja nyata di tab lokal (tanpa tebakan): waktu bingkai saat `Get-Content` file 20 MB, saat tempel 1 MB, dan saat jendela diubah ukurannya; catat angka dan bandingkan dengan syarat (UI tidak macet > 200 ms).
- [ ] **Step 2:** Daftar periksa manual dengan Claude Code di tab lokal **dan** tab SSH (bila ada akses): warna truecolor dan karakter lebar terlihat benar; `Shift+Enter` membuat baris baru di prompt; `Alt+Enter`; roda mouse menggulir dan klik di dalam aplikasi bekerja; `Ctrl+V` menempel teks panjang tanpa mengeksekusi per baris; teks yang disalin Claude Code (OSC 52) muncul di papan klip Windows; ubah ukuran jendela menyusun ulang tampilan; `Esc` cepat dua kali berfungsi. Tandai setiap butir **terlihat / tidak terlihat / gagal**; yang gagal menjadi tugas perbaikan di catatan, bukan diabaikan.
- [ ] **Step 3:** Tulis catatan hasil, tambahkan baris README ("Terminal: tab lokal dan SSH ...") dan DEC-012 (keputusan: satu terminal dua sumber, `portable-pty`, `Alt+V` ditunda ke setelah Rencana 2).
- [ ] **Step 4:** `cargo test --workspace` dan `cargo build --release --locked` (lulus). Commit `docs: verifikasi terminal dengan Claude Code dan catatan keputusan`.

---

## Self-review (dijalankan penulis)

- **Cakupan spec:** mouse dan papan klip (8a): Task 3, 4, 8; Claude Code (8b): Task 1 (mode, OSC 52, lebar), 2 (kitty, Shift+Enter), 8 (Text/IME, kunci aplikasi), 11 (verifikasi nyata); tata letak A/B dan tab (5): Task 9; terminal lokal dari folder (14): Task 5, 9, 10; terminal SSH (2.4): Task 6, 10; tidak membekukan UI (spec 8 butir 8): Task 4, 7; sambung ulang tidak otomatis (spec 8): Task 10. `Alt+V` (8b) sengaja di luar rencana ini.
- **Konsistensi tipe:** `TermModes`, `SourceIo`, `TermSession`, `TermEngine`, `Key/Mods`, `TermInput` dipakai dengan nama sama di Task 1-10.
- **Proporsi:** rencana memuat tanda tangan, nama uji, dan nilai spec; badan fungsi tidak ditulis. Tugas UI (8-10) tidak punya uji gambar; verifikasinya manual dan dicatat jujur.
- **Risiko:** pilihan `portable-pty` adalah keputusan pembuat; bila ConPTY-nya bermasalah di Windows target, Task 5 berhenti dan mengganti ke `alacritty_terminal::tty` (catat sebagai ruling).
