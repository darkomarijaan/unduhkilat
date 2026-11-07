# Berkas Server, Pratinjau, dan UI Penjelajah: Rencana Implementasi (Rencana 2 + 4 + 5)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (Eko memilih eksekusi native). Langkah memakai kotak centang.

**Goal:** Penjelajah berkas SFTP/FTP/FTPS dua panel, unduh dan unggah lewat antrean yang sudah ada, pratinjau (teks, Markdown, gambar, PDF, docx/odt, xlsx/ods), `Alt+V` tempel gambar di terminal SSH, lalu satu rilis 0.2.0.

**Architecture:** Trait `RemoteFs` (daftar, stat, mkdir, rename, hapus, baca/tulis dari offset) dengan dua implementasi: SFTP (`russh-sftp` di atas kanal subsistem `sftp` dari `SshSession`) dan FTP/FTPS (`suppaftp` async + rustls). Transfer masuk ke tabel `downloads` yang ada lewat skema URL baru, `server://<server_id>/<jalur>` (unduh) dan `server-up://<server_id>/<jalur>` (unggah; `target_path` = berkas lokal), sehingga jeda, lanjut, prioritas, batas kecepatan, dan notifikasi memakai mesin yang ada; `run_transfer` bercabang menurut skema ke `engine/remote.rs`. Pratinjau mengambil sebagian lewat `RemoteFs`.

**Tech Stack:** `russh-sftp` 3.0.1, `suppaftp` 12 (rustls), `hayro` 0.9.0 (PDF, lihat spike), `zip` (sudah ada) + parser XML kecil, `pulldown-cmark` (Markdown).

**Spec:** `docs/superpowers/specs/2026-11-07-server-client-design.md` (bagian 2, 5, 6, 7, 8b, 12.3, 12.7). Catatan spike: `docs/superpowers/spikes/2026-11-07-pdf.md`.

## Global Constraints

- Windows 10/11 x64, portabel; tidak menulis di luar `data/` kecuali folder sementara. Semua teks UI dan galat dalam bahasa Indonesia; kode galat memakai `ErrorCode`.
- Tidak ada sambungan ke server tanpa tindakan pengguna. Kredensial, frasa sandi, dan isi berkas tidak masuk log.
- Menimpa berkas (lokal atau server) **selalu** minta konfirmasi di UI sebelum masuk antrean; tabrakan nama saat dijalankan dari antrean menghentikan item dengan `ERR_NAME_COLLISION` dan tidak pernah menimpa.
- Pratinjau tidak mengunduh berkas lebih besar dari batas (bawaan 25 MB, pengaturan `PREVIEW_MAX_MB`) dan tidak menjalankan makro/skrip. Folder sementara dibersihkan saat aplikasi dibuka dan ditutup.
- Uji tidak boleh menyentuh internet: server SFTP/FTP/FTPS uji dalam-proses di loopback.
- Skema database: tambahan saja.

## Review Focus

