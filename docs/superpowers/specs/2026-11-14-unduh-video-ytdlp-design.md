# Desain: Unduh Video dan MP3 dari YouTube dan Situs Video Lain (yt-dlp + ffmpeg)

Tanggal: 2026-11-14. Status: draf untuk persetujuan pengguna.

## 1. Maksud dan alasan

Link seperti `https://www.youtube.com/watch?v=DjJbAHis1uQ` gagal diunduh, baik sebagai video maupun MP3.
Penyebabnya, UnduhKilat tidak punya ekstraktor situs video:

- Halaman `watch?v=` adalah HTML. Video YouTube dikirim sebagai potongan DASH bertanda tangan dari
  `googlevideo.com`, dengan video dan audio terpisah, dibatasi `range`, dan URL-nya cepat kedaluwarsa.
- Kalau link ditempel di dialog Tambah, yang terunduh hanya HTML atau unduhan gagal. Kalau ditangkap
  ekstensi, yang tertangkap hanya potongan stream yang ditolak saat diambil ulang di luar browser.
- Penggabungan DASH/HLS di dokumen berstatus `Ditunda` (FR-044, TASK-44 / TBD-016).

Pengguna ingin link video publik dari YouTube dan situs sejenis (Reels Instagram, Facebook, TikTok, X, dan
lain-lain) bisa diunduh sebagai **video** atau **MP3** dari dialog Tambah, dengan perilaku antrian yang sama
seperti unduhan lain.

## 2. Tujuan, bukan tujuan, anti-tujuan

**Tujuan**
- Link video publik dari situs yang didukung yt-dlp bisa diunduh sebagai video MP4 atau audio MP3.
- Item video masuk ke antrian biasa: prioritas, pause/resume, batal, progres, dan kategori ikut berlaku.
- Komponen (yt-dlp, ffmpeg) didapat atas persetujuan pengguna, dan yt-dlp diperbarui otomatis supaya
  unduhan tidak rusak setiap kali YouTube berubah.

**Bukan tujuan (versi pertama)**
- Konten yang butuh login (cookie lewat ekstensi). Ditunda, dan pengguna sudah setuju.
- Playlist atau channel utuh. Link `watch?v=...&list=...` hanya mengunduh video itu (`--no-playlist`).
- Subtitle, thumbnail, metadata embed, potong video, dan siaran langsung.
- Mesin segmen UnduhKilat untuk stream video (pendekatan B/C, lihat §4).

**Anti-tujuan (dianggap gagal walau secara teknis jalan)**
- Ada aktivitas jaringan yang tidak diminta pengguna, di luar pengecekan pembaruan yang tunduk pada setelan
  "Cek pembaruan".
- Berkas yang sudah ada tertimpa (kontraknya `ERR_NAME_COLLISION`).
- Jendela konsol hitam muncul saat yt-dlp atau ffmpeg berjalan.
- Unduhan berkas biasa (HTTP/FTP/torrent) berubah perilaku.
- Hasil video tidak bisa diputar di pemutar standar Windows.

## 3. Batasan

- Windows 10/11 x64 saja (DEC-005), ZIP portabel (DEC-007). Data portabel ada di `./data`.
- Kode UnduhKilat tertutup (`UNLICENSED`). yt-dlp berlisensi Unlicense. ffmpeg harus build **LGPL**,
  dijalankan sebagai proses terpisah dan tidak di-link.
- Tidak ada telemetri. Jaringan hanya dipakai untuk: unduhan yang diminta pengguna, pengambilan komponen
  yang disetujui pengguna, dan pengecekan pembaruan yt-dlp (ikut setelan `update_check_enabled`).
- Komentar kode, string UI, dan commit dalam bahasa Indonesia.

## 4. Pendekatan yang dipertimbangkan

| | Pendekatan | Untung | Rugi |
|---|---|---|---|
| **A (dipilih)** | yt-dlp mengunduh semuanya sebagai proses anak; UnduhKilat membaca progres dan mengatur antrian | Paling tahan perubahan YouTube; perbaikan datang dari pembaruan otomatis yt-dlp | Tidak memakai mesin multi-koneksi; batas kecepatan global hanya didekati |
| B | yt-dlp `-J` mencari URL stream, mesin segmen UnduhKilat mengunduh, ffmpeg menggabung | Memakai segmen, throttle, dan proxy UnduhKilat apa adanya | Sering rusak (SABR, PO token, URL kedaluwarsa); perbaikan tidak ikut pembaruan yt-dlp |
| C | B bila format berupa https langsung, A bila tidak | Kecepatan terbaik | Dua jalur yang harus dirawat dan diuji |

Pengguna memilih **A**, karena keluhannya adalah "masih gagal". Keandalan lebih penting daripada kecepatan
maksimal.

