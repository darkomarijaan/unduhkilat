# Potong Video Saat Unduh — Rencana Implementasi

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Isian Dari/Sampai di dialog Tambah untuk link video, sehingga hanya rentang itu yang diunduh lewat `yt-dlp --download-sections`.

**Architecture:** Rentang disimpan di bentuk URL item (`video:v=1080;t=90-165|url`) dan diurai ke `VideoRequest.section`. `build_download_args` menambah `--download-sections`, `final_name` menambah akhiran rentang, `run_video` mewajibkan ffmpeg dan selalu mulai dari folder kerja kosong untuk item potongan. Dialog memvalidasi isian lewat fungsi core yang bisa dites.

**Tech Stack:** Rust, egui 0.37, yt-dlp + ffmpeg (proses anak).

**Spec:** `docs/superpowers/specs/2026-11-17-potong-video-design.md` (melanjutkan `2026-11-14-unduh-video-ytdlp-design.md`).

## Global Constraints

- Komentar kode, string UI, dan commit dalam bahasa Indonesia; format commit `feat(video): ...`.
- Bentuk URL lama tanpa `;t=` tetap terbaca persis seperti sekarang (tanpa migrasi skema).
- Item video tanpa potongan tidak berubah perilaku: argumen, nama berkas, dan resume `.part` tetap sama.
- Jangan pernah menimpa berkas yang ada (`ERR_NAME_COLLISION`).
- Tanpa `--force-keyframes-at-cuts` (tanpa encode ulang).
- Potongan tanpa `ffmpeg.exe` di folder tools → `ERR_VIDEO_TOOLS_MISSING`.

## Review Focus

1. Isian dengan spasi di tepi (`" 1:30 "`) → dianggap valid setelah di-trim. (Tes di Task 1.)
2. Dari `0:00` dengan Sampai kosong → berarti seluruh video, item dibuat TANPA `;t=`. (Tes di Task 1.)
3. Sampai melebihi durasi hasil Periksa → diperlakukan sampai akhir, bukan error. (Tes di Task 1.)
4. Item potongan yang dijeda lalu dilanjutkan tidak memakai `.part` lama. (Tes integrasi di Task 2.)
5. Pengguna mengubah rentang setelah Periksa → nama berkas otomatis ikut rentang terakhir, tetapi nama yang diketik sendiri tidak diubah. (Verifikasi manual di Task 3.)

---

### Task 1: Rentang di bentuk URL dan validasi isian (core)

**Files:**
- Modify: `crates/unduhkilat-core/src/engine/ytdlp.rs` (tipe di atas, `wrap`/`auto_wrap`/`parse` baris 43–78, tes di modul `tests`)

**Interfaces:**
- Produces:
  - `pub struct Section { pub from: Option<u32>, pub to: Option<u32> }` — `#[derive(Debug, Clone, Copy, PartialEq, Eq)]`, detik bulat; invarian: minimal satu `Some`, dan `from < to` bila keduanya ada.
  - `VideoRequest` mendapat field `pub section: Option<Section>`.
  - `pub fn wrap(url: &str, kind: &VideoKind, section: Option<Section>) -> String` (tanda tangan berubah; `auto_wrap` memanggilnya dengan `None`).
  - `pub fn parse_clock(s: &str) -> Option<u32>` — `ss`, `m:ss`, `h:mm:ss`; di-trim; bagian setelah `:` pertama harus < 60; maksimal 3 bagian; bagian kosong/non-digit → `None`.
  - `pub fn section_from_inputs(from: &str, to: &str, duration: Option<i64>) -> std::result::Result<Option<Section>, String>`

- [ ] **Step 1: Tulis tes yang gagal**

Perbarui konstruksi `VideoRequest { .. }` di tes lama dengan `section: None`, lalu tambahkan:

```rust
#[test]
fn parse_clock_accepts_seconds_minutes_hours() {
    assert_eq!(parse_clock("90"), Some(90));
    assert_eq!(parse_clock(" 1:30 "), Some(90));
    assert_eq!(parse_clock("1:02:03"), Some(3723));
    assert_eq!(parse_clock("0:05"), Some(5));
    for bad in ["", "1:60", "1:2:60", "a", "1::2", "-5", "1:2:3:4", ":30"] {
        assert_eq!(parse_clock(bad), None, "{bad}");
    }
}

#[test]
fn parse_and_wrap_roundtrip_with_section() {
    let s = Some(Section { from: Some(90), to: Some(165) });
    let k = VideoKind::Video { max_height: Some(1080) };
    let w = wrap("https://youtu.be/x", &k, s);
    assert_eq!(w, "video:v=1080;t=90-165|https://youtu.be/x");
    assert_eq!(parse(&w).unwrap().section, s);
    let open_end = parse("video:a=mp3-192;t=90-|https://a").unwrap();
    assert_eq!(open_end.section, Some(Section { from: Some(90), to: None }));
    assert_eq!(open_end.kind, VideoKind::AudioMp3 { kbps: 192 });
    let open_start = parse("video:v=best;t=-165|https://a").unwrap();
    assert_eq!(open_start.section, Some(Section { from: None, to: Some(165) }));
    assert_eq!(parse("video:v=best|https://a").unwrap().section, None);
    assert_eq!(wrap("https://a", &VideoKind::Video { max_height: None }, None), "video:v=best|https://a");
}

#[test]
fn parse_rejects_bad_sections() {
    assert!(parse("video:v=best;t=-|https://a").is_none());
    assert!(parse("video:v=best;t=165-90|https://a").is_none());
    assert!(parse("video:v=best;t=90-90|https://a").is_none());
    assert!(parse("video:v=best;t=x-9|https://a").is_none());
    assert!(parse("video:v=best;q=1|https://a").is_none());
}

#[test]
fn section_from_inputs_validates_and_normalizes() {
    assert_eq!(section_from_inputs("", "", None), Ok(None));
    assert_eq!(section_from_inputs("0:00", "", None), Ok(None));
    assert_eq!(section_from_inputs(" 1:30 ", "2:45", None), Ok(Some(Section { from: Some(90), to: Some(165) })));
    assert_eq!(section_from_inputs("", "2:45", None), Ok(Some(Section { from: None, to: Some(165) })));
    assert_eq!(section_from_inputs("1:30", "10:00", Some(300)), Ok(Some(Section { from: Some(90), to: None })));
    assert!(section_from_inputs("2:45", "1:30", None).is_err());
    assert!(section_from_inputs("1:30", "1:30", None).is_err());
    assert!(section_from_inputs("1:61", "", None).is_err());
    assert!(section_from_inputs("6:00", "", Some(300)).is_err());
}
```

- [ ] **Step 2: Jalankan, pastikan gagal kompilasi/gagal**

Run: `cargo test -p unduhkilat-core --lib ytdlp`
Expected: FAIL (`Section`, `parse_clock`, `section_from_inputs` belum ada).

- [ ] **Step 3: Implementasi**

- `parse`: pisahkan opsi dengan `split_once(';')`; bagian dasar diurai seperti sekarang; ekstra hanya boleh `t=<a>-<b>` (lainnya → `None`). `from = Some(0)` disimpan apa adanya oleh `parse` (normalisasi hanya di `section_from_inputs`).
- `wrap`: tambah `;t={from or ""}-{to or ""}` bila `section` ada.
- `section_from_inputs`: kosong (setelah trim) = `None`; parse gagal → `Err("Format waktu tidak valid (pakai ss, m:ss, atau h:mm:ss)")`; `from == Some(0)` → `None`; `to >= duration` → `to = None`; `from >= duration` → `Err("Dari melewati durasi video")`; `from >= to` → `Err("Dari harus lebih kecil dari Sampai")`; keduanya `None` setelah normalisasi → `Ok(None)`.
- Perbarui pemanggil `wrap` di `crates/unduhkilat-app/src/app.rs:1917` dengan `None` sementara (Task 3 mengisinya) supaya workspace tetap terkompilasi.

- [ ] **Step 4: Jalankan tes, pastikan lulus**

Run: `cargo test -p unduhkilat-core --lib ytdlp` lalu `cargo build --workspace`
Expected: semua tes ytdlp PASS, build sukses.

- [ ] **Step 5: Commit**

```bash
git add crates/unduhkilat-core/src/engine/ytdlp.rs crates/unduhkilat-app/src/app.rs
git commit -m "feat(video): rentang potong di bentuk URL video dan validasi isian"
```

---

### Task 2: Argumen, nama berkas, dan eksekusi potongan (core)

**Files:**
- Modify: `crates/unduhkilat-core/src/engine/ytdlp.rs` (`build_download_args` ±140, `final_name` ±206, `run_video` ±323–405)
- Modify: `crates/unduhkilat-app/src/app.rs:621` (pemanggil `final_name`, isi `None`)
- Test: `crates/unduhkilat-core/tests/video.rs`

**Interfaces:**
- Consumes: `Section`, `VideoRequest.section`, `wrap` dari Task 1.
- Produces: `pub fn final_name(title: &str, kind: &VideoKind, section: Option<Section>) -> String` (tanda tangan berubah).

