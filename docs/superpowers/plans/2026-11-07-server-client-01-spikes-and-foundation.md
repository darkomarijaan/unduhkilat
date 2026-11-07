# Klien Server, Bagian 1: Purwarupa dan Fondasi (Implementation Plan)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Menyimpan server (SSH/SFTP/FTP/FTPS) beserta kredensial dan kunci host, menyambung lewat SSH dengan jump host berantai, dan menampilkannya di layar "Server" dengan tombol "Uji sambungan"; plus tiga purwarupa yang menentukan pustaka untuk rencana 2-4.

**Architecture:** Modul baru `server/` di crate `unduhkilat-core` (model, penyimpanan SQLite, rahasia DPAPI, kunci host, sambungan SSH). Skema database bertambah **tabel baru saja** (versi 2 ke 3). Layar UI baru di `unduhkilat-app/src/servers.rs`. Interaksi pengguna (konfirmasi kunci host, minta password) lewat trait `Interact` agar inti bisa diuji tanpa UI.

**Tech Stack:** Rust, rusqlite, tokio, egui/eframe 0.37, DPAPI (`dpapi.rs`), `russh` + `ssh-key` (versi final ditentukan Task 1), server SSH uji dalam-proses di loopback.

**Spec:** `docs/superpowers/specs/2026-11-07-server-client-design.md`. Peta rencana: `…-00-overview.md`.

## Global Constraints

- Windows 10/11 x64 saja; portabel (satu folder, data di `data/`); tanpa pemasang.
- Tidak ada sambungan jaringan tanpa tindakan pengguna: tidak menyambung saat aplikasi dibuka (spec §4).
- Kredensial dienkripsi DPAPI lewat `dpapi_encrypt`/`dpapi_decrypt`; dibersihkan dari memori (`zeroize`); **tidak pernah** muncul di log, `Debug`, atau pesan galat (spec §7).
- Kunci host diverifikasi untuk **setiap lompatan**; kunci berubah = blokir (spec §7).
- Jump host: maksimal **5** lompatan; deteksi siklus; tiap lompatan adalah server SSH tersimpan (spec §8, §12.4).
- Skema database: tambahan saja, tidak mengubah tabel/kolom lama (spec §12.9).
- Semua uji berjalan di loopback tanpa internet; server SSH uji dalam-proses (spec §10).
- Seluruh teks UI dan pesan galat berbahasa Indonesia; galat memakai `ErrorCode` (spec §10).
- UI mengikuti `DESIGN.md`/`theme.rs`: kartu bergaris hitam, pita jingga, font mono untuk alamat.
- Satu commit per tugas; pesan commit berbahasa Indonesia; jangan push dan jangan rilis di rencana ini.

## Review Focus

Masukan yang tidak disebut spec tetapi pasti ditemui pengguna. Setiap baris punya uji di tugas yang disebut.

1. Nama server dengan spasi di tepi, Unicode, host IPv6 (`[::1]`), host berhuruf besar, port 0 atau 70000: ditolak atau dinormalkan dengan pesan jelas. (Task 6)
2. Menghapus server yang dipakai sebagai jump host, atau mengubah rantai sehingga membentuk siklus: ditolak dengan nama server yang terdampak. (Task 6)
3. Kunci host berubah pada lompatan **tengah** (bukan hanya tujuan): sambungan diblokir dan galat menyebut lompatan itu. (Task 10)
4. Lompatan yang mati atau lambat tidak boleh menggantung selamanya; pembatalan oleh pengguna menghentikan sambungan. (Task 10)
5. Password salah tidak boleh memuat password itu di pesan galat, log, atau `Debug`; kredensial yang tidak bisa didekripsi (folder dipindah akun) meminta ulang, bukan crash. (Task 7 dan 10)

---

## Bagian A: Purwarupa (kode dibuang, catatan keputusan disimpan)

Aturan bersama: kode purwarupa hidup di `spikes/<nama>/` dengan `Cargo.toml` sendiri berisi `[workspace]` kosong (tidak ikut workspace utama). Setelah catatan keputusan di-commit, hapus `spikes/<nama>/` (`git rm -r`). Yang bertahan hanya catatan di `docs/superpowers/spikes/`. Purwarupa tidak diberi uji dan tidak dibuat tahan banting.

### Task 1: Purwarupa rantai jump dengan `russh`

**Files:**
- Create: `spikes/ssh-jump/` (sementara), `docs/superpowers/spikes/2026-11-07-ssh-jump.md`

**Interfaces:**
- Produces: catatan keputusan berisi versi `russh`/`ssh-key` yang dipilih, resep memanggil `channel_open_direct_tcpip` untuk lompatan, cara membaca sidik jari kunci host dari handler, dan cara membatalkan sambungan. Task 10 membaca catatan ini.

