# Keputusan Produk: UnduhKilat

Dokumen ini mencatat jawaban pengguna sesuai pilihan dan teks yang diberikan. Label `Dijawab` berarti opsi dipilih, bukan berarti semua detail dalam opsinya sudah ditentukan. Deskripsi opsi adalah konteks, bukan izin untuk mengarang nominal, SLA, atau kebijakan. Jika detail belum eksplisit, tandai `TBD` pada spesifikasi turunannya.

## DEC-001 — Berapa batas bawaan jumlah unduhan bersamaan dan koneksi per unduhan?

**Status:** Dijawab

**Jawaban pengguna:** Tanpa batas atas, sepenuhnya diatur pengguna

## DEC-002 — Saat antrian penuh, aturan mana yang menentukan unduhan berikutnya dijalankan?

**Status:** Dijawab

**Jawaban pengguna:** Prioritas manual per item, sisanya FIFO

## DEC-003 — Bagaimana penangkap video/audio menemukan media di halaman web?

**Status:** Dijawab

**Jawaban pengguna:** Hibrida: sniffing plus hook ekstensi

## DEC-004 — Bagaimana perilaku jendela, tray, dan notifikasi yang diinginkan?

**Status:** Dijawab

**Jawaban pengguna:** Tutup ke tray, unduhan tetap jalan, toast saat selesai

## DEC-005 — Versi dan arsitektur Windows mana yang wajib didukung?

**Status:** Dijawab

**Jawaban pengguna:** Windows 10 dan 11, x64 saja

## DEC-006 — Browser mana yang wajib dijangkau mekanisme penangkapan tautan?

**Status:** Dijawab

**Jawaban pengguna:** Chromium saja (Chrome, Edge, Brave)

## DEC-007 — Bagaimana aplikasi ini akan dipasang dan dioperasikan?

**Status:** Dijawab

**Jawaban pengguna:** Portable ZIP tanpa instalasi

## DEC-008 — Seberapa lengkap dukungan BitTorrent yang diinginkan?

**Status:** Dijawab

**Jawaban pengguna:** Lengkap (file .torrent dan magnet, tracker, DHT, PEX, seeding, dan pembatasan rasio)

**Catatan implementasi:** Pertanyaan lanjutan (boleh memakai pustaka BitTorrent besar; apakah batas koneksi/kecepatan global berlaku untuk torrent) belum dijawab. Sampai dijawab, pustaka `librqbit` dipakai sebagai asumsi kerja dan batas kecepatan global diterapkan juga pada torrent.

## DEC-009 — Dari sumber mana pencarian file boleh mengambil hasil?

**Status:** Dijawab

**Jawaban pengguna:** Internet Archive, Academic Torrents, dan indeks torrent umum (apibay/Nyaa); hanya saat tombol Cari diklik, tanpa saran otomatis saat mengetik.

**Catatan implementasi:** Kata kunci hanya dikirim ke sumber yang dicentang dan hanya saat Cari ditekan (lewat klien HTTP aplikasi, jadi proxy berlaku). Dua katalog legal aktif secara bawaan; apibay dan Nyaa dimatikan sampai dicentang (nilai bawaan PROVISIONAL). Academic Torrents tidak punya API pencarian, jadi feed `rss.xml` resminya (entri terbaru) disaring secara lokal.

## DEC-010 — Bagaimana mencari file Google Drive yang dibagikan publik?

**Status:** Dijawab

**Jawaban pengguna:** Tambahkan pencarian `site:drive.google.com` agar file Drive yang memang dibagikan publik bisa dicari.

**Catatan implementasi:** Mesin pencari menolak permintaan otomatis (DuckDuckGo dan Yandex menampilkan captcha, Brave membalas 429, Bing mengabaikan `site:`), dan men-scrape-nya melanggar syarat layanannya. Maka tombol "Cari file Drive publik di browser" di layar Cari membuka Google dengan `site:drive.google.com <kata kunci>` di browser pengguna; UnduhKilat sendiri tidak mengirim apa pun. Tautan Drive file (`/file/d/ID`, `open?id=`, `uc?id=`) otomatis diubah menjadi URL unduhan langsung `drive.usercontent.google.com/download?id=ID&export=download&confirm=t`; folder ditolak. Belum diuji terhadap file Drive publik nyata. Jalur alternatif di dalam aplikasi (Google Programmable Search API) butuh kunci API dari pengguna dan belum dibuat.

## DEC-011 — Bisakah 1337x.to menjadi sumber pencarian?

**Status:** Dijawab

**Jawaban pengguna:** Tambahkan https://1337x.to/ sebagai sumber pencarian file.

