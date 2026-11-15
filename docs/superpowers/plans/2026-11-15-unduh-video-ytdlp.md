# Unduh Video (yt-dlp + ffmpeg) — Rencana Implementasi

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Link video publik (YouTube, Instagram, TikTok, dll.) bisa diunduh sebagai MP4 atau MP3 dari dialog Tambah, antrian, dan CLI, memakai yt-dlp + ffmpeg sebagai proses anak.

**Architecture:** Item video adalah baris biasa di tabel `downloads` dengan awalan URL `video:<opsi>|<url>` (pola `server://`). `run_inner` men-dispatch ke `engine/ytdlp.rs::run_video`, yang menjalankan `yt-dlp.exe` (progres diurai dari `--progress-template`), menggabungkan lewat ffmpeg, lalu memindahkan hasilnya ke nama akhir. Komponen dikelola `core/src/tools.rs` (unduh + verifikasi SHA-256 + pembaruan otomatis).

**Tech Stack:** yt-dlp.exe (Unlicense), ffmpeg win64 LGPL (proses terpisah, tidak di-link), windows Job Object (kill pohon proses), reqwest (unduh komponen), rusqlite (setelan).

**Spec:** `docs/superpowers/specs/2026-11-14-unduh-video-ytdlp-design.md` (pendekatan A, sudah dipilih pengguna).

## Global Constraints

- Windows 10/11 x64 saja (DEC-005). Kode, komentar, string UI, commit: bahasa Indonesia.
- Tidak ada aktivitas jaringan di luar: unduhan yang diminta, pengambilan komponen yang disetujui, cek pembaruan yt-dlp ikut `update_check_enabled`.
- Berkas yang ada tidak pernah tertimpa: `ERR_NAME_COLLISION` (sebelum mulai dan sebelum hasil akhir dipindah).
- Tidak ada jendela konsol: `CREATE_NO_WINDOW` (0x08000000) di semua spawn.
- yt-dlp diverifikasi SHA-256 dari `SHA2-256SUMS` rilis yang sama; ffmpeg build LGPL dipin versi + SHA-256 di kode.
- Kontrak app lama tidak berubah: unduhan HTTP/FTP/torrent/servers perilakunya persis seperti sebelumnya.

## Review Focus

1. **URL asli mengandung `|`** (mis. query param): `video:` diurai sampai `|` PERTAMA saja; sisa tetap bagian URL. → test `parse_splits_on_the_first_pipe`.
2. **yt-dlp keluar 0 tapi berkas hasil tidak ada**: harus gagal `ERR_VIDEO_EXTRACT_FAILED`, bukan sukses diam. → test integrasi `missing_output_fails`.
3. **Judul sangat panjang / karakter ilegal Windows** (`<>:"/\|?*`, emoji): disanitasi + dipotong ±120 karakter sebelum jadi nama berkas. → test `sanitize_title_name`.
4. **"Sign in to confirm you're not a bot"** = tanda versi usang (coba perbarui + ulang sekali), BUKAN langsung login; kalau setelah ulang masih gagal → `ERR_VIDEO_LOGIN_REQUIRED`. → test `map_error` + integrasi.
5. **Batas kecepatan**: `--limit-rate` = min(batas item, batas global); 0 = tanpa batas. → test arg builder.

---

### Task 1: Fondasi — kode error, kunci setelan, `EngineConfig.tools_dir`

**Files:**
- Modify: `crates/unduhkilat-core/src/error.rs` (enum + `as_str`)
- Modify: `crates/unduhkilat-core/src/config.rs` (`keys`, `EngineConfig`)
- Modify: `crates/unduhkilat-core/src/lib.rs` (deklarasi modul `tools` — modulnya dibuat Task 5; boleh ditambah di sini sebagai stub `pub mod tools;` dengan file kosong, atau ditunda ke Task 5)

**Interfaces:**
- Produces: `ErrorCode::{ErrVideoToolsMissing, ErrVideoLoginRequired, ErrVideoExtractFailed}` → `"ERR_VIDEO_TOOLS_MISSING"`, `"ERR_VIDEO_LOGIN_REQUIRED"`, `"ERR_VIDEO_EXTRACT_FAILED"`.
- Produces: `config::keys::VIDEO_TOOLS_LAST_CHECK: &str = "video_tools_last_check"`.
- Produces: `EngineConfig.tools_dir: Option<PathBuf>` (default `None`; `None` atau isinya tanpa `yt-dlp.exe` → fitur video gagal `ERR_VIDEO_TOOLS_MISSING`).