- [ ] **Step 1:** Buat proyek `spikes/ssh-jump` yang menjalankan dua server `russh` di loopback (B dan A, A hanya dapat dicapai lewat B), lalu klien yang masuk ke A **lewat B** dengan satu `Handle<Handler>` per lompatan dan aliran `direct-tcpip` dari B sebagai transport ke A.
- [ ] **Step 2:** Buktikan: (a) login password dan login kunci ed25519 berfrasa sandi; (b) handler `check_server_key` menerima sidik jari dan bisa menolak; (c) rantai tiga lompatan; (d) timeout sambungan 2 detik pada lompatan yang tidak menjawab; (e) `Handle` dapat ditutup dari tugas lain (pembatalan).
- [ ] **Step 3:** Tulis `docs/superpowers/spikes/2026-11-07-ssh-jump.md`: versi pustaka, potongan resep (maksimal 40 baris), hal yang gagal atau mengejutkan, dan keputusan "pakai `russh` ya/tidak". Jika **tidak**, nyatakan alternatifnya dan hentikan rencana ini untuk didiskusikan dengan Eko.
- [ ] **Step 4:** `git rm -r spikes/ssh-jump`; commit `docs: catatan purwarupa rantai jump SSH`.

### Task 2: Purwarupa terminal di egui

**Files:**
- Create: `spikes/terminal-egui/` (sementara), `docs/superpowers/spikes/2026-11-07-terminal-egui.md`

**Interfaces:**
- Produces: keputusan pemilihan emulator (`alacritty_terminal` atau `vt100`), ukuran sel yang dipakai, dan biaya penggambaran. Dibaca oleh Rencana 3.

- [ ] **Step 1:** Buat fixture `spikes/terminal-egui/fixtures/claude-like.bin`: aliran byte yang memuat layar alternatif (`ESC[?1049h`), warna 256 dan truecolor, karakter lebar (CJK, emoji), garis kotak, bracketed paste (`ESC[?2004h`), pelaporan mouse SGR (`ESC[?1006h`), OSC 52, pembaruan tersinkron (`ESC[?2026h`), dan 5.000 baris keluaran deras.
- [ ] **Step 2:** Umpankan fixture ke kedua kandidat emulator, gambar grid ke egui, ukur: rata-rata dan terburuk waktu satu bingkai pada 120x40 sel, kebenaran lebar sel untuk karakter lebar, dan apakah tiap fitur di Step 1 dikenali (OSC 52 harus dapat diambil sebagai peristiwa, bukan hilang).
- [ ] **Step 3:** Catat apakah protokol papan ketik yang diperluas (kitty/`modifyOtherKeys`) didukung emulator atau harus ditangani di sisi pengirim tombol.
- [ ] **Step 4:** Tulis catatan keputusan dengan angka terukur dan pilihan akhir; `git rm -r spikes/terminal-egui`; commit `docs: catatan purwarupa terminal egui`.

### Task 3: Purwarupa perender PDF

**Files:**
- Create: `spikes/pdf/` (sementara), `docs/superpowers/spikes/2026-11-07-pdf.md`

**Interfaces:**
- Produces: keputusan `hayro` atau `pdfium-render` (dan apakah `pdfium.dll` perlu dibawa). Dibaca oleh Rencana 4 dan, bila DLL dipilih, Rencana 5 (pembaruan otomatis harus menukar semua berkas dalam zip, spec §12.6).

- [ ] **Step 1:** Kumpulkan lima PDF uji di `spikes/pdf/samples/`: teks biasa, hasil pindai (gambar), huruf CJK, 100 halaman, dan satu formulir. (Bila Eko memberi PDF nyata, pakai itu.)
- [ ] **Step 2:** Render halaman 1 setiap PDF dengan kedua pustaka ke PNG; ukur waktu halaman pertama, kebenaran teks (periksa mata), dan penambahan ukuran `UnduhKilat.exe` (`cargo build --release` sebelum/sesudah) serta ada tidaknya DLL.
- [ ] **Step 3:** Tulis catatan keputusan. Aturan spec: utamakan `hayro` bila hasilnya terbaca untuk lima PDF; `pdfium` hanya bila `hayro` gagal pada teks atau CJK.
- [ ] **Step 4:** `git rm -r spikes/pdf`; commit `docs: catatan purwarupa perender PDF`.

---

## Bagian B: Fondasi di `unduhkilat-core`

File yang dibuat/diubah (satu tanggung jawab per file):

- `crates/unduhkilat-core/src/error.rs` (ubah): kode galat baru.
- `crates/unduhkilat-core/src/schema.sql`, `db.rs` (ubah): tabel baru, `SCHEMA_VERSION = 3`.
- `crates/unduhkilat-core/src/server/mod.rs` (baru): ekspor modul.
- `…/server/model.rs` (baru): `Protocol`, `AuthMethod`, `Server`, `NewServer`.
- `…/server/store.rs` (baru): CRUD server dan rantai jump.
- `…/server/secrets.rs` (baru): simpan/baca rahasia terenkripsi.
- `…/server/hostkeys.rs` (baru): kunci host tepercaya.
- `…/server/ssh.rs` (baru): sambungan SSH dan rantai jump.
- `…/server/service.rs` (baru): fasad untuk UI (`ServerService`).
- `crates/unduhkilat-core/tests/common/ssh_server.rs` (baru): server SSH uji dalam-proses.
- `crates/unduhkilat-core/tests/servers.rs` (baru): uji integrasi.