**Catatan implementasi:** 1337x.to dilindungi tantangan Cloudflare: permintaan otomatis dijawab HTTP 403 dengan halaman "Just a moment...". Melewatinya berarti menembus perlindungan anti-bot dan akan rapuh, sehingga 1337x tidak dimasukkan sebagai penyedia di dalam aplikasi. Sebagai gantinya, tombol "Cari di 1337x (browser)" di layar Cari membuka `https://1337x.to/search/<kata kunci>/1/` di browser pengguna; UnduhKilat sendiri tidak mengirim apa pun. Magnet yang diklik di sana diteruskan ke UnduhKilat lewat penangan magnet (pengguna harus memilih UnduhKilat sebagai aplikasi magnet default).

## DEC-012 — Bagaimana terminal dibuat?

**Status:** Dijawab

**Jawaban pengguna:** Terminal dibuka juga dari folder di Windows (tanpa SSH), dengan tab dan tata letak yang bisa dipilih; mouse dan Ctrl+V seperti editor teks; kebutuhan utama menjalankan Claude Code. `Alt+V` (tempel gambar) belakangan.

**Catatan implementasi:** Satu terminal dengan dua sumber, shell lokal (ConPTY lewat `portable-pty`) dan shell SSH (kanal PTY `russh`), keduanya lewat antarmuka `SourceIo`. Emulator memakai `alacritty_terminal`; keluaran diproses per 64 KB agar UI tidak macet. Sambungan SSH yang putus tidak disambung otomatis (tombol "Sambung ulang"). `Alt+V` ditunda ke setelah Rencana 2 (SFTP), karena perlu jalur unggah berkas. Verifikasi dengan Claude Code nyata belum dilakukan; lihat `docs/superpowers/spikes/2026-11-07-terminal-verification.md`.

## DEC-013 — Bagaimana berkas server, unggahan, dan pratinjau dibuat?

**Status:** Dijawab

**Jawaban pengguna:** SFTP/FTP/FTPS dengan penjelajah dua panel, unggahan lewat antrean yang sama, pratinjau txt/md/jpg/png/docx/pdf/xlsx/odt, `Alt+V` tempel gambar; satu rilis untuk rencana 2, 4, dan 5.

**Catatan implementasi:** Satu antarmuka `RemoteFs` dengan klien SFTP (`russh-sftp` di atas kanal SSH yang sama, termasuk rantai jump) dan klien FTP/FTPS (`suppaftp` + rustls). Sertifikat FTPS dipin seperti kunci host SSH (tabel `known_hosts`, jenis `tls-cert`, dikonfirmasi sebelum user/password dikirim; berubah = diblokir). Transfer server masuk ke tabel `downloads` yang ada lewat skema URL `server://<id>/<jalur>` (unduh) dan `server-up://<id>/<jalur>` (unggah; `?overwrite=1` hanya setelah konfirmasi), sehingga jeda, lanjut, prioritas, batas kecepatan, dan notifikasi dipakai bersama; menghapus unggahan tidak pernah menghapus berkas lokal. Item antrean tidak bisa menampilkan dialog: bila kunci host atau password belum dikonfirmasi, item gagal dengan petunjuk "Uji sambungan". FTP/FTPS menyambung langsung (tanpa proxy aplikasi). Pratinjau: teks dan Markdown dibaca bagian awalnya, gambar/PDF/Office diambil utuh di bawah batas (25 MB, dapat diatur); PDF dirender `hayro` (tanpa DLL), docx/odt/xlsx/ods dibaca dari ZIP+XML dengan batas anti-ZIP-bom. `Alt+V` hanya di tab SSH: gambar papan klip diunggah ke `/tmp/unduhkilat/` lalu jalurnya diketik; tab lokal meneruskan `Alt+V` ke program. Impor `~/.ssh/config` dan SSH agent (butir tambahan spec bagian 13) belum dikerjakan.

## DEC-014 — Bisakah jenis berkas tertentu tetap diunduh browser?

**Status:** Dijawab

**Jawaban pengguna:** Ya, seperti daftar tipe berkas di IDM. Contoh: PDF diunduh langsung oleh browser, tidak lewat UnduhKilat.

**Catatan implementasi:** Daftar pengecualian (bukan daftar izin), agar unduhan tanpa ekstensi tetap ditangkap seperti sebelumnya. Pengaturan `browser_skip_extensions` (bawaan kosong) di Pengaturan → Integrasi Browser. Pencocokan memakai ekstensi nama berkas, ekstensi jalur URL, dan tabel MIME umum (`core/src/capture_filter.rs`, kembar dengan `shouldSkipCapture` di `extension/shared.js`). Aplikasi menolak `browser.intercept` yang cocok dengan `ERR_CAPTURE_SKIPPED`; ekstensi menerima daftar yang sama lewat `ping` dan menyimpannya, sehingga unduhan yang dikecualikan tidak diteruskan dan tidak membuka aplikasi meskipun aplikasi sedang tertutup. "Kirim ke UnduhKilat" dari panel media adalah tindakan eksplisit sehingga tidak terkena daftar ini. Batasan: unduhan yang saat dibuat belum punya nama berkas, alamatnya tanpa ekstensi, dan MIME-nya generik (`application/octet-stream`) tidak bisa dikenali, jadi tetap ditangkap.