- [ ] **Step 1: Test gagal dulu** — di `error.rs` (modul `tests` yang sudah ada) tambah assertion kode baru; di `config.rs` test `tools_dir_default_is_none`.
- [ ] **Step 2:** `cargo test -p unduhkilat-core --lib` → FAIL.
- [ ] **Step 3:** Implementasikan tiga varian + kunci + field.
- [ ] **Step 4:** `cargo test -p unduhkilat-core --lib` → PASS. Commit `feat(video): kode error dan kunci setelan komponen video`.

### Task 2: `engine/ytdlp.rs` — bentuk URL, deteksi host, parser

**Files:**
- Create: `crates/unduhkilat-core/src/engine/ytdlp.rs` (modul baru; daftar di `engine/mod.rs`)

**Interfaces:**
- Produces:
  - `pub enum VideoKind { Video { max_height: Option<u32> }, AudioMp3 { kbps: u32 } }`
  - `pub struct VideoRequest { pub url: String, pub kind: VideoKind }`
  - `pub fn is_video_host(url: &str) -> bool` — host: youtube.com, youtu.be, instagram.com, facebook.com, fb.watch, tiktok.com, x.com, twitter.com, vimeo.com, dailymotion.com, twitch.tv; subdomain `www.`, `m.`, `music.` diabaikan; skema http/https.
  - `pub fn wrap(url: &str, kind: &VideoKind) -> String` — `video:v=best|…`, `video:v=1080|…`, `video:a=mp3-192|…`.
  - `pub fn parse(original_url: &str) -> Option<VideoRequest>` — `None` bila bukan `video:`; `|` pertama saja; `v=best|v=<angka>|a=mp3-<angka>`.
  - `pub fn auto_wrap(url: &str) -> Option<String>` — `Some(wrap(url, best))` bila host dikenal dan BELUM berawalan `video:`; else `None`.

- [ ] **Step 1: Test gagal** (unit test dalam `ytdlp.rs`): `parse_roundtrips_video_and_audio`, `parse_splits_on_the_first_pipe` (`video:v=best|https://x/a?b=1|2` → url `https://x/a?b=1|2`), `parse_rejects_garbage` (`video:` kosong, `video:x|u`), `is_video_host_matches_subdomains_and_scheme`, `is_video_host_rejects_other_hosts`, `auto_wrap_never_double_wraps`.
- [ ] **Step 2:** `cargo test -p unduhkilat-core --lib ytdlp` → FAIL.
- [ ] **Step 3:** Implementasikan.
- [ ] **Step 4:** PASS. Commit `feat(video): bentuk URL video dan deteksi host`.

### Task 3: `ytdlp.rs` — argumen, progres, nama, pemetaan error

**Files:**
- Modify: `crates/unduhkilat-core/src/engine/ytdlp.rs`

**Interfaces:**
- Consumes: `VideoRequest` (Task 2), `http::sanitize_file_name`.
- Produces:
  - `pub struct ProgressSample { pub downloaded: i64, pub total: i64, pub speed: u64 }`
  - `pub fn build_download_args(req: &VideoRequest, out_stub: &Path, ffmpeg_dir: &Path, proxy: Option<&str>, limit_kbps: u32) -> Vec<String>` — selalu: `--newline`, `--progress-template "download:UK %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.speed)s"`, `--no-playlist`, `--no-overwrites`, `--continue`, `-N 4`, `--ffmpeg-location <dir>`, `-o <out_stub>.%(ext)s`; video: `-f "bv*[height<=H][vcodec^=avc1]+ba[acodec^=mp4a]/bv*[height<=H]+ba/b"` (H besar = tanpa batas) + `--merge-output-format mp4`; audio: `-x --audio-format mp3 --audio-quality <kbps>K`; proxy bila `Some`; `--limit-rate <n>K` bila limit > 0.
  - `pub fn parse_progress_line(line: &str) -> Option<ProgressSample>` — awalan `UK `; bila `downloaded` mundur (berkas baru: video→audio), penelepon menjumlahkan; parser murni per baris.
  - `pub fn final_name(title: &str, kind: &VideoKind) -> String` — sanitasi + potong 120 karakter + ekstensi `.mp4`/`.mp3`.
  - `pub fn is_stale_error(text: &str) -> bool` — memuat salah satu: "nsig extraction failed", "Unable to extract", "HTTP Error 403", "Sign in to confirm you're not a bot".
  - `pub fn map_error(text: &str) -> AppError` — "Sign in to confirm" / "login" / "private" → `ErrVideoLoginRequired`; "Video unavailable"/"not available" → `ErrVideoExtractFailed` ("video tidak tersedia"); selainnya → `ErrVideoExtractFailed` dengan baris error terakhir.