### Task 4: Kode galat baru

**Files:**
- Modify: `crates/unduhkilat-core/src/error.rs`
- Test: modul `tests` di `error.rs`

**Interfaces:**
- Produces: `ErrorCode::{ErrHostKeyUnknown, ErrHostKeyChanged, ErrSshAuthFailed, ErrJumpFailed, ErrCredentialUnreadable, ErrTooManyHops, ErrServerInUse}` dengan `as_str()` `ERR_HOST_KEY_UNKNOWN`, `ERR_HOST_KEY_CHANGED`, `ERR_SSH_AUTH_FAILED`, `ERR_JUMP_FAILED`, `ERR_CREDENTIAL_UNREADABLE`, `ERR_TOO_MANY_HOPS`, `ERR_SERVER_IN_USE`.

- [ ] **Step 1:** Tulis uji `new_server_codes_serialize_screaming_snake` yang memeriksa `serde_json::to_string(&ErrorCode::ErrHostKeyChanged) == "\"ERR_HOST_KEY_CHANGED\""` untuk ketujuh kode, dan `as_str()` sama.
- [ ] **Step 2:** Jalankan `cargo test -p unduhkilat-core --lib error::` (gagal: varian belum ada).
- [ ] **Step 3:** Tambahkan varian dan cabang `as_str()` mengikuti pola yang ada.
- [ ] **Step 4:** Jalankan uji yang sama (lulus), lalu `cargo build -p unduhkilat-app` (tidak ada `match` yang tidak lengkap).
- [ ] **Step 5:** Commit `feat(server): kode galat untuk kunci host, jump, dan kredensial`.

### Task 5: Skema tabel server (versi 3, tambahan saja)

**Files:**
- Modify: `crates/unduhkilat-core/src/schema.sql`, `crates/unduhkilat-core/src/db.rs` (`SCHEMA_VERSION` menjadi 3)
- Test: modul `tests` di `db.rs`

**Interfaces:**
- Produces: tabel `servers(id TEXT PK, name TEXT NOT NULL UNIQUE COLLATE NOCASE, protocol TEXT CHECK IN ('ssh','sftp','ftp','ftps'), host TEXT, port INTEGER CHECK 1..65535, username TEXT, auth TEXT CHECK IN ('password','key','agent'), key_path TEXT NULL, save_secret INTEGER NOT NULL DEFAULT 0, created_at TEXT, updated_at TEXT)`; `server_hops(server_id TEXT REFERENCES servers ON DELETE CASCADE, position INTEGER, hop_id TEXT REFERENCES servers ON DELETE RESTRICT, PRIMARY KEY (server_id, position))`; `server_secrets(server_id TEXT REFERENCES servers ON DELETE CASCADE, kind TEXT CHECK IN ('password','passphrase'), ciphertext TEXT NOT NULL, PRIMARY KEY (server_id, kind))`; `host_keys(host TEXT, port INTEGER, key_type TEXT, fingerprint TEXT NOT NULL, trusted_at TEXT NOT NULL, PRIMARY KEY (host, port, key_type))`. Semua `CREATE TABLE IF NOT EXISTS`.

- [ ] **Step 1:** Tulis uji `v2_database_migrates_to_v3_without_losing_data`: buat berkas DB, jalankan hanya skema versi 2 (simpan salinan teks skema lama di uji), isi satu `downloads`, `settings`, dan `credentials`, set `user_version=2`, buka lewat `Db::open`, lalu cek `user_version == 3`, baris lama utuh, dan keempat tabel baru ada (`sqlite_master`).
- [ ] **Step 2:** Tulis uji `server_tables_enforce_constraints`: nama ganda tanpa membedakan huruf besar-kecil ditolak; port 0 dan 65536 ditolak; menghapus server yang masih dirujuk `server_hops.hop_id` ditolak (RESTRICT); menghapus server menghapus baris `server_secrets` miliknya (CASCADE).
- [ ] **Step 3:** Jalankan `cargo test -p unduhkilat-core --lib db::` (gagal).
- [ ] **Step 4:** Tambahkan tabel ke `schema.sql`, naikkan `SCHEMA_VERSION`; pastikan `PRAGMA foreign_keys=ON` sudah aktif di koneksi (periksa `Db::open`; bila belum, aktifkan dan uji).
- [ ] **Step 5:** Jalankan uji (lulus) dan `cargo test -p unduhkilat-core --lib` (tidak ada regresi). Commit `feat(server): skema tabel server, rantai jump, rahasia, dan kunci host`.

### Task 6: Model dan penyimpanan server

**Files:**
- Create: `crates/unduhkilat-core/src/server/mod.rs`, `model.rs`, `store.rs`
- Modify: `crates/unduhkilat-core/src/lib.rs` (`pub mod server;`)
- Test: modul `tests` di `store.rs` (memakai `Db::open` pada berkas sementara)

