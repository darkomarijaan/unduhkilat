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