- [ ] **Step 1: Tulis tes unit yang gagal** (modul `tests` di `ytdlp.rs`)

```rust
#[test]
fn args_add_download_sections_only_when_cut() {
    let mut req = VideoRequest { url: "https://youtu.be/1".into(), kind: VideoKind::Video { max_height: None }, section: None };
    let s = build_download_args(&req, Path::new("C:/w/out"), Path::new("C:/t"), true, None, 0).join(" ");
    assert!(!s.contains("--download-sections"), "{s}");
    req.section = Some(Section { from: Some(90), to: Some(165) });
    let a = build_download_args(&req, Path::new("C:/w/out"), Path::new("C:/t"), true, None, 0);
    let i = a.iter().position(|x| x == "--download-sections").expect("ada");
    assert_eq!(a[i + 1], "*90-165");
    assert!(!a.iter().any(|x| x == "--force-keyframes-at-cuts"));
    assert_eq!(&a[a.len() - 2..], ["--".to_string(), "https://youtu.be/1".to_string()]);
    req.section = Some(Section { from: Some(90), to: None });
    assert!(build_download_args(&req, Path::new("C:/w/out"), Path::new("C:/t"), true, None, 0).contains(&"*90-inf".to_string()));
    req.section = Some(Section { from: None, to: Some(165) });
    assert!(build_download_args(&req, Path::new("C:/w/out"), Path::new("C:/t"), true, None, 0).contains(&"*0-165".to_string()));
}

#[test]
fn final_name_appends_the_range() {
    let v = VideoKind::Video { max_height: None };
    assert_eq!(final_name("Judul", &v, None), "Judul.mp4");
    assert_eq!(final_name("Judul", &v, Some(Section { from: Some(90), to: Some(165) })), "Judul [01.30-02.45].mp4");
    assert_eq!(final_name("Judul", &VideoKind::AudioMp3 { kbps: 192 }, Some(Section { from: Some(90), to: None })), "Judul [01.30-akhir].mp3");
    assert_eq!(final_name("Judul", &v, Some(Section { from: None, to: Some(3723) })), "Judul [awal-1.02.03].mp4");
}
```

Tes `final_name` lama (`final_name_sanitizes_and_truncates`) diperbarui dengan argumen `None`.

- [ ] **Step 2: Tulis tes integrasi yang gagal** (`tests/video.rs`)

Tambahkan helper `fn cut_req(env: &Env) -> NewDownload` = `video_req` dengan `url: "video:v=best;t=10-20|https://example.com/watch?v=ok"`, dan helper yang menulis berkas kosong `ffmpeg.exe` ke folder tools. Tes:

- `cut_video_lands_under_name_with_range` (mode `ok`, dengan `ffmpeg.exe` dummy dan `meta.json` judul "Judul Video"): state `Completed`, berkas `videos/Judul Video [00.10-00.20].mp4` ada.
- `cut_video_without_ffmpeg_fails_with_tools_missing` (mode `ok`, tanpa `ffmpeg.exe`): state `Error`, `error_code` = kode `ERR_VIDEO_TOOLS_MISSING` (bandingkan dengan cara tes lain di berkas ini memeriksa kode error).
- `paused_cut_restarts_without_old_part` (mode `resume`, dengan `ffmpeg.exe` dummy): ikuti alur `pause_keeps_the_partial_download_for_resume`, lalu lanjutkan; setelah `Completed`, assert `!tools.join("resume-saw-part.txt").exists()`.

- [ ] **Step 3: Jalankan, pastikan gagal**

Run: `cargo test -p unduhkilat-core --lib ytdlp` dan `cargo test -p unduhkilat-core --test video cut`
Expected: FAIL.

- [ ] **Step 4: Implementasi**

- `build_download_args`: bila `req.section` ada, sisipkan `--download-sections` dan `*{from or 0}-{to or "inf"}` sebelum `--`.
- `final_name`: akhiran ` [<dari>-<sampai>]` sebelum ekstensi; tiap ujung `< 3600` → `MM.SS` (dua digit), `>= 3600` → `H.MM.SS`; ujung kosong → `awal`/`akhir`. Pemotongan 120 karakter tetap berlaku pada judul saja (akhiran tidak ikut terpotong).
- `run_video`: (a) cek ffmpeg untuk potongan sama seperti MP3: `req.section.is_some() && !has_ffmpeg` → `ERR_VIDEO_TOOLS_MISSING` "ffmpeg belum terpasang; potong video butuh ffmpeg"; (b) `final_name(title, &req.kind, req.section)`; (c) sebelum `create_dir_all(&work)`, bila `req.section.is_some()`, `remove_dir_all(&work).ok()` dengan komentar bahwa ffmpeg tidak bisa meneruskan `.part`.
- `app.rs:621`: tambahkan argumen `None`.