**Interfaces:**
- Consumes: `repo::now() -> String`, `Db::{read, write}`, `ErrorCode` baru dari Task 4.
- Produces (`model.rs`):
  - `enum Protocol { Ssh, Sftp, Ftp, Ftps }` dengan `as_str() -> &'static str`, `parse(&str) -> Result<Protocol>`, `default_port(&self) -> u16` (22, 22, 21, 21).
  - `enum AuthMethod { Password, Key, Agent }` dengan `as_str`, `parse`.
  - `struct Server { id: String, name: String, protocol: Protocol, host: String, port: u16, username: String, auth: AuthMethod, key_path: Option<String>, save_secret: bool, hops: Vec<String> /* id server, terluar dulu */, created_at: String, updated_at: String }`.
  - `struct NewServer { name, protocol, host, port: Option<u16>, username, auth, key_path: Option<String>, save_secret: bool, hops: Vec<String> }` (tanpa rahasia; rahasia lewat Task 7).
- Produces (`store.rs`, semuanya menerima `&rusqlite::Connection` agar bisa dipakai dalam `Db::read/write`):
  - `insert_server(conn, &NewServer) -> Result<Server>`
  - `update_server(conn, id: &str, &NewServer) -> Result<Server>`
  - `delete_server(conn, id: &str) -> Result<()>`
  - `get_server(conn, id: &str) -> Result<Server>`
  - `list_servers(conn) -> Result<Vec<Server>>` (urut nama, tanpa membedakan huruf besar-kecil)
  - `chain_for(conn, id: &str) -> Result<Vec<Server>>` (lompatan terluar dulu, tujuan di akhir)
- Aturan validasi: nama dipangkas dan tidak kosong; host dipangkas, huruf kecil, boleh `[v6]` tanpa kurung disimpan sebagai alamat v6, tidak boleh mengandung spasi; port 1..=65535 (bawaan dari protokol); hanya server `Ssh` boleh menjadi lompatan; maksimal 5 lompatan (`ErrTooManyHops`); tujuan tidak boleh ada di rantainya sendiri dan rantai tidak boleh berulang/siklus (`ErrInvalidArgument` dengan nama server); hanya server `Ssh` dan `Sftp` boleh punya `hops` (FTP/FTPS menolak jump dengan pesan jelas).

- [ ] **Step 1:** Tulis uji, semua di `store.rs`: `insert_normalizes_name_host_and_default_port` (`"  Web Prod "` menjadi `"Web Prod"`, host `"10.0.1.20 "` dan `"EXAMPLE.com"` menjadi `"10.0.1.20"`/`"example.com"`, SSH tanpa port memberi 22, FTP memberi 21); `rejects_bad_names_hosts_and_ports` (nama kosong dan hanya spasi, host `"a b"`, port 0, port 70000 lewat `u16` diuji sebagai `Some(0)`); `unicode_name_and_ipv6_host_round_trip` (`"服务器 ①"`, host `"::1"`); `duplicate_name_is_case_insensitive_error` (kode `ErrAlreadyExists`); `hops_only_ssh_and_max_five` (enam lompatan memberi `ErrTooManyHops`; lompatan berprotokol FTP memberi `ErrInvalidArgument`); `cycle_is_rejected_naming_the_server` (A lewat B, lalu ubah B lewat A: pesan memuat `"A"`); `chain_for_returns_hops_then_target_in_order`; `delete_refused_when_used_as_hop_names_dependents` (kode `ErrServerInUse`, pesan memuat nama server yang memakainya) dan `delete_ok_when_unused`; `ftp_server_with_hops_is_rejected`.
- [ ] **Step 2:** Jalankan `cargo test -p unduhkilat-core --lib server::store` (gagal: modul belum ada).
- [ ] **Step 3:** Implementasikan `model.rs` dan `store.rs` dengan tanda tangan di atas. Id memakai `uuid::Uuid::new_v4().to_string()`; rantai disimpan di `server_hops` dalam satu transaksi bersama update server; deteksi siklus dengan penelusuran rantai tiap lompatan.
- [ ] **Step 4:** Jalankan uji (lulus). Commit `feat(server): model dan penyimpanan server dengan validasi rantai jump`.

### Task 7: Rahasia terenkripsi

**Files:**
- Create: `crates/unduhkilat-core/src/server/secrets.rs`
- Modify: `crates/unduhkilat-core/src/server/mod.rs`
- Test: modul `tests` di `secrets.rs`