- [ ] **Step 1: Test gagal**: `args_video_caps_height_and_merges_mp4`, `args_audio_uses_mp3_kbps`, `args_limit_rate_min_item_and_global` (hitung min di penelepon; test builder menerima angka jadi), `args_include_proxy_when_set`, `progress_line_parses_and_ignores_other_lines`, `final_name_sanitizes_and_truncates` (judul 300 karakter + `a<b>:c"d` → ≤120+4, tanpa karakter ilegal), `is_stale_error_matches_samples`, `map_error_maps_login_unavailable_and_generic` (contoh teks dari spec §5.1).
- [ ] **Step 2:** FAIL. **Step 3:** implementasi. **Step 4:** PASS. Commit `feat(video): argumen yt-dlp, pengurai progres, nama akhir, pemetaan error`.

### Task 4: `EngineEvent::Progress.phase`

**Files:**
- Modify: `crates/unduhkilat-core/src/engine/mod.rs` — `Progress { …, phase: Option<String> }`.
- Modify: semua titik konstruksi `EngineEvent::Progress` (transfer.rs, torrent.rs, remote.rs) → `phase: None`.

**Interfaces:**
- Produces: bidang `phase` — `Some("menggabungkan")` saat merge (dipakai Task 6); pengirim lain `None`. Bridge/ekstensi mengabaikan bidang asing (JSON aditif).

- [ ] **Step 1:** `cargo test --workspace` masih PASS (hanya bidang baru, `None` di pengirim lama) — tidak ada test baru; verifikasi kompilasi + test.
- [ ] **Step 2:** Commit `feat(engine): bidang fase pada event progres`.

### Task 5: `core/src/tools.rs` — komponen yt-dlp/ffmpeg

**Files:**
- Create: `crates/unduhkilat-core/src/tools.rs`; daftar di `lib.rs`.

**Interfaces:**
- Consumes: `Db` (setelan), `reqwest::Client`.
- Produces:
  - `pub struct Tools { pub dir: PathBuf }`; `yt_dlp()/ffmpeg()/ffprobe() -> PathBuf`.
  - `pub async fn versions(&self) -> (Option<String>, Option<String>)` — `yt-dlp --version`, `ffmpeg -version` (baris pertama), `CREATE_NO_WINDOW`.
  - `pub async fn install(&self, client: &reqwest::Client, on_progress: &(dyn Fn(u64) + Send + Sync)) -> Result<()>` — yt-dlp: unduh `https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe` + `SHA2-256SUMS` dari URL latest yang sama, cocokkan hash, tulis atomik (`.new` → rename). ffmpeg: URL + SHA-256 DIPIN di kode (konstanta `FFMPEG_URL`, `FFMPEG_SHA256` — pilih rilis `BtbN/FFmpeg-Builds` win64-lgpl bertanggal saat implementasi, ekstrak `bin/ffmpeg.exe` + `bin/ffprobe.exe` dari zip memakai crate `zip` yang sudah ada).
  - `pub fn parse_sha256_sums(text: &str) -> Vec<(String, String)>` — format `<hash>  <nama>`.
  - `pub fn is_ytdlp_newer(latest_tag: &str, installed: &str) -> bool` — bandingkan `YYYY.MM.DD[.n]`.
  - `pub async fn maybe_auto_update(db: &Db, client: &reqwest::Client, dir: &Path, force: bool) -> Result<bool>` — `force=false`: hormati `update_check_enabled` (bawaan aktif) dan jarak ≥ 24 jam sejak `video_tools_last_check`; `force=true`: selalu cek (jaring pengaman). Ada baru → unduh + verifikasi + ganti atomik (rename boleh gagal → tunggu 200 ms, maks 25×); simpan waktu cek.

- [ ] **Step 1: Test gagal**: `parse_sha256_sums_parses_real_format`, `is_ytdlp_newer_compares_date_tags` (`2026.11.10` > `2026.10.30`, `.1` tie-break, sampah = false), `tools_paths_lay_under_dir`.
- [ ] **Step 2:** FAIL. **Step 3:** implementasi (logika unduh+verifikasi jadi fungsi kecil `verify_and_write(bytes, expected)` yang diuji; jaringan nyata: satu test `#[ignore]` `install_downloads_and_verifies_real_components`).
- [ ] **Step 4:** `cargo test -p unduhkilat-core --lib tools` PASS; `cargo test -p unduhkilat-core --lib -- --ignored tools` dijalankan SEKALI manual saat jaringan tersedia. Commit `feat(video): pengelola komponen yt-dlp dan ffmpeg`.