1. Jalur server berbahaya: nama berkas dengan `..`, `/`, `\`, NUL, atau panjang berlebih saat mengunduh ke folder lokal; tidak boleh keluar dari folder tujuan.
2. Unggahan/unduhan yang diputus di tengah lalu dilanjutkan: ukuran akhir benar, tidak ada berkas rusak atau ganda.
3. Berkas 0 byte, direktori besar (10.000 entri), nama Unicode/spasi, dan symlink: daftar tidak membekukan UI dan tidak panik.
4. Pratinjau: berkas lebih besar dari batas, ZIP bom pada docx/xlsx, PDF terenkripsi, teks bukan UTF-8, gambar rusak: pesan jelas, tidak panik, memori terbatas.
5. FTPS dengan sertifikat berubah: diblokir seperti kunci host; sambungan FTP/FTPS yang putus saat daftar/transfer menjadi galat jelas, bukan gantung.

---

### Task 1: Tipe `RemoteFs`, kode galat, dan keamanan nama

**Files:** Create `crates/unduhkilat-core/src/server/fs/mod.rs`; Modify `error.rs`, `server/mod.rs`, `Cargo.toml`.

**Interfaces:**
- Produces: `trait RemoteFs: Send { async list(&mut self, dir:&str)->Result<Vec<Entry>>; stat; mkdir; rename; remove(path,is_dir); read_at(path,offset,len)->Result<Vec<u8>>; open_read(path,offset)->Result<Box<dyn AsyncRead+Send+Unpin>>; open_write(path,offset,append)->Result<Box<dyn AsyncWrite+Send+Unpin>>; async close(self) }` (pakai `async_trait` bila perlu); `Entry{name,path,is_dir,size:u64,modified:Option<i64>,is_link:bool}`; `sort_entries(&mut [Entry], key, asc)` (folder dulu); `join_remote(dir,name)->String`; `safe_local_name(remote_name:&str)->Option<String>`; `ErrorCode::{ErrNameCollision, ErrRemoteFs, ErrPreviewTooLarge, ErrPreviewUnsupported}`.

- [ ] **Step 1:** Uji `safe_local_name_rejects_traversal` (`..`, `a/b`, `a\b`, NUL, nama kosong → `None`; `laporan 2026.pdf` dan `\u{4F60}\u{597D}.txt` lolos; nama > 200 karakter dipotong tanpa memotong karakter), `sort_puts_folders_first_and_is_case_insensitive`, `join_remote_handles_root_and_trailing_slash`.
- [ ] **Step 2:** `cargo test -p unduhkilat-core --lib server::fs` gagal (kompilasi).
- [ ] **Step 3:** Implementasi tipe dan fungsi murni; tambah kode galat dengan pesan Indonesia.
- [ ] **Step 4:** Uji lulus. Commit `feat(fs): tipe RemoteFs, urutan, dan nama berkas aman`.

### Task 2: Klien SFTP

**Files:** Create `server/fs/sftp.rs`, `tests/common/sftp_server.rs` (perluas harness SSH uji dengan subsistem SFTP berbasis folder sementara), `tests/server_sftp.rs`; Modify `Cargo.toml` (`russh-sftp`).

**Interfaces:**
- Consumes: `SshSession` (`handle()`), `RemoteFs`.
- Produces: `pub async fn open_sftp(session: SshSession) -> Result<Box<dyn RemoteFs>>` (kanal `request_subsystem("sftp")`, sesi memiliki `SshSession`).

- [ ] **Step 1:** Uji (server SFTP uji dalam-proses): `list_returns_files_and_folders_with_sizes`, `mkdir_rename_remove_roundtrip`, `read_at_returns_a_range`, `write_then_resume_from_offset_gives_the_whole_file`, `unicode_and_space_names_roundtrip`, `closing_the_session_fails_pending_calls_instead_of_hanging`.
- [ ] **Step 2:** Jalankan (gagal). Step 3: implementasi; baca `modified` dan `is_link` dari atribut. Step 4: lulus; commit `feat(fs): klien SFTP`.

### Task 3: Klien FTP dan FTPS

**Files:** Create `server/fs/ftp.rs`, `tests/common/ftp_server.rs` (server FTP uji minimal: USER/PASS/TYPE/PASV/LIST/RETR/STOR/REST/SIZE/MKD/RMD/DELE/RNFR/RNTO/CWD/PWD/QUIT + AUTH TLS dengan sertifikat uji), `tests/server_ftp.rs`; Modify `Cargo.toml` (`suppaftp` fitur async rustls, `rcgen` dev).

**Interfaces:**
- Produces: `pub async fn open_ftp(server:&Server, password:Option<SecretString>, tls_pin: &dyn TlsPin, proxy:&ProxyConfig) -> Result<Box<dyn RemoteFs>>`; `trait TlsPin { async fn check(&self, host, port, sha256_hex) -> Result<()> }` (implementasi nyata memakai tabel `known_hosts` yang sama dengan SSH: jenis `tls`; tak dikenal → `Interact::confirm_host_key`; berubah → `ErrHostKeyChanged`).

- [ ] **Step 1:** Uji FTP polos: list/mkdir/rename/hapus/`read_at`/tulis-lanjut (`REST`/`APPE`), `list_parses_unix_and_windows_formats`. Uji FTPS: `explicit_ftps_connects_after_the_fingerprint_is_confirmed`, `changed_certificate_is_blocked`, `refused_fingerprint_aborts`, `implicit_ftps_connects`.
- [ ] **Step 2-4:** gagal, implementasi (daftar `MLSD` bila ada, jika tidak `LIST`), lulus; commit `feat(fs): klien FTP dan FTPS dengan pin sertifikat`.

### Task 4: `ServerService::open_fs` dan pratinjau sebagian

**Files:** Modify `server/service.rs`; Create `server/fs/session.rs`.

**Interfaces:**
- Produces: `ServerService::open_fs(&self, id:&str, interact:Arc<dyn Interact>, cancel:CancellationToken) -> Result<Box<dyn RemoteFs>>` (SSH/SFTP → `open_sftp`; FTP/FTPS → `open_ftp`; menolak bila protokol SSH murni? SSH juga boleh memakai SFTP).

- [ ] **Step 1:** Uji `open_fs_uses_sftp_for_ssh_servers_and_pins_host_keys` (harness SSH+SFTP), `open_fs_ftp_without_saved_password_asks_the_user`. Step 2-4 seperti biasa; commit `feat(server): open_fs untuk SFTP, FTP, FTPS`.

### Task 5: Unduh dan unggah lewat antrean

**Files:** Create `engine/remote.rs`, `tests/remote_transfers.rs`; Modify `engine/transfer.rs` (cabang skema), `engine/manager.rs` (`enqueue_remote_download`, `enqueue_remote_upload`), `repo.rs` bila perlu.

**Interfaces:**
- Consumes: `ServerService::open_fs`, `TokenBucket`/`Throttler`.
- Produces: `Manager::enqueue_remote_download(server_id:&str, remote_path:&str, local_dir:&Path, overwrite:bool) -> Result<Download>`; `Manager::enqueue_remote_upload(server_id, local_file:&Path, remote_dir:&str, overwrite:bool) -> Result<Download>`. URL `server://<id>/<jalur>` dan `server-up://<id>/<jalur>`; `connection_count=1`, `segmentation_supported=0`; lanjut dari `downloaded_bytes`. Tabrakan nama dan `overwrite=false` → `ErrNameCollision`. Sambungan memakai `UiInteract`-setara untuk antrean: bila perlu pertanyaan (host baru/password), item gagal dengan galat "buka Server dan Uji sambungan dulu" (tidak ada dialog dari latar belakang).