**Interfaces:**
- Consumes: `dpapi::{dpapi_encrypt(&str) -> Result<String>, dpapi_decrypt(&str) -> Result<Zeroizing<String>>}`, tabel `server_secrets`.
- Produces:
  - `enum SecretKind { Password, Passphrase }` dengan `as_str()` (`"password"`/`"passphrase"`).
  - `store_secret(conn, server_id: &str, kind: SecretKind, secret: &str) -> Result<()>` (menimpa bila ada).
  - `load_secret(conn, server_id: &str, kind: SecretKind) -> Result<Option<Zeroizing<String>>>`; `Ok(None)` bila tidak ada; `ErrCredentialUnreadable` dengan pesan `"Kredensial ini tersimpan di akun Windows lain dan tidak bisa dibaca; masukkan ulang."` bila dekripsi gagal.
  - `delete_secrets(conn, server_id: &str) -> Result<()>`.
  - `struct SecretString(Zeroizing<String>)` dengan `Debug` yang mencetak `SecretString(***)` dan tanpa `Display`; dipakai di `NewServerSecrets { password: Option<SecretString>, passphrase: Option<SecretString> }`.

- [ ] **Step 1:** Tulis uji: `secret_round_trip` (simpan `"p@ss w0rd ü"`, baca kembali sama); `ciphertext_in_db_is_not_plaintext` (kolom `ciphertext` tidak memuat teks asli); `missing_secret_is_none`; `unreadable_ciphertext_gives_credential_unreadable_with_message` (isi kolom dengan base64 acak, cek kode dan bahwa pesan memuat "masukkan ulang"); `debug_output_never_contains_the_secret` (`format!("{:?}", NewServerSecrets{...})` tidak memuat nilai); `delete_secrets_removes_rows`.
- [ ] **Step 2:** Jalankan `cargo test -p unduhkilat-core --lib server::secrets` (gagal).
- [ ] **Step 3:** Implementasikan fungsi di atas; kegagalan `dpapi_decrypt` dipetakan ke `ErrCredentialUnreadable` tanpa menyertakan isi blob.
- [ ] **Step 4:** Jalankan uji (lulus). Commit `feat(server): penyimpanan rahasia terenkripsi DPAPI`.

### Task 8: Kunci host tepercaya

**Files:**
- Create: `crates/unduhkilat-core/src/server/hostkeys.rs`
- Modify: `crates/unduhkilat-core/src/server/mod.rs`
- Test: modul `tests` di `hostkeys.rs`

**Interfaces:**
- Produces:
  - `fingerprint_sha256(key_blob: &[u8]) -> String` berformat `SHA256:` + base64 tanpa padding dari SHA-256 (sama dengan keluaran `ssh-keygen -lf`). Gunakan `sha2` bila belum jadi dependensi langsung, atau fungsi sidik jari dari `ssh-key` jika sudah dipilih di Task 1.
  - `enum HostKeyStatus { Unknown, Trusted, Changed { known: String } }`.
  - `check(conn, host: &str, port: u16, key_type: &str, fingerprint: &str) -> Result<HostKeyStatus>`; host dibandingkan dalam huruf kecil.
  - `trust(conn, host, port, key_type, fingerprint) -> Result<()>` (hanya menyisipkan; bila sudah ada dengan sidik jari berbeda mengembalikan `ErrHostKeyChanged`).
  - `replace(conn, host, port, key_type, fingerprint) -> Result<()>` (tindakan eksplisit pengguna mengganti kunci).

- [ ] **Step 1:** Tulis uji: `fingerprint_matches_known_vector` (vektor SHA-256 untuk byte `b"abc"` dalam format `SHA256:ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0`); `unknown_then_trusted_then_changed` (`check` memberi `Unknown`, setelah `trust` memberi `Trusted`, dengan sidik jari lain memberi `Changed{known}`); `trust_never_overwrites_a_different_key` (`ErrHostKeyChanged`); `replace_updates_after_explicit_call`; `host_is_case_insensitive_and_port_specific` (`Example.com:22` dan `example.com:2222` terpisah).
- [ ] **Step 2:** Jalankan `cargo test -p unduhkilat-core --lib server::hostkeys` (gagal).
- [ ] **Step 3:** Implementasikan; `check`/`trust`/`replace` memakai `INSERT … ON CONFLICT` hanya pada `replace`.
- [ ] **Step 4:** Jalankan uji (lulus). Commit `feat(server): penyimpanan dan pemeriksaan kunci host`.

### Task 9: Server SSH uji dalam-proses

**Files:**
- Create: `crates/unduhkilat-core/tests/common/ssh_server.rs`
- Modify: `crates/unduhkilat-core/tests/common/mod.rs` (`pub mod ssh_server;`), `crates/unduhkilat-core/Cargo.toml` (`[dev-dependencies]`: pustaka server SSH sesuai catatan Task 1)
- Test: `crates/unduhkilat-core/tests/servers.rs` (uji kecil harness)

**Interfaces:**
- Produces:
  - `struct TestSsh { pub addr: SocketAddr, pub host_key_fingerprint: String, ... }`
  - `TestSsh::start(cfg: TestSshConfig) -> TestSsh` dengan `TestSshConfig { users: Vec<(String /*user*/, TestCred)>, allow_forwarding: bool, accept_delay: Option<Duration> }` dan `enum TestCred { Password(String), Key(String /*public key openssh*/) }`.
  - `TestSsh::rotate_host_key(&self)` (mengganti kunci host untuk uji "kunci berubah"), `TestSsh::login_count(&self) -> usize`, `TestSsh::stop(self)`.
  - Server mendukung autentikasi password dan kunci publik, dan membuka kanal `direct-tcpip` ke alamat yang diminta (inti jump). Sesi shell/SFTP/PTY **belum** (rencana 2 dan 3 menambahnya).