### Task 6: Dispatch engine — `run_video` + integrasi palsu

**Files:**
- Modify: `crates/unduhkilat-core/src/engine/transfer.rs` (`run_inner`: setelah cek magnet/remote, `if let Some(v) = ytdlp::parse(&dl.original_url) { return ytdlp::run_video(...) }`).
- Modify: `crates/unduhkilat-core/src/engine/ytdlp.rs` — tambah `run_video`, `probe_metadata`, Job Object kill (`cfg(windows)`, feature `Win32_System_JobObjects` di `windows-sys`).
- Modify: `crates/unduhkilat-core/src/engine/manager.rs` — `create_inner`: `auto_wrap` sebelum validasi skema; URL berawalan `video:` lolos validasi skema; placeholder nama `video.mp4`/`audio.mp3` bila kosong.
- Test: Create: `crates/unduhkilat-core/tests/video.rs`.

**Interfaces:**
- Consumes: Task 2/3/5; `TransferCtx.cfg.tools_dir`.
- Produces:
  - `EngineConfig.video_ytdlp_name: String` — default `"yt-dlp.exe"`; **hanya untuk test** agar `run_video` bisa men-spawn skrip `.cmd` palsu (dokumentasikan di field).
  - `pub struct VideoMeta { pub title: String, pub duration: Option<i64>, pub heights: Vec<u32> }`; `pub async fn probe_metadata(exe: &Path, url: &str) -> Result<VideoMeta>` (`-J --no-playlist`; JSON `title`, `duration`, `formats[].height` unik terurut menurun).
  - `pub async fn run_video(ctx, id, req: &VideoRequest, cancel, bucket) -> Result<Outcome>` — alur: (1) tools_dir/yt-dlp tidak ada → `ERR_VIDEO_TOOLS_MISSING`; (2) `-J` metadata (judul untuk nama akhir; gagal jaring → `map_error`); (3) nama akhir = `file_name` DB bila bukan placeholder, else `final_name(judul)`; collision → `ERR_NAME_COLLISION`; (4) folder kerja `<tujuan>/.uk-video-<id>/`, spawn yt-dlp dalam Job Object (KILL_ON_JOB_CLOSE), `CREATE_NO_WINDOW`; (5) progres: jumlah sampai `total` kedua berkas (deteksi reset `downloaded`), event `Progress`; setelah baris unduhan habis → `Progress { downloaded=total, speed=0, phase:Some("menggabungkan") }`; (6) sukses: hasil stub `.mp4/.mp3` dicek collision lalu dipindah ke nama akhir; baris DB diperbarui (`target_path`, `file_name`, `total_bytes`, `downloaded_bytes=total`); folder kerja dihapus; (7) yt-dlp exit ≠ 0 ATAU hasil tidak ada → `map_error`; bila `is_stale_error` → `tools::maybe_auto_update(force=true)` lalu ulang (maks 1×); masih gagal → error tsb; (8) cancel → kill job, simpan progres, `Outcome::Stopped`.
  - `total_bytes` ditulis ke DB begitu diketahui (dari metadata `-J` bila ada, else dari progres).

- [ ] **Step 1: Test integrasi gagal** (`tests/video.rs`; `Env::with_cfg` diset `tools_dir: Some(<temp>/tools)`, `video_ytdlp_name: "fake-ytdlp.cmd"`; test menulis `.cmd` yang mencetak progres dan membuat berkas — offline, tanpa jaringan): `video_download_completes_and_lands_under_final_name` (fake mencetak `UK 500 1000 8000` lalu `UK 1000 1000 8000`, membuat `out.mp4` di stub; item selesai, `file_name` = judul, berkas ada di nama akhir, folder kerja hilang), `video_without_tools_fails_with_tools_missing` (tools_dir `None`), `video_login_error_maps_and_fails` (fake exit 1, stderr "Sign in to confirm your age" → `ERR_VIDEO_LOGIN_REQUIRED`), `missing_output_fails` (exit 0 tanpa berkas → `ERR_VIDEO_EXTRACT_FAILED`), `cancel_stops_and_pauses` (fake lambat; cancel → item berhenti, bukan selesai), `collision_at_final_name_refuses` (berkas judul sudah ada → gagal `ERR_NAME_COLLISION`, berkas asli utuh).
- [ ] **Step 2:** FAIL. **Step 3:** implementasi `run_video` + dispatch + manager. **Step 4:** `cargo test -p unduhkilat-core --test video` PASS; `cargo test -p unduhkilat-core --test engine --test torrent` masih PASS. Commit `feat(video): jalankan unduhan video di antrian`.