- [ ] **Step 1:** Uji (SFTP dan FTP uji): `download_file_completes_with_correct_bytes`, `download_resumes_after_pause`, `upload_resumes_after_pause_without_corruption`, `upload_collision_stops_with_err_name_collision`, `download_name_is_sanitised_inside_target_folder`, `global_speed_limit_applies_to_uploads`, `zero_byte_file_transfers`.
- [ ] **Step 2-4:** gagal, implementasi, lulus; commit `feat(queue): unduh dan unggah berkas server lewat antrean`.

### Task 6: Pratinjau inti

**Files:** Create `crates/unduhkilat-core/src/preview/{mod,text,markdown,image,office,pdf}.rs`; Modify `Cargo.toml` (`hayro`, `pulldown-cmark`, `image`), `config.rs` (`PREVIEW_MAX_MB`).

**Interfaces:**
- Produces: `preview::kind_for(name:&str)->Kind{Text,Markdown,Image,Pdf,Docx,Odt,Xlsx,Ods,External,Unknown}`; `async preview::load(fs:&mut dyn RemoteFs, path:&str, size:u64, max_bytes:u64, more:bool)->Result<Preview>`; `enum Preview{Text{text:String,truncated:bool},Markdown(Vec<MdBlock>),Image{rgba:Vec<u8>,w:u32,h:u32},Pdf{pages:usize,first:PageImage},Doc{blocks:Vec<DocBlock>},Sheet{sheets:Vec<Sheet>}, External}`; `preview::pdf_page(bytes:&[u8], page:usize, scale:f32)->Result<PageImage>`; teks bukan UTF-8 dideteksi (UTF-8/UTF-16 BOM, selain itu "berkas biner": tawarkan Unduh).

- [ ] **Step 1:** Uji (dengan berkas fixture kecil dibuat dalam uji): `kind_for_maps_extensions`, `text_preview_truncates_at_one_megabyte_and_loads_more`, `non_utf8_binary_is_not_shown_as_text`, `markdown_blocks_have_headings_lists_tables_code`, `docx_text_headings_lists_tables`, `odt_text_and_tables`, `xlsx_sheets_as_value_tables_with_shared_strings`, `ods_sheets`, `zip_bomb_is_refused` (batas hasil dekompresi 50 MB dan 10.000 berkas), `pdf_first_page_renders`, `encrypted_pdf_gives_clear_error`, `image_preview_decodes_png_and_jpeg_and_rejects_corrupt`, `file_over_limit_is_not_fetched` (server uji menghitung byte terbaca).
- [ ] **Step 2-4:** gagal, implementasi, lulus; commit `feat(preview): pratinjau teks, Markdown, gambar, PDF, docx/odt, xlsx/ods`.