- [ ] **Step 1:** Tulis uji `harness_accepts_password_and_forwards_to_another_instance` di `tests/servers.rs`: mulai dua `TestSsh`, sambungkan klien mentah (pustaka klien pilihan Task 1) ke yang pertama dengan password benar (berhasil) dan salah (gagal), buka `direct-tcpip` ke alamat server kedua dan baca banner SSH `SSH-2.0-` dari sana.
- [ ] **Step 2:** Jalankan `cargo test -p unduhkilat-core --test servers harness` (gagal).
- [ ] **Step 3:** Implementasikan harness dengan pustaka server dari catatan Task 1. Kunci host dibuat di memori per instans.
- [ ] **Step 4:** Jalankan uji (lulus). Commit `test(server): server SSH uji dalam-proses dengan forwarding`.

### Task 10: Sambungan SSH dengan rantai jump

**Files:**
- Create: `crates/unduhkilat-core/src/server/ssh.rs`
- Modify: `crates/unduhkilat-core/src/server/mod.rs`, `crates/unduhkilat-core/Cargo.toml` (dependensi `russh`, `ssh-key` pada versi dari catatan Task 1)
- Test: `crates/unduhkilat-core/tests/servers.rs`

**Interfaces:**
- Consumes: `store::chain_for`, `secrets::{load_secret, store_secret, SecretKind}`, `hostkeys::{check, trust, fingerprint_sha256, HostKeyStatus}`, `ProxyConfig::ftp_proxy() -> Result<Option<HttpProxy>>`, `ftp::connect_tcp(host, port, Option<&HttpProxy>, &Timeouts) -> Result<Box<dyn Io>>`, `tokio_util::sync::CancellationToken`, `futures_util::future::BoxFuture`.
- Produces:
  - `trait Interact: Send + Sync { fn confirm_host_key(&self, host: &str, port: u16, key_type: &str, fingerprint: &str) -> BoxFuture<'_, bool>; fn secret(&self, server_name: &str, kind: SecretKind) -> BoxFuture<'_, Option<SecretString>>; }`
  - `struct ConnectOptions { connect_timeout: Duration /* bawaan 15 s */, auth_timeout: Duration /* 20 s */, proxy: ProxyConfig }`
  - `struct SshSession { /* menyimpan Handle tiap lompatan agar tetap hidup */ pub target: Server, pub rtt: Duration }` dengan `async fn close(self)`.
  - `pub async fn connect(db: &Db, target_id: &str, interact: Arc<dyn Interact>, opts: ConnectOptions, cancel: CancellationToken) -> Result<SshSession>`.
- Perilaku: ambil `chain_for(target_id)`; sambungan pertama lewat `connect_tcp` (proxy HTTP bila ada; mode SOCKS5 menghasilkan `ErrInvalidArgument` "SOCKS5 belum didukung untuk SSH"); tiap lompatan berikutnya lewat kanal `direct-tcpip` dari lompatan sebelumnya; di **setiap** lompatan: periksa kunci host (`Unknown` memanggil `confirm_host_key`, bila `true` simpan dengan `trust`, bila `false` galat `ErrHostKeyUnknown` tanpa menyimpan; `Changed` langsung `ErrHostKeyChanged` tanpa memanggil `Interact`) lalu autentikasi; rahasia diambil dari `load_secret`, bila tidak ada atau `ErrCredentialUnreadable` meminta lewat `Interact::secret` (dan `store_secret` hanya bila `save_secret` aktif); galat pada lompatan ke-N dibungkus `ErrJumpFailed` dengan pesan yang menyebut nama server lompatan; password salah memberi `ErrSshAuthFailed` tanpa memuat rahasia.