- [ ] **Step 5: Jalankan tes, pastikan lulus**

Run: `cargo test -p unduhkilat-core --lib ytdlp` dan `cargo test -p unduhkilat-core --test video`
Expected: semua PASS (termasuk tes video lama).

- [ ] **Step 6: Commit**

```bash
git add crates/unduhkilat-core/src/engine/ytdlp.rs crates/unduhkilat-core/tests/video.rs crates/unduhkilat-app/src/app.rs
git commit -m "feat(video): unduh potongan lewat --download-sections dan nama berkas berakhiran rentang"
```

---

### Task 3: Isian Dari/Sampai di dialog Tambah (app)

**Files:**
- Modify: `crates/unduhkilat-app/src/app.rs` — `AddDialog` (±92–117), `VideoChoice` (±121–126), pembuatan `VideoChoice` (±356, ±1719), penanganan `Msg::VideoProbe` (±612–630), baris pilihan video (±1723–1745), tombol Mulai (±1875), pembungkusan saat submit (±1912–1920).

**Interfaces:**
- Consumes: `section_from_inputs`, `wrap(url, kind, section)`, `final_name(title, kind, section)` dari Task 1–2.

- [ ] **Step 1: State**

`VideoChoice` mendapat `cut_from: String` dan `cut_to: String` (kosong bawaan, di semua tempat `VideoChoice { .. }` dibuat). `AddDialog` mendapat `video_title: Option<String>` dan `video_duration: Option<i64>` (diisi di `Msg::VideoProbe` saat `Ok`, `None` di konstruktor).

- [ ] **Step 2: Baris UI**

Di bawah baris Format/Kualitas, di dalam blok `if a.video.is_some() && urls.len() == 1`:

```
Potong:  Dari [ 1:30 ]  Sampai [ 2:45 ]   kosong = seluruh video
```

Dua `TextEdit::singleline` lebar ±70 px dengan `hint_text("m:ss")`, lalu label abu-abu "kosong = seluruh video". Hitung `section_from_inputs(&v.cut_from, &v.cut_to, a.video_duration)`; bila `Err(msg)`, tampilkan `msg` di baris bawahnya dengan warna error yang sudah dipakai dialog untuk `a.err`.

- [ ] **Step 3: Mulai dan Antrikan dinonaktifkan saat rentang tidak valid**

Tombol "Mulai Mengunduh" dan tombol antrikan di sebelahnya memakai `add_enabled(cut_ok, ..)`, dengan `cut_ok = a.video.is_none() || section_from_inputs(..).is_ok()`.

- [ ] **Step 4: Submit**

Di pembungkusan (±1914): `let section = section_from_inputs(&v.cut_from, &v.cut_to, a.video_duration).ok().flatten();` lalu `wrap(&url, &kind, section)`. Nama: bila `a.video_title` ada dan `a.file_name.trim()` sama dengan `final_name(title, &kind, None)`, ganti dengan `final_name(title, &kind, section)` (nama hasil Periksa ikut rentang; nama yang diketik pengguna tidak diubah). Nama kosong tetap kosong (engine yang memberi nama, sudah berakhiran rentang dari Task 2).

- [ ] **Step 5: Build dan verifikasi manual**

Run: `cargo build --release` lalu `cargo test --workspace`
Expected: build sukses, semua tes PASS.

Jalankan app dengan data terpisah: `UNDUHKILAT_DATA_DIR=<scratch> UNDUHKILAT_NO_REGISTER=1 UNDUHKILAT_NO_AUTOLAUNCH=1 UNDUHKILAT_START_SCREEN=add target/release/UnduhKilat.exe`. Tempel link YouTube. Periksa:
- baris Potong muncul hanya untuk link video, hilang untuk link biasa;
- `2:45`/`1:30` → catatan merah dan Mulai nonaktif; `1:61` → catatan format;
- setelah Periksa, ubah rentang → submit, nama item `Judul [01.30-02.45].mp4`; ketik nama sendiri → nama itu dipakai apa adanya;
- (dengan komponen terpasang) unduh potongan pendek benar-benar menghasilkan klip sekitar rentang itu dan bisa diputar.

- [ ] **Step 6: Commit**

```bash
git add crates/unduhkilat-app/src/app.rs
git commit -m "feat(video): isian Dari/Sampai untuk potong video di dialog Tambah"
```