### Task 7: UI aplikasi — dialog Tambah, tawaran komponen, Pengaturan, boot

**Files:**
- Modify: `crates/unduhkilat-app/src/app.rs` (AddDialog + `Msg` + Pengaturan), `crates/unduhkilat-app/src/boot.rs` (set `cfg.tools_dir = Some(paths.data_dir.join("tools"))`, `cfg.video_ytdlp_name` default), spawn `tools::maybe_auto_update` harian di tempat `restore_seeding` dipanggil.

**Interfaces:**
- Consumes: `ytdlp::{is_video_host, wrap, parse, probe_metadata, VideoKind}`, `tools::{Tools, versions, install, maybe_auto_update}`.
- Produces ( perilaku):
  - AddDialog: URL host video → baris pilihan `Format: (•) Video ( ) Audio MP3`, `Kualitas: [Terbaik ▾]` (tinggi dari probe; sebelum probe: Terbaik), `MP3: [192 kbps ▾]` (128/192/320, tampil bila Audio); Periksa menjalankan `probe_metadata` → `Msg::VideoProbe`; judul mengisi "Simpan Sebagai" (bisa diubah).
  - Periksa pada link non-video yang MIME-nya `text/html` → tombol `Coba sebagai video` (menjalankan probe yt-dlp untuk link itu; bila berhasil, url di dialog diganti `video:v=best|<url>`).
  - Mulai/Periksa saat komponen belum ada → tawaran inline "Unduh komponen video (~90 MB)?" + tombol `Unduh komponen` (progres di baris probe, `Msg::ToolsNote`); menolak → tidak ada item dibuat, pesan jelas. Setelah terpasang: "Komponen video siap — klik Periksa atau Mulai lagi."
  - Saat Mulai: url host video dibungkus `wrap(url, kind)` dengan pilihan dialog sebelum `manager.create`.
  - Pengaturan → bagian Video: versi terpasang (yt-dlp/ffmpeg), tombol `Perbarui komponen video` (install ulang/terbaru).
  - CLI tidak diubah (auto-wrap lewat `manager.create`; tanpa komponen → gagal jelas `ERR_VIDEO_TOOLS_MISSING`).

- [ ] **Step 1:** `cargo build --workspace` + `cargo test --workspace` PASS; jalankan aplikasi (`UNDUHKILAT_DATA_DIR` sementara): tempel link YouTube → dialog menampilkan pilihan Format/Kualitas; tanpa komponen muncul tawaran; tolak → tidak ada item. (Verifikasi manual — UI punya cakupan otomatis minim; dokumentasikan hasil di catatan commit.)
- [ ] **Step 2:** Commit `feat(video): dialog tambah, tawaran komponen, dan pengaturan video`.

### Task 8: Ekstensi browser — kandidat "Video halaman ini"

**Files:**
- Modify: `extension/shared.js`; Test: `extension/tests/` (node --test).

**Interfaces:**
- Produces: pada host video yang dikenal (daftar sama dengan core, dikembalikan lewat `ping`/daftar statis di shared.js): kandidat potongan `googlevideo.com` disembunyikan; satu kandidat baru **"Video halaman ini"** berisi URL HALAMAN (bukan stream) mengikuti alur proposal yang ada.

- [ ] **Step 1: Test gagal** (`node --test extension/tests`): test baru memastikan (a) kandidat googlevideo difilter di host video, (b) kandidat halaman ditambahkan sekali, (c) host non-video tidak berubah.
- [ ] **Step 2:** FAIL. **Step 3:** implementasi di shared.js. **Step 4:** PASS. Commit `feat(video): kandidat video halaman di ekstensi`.

### Task 9: Dokumen keputusan + verifikasi penuh

**Files:**
- Modify: `docs/16_DECISIONS.md` (DEC-015: ringkasan §4–§5 spec, rujuk silang FR-044), `docs/17_TASKS.md` bila perlu.

- [ ] **Step 1:** Tulis DEC-015 (gaya entri lain: pertanyaan, jawaban pengguna, catatan implementasi).
- [ ] **Step 2:** `cargo test --workspace` PASS + `cargo clippy --workspace --all-targets` tanpa warning baru + `node --test extension/tests` PASS.
- [ ] **Step 3:** Commit `docs: DEC-015 unduh video via yt-dlp`.

**Setelah Task 9:** rilis memakai skill `rilis` (bump versi, build, zip, tanda tangan, GitHub pre-release) dengan catatan rilis bahasa Indonesia.