- [ ] **Step 1:** Tulis uji di `tests/servers.rs` (semua memakai `TestSsh`, `Db` sementara, dan `Interact` palsu yang mencatat panggilan): `password_login_reaches_the_target`; `key_login_with_passphrase_asks_interact_when_not_saved`; `two_hop_chain_connects_through_both` (server A hanya mau diakses lewat B: A mendengarkan di alamat yang hanya dikenal B, `allow_forwarding` pada B, dan `login_count` kedua server naik 1); `unknown_host_key_asks_once_and_rejection_aborts_without_saving` (galat `ErrHostKeyUnknown`, tabel `host_keys` kosong); `accepted_host_key_is_saved_and_not_asked_again`; `changed_host_key_on_the_middle_hop_is_blocked` (setelah menyambung dan menyimpan, panggil `rotate_host_key` pada B, sambung lagi: galat `ErrHostKeyChanged`, pesan memuat nama B, `Interact::confirm_host_key` tidak dipanggil, dan server A tidak pernah dihubungi); `failing_hop_error_names_the_hop` (password B salah: `ErrJumpFailed`, pesan memuat nama B); `dead_hop_times_out` (`accept_delay` 10 s dengan `connect_timeout` 1 s: galat dalam kurang dari 3 s); `cancel_aborts_a_pending_connect` (batalkan setelah 200 ms: kembali dalam kurang dari 1 s); `wrong_password_error_never_contains_the_password` (password uji `"s3cr3t-XYZ"`, periksa pesan galat dan keluaran log tangkapan `tracing`); `unreadable_saved_secret_falls_back_to_asking` (isi ciphertext rusak, `Interact::secret` dipanggil sekali dan sambungan berhasil); `socks5_proxy_is_refused_clearly`; `max_five_hops_is_enforced_on_connect` (rantai enam ditolak sebelum membuka soket apa pun).
- [ ] **Step 2:** Jalankan `cargo test -p unduhkilat-core --test servers` (gagal).
- [ ] **Step 3:** Implementasikan `ssh.rs`. Gunakan resep dari `docs/superpowers/spikes/2026-11-07-ssh-jump.md` untuk `direct-tcpip` dan handler kunci host. Seluruh `connect` dibungkus `tokio::select!` dengan `cancel.cancelled()` dan `tokio::time::timeout` per tahap (TCP, jabat tangan, autentikasi). Jangan pernah mencatat rahasia ke `tracing`; log hanya nama server, host, dan hasil.
- [ ] **Step 4:** Jalankan `cargo test -p unduhkilat-core --test servers` (lulus). Jalankan juga `cargo test -p unduhkilat-core` penuh (tidak ada regresi).
- [ ] **Step 5:** Commit `feat(server): sambungan SSH dengan jump host berantai dan verifikasi kunci host`.

### Task 11: Fasad `ServerService` untuk UI

**Files:**
- Create: `crates/unduhkilat-core/src/server/service.rs`
- Modify: `crates/unduhkilat-core/src/server/mod.rs`, `crates/unduhkilat-core/src/engine/manager.rs` (tambah `pub fn servers(&self) -> ServerService`)
- Test: `crates/unduhkilat-core/tests/servers.rs`

**Interfaces:**
- Consumes: semua fungsi Task 6-10, `Manager::proxy_config() -> ProxyConfig`, `Manager::db() -> &Db`.
- Produces: `#[derive(Clone)] struct ServerService` dengan:
  - `async fn list(&self) -> Result<Vec<Server>>`
  - `async fn save(&self, id: Option<String>, server: NewServer, secrets: NewServerSecrets) -> Result<Server>` (menyimpan rahasia hanya bila `save_secret`; bila `save_secret` dimatikan, rahasia lama dihapus)
  - `async fn delete(&self, id: &str) -> Result<()>` (menghapus rahasia lalu server; galat `ErrServerInUse` diteruskan)
  - `async fn test_connection(&self, id: &str, interact: Arc<dyn Interact>, cancel: CancellationToken) -> Result<TestReport>` dengan `TestReport { rtt_ms: u64, hop_names: Vec<String> }`; membuka sesi lalu menutupnya.
  - `async fn forget_host_key(&self, host: &str, port: u16) -> Result<()>` (untuk mengganti kunci host secara eksplisit).

- [ ] **Step 1:** Tulis uji `service_saves_lists_tests_and_deletes` (simpan B, simpan A lewat B dengan password disimpan, `list` memuat keduanya berurutan nama, `test_connection(A)` berhasil dengan `hop_names == ["B","A"]`, `delete(B)` ditolak `ErrServerInUse`, `delete(A)` lalu `delete(B)` berhasil dan `server_secrets` kosong) dan `unsaved_secret_is_not_persisted` (`save_secret=false` tidak menulis baris `server_secrets`).
- [ ] **Step 2:** Jalankan `cargo test -p unduhkilat-core --test servers service` (gagal).
- [ ] **Step 3:** Implementasikan fasad (tipis, hanya merangkai fungsi yang sudah ada dan memanggil `Db::read/write`).
- [ ] **Step 4:** Jalankan uji (lulus) dan `cargo test --workspace`. Commit `feat(server): fasad ServerService dan akses lewat Manager`.

---

## Bagian C: UI

### Task 12: Layar Server (daftar, dialog, uji sambungan)

**Files:**
- Create: `crates/unduhkilat-app/src/servers.rs`
- Modify: `crates/unduhkilat-app/src/main.rs` (`mod servers;`), `crates/unduhkilat-app/src/app.rs` (tambah `Screen::Servers`, entri sidebar "Server" dengan jumlah, penerusan `Msg` baru, hook `UNDUHKILAT_START_SCREEN=servers`)
- Test: tidak ada uji otomatis untuk egui; verifikasi dengan peluncuran dan tangkapan layar (praktik proyek ini)