Untuk sumber komponen juga dipertimbangkan: (1) unduh saat pertama dipakai, (2) dibundel di ZIP, dan
(3) pengguna menyediakan path sendiri. Pengguna memilih **(1)**, dengan pembaruan yt-dlp **otomatis**.

## 5. Desain yang sudah diputuskan pengguna

### 5.1 Komponen video (yt-dlp dan ffmpeg)
- Saat link video pertama kali dipakai (Periksa atau Mulai), muncul tawaran
  "Unduh komponen video (~90 MB)?". Kalau pengguna menolak, item tidak dibuat dan pesannya jelas.
- Sumber hanya rilis resmi: yt-dlp dari `github.com/yt-dlp/yt-dlp/releases` (`yt-dlp.exe`), dicocokkan
  dengan `SHA2-256SUMS` dari rilis yang sama. ffmpeg dari build LGPL win64 (lihat §6).
- Lokasi: `<data>/tools/yt-dlp.exe` dan `<data>/tools/ffmpeg.exe` (dan `ffprobe.exe`). `<data>` mengikuti
  `paths.rs`, jadi portabel atau `%LOCALAPPDATA%`.
- **Pembaruan otomatis yt-dlp**:
  - Ikut setelan "Cek pembaruan" (`update_check_enabled`). Kalau setelan itu mati, tidak ada pengecekan.
  - Paling sering sekali per 24 jam. Waktu cek terakhir disimpan di setelan. Tag rilis terbaru dibandingkan
    dengan `yt-dlp --version` (format tanggal `YYYY.MM.DD[.n]`).
  - Kalau ada versi baru, unduh di latar belakang, verifikasi SHA-256, lalu ganti secara atomik. Penggantian
    menunggu sampai tidak ada proses yt-dlp yang berjalan.
- **Jaring pengaman**: kalau yt-dlp gagal dengan tanda versi usang (misalnya "nsig extraction failed",
  "Unable to extract", "HTTP Error 403", atau "Sign in to confirm you're not a bot"), aplikasi mengecek
  pembaruan sekali, memperbarui bila ada, lalu mencoba ulang item itu sekali. Ini tetap berjalan walau cek
  harian dimatikan, karena dipicu unduhan yang diminta pengguna.
- ffmpeg tidak dicek otomatis. Pengaturan punya tombol "Perbarui komponen video" (yt-dlp dan ffmpeg)
  serta tampilan versi terpasang.

### 5.2 Kapan link diproses lewat yt-dlp
- Host yang dikenal (termasuk subdomain `www.`, `m.`, `music.`): youtube.com, youtu.be, instagram.com,
  facebook.com, fb.watch, tiktok.com, x.com, twitter.com, vimeo.com, dailymotion.com, twitch.tv.
  Link dari host ini **otomatis** lewat yt-dlp.
- Link lain tetap jadi unduhan berkas biasa. Kalau **Periksa** menemukan bahwa respons berupa `text/html`,
  muncul tombol **"Coba sebagai video"** yang menjalankan pemeriksaan yt-dlp untuk link itu.

### 5.3 Dialog "Mulai Unduhan Baru"
Tombol **Periksa** untuk link video menjalankan `yt-dlp -J --no-playlist <url>`, lalu menampilkan:

```
[Periksa]  YouTube | "Judul Video" | 4:12

Format:  (•) Video   ( ) Audio MP3
Kualitas: [ Terbaik (1080p) ▾ ]     ← daftar tinggi yang tersedia: 2160/1440/1080/720/480/360
          MP3: [ 192 kbps ▾ ]       ← muncul kalau Audio MP3 dipilih (128/192/320)
Simpan Sebagai: Judul Video.mp4     ← diisi dari judul, bisa diubah; ekstensi ikut format
```

- Tanpa Periksa, bawaannya Video kualitas terbaik, dan nama berkas diambil dari judul saat diunduh.
- Video selalu dikeluarkan sebagai **MP4**. Utamakan H.264 + AAC (m4a). Kalau resolusi yang dipilih hanya
  ada dalam VP9/AV1, gabungkan ke MP4 apa adanya (pemutar Windows modern bisa memutarnya). Ini panggilan
  saya, lihat §7.
- Audio MP3 memakai `-x --audio-format mp3 --audio-quality <kbps>K`.

### 5.4 Eksekusi di antrian (pendekatan A)
- Item video adalah unduhan biasa di tabel `downloads`, dengan `original_url` diberi awalan khusus (§7).
  `run_inner` di `engine/transfer.rs` mendeteksi awalan itu, seperti magnet dan `server://`, lalu memanggil
  `engine/ytdlp.rs::run_video`. Item berjalan di lajur unduhan biasa, bukan lajur server.