### Task 7: Penjelajah dua panel (UI)

**Files:** Create `crates/unduhkilat-app/src/explorer.rs` (child module `app`); Modify `terminal.rs` (panel "Berkas": lokal memakai penjelajah lokal, SSH/SFTP memakai penjelajah server), `servers.rs` (tombol "Berkas" aktif, buka tab penjelajah), `app.rs`.

**Interfaces:**
- Produces: pure fn `crumbs(path:&str)->Vec<(String,String)>`, `sorted_view(entries, key, asc, filter)`, `plan_drop(...)` diuji; UI: kiri komputer ini, kanan server; telusuri (klik dua kali), breadcrumb, urutan, buat folder, ganti nama, hapus (konfirmasi), unduh/unggah (tombol dan seret antar panel) lewat `enqueue_remote_*`; konfirmasi timpa sebelum antre; daftar dimuat di tugas latar (UI tidak membeku), batalkan saat berpindah folder.

- [ ] **Step 1:** Uji murni `crumbs_split_unix_and_windows_paths`, `sorted_view_filters_and_orders`, `overwrite_check_lists_existing_names`. Step 2-4: gagal, implementasi, lulus; verifikasi visual (tangkapan layar via PrintWindow, tanpa mouse otomatis); commit `feat(ui): penjelajah berkas dua panel`.

### Task 8: Panel pratinjau (UI)

**Files:** Modify `explorer.rs`; Create `crates/unduhkilat-app/src/preview_view.rs`.

- [ ] **Step 1:** Uji murni `preview_choice_follows_kind_and_limit` (tombol "Muat lebih banyak", "Unduh", "Buka di aplikasi Windows"). Implementasi: teks bernomor baris, Markdown dirender egui, gambar zoom/geser, PDF per halaman + zoom, tabel lembar dengan pilih lembar; "Buka di aplikasi" mengunduh ke folder sementara lalu membuka. Folder sementara dibersihkan saat buka/tutup. Commit `feat(ui): panel pratinjau berkas`.

### Task 9: `Alt+V` tempel gambar di terminal SSH

**Files:** Modify `terminal.rs`, `explorer.rs` (pakai `open_fs`), core `server/service.rs` (`upload_bytes(fs, remote_path, bytes)`).

- [ ] **Step 1:** Uji `alt_v_with_image_on_clipboard_is_an_upload_request` (terjemahan `Key{V, alt}` → `TermInput::PasteImage`), `image_png_encoding_of_rgba` dan `remote_temp_name_is_unique_and_safe` (`/tmp/unduhkilat/img-<waktu>-<acak>.png`). Implementasi: bila clipboard berisi gambar (egui `PasteImage`), unggah lewat SFTP ke `/tmp/unduhkilat/...` (butuh server SSH dengan SFTP), ketik jalur ke terminal; berkas sementara dihapus saat tab ditutup. Tab lokal: ketik jalur berkas sementara lokal. Commit `feat(terminal): Alt+V tempel gambar lewat SFTP`.

### Task 10: Pengaturan, dokumen, dan tindak lanjut

- [ ] **Step 1:** Pengaturan "Pratinjau" (batas MB). Impor `~/.ssh/config` (Host, HostName, User, Port, IdentityFile, ProxyJump) bila murah (fungsi murni `parse_ssh_config` dengan uji); jika tidak, catat ditunda. README, DEC-013 (skema URL `server://`, pustaka), catatan verifikasi. Commit.

### Task 11: Verifikasi, review akhir, dan rilis 0.2.0

- [ ] **Step 1:** `cargo test --workspace --no-fail-fast`; verifikasi manual terhadap server nyata hanya bila Eko menjalankannya (catat apa yang **tidak** diuji).
- [ ] **Step 2:** Review akhir seluruh cabang oleh reviewer baru (model paling mampu) dengan Review Focus di atas; perbaiki Critical/Important dengan RED→GREEN; kumpulkan "Rulings I made" dan "Deferred minors".
- [ ] **Step 3:** Naikkan versi 0.2.0, bangun `--release --locked`, zip portabel, `.sha256`, `.sig` (kunci `~/.unduhkilat-signing/release.key`), merge ke `main`, push, tag `v0.2.0`, `gh release create --prerelease` (Eko sudah meminta satu rilis untuk 2+4+5).
