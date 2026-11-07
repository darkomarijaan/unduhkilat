# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

(Catatan: aplikasinya adalah aplikasi desktop native Windows 10/11 x64 berbasis Rust + egui, bukan situs web. Mockup dibuat dalam HTML hanya sebagai media pratinjau; nilai `web` dipilih karena skema hanya mengenal web/ios/android/adaptive.)

## Users

Pengguna umum Windows di Indonesia yang mengunduh berkas, video, dan torrent sehari-hari dan ingin cepat paham tanpa belajar (dikonfirmasi pemilik). Pengguna mahir tetap harus terlayani lewat jalan pintas dan panel detail, tetapi bukan pengguna utama.

## Product Purpose

UnduhKilat adalah manajer unduhan berkecepatan tinggi, portabel (ZIP tanpa instalasi), tanpa telemetri. Ia memecah unduhan HTTP/HTTPS/FTP ke beberapa koneksi, melanjutkan unduhan yang terputus, mengantre dengan prioritas, menangkap tautan dari Chrome/Edge/Brave lewat ekstensi (selalu dengan konfirmasi), mengunduh torrent/magnet (termasuk seeding), memindai halaman untuk unduhan massal (Site Grabber), dan mencari berkas dari beberapa sumber. Sukses = pengguna menambah unduhan, melihat kemajuannya sekilas, dan percaya data tidak hilang saat koneksi putus atau aplikasi ditutup.

## Positioning

Cepat, lanjut tanpa mengulang, dan jujur: tidak ada telemetri, tidak ada unduhan dimulai tanpa konfirmasi pengguna, tidak pernah menimpa berkas. Pesaing lain tidak bisa mengklaim ketiganya sekaligus bersama peta segmen koneksi yang terlihat.

## Operating Context

Aplikasi berjalan di Windows desktop (jendela utama ~1280x800 pada DPI 125%, tray, toast saat selesai). Tugas utama: menempel tautan atau mengklik tangkapan browser, memantau daftar unduhan (status, kecepatan, ETA, koneksi/peer), menjeda/melanjutkan, membuka berkas hasil. Layar sekunder: Pengaturan, Diagnostik, Site Grabber, Cari.

## Capabilities and Constraints

- Semua teks UI berbahasa Indonesia (istilah mesin seperti Range, peer, seeding boleh tetap).
- Fitur yang harus tetap ada: tabel unduhan (nama, ukuran, status, progres, kecepatan, sisa waktu, koneksi, prioritas), filter status dan kategori (Compressed, Documents, General, Images, Music, Programs, Video), toolbar (Tambah URL, Lanjutkan/Jeda Semua, Bersihkan Selesai), batas kecepatan global, strip peta segmen, dialog Tambah (multi-URL, kategori, folder, koneksi, prioritas, referer/cookie, magnet, .torrent), dialog konfirmasi tangkapan browser dengan lencana "Tangkap Otomatis", layar Pengaturan (Mesin, Kategori, Kecepatan, Jendela, Jaringan & Proxy, Torrent, Integrasi Browser), Diagnostik (log), Site Grabber, Cari (sumber dicentang, hasil dicentang lalu Antrikan), tray.
- Implementasi: egui/eframe 0.37 (immediate mode). Rancangan harus bisa diwujudkan dengan egui: panel, tabel, painter kustom, font yang dimuat dari berkas; tanpa blur kaca atau efek CSS yang tidak ada padanannya.
- Batasan keputusan terbuka: nilai bawaan torrent, apibay/Nyaa, dan beberapa TBD di docs masih PROVISIONAL.

## Brand Commitments

Nama "UnduhKilat" dan ikon petir (kilat) sudah ada. Bahasa Indonesia. Pemilik menyatakan tampilan sekarang "jelek": datar, gelap, kosong, toolbar/navigasi berantakan, tipografi dan ikon lemah, dan tidak terasa "kilat". Pemilik mengizinkan rombak total tata letak (fungsi dan teks Indonesia tetap).

## Evidence on Hand

Tangkapan layar versi sekarang (`...\images\1.png`), `docs/06_DESIGN.md` (rancangan lama: gelap #0F1115/#1A1E26, aksen #00D2FF), seluruh PRD/Requirements. Tidak ada testimoni, pelanggan, atau tolok ukur; jangan dikarang.

## Product Principles

1. Kemajuan harus terbaca sekilas: status, persen, kecepatan, dan sisa waktu tanpa membuka apa pun.
2. Pengguna selalu memegang kendali: tidak ada yang dimulai, ditimpa, atau dihapus tanpa konfirmasi jelas.
3. Kilat itu rasa, bukan hiasan: kecepatan ditunjukkan lewat data nyata (kecepatan, koneksi, segmen), bukan efek.
4. Satu cara untuk satu hal: aksi yang sering ada di depan, yang jarang dikelompokkan rapi.
5. Jujur soal batas: peringatan privasi (torrent, indeks umum), kesalahan berkode, dan keadaan kosong yang memberi arah.

## Accessibility & Inclusion

Dipakai pada DPI 100-150% dan layar laptop; kontras teks minimal WCAG AA, status tidak boleh hanya mengandalkan warna (selalu ikon/teks), target klik minimal 32px, dapat dioperasikan dengan keyboard (Ctrl+N, Delete, Enter, panah).