- yt-dlp dijalankan dengan `CREATE_NO_WINDOW`, `--newline`, `--progress-template` yang bisa diurai
  (byte terunduh, total, kecepatan), `--ffmpeg-location <data>/tools`, `--no-playlist`, `--no-overwrites`,
  `--continue`, dan `-N 4`. Opsi `--proxy` dan `--limit-rate` diambil dari setelan UnduhKilat.
- Progres diubah menjadi `EngineEvent::Progress` yang sama dengan unduhan lain. Video dan audio YouTube
  diunduh terpisah, jadi progres total = jumlah kedua bagian, dan fase "Menggabungkan…" ditampilkan
  sebagai langkah terakhir.
- **Pause / batal**: hentikan proses beserta anaknya (ffmpeg) memakai Job Object Windows. **Resume**:
  jalankan ulang dengan argumen yang sama. yt-dlp melanjutkan dari `.part`.
- **Tabrakan nama**: sebelum mulai dan sebelum hasil akhir dipindah, kalau berkas tujuan sudah ada, gagal
  dengan `ERR_NAME_COLLISION`. yt-dlp menulis ke folder kerja sementara di folder tujuan, lalu hasilnya
  dipindah ke nama akhir.
- **Kesalahan** dipetakan ke kode stabil: komponen belum ada / ditolak, konten butuh login atau privat
  ("Perlu login; versi ini hanya mendukung konten publik"), video tidak tersedia, dan kegagalan ekstraksi
  lain beserta baris error terakhir dari yt-dlp.

## 6. Hal teknis yang perlu diverifikasi oleh pembangun
- Sumber ffmpeg: pilih build win64 **LGPL** yang menyertakan encoder MP3 (libmp3lame), misalnya
  `BtbN/FFmpeg-Builds` varian `win64-lgpl`, atau varian LGPL dari `yt-dlp/FFmpeg-Builds`. Periksa
  ketersediaan libmp3lame dan checksum yang ditandatangani atau disediakan. Kalau tidak ada checksum resmi,
  pin versi beserta SHA-256-nya di kode.
- Ukuran total komponen (perkiraan ~90 MB) diukur dan ditampilkan apa adanya.
- `-N 4` (fragmen bersamaan) dan pembagian `--limit-rate` per proses: ukur apakah batas global tetap
  wajar saat beberapa item video berjalan.

## 7. Panggilan saya (pengguna bisa mengoreksi)
- **Bentuk URL item**: `video:<opsi>|<url asli>`, misalnya `video:v=1080|https://www.youtube.com/watch?v=…`
  atau `video:a=mp3-192|…`. Polanya sama dengan awalan `server://` yang sudah ada, dan tidak butuh migrasi
  skema. Pembangun boleh memilih kolom terpisah kalau ternyata lebih bersih.
- **Kode error baru**: `ERR_VIDEO_TOOLS_MISSING`, `ERR_VIDEO_LOGIN_REQUIRED`,
  `ERR_VIDEO_EXTRACT_FAILED`. `ERR_MEDIA_PARSE_FAILED` tetap untuk manifest yang disniff ekstensi.
- **Ekstensi browser (minimal)**: di host video yang dikenal, sembunyikan kandidat potongan
  `googlevideo.com` (selalu gagal) dan tawarkan satu kandidat **"Video halaman ini"** yang mengirim URL
  halaman ke aplikasi lewat alur proposal yang sudah ada.
- **Mode CLI** (`UnduhKilat.exe cli <url>`) ikut memakai jalur yang sama untuk host video, asalkan
  komponen sudah terpasang. CLI tidak mengunduh komponen tanpa ditanya.
- **Keputusan dicatat** sebagai DEC-015 di `docs/16_DECISIONS.md`, dan FR-044 dirujuk silang.
- **Pengujian**: unit test untuk deteksi host, pengurai baris progres, pembentukan argumen, dan pemetaan
  error dari contoh keluaran yt-dlp. Integration test memakai yt-dlp palsu (skrip/exe kecil yang mencetak
  baris progres) supaya tetap offline. Uji jaringan nyata diberi `#[ignore]`, seperti `search`.

## 8. Diserahkan ke pembangun
- Tata letak persis kontrol di dialog dan di Pengaturan, mengikuti `DESIGN.md` ("Paket Ekspres").
- Rincian pengurai `--progress-template` dan pola teks error yang dicocokkan.
- Mekanisme penggantian atomik yt-dlp.exe (tulis `.new` lalu rename, dengan cadangan).
- Struktur modul di dalam `engine/ytdlp.rs` dan bagian unduh komponen (misalnya `core/src/tools.rs`).
