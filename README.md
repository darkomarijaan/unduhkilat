# UnduhKilat

Manajer unduhan berkecepatan tinggi untuk Windows 10/11 (x64), ditulis dengan Rust. Portabel (cukup ZIP, tanpa instalasi), tanpa telemetri, dan tidak pernah memulai atau menimpa berkas tanpa konfirmasi Anda.

> **Status: pra-rilis (0.1.5).** Mesin unduhan, torrent, ekstensi browser, dan antarmuka sudah berfungsi dan teruji, tetapi beberapa bagian belum diuji di lingkungan nyata. Lihat [Status](#status) sebelum memakainya untuk hal penting. Binari belum ditandatangani kode, jadi Windows SmartScreen mungkin memperingatkan saat pertama dijalankan.

## Fitur

- **Unduhan bersegmen** HTTP/HTTPS dan FTP: beberapa koneksi per unduhan (tanpa batas atas), dilanjutkan dari byte terakhir setelah terputus atau aplikasi ditutup, jumlah koneksi dan kecepatan terlihat per paket.
- **Antrean** dengan prioritas lalu urutan masuk, batas unduhan bersamaan, batas kecepatan global dan per unduhan.
- **BitTorrent**: berkas `.torrent` dan tautan magnet, tracker, DHT, PEX, seeding dengan target rasio. Memakai [`librqbit`](https://github.com/ikatson/rqbit).
- **Pencarian berkas** dari Internet Archive, Academic Torrents, serta (opsional, mati secara bawaan) apibay dan Nyaa. Kata kunci hanya dikirim saat Anda menekan Cari.
- **Google Drive publik**: tombol di layar Cari membuka `site:drive.google.com ...` di browser Anda; tautan file Drive yang Anda tempel otomatis diubah menjadi unduhan langsung (folder tidak didukung). Belum diuji dengan file Drive publik nyata.
- **Server (tahap awal)**: layar Server menyimpan server SSH, SFTP, FTP, dan FTPS beserta kredensialnya (dienkripsi Windows), mendukung jump host berantai, verifikasi kunci host, dan tombol Uji sambungan. Penjelajah berkas dan pratinjau menyusul; tombolnya masih nonaktif.
- **Terminal**: tab terminal lokal (PowerShell, CMD, Git Bash, WSL; dibuka dari folder unduhan atau tombol Terminal) dan tab SSH lewat jump host, dengan seleksi/klik mouse, Ctrl+V, dan kompatibel dengan Claude Code. `Alt+V` (tempel gambar) menyusul.
- **1337x**: tombol "Cari di 1337x (browser)" di layar Cari membuka pencarian 1337x.to di browser Anda. Situs itu memakai perlindungan Cloudflare sehingga tidak bisa dicari dari dalam aplikasi; magnet yang Anda klik di sana diteruskan ke UnduhKilat bila aplikasi ini dipilih sebagai penangan magnet.
- **Site Grabber**: pindai satu halaman web, saring tautan dengan pola wildcard (`*.pdf`, `foto_???.jpg`, `*/a/*.zip`), pratinjau, lalu antrikan yang dicentang.
- **Ekstensi Chromium** (Chrome, Edge, Brave) lewat Native Messaging: tangkap tautan unduhan dan media. Selalu ada dialog konfirmasi; tidak ada yang dimulai otomatis.
- **Pemantau clipboard** (opsional, mati secara bawaan): menawarkan tautan HTTP/HTTPS/FTP yang Anda salin.
- **Proxy**: langsung, sistem, atau manual (HTTP/HTTPS/SOCKS5). Kata sandi disimpan terenkripsi DPAPI. Torrent hanya mau lewat SOCKS5 dan ditolak (bukan dialihkan langsung) bila proxy HTTP aktif.
- **Tanpa telemetri**, log lokal dengan redaksi data sensitif, tray, notifikasi selesai.
- **Antarmuka "Paket Ekspres"**: setiap unduhan dilacak seperti paket, dengan nomor resi dan lintasan lima langkah (Antre, Terhubung, Mengunduh, Verifikasi, Selesai). Lihat [DESIGN.md](DESIGN.md).

## Memakai

1. Unduh `UnduhKilat-portable-*.zip` dari halaman [Releases](https://github.com/darkomarijaan/unduhkilat/releases) dan ekstrak ke folder mana pun.
2. Jalankan `UnduhKilat.exe`. Data (database, log) disimpan di folder `data` di sampingnya karena ada berkas `unduhkilat-portable.marker`. Hapus marker itu untuk memakai `%LOCALAPPDATA%`.
3. Untuk integrasi browser, buka `chrome://extensions` (atau `edge://extensions`), aktifkan mode pengembang, lalu **Muat yang belum dikemas** dan pilih folder `extension`. Pendaftaran host native untuk akun Anda dibuat otomatis saat aplikasi dibuka. Jangan memindahkan `unduhkilat-host.exe` keluar dari folder aplikasi.

Pintasan: `Ctrl+N` menambah unduhan. Klik kanan sebuah paket untuk Jeda, Lanjutkan, Mulai ulang, Prioritas, dan Hapus.

### Perintah baris perintah

```
UnduhKilat.exe cli <url|magnet|berkas.torrent> [...] [--connections N] [--out FOLDER]
UnduhKilat.exe register-host | unregister-host            # host native untuk pengguna ini
UnduhKilat.exe --admin-register-host                       # HKLM, perlu Administrator
UnduhKilat.exe --admin-reset-data --confirm-reset          # hapus database dan log, perlu Administrator
```

Perintah `--admin-*` menolak dengan kode keluar 5 (`ERR_PERMISSION_DENIED`) tanpa hak Administrator.

### Variabel lingkungan (pengujian)

| Variabel | Fungsi |
|---|---|
| `UNDUHKILAT_DATA_DIR` | Folder data terpisah |
| `UNDUHKILAT_NO_REGISTER` | Jangan mendaftarkan host native ke registry |
| `UNDUHKILAT_NO_AUTOLAUNCH` | Host native tidak menjalankan aplikasi otomatis |
| `UNDUHKILAT_START_SCREEN` | Buka langsung di `settings`, `search`, `grabber`, `diagnostics`, atau `add` |

## Membangun dari sumber

Butuh Rust (MSVC, target `x86_64-pc-windows-msvc`) dan Node.js (hanya untuk tes ekstensi).

```
cargo build --release          # target/release/UnduhKilat.exe dan unduhkilat-host.exe
cargo test --workspace         # tes Rust
node --test extension/tests    # tes logika ekstensi
```

Tes jaringan nyata ditandai `#[ignore]`: `cargo test -p unduhkilat-core --test search -- --ignored --nocapture`.

Paket portabel: salin `UnduhKilat.exe`, `unduhkilat-host.exe`, folder `extension` (tanpa `tests`), dan berkas kosong `unduhkilat-portable.marker` ke satu folder.

## Struktur

| Jalur | Isi |
|---|---|
| `crates/unduhkilat-core` | Mesin: HTTP/FTP/torrent, antrean, SQLite, proxy, pencarian, grabber, jembatan native host |
| `crates/unduhkilat-app` | Aplikasi desktop (egui/eframe), tray, CLI |
| `crates/unduhkilat-host` | Native Messaging host untuk ekstensi |
| `extension/` | Ekstensi Chromium (Manifest V3) |
| `docs/` | Cetak biru produk: PRD, persyaratan, arsitektur, keamanan, API, daftar tugas ([mulai dari `18_README.md`](docs/18_README.md)) |
| `PRODUCT.md`, `DESIGN.md` | Konteks produk dan sistem desain antarmuka |

## Keamanan dan privasi

- Tidak ada telemetri. Jaringan hanya dipakai untuk unduhan yang Anda minta, pemindaian halaman yang Anda jalankan, dan pencarian yang Anda tekan.
- Berkas yang sudah ada tidak pernah ditimpa (`ERR_NAME_COLLISION`).
- Native host hanya menerima ekstensi dengan ID yang dipasang dan berbicara dengan aplikasi lewat named pipe per pengguna yang dilindungi token.
- Tautan FTP yang memuat karakter kontrol ditolak, dan alamat dalam balasan `PASV` diabaikan.
- Indeks torrent umum (apibay, Nyaa) dapat memuat materi berhak cipta. Memakainya, dan apa yang diunduh, adalah tanggung jawab Anda.

## Status

Teruji otomatis (Rust dan JavaScript) dan, untuk sebagian, dijalankan nyata: unduhan HTTPS dan FTP sungguhan, torrent publik lewat DHT, pencarian ke keempat sumber, clipboard Windows, antarmuka di jendela nyata.

Belum terbukti atau belum selesai:

- Ekstensi di dalam Chrome/Edge/Brave sungguhan, dialog konfirmasi tangkapan browser, toast selesai, dialog keluar.
- Jalur sukses perintah `--admin-*` dengan hak Administrator.
- Menyeret dan mengubah ukuran jendela tanpa bilah judul bawaan Windows.
- Pembaruan otomatis bertanda tangan, penelusuran situs berkedalaman, penjadwal, dan beberapa tugas lain masih menunggu keputusan pemilik; lihat [`docs/17_TASKS.md`](docs/17_TASKS.md) dan [`docs/16_DECISIONS.md`](docs/16_DECISIONS.md).
- Nilai bawaan torrent (seeding aktif, rasio 1,0, DHT aktif, UPnP mati) dan sumber pencarian masih sementara.

## Lisensi

Belum ditetapkan. Sampai pemilik repositori memilih lisensi, semua hak dilindungi.