**Interfaces:**
- Consumes: `Manager::servers() -> ServerService` dan semua tipe `server::*`; `theme::*` dan `app.rs` (`Msg`, pola dialog `add_dialog`).
- Produces: `pub struct ServersState { servers: Vec<Server>, dialog: Option<ServerDialog>, test: Option<TestState>, prompt: Option<Prompt> }`; `impl App { pub(crate) fn servers_screen(&mut self, ui: &mut Ui); fn server_dialog(&mut self, ctx: &egui::Context); }`; `struct UiInteract` yang mengimplementasikan `Interact` dengan mengirim `Msg::HostKeyPrompt { host, port, key_type, fingerprint, reply: tokio::sync::oneshot::Sender<bool> }` dan `Msg::SecretPrompt { server_name, kind, reply: oneshot::Sender<Option<String>> }` ke UI.
- Perilaku: sidebar "Server" di bawah "Cari berkas"; daftar kartu per mockup bagian 1 (badge protokol berwarna: SSH ungu, SFTP biru, FTP abu, FTPS hijau; baris mono `pengguna@host:port · cara masuk · via <nama lompatan>`); tombol "+ Server baru", "Ubah", "Hapus" (dengan konfirmasi; galat `ErrServerInUse` ditampilkan), "Uji sambungan"; dialog per mockup bagian 2 (nama, protokol, alamat/port, pengguna, cara masuk Password/Kunci/Agent dengan Agent dinonaktifkan dan diberi keterangan "segera", berkas kunci dengan "Telusuri" via `rfd`, frasa sandi, kotak "Simpan kredensial", pemilih jump host berulang, diagram jalur `Komputer ini → … → tujuan`); dialog konfirmasi kunci host menampilkan sidik jari SHA256 dan tombol "Percayai dan lanjut"/"Batal"; kunci berubah menampilkan galat merah dengan jalur lompatan dan tombol "Ganti kunci…" yang meminta konfirmasi kedua; permintaan password/frasa sandi lewat dialog dengan kolom tersembunyi dan kotak "Simpan"; hasil uji "Terhubung lewat X → Y dalam N ms" atau galat berbahasa Indonesia. Tidak ada sambungan saat layar dibuka; hanya saat tombol ditekan. Tombol Terminal dan Berkas pada kartu ditampilkan nonaktif dengan keterangan "segera" (diaktifkan Rencana 2 dan 3).

- [ ] **Step 1:** Tambahkan `Screen::Servers`, entri sidebar, dan kerangka `servers.rs` (daftar kosong dengan keadaan kosong yang menjelaskan fitur dan tombol "+ Server baru"); bangun dan buka dengan `UNDUHKILAT_START_SCREEN=servers`.
- [ ] **Step 2:** Implementasikan daftar kartu dan memuatnya dari `servers().list()` saat layar dibuka (bukan saat aplikasi dibuka).
- [ ] **Step 3:** Implementasikan dialog tambah/ubah/hapus dengan diagram jalur dan validasi galat dari `store` ditampilkan di dialog.
- [ ] **Step 4:** Implementasikan `UiInteract`, dialog konfirmasi kunci host, dan dialog rahasia; hubungkan "Uji sambungan" ke `test_connection` di `rt.spawn` dengan `CancellationToken` yang dibatalkan oleh tombol "Batal".
- [ ] **Step 5:** Verifikasi dengan server SSH nyata atau harness: jalankan satu `TestSsh` lewat program kecil sementara (atau OpenSSH lokal bila ada), lalu periksa secara manual: tambah server, uji sambungan, konfirmasi kunci host, salah password, jump dua lompatan, dan tangkapan layar pada jendela 1280x800 dan jendela pendek; bandingkan dengan mockup (`docs/superpowers/specs/mockups/server-features.html`). Catat hasil di pesan commit.
- [ ] **Step 6:** `cargo test --workspace` (lulus) dan `cargo build --release`. Commit `feat(server): layar Server dengan dialog, jump host, dan uji sambungan`.

---

## Self-review (dijalankan penulis)

- **Cakupan spec untuk bagian 1:** server tersimpan (§2.1: Task 5, 6, 12), kredensial (§2.2: Task 7), jump berantai (§2.3: Task 6, 10), kunci host (§2.8, §7: Task 8, 10), tanpa sambungan otomatis (§4: Task 12), purwarupa (§12.1: Task 1-3). SSH agent dan impor `ssh_config` sengaja ditunda ke Rencana 5 (peta).
- **Konsistensi tipe:** `Server`, `NewServer`, `NewServerSecrets`, `SecretKind`, `Interact`, `ConnectOptions`, `ServerService`, `TestReport`, `HostKeyStatus` dipakai dengan nama sama di Task 6-12.
- **Proporsi:** rencana ini memuat tanda tangan, nama uji, dan nilai spec; badan fungsi tidak ditulis.
- **Risiko yang diketahui:** Task 1 dapat menolak `russh`; bila begitu rencana berhenti untuk didiskusikan, bukan diteruskan dengan pustaka lain tanpa persetujuan.
