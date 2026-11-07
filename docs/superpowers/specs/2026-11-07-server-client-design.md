# Klien Server (SSH, SFTP, FTP, FTPS) untuk UnduhKilat: Desain

Status: draf untuk persetujuan. Belum ada kode. Tanggal: 2026-11-07.
Bahasa dokumen mengikuti `docs/` proyek ini (Indonesia).

## 1. Maksud dan alasan

Pemilik proyek (Eko) mengelola server lewat SSH dan mengambil/mengirim berkas lewat FTP/SFTP. Ia ingin semuanya di dalam UnduhKilat: menyimpan sambungan beserta kredensialnya, masuk ke server yang hanya bisa dicapai lewat server lain (jump host, pengganti VPN), memakai terminal penuh, menjelajah berkas, dan membaca isi berkas di server tanpa mengunduhnya lebih dulu.

Ukuran keberhasilan menurut Eko: **semua fitur di bawah ada di rilis pertama fitur ini**; tidak ada yang boleh ditunda ke rilis lain. Urutan pengerjaan di dalam rilis itu boleh bertahap (bagian 9).

## 2. Tujuan

1. Daftar **Server** tersimpan: nama, protokol (SSH, SFTP, FTP, FTPS), alamat, port, pengguna, cara masuk, jalur jump.
2. **Kredensial tersimpan** (password, kunci pribadi berikut frasa sandinya) dan dipakai otomatis saat menyambung.
3. **Jump host berantai** (A lewat B, bahkan lewat C lalu B). Berlaku untuk terminal, SFTP, dan pratinjau. Tanpa VPN, tanpa membuka port lokal ke luar.
4. **Terminal** penuh di tab, beberapa sesi bersamaan. Program layar penuh (`vim`, `htop`, `less`, `nano`, `tmux`) harus berfungsi.
5. **Penjelajah berkas** SFTP/FTP/FTPS: dua panel (komputer ini dan server). Aksi: telusuri, urutkan, buat folder, ganti nama, hapus, unduh, unggah.
6. **Pemindahan berkas lewat antrean unduhan yang sama** (kecepatan, jeda, lanjut, batas kecepatan, notifikasi selesai). Termasuk **unggahan**, yang belum ada di antrean sekarang.
7. **Pratinjau langsung** isi berkas di server (bagian 6).
8. Verifikasi **kunci host** (bagian 7).

## 3. Bukan tujuan

- Tidak memasang atau menyertakan LibreOffice atau mesin dokumen sebesar itu (keputusan Eko).
- Tidak ada UI port forwarding / terowongan mandiri. Jump host hanya sebagai jalan menuju server tujuan.
- Tidak ada X11 forwarding, server SSH/SFTP, mosh, rsync, sinkronisasi folder, penyimpanan awan (S3, Drive, dst.).
- Tidak ada penyuntingan berkas jarak jauh dengan tulis-balik otomatis di versi ini.
- Tidak ada kata sandi utama (master password) untuk kredensial; lihat bagian 8 (keputusan pembuat).

## 4. Anti-tujuan (dianggap gagal walau secara teknis jalan)

- Kredensial, frasa sandi, atau isi sesi tampil di log, pesan galat, atau berkas ekspor.
- Menyambung ke host yang kuncinya berubah tanpa peringatan yang jelas.
- Terminal yang rusak pada program layar penuh, atau yang membeku saat memotong/menempel teks besar.
- Pratinjau yang diam-diam mengunduh berkas besar sepenuhnya.
- Menimpa berkas di server atau di komputer tanpa konfirmasi.
- Aplikasi menghubungi server mana pun tanpa tindakan pengguna (tidak ada sambungan otomatis saat aplikasi dibuka).

## 5. Keputusan Eko (diteruskan apa adanya)

- Cakupan: semua fitur di atas, sekaligus.
- FTP dan SFTP, serta SSH dengan kredensial tersimpan; SSH dengan jump host; terminal wajib.
- Pratinjau berkas: txt, md, jpg, png, docx, pdf, xlsx, odt, dll. Tingkat yang disetujui: teks/Markdown/gambar penuh; PDF dirender per halaman; docx/odt hanya teks, judul, tabel; xlsx/ods sebagai tabel nilai; format lain dibuka lewat aplikasi Windows (diunduh ke folder sementara).
- Jangan memasang mesin LibreOffice.
- Tampilan mengikuti mockup yang disetujui di sesi brainstorming, disalin ke repo di `docs/superpowers/specs/mockups/` (`server-features.html` dan `file-preview.html`; buka di browser). Mockup menentukan **tata letak dan isi kartu/dialog**, bukan perilaku piksel demi piksel: menu "Server" di sidebar; kartu per server; dialog dengan diagram jalur jump; tab sesi dengan sakelar Terminal/Berkas; penjelajah dua panel; panel pratinjau. **Tata letak sesi: keputusan Eko = keduanya, dengan pengaturan untuk berpindah.** A (Terminal dan Berkas bergantian lewat sakelar) dan B (berdampingan) sama-sama wajib ada. Pengaturan tersedia di Pengaturan aplikasi (bawaan) dan sebagai tombol cepat di dalam sesi; pilihan diingat. Bawaan: A (pilihan pembuat, karena terminal untuk Claude Code butuh lebar penuh).

## 6. Pratinjau berkas

| Format | Tampilan |
|---|---|
| txt, md, log, json, xml, yaml, ini, kode | Teks bernomor baris; Markdown dirender (judul, daftar, tabel, kode) |
| jpg, png, gif, webp, bmp | Gambar, zoom dan geser |
| pdf | Halaman dirender menjadi gambar, gulir dan zoom |
| docx, odt | Teks, judul, daftar, tabel. Tanpa font asli, gambar sebaris, header/footer, tata letak halaman |
| xlsx, ods | Lembar sebagai tabel nilai (bukan rumus/grafik). Pilih lembar. |
| doc, xls, ppt, pptx, lainnya | Tombol "Buka di aplikasi Windows": unduh ke folder sementara lalu buka dengan aplikasi bawaan |

Aturan:
- Pengambilan **sebagian** untuk teks (misalnya 1 MB pertama, dengan tombol "muat lebih banyak") dan gambar. Batas ukuran total (bawaan 25 MB, dapat diatur) sebelum aplikasi menawarkan "Unduh" atau "Buka di aplikasi" alih-alih pratinjau.
- Folder sementara pratinjau dibersihkan saat aplikasi ditutup dan dibuka.
- Berkas yang tidak dikenali tidak pernah ditampilkan sebagai teks sembarangan; tawarkan "Unduh".
- Pratinjau tidak menjalankan makro atau skrip apa pun dari dokumen.

## 7. Keamanan (wajib)

- **Kunci host**: saat pertama menyambung tampilkan sidik jari (SHA256) untuk dikonfirmasi lalu simpan (seperti `known_hosts`). Jika berubah, **blokir** dengan peringatan jelas; mengganti kunci butuh tindakan eksplisit pengguna. Berlaku untuk setiap lompatan, bukan hanya tujuan akhir.
- Kredensial dienkripsi dengan DPAPI per pengguna Windows (infrastruktur `dpapi.rs` yang sudah ada), dibersihkan dari memori setelah dipakai (zeroize), tidak pernah ditulis ke log (`12_SECURITY.md` berlaku).
- Isi sesi terminal tidak dicatat ke log aplikasi.
- Sambungan hanya terjadi atas tindakan pengguna.
- Kunci pribadi tetap di berkasnya; aplikasi menyimpan hanya jalurnya dan frasa sandi terenkripsi (jika pengguna memilih menyimpan).

## 8. Keputusan pembuat (dugaan; belum disetujui Eko, boleh dikoreksi)

- **Pustaka** (hasil pengecekan di crates.io, belum dicoba): `russh` (SSH + kanal `direct-tcpip` untuk jump) dan `russh-sftp` (SFTP); `ssh-key` untuk membaca kunci; emulator terminal `alacritty_terminal` atau `vt100` yang digambar sendiri di egui; FTP/FTPS: perluas `engine/ftp.rs` yang ada (FTP polos, sudah mendukung proxy dan REST) dengan TLS (rustls sudah ada di proyek), atau `suppaftp`. Pembuat memilih berdasarkan uji kecil.
- **PDF**: pilih perender yang cukup baik dengan beban distribusi terkecil. `hayro` (Rust murni) lebih ringan; `pdfium-render` butuh `pdfium.dll` (±10 MB). Pembuat menguji keduanya pada beberapa PDF nyata dan memutuskan. Portabilitas (satu folder, tanpa pemasang) harus terjaga.
- **docx/odt/xlsx/ods**: baca ZIP + XML sendiri (`zip` sudah menjadi dependensi). Markdown: pustaka render ringan.
- **Kredensial**: tanpa kata sandi utama (DPAPI sudah mengikat ke akun Windows). Tabel `credentials` yang ada hanya mengizinkan protokol `http/https/ftp/proxy` dan hanya password; fitur ini butuh tabel baru (mis. `servers`, `server_hops`, `known_hosts`) dan migrasi skema, bukan memaksa tabel lama.
- **Cara masuk**: password, kunci pribadi (dengan/ tanpa frasa sandi), dan SSH agent sebagai tambahan jika murah (Pageant/OpenSSH agent di Windows); keyboard-interactive hanya jika dibutuhkan server umum.
- **Impor** dari `~/.ssh/config` (Host, HostName, User, Port, IdentityFile, ProxyJump menjadi rantai jump): fitur tambahan yang diusulkan di mockup; Eko tidak menolak; jadikan prioritas terendah.
- **Proxy aplikasi**: sambungan pertama mengikuti pengaturan proxy aplikasi bila ada (HTTP CONNECT/SOCKS5), konsisten dengan unduhan lain.
- **Batas jump**: maksimal 5 lompatan; deteksi siklus; tiap lompatan mengautentikasi sendiri dan diverifikasi kunci hostnya.
- **Terminal**: `xterm-256color`, UTF-8, ukuran mengikuti jendela (kirim perubahan ukuran), gulir balik 10.000 baris, tab banyak. Dukungan mouse dan papan klip: **wajib**, lihat bagian 8a. Hyperlink yang bisa diklik: opsional.
- **Keepalive** dan sambung-ulang sederhana: tanya pengguna sebelum menyambung ulang sesi terminal yang putus (tidak otomatis).

## 8a. Terminal: mouse dan papan klip (keputusan Eko: harus seperti editor teks)

Wajib (dan `Alt+V` di terminal berarti **tempel gambar**, bagian 8b; `Alt+V` tidak menempel teks):
- **Mouse untuk memilih teks**: seret untuk memilih, klik dua kali memilih kata, klik tiga kali memilih baris, Shift+klik memperluas pilihan. Roda mouse menggulir riwayat; menggulir ke bawah kembali ke prompt.
- **Mouse untuk program**: saat program meminta pelaporan mouse (`vim`, `tmux`, `htop`, `mc`, `less`), klik, seret, dan roda dikirim ke program (mode xterm 1000/1002/1003 dengan penyandian SGR 1006). Menahan **Shift** saat menyeret memaksa pemilihan teks oleh aplikasi, seperti di terminal lain.
- **Menempel**: `Ctrl+V` menempel, `Shift+Insert` dan klik kanan juga. `Ctrl+Shift+V` selalu menempel.
- **Menyalin**: `Ctrl+C` menyalin **jika ada teks terpilih**, jika tidak mengirim interupsi `^C` ke program (perilaku Windows Terminal). `Ctrl+Shift+C` dan `Ctrl+Insert` selalu menyalin. Pilihan otomatis tidak menyalin diam-diam.
- Menempel memakai *bracketed paste* bila program memintanya, dan dikirim berpotong-potong (bagian 12 butir 8). Tempelan multibaris dari papan klip meminta konfirmasi jika terlihat seperti beberapa perintah dan program tidak mendukung bracketed paste.
- Menu klik kanan: Salin, Tempel, Pilih semua, Bersihkan layar.
- Tombol pintas aplikasi tidak boleh merebut ketikan terminal saat terminal terfokus (mis. `Ctrl+N` milik aplikasi tidak boleh menelan ketikan program).

Catatan benturan: pada terminal, `Ctrl+V` yang menempel berarti `vim` tidak lagi menerima `Ctrl+V` untuk mode blok visual (pengguna `vim` memakai `Ctrl+Q` atau `Ctrl+Shift+V` lalu menempel). Pengaturan "Ctrl+V menempel" dapat dimatikan per aplikasi.

## 8b. Pemakaian utama terminal: menjalankan Claude Code lewat SSH (keterangan Eko)

Eko terutama memakai terminal untuk mengakses Claude Code di server. Itu aplikasi layar penuh yang menuntut emulator lebih dari `bash` biasa. Keterangan ini lebih penting daripada daftar fitur terminal umum; jika ada bentrok, kebutuhan ini yang menang. Kebutuhan teknis (kesimpulan pembuat dari cara aplikasi seperti itu bekerja, **perlu dibuktikan lewat uji dengan Claude Code sungguhan**):

- Warna 24-bit (truecolor), karakter lebar/emoji/CJK dengan lebar kolom benar, garis kotak dan blok Unicode.
- Layar alternatif, penggambaran ulang cepat tanpa kedip (pembaruan tersinkron, mode 2026 jika didukung), ukuran berubah mengikuti jendela dengan benar.
- *Bracketed paste* (tempelan teks panjang/multibaris tidak dieksekusi per baris).
- Tombol modifikator: `Shift+Enter` untuk baris baru, `Alt+Enter`, `Alt+huruf` sebagai Meta (awalan ESC), `Ctrl+huruf`, tombol panah dengan modifikator, `Esc` dan `Esc Esc` tanpa jeda aneh. Protokol papan ketik yang diperluas (kitty keyboard / `modifyOtherKeys`) agar `Shift+Enter` terbedakan dari `Enter`.
- Mouse: klik dan roda mouse di dalam aplikasi layar penuh (bagian 8a).
- Penyalinan dari jarak jauh: urutan **OSC 52** dari aplikasi di server mengisi papan klip Windows (penting: teks yang disalin Claude Code di server sampai ke komputer Eko). Tautan **OSC 8** bisa diklik (opsional).
- Keluaran deras tanpa membuat jendela tidak responsif.

**Tempel gambar (keputusan Eko: ya, ini yang dimaksud dengan `Alt+V`; rancangan di bawah adalah usulan pembuat yang ia setujui secara garis besar).** Di Claude Code pada Windows, `Alt+V` menempel gambar dari papan klip, tetapi Claude Code yang berjalan di server membaca papan klip **server**, bukan papan klip Windows Eko, sehingga gambar yang disalin di komputer Eko tidak sampai. Usulan: pada `Alt+V` atau "Tempel gambar" di terminal, bila papan klip Windows berisi gambar, aplikasi mengunggahnya lewat SFTP ke folder sementara di server (mis. `/tmp/unduhkilat/…png`), lalu mengetikkan **jalur berkasnya** ke terminal (Claude Code membaca gambar dari jalur berkas). Hal yang sama untuk berkas yang diseret ke jendela terminal. Ini memakai SFTP yang memang dibangun; unggahannya tidak diam-diam, dan berkas sementara dibersihkan saat sesi ditutup (atau tetap bila pengguna memilih).

## 9. Urutan pengerjaan di dalam satu rilis

1. Fondasi: skema dan penyimpanan server, kredensial, kunci host, sambungan SSH dasar, rantai jump, dengan uji menggunakan server SSH dalam-proses (tidak menyentuh internet).
2. SFTP, FTP, FTPS: daftar direktori, operasi berkas, unduh dan **unggah** lewat antrean (unggahan adalah konsep baru di antrean).
3. Terminal (emulasi, tab, salin/tempel, ukuran).
4. Pratinjau (teks, Markdown, gambar, PDF, ekstraksi dokumen Office, buka di aplikasi).
5. UI: menu Server, dialog, penjelajah, sesi; pesan galat berbahasa Indonesia sesuai pola yang ada.

Rilis hanya keluar setelah kelima bagian selesai dan lulus uji.

## 10. Kendala dari kode yang sudah ada

- Windows 10/11 x64 saja, portabel (satu folder, data di `data/` bila ada penanda), tanpa pemasang; ukuran biner penting.
- UI: egui/eframe; gaya dan komponen mengikuti `DESIGN.md` dan `theme.rs`.
- Mesin FTP yang ada adalah FTP **polos** untuk unduhan (tanpa TLS, tanpa daftar direktori); antrean dan pengelola unduhan saat ini hanya mengenal **unduhan** (tidak ada unggahan).
- Satu instans aplikasi per folder data; pembaruan otomatis menukar `UnduhKilat.exe`; fitur baru tidak boleh menulis di luar `data/`, kecuali folder sementara.
- Uji: tidak boleh ada uji yang menyentuh internet; server SSH/FTP uji dijalankan dalam-proses di loopback (pola yang sama dengan uji torrent).
- Seluruh teks UI dan galat berbahasa Indonesia; kode galat memakai `ErrorCode` yang ada, tambah yang baru bila perlu.

## 11. Diserahkan kepada pembuat

Pilihan pustaka dan versi, bentuk skema dan migrasi, rancangan modul, cara menampilkan unggahan di daftar utama, perilaku detail terminal (tombol pintas, tema warna), batas ukuran pratinjau yang tepat, serta detail pembatas dan tombol cepat pemindah tata letak.

## 12. Keputusan tambahan setelah pemeriksaan pembuat

Semua butir di bagian ini adalah **keputusan pembuat** (belum disetujui Eko) kecuali yang tertulis sebaliknya.

1. **Risiko dikerjakan lebih dulu.** Sebelum perencanaan rinci, buat purwarupa kecil yang dibuang untuk tiga hal paling berisiko: terminal di egui (emulator + penggambaran), rantai jump dengan `russh`, dan perender PDF. Hasilnya menentukan pilihan pustaka (bagian 8).
2. **Mockup** ada di repo (bagian 5). Layout sesi: Terminal dan Berkas adalah dua tampilan independen milik satu sesi (state terpisah) sehingga A dan B hanyalah dua cara menaruhnya; keduanya dikirim (keputusan Eko). Berpindah A/B tidak boleh memutus sesi atau menghapus isi terminal. Pada B, lebar terminal minimum dijaga (pembatas yang dapat digeser) agar Claude Code tetap terbaca; dan ukuran pseudo-terminal dikirim ulang saat lebar berubah.
3. **Unggahan di antrean.** Unggahan memakai status, batas kecepatan, jeda/lanjut, dan notifikasi selesai yang sama dengan unduhan, bertahan setelah aplikasi ditutup, dan dilanjutkan (SFTP: tulis dari offset; FTP: `REST`/`APPE`). Tabel `downloads` dan antrean mendapat arah (unduh/unggah) serta sumber/tujuan; migrasi bersifat **tambahan** (tidak mengubah kolom lama). Menimpa berkas yang sudah ada di server **selalu** meminta konfirmasi di penjelajah **sebelum** masuk antrean; tabrakan nama saat dijalankan dari antrean menghentikan item dengan `ERR_NAME_COLLISION`, tidak pernah menimpa diam-diam.
4. **Jump host.** Setiap lompatan adalah entri **Server** tersimpan yang berprotokol SSH, dapat dipakai ulang oleh banyak tujuan (tabel penghubung urutan lompatan). Jika kredensial suatu lompatan tidak ada atau tidak bisa dibaca, aplikasi **meminta saat menyambung** dengan opsi menyimpannya.
5. **DPAPI dan folder portabel.** Kredensial terikat akun Windows dan komputer. Jika folder dipindah ke akun/komputer lain, kredensial tidak terbaca: tampilkan pesan jelas "Kredensial ini tersimpan di akun Windows lain dan tidak bisa dibaca; masukkan ulang" dan minta ulang saat menyambung. Perilaku ini sama dengan password proxy yang sudah ada.
6. **PDF.** Utamakan `hayro` (Rust murni, tanpa DLL). `pdfium.dll` di samping exe hanya boleh jika kualitas `hayro` terbukti tidak memadai; dalam kasus itu pembaruan otomatis harus diperluas agar menukar **semua** berkas dalam zip (sekarang hanya exe, host, extension, BACA-SAYA), dan bobot tambahan ±10 MB disebutkan di catatan rilis.
7. **FTPS.** Dukung eksplisit (`AUTH TLS`) dan implisit. Sertifikat yang tidak dipercaya (mis. self-signed) lewat konfirmasi sidik jari yang sama seperti kunci host SSH: tampilkan, simpan setelah disetujui, blokir bila berubah.
8. **Terminal: target dan verifikasi.** Tempel 1 MB tidak boleh membekukan UI lebih dari 200 ms (kirim berpotong-potong); keluaran deras tidak boleh membuat jendela tidak responsif. Verifikasi otomatis: uji urutan escape (urutan layar penuh, layar alternatif, pindah kursor, warna 256, ubah ukuran) terhadap emulator, dan server SSH dalam-proses dengan PTY yang memantulkan data. Program nyata (`vim`, `htop`) diperiksa **manual** oleh Eko di akhir; ini dicatat sebagai batas pengujian otomatis.
9. **Migrasi.** Skema maju saja, tambahan saja; build lama yang kebetulan membuka `data/` baru mengabaikan tabel yang tidak dikenalnya (tidak ada dukungan turun versi dan tidak dijanjikan).

## 13. Belum diputuskan oleh Eko

- Apakah rilis ditahan sampai **setiap** butir terkirim, atau butir tambahan boleh dilepas bila terbukti tidak layak? Usulan pembuat: butir inti (bagian 2 nomor 1-8) wajib; butir tambahan (SSH agent, impor `ssh_config`) boleh dilepas dengan pemberitahuan. Tata letak A dan B termasuk butir wajib (keputusan Eko). Dukungan mouse dan papan klip di terminal **bukan** butir tambahan; keduanya wajib (keputusan Eko).

## 14. Tambahan: terminal lokal yang dibuka dari folder (permintaan Eko, 2026-11-07)

Eko ingin juga bisa membuka **terminal komputer ini** dari sebuah folder di Windows, dan memilih bentuknya: **tab terminal lokal di dalam UnduhKilat**, memakai emulator yang sama dengan terminal SSH (bukan Windows Terminal terpisah dan bukan menu klik-kanan Explorer).

Keputusan Eko: tab terminal lokal di dalam aplikasi. Hal-hal berikut adalah **dugaan pembuat** (belum disetujui Eko):
- **Satu terminal, dua sumber.** Tampilan terminal (emulator, mouse, papan klip, tab) dibangun sekali di Rencana 3. Sumber byte-nya ada dua: kanal SSH (server) atau pseudo-console Windows (ConPTY) yang menjalankan shell lokal. Dengan begitu semua syarat §8a dan §8b (mouse, `Ctrl+V`, OSC 52, kitty keyboard, Claude Code berjalan baik) berlaku juga untuk terminal lokal.
- **Dari folder:** kartu unduhan dan penjelajah berkas mendapat aksi "Terminal" di samping "Folder"; klik membukanya sebagai tab terminal lokal yang direktori awalnya folder itu. Ada juga tombol "Terminal baru" (folder Unduhan) di bagian Server/Terminal.
- **Shell:** PowerShell bawaan Windows; pilihan lain (CMD, Git Bash, WSL) dapat dipilih di Pengaturan. Tidak ada admin/elevasi.
- **Keamanan:** shell berjalan dengan hak pengguna yang membuka aplikasi, tidak lebih. Isi sesi tidak masuk log. Tidak ada terminal yang terbuka sendiri.
- **ConPTY** butuh Windows 10 versi 1809 atau lebih baru; spec menargetkan Windows 10/11 x64, jadi cukup. Jika ConPTY tidak tersedia, tampilkan pesan jelas.
- Tab lokal dan tab SSH boleh bercampur dan memakai tata letak A/B yang sama (bagian 5); "Berkas" untuk tab lokal membuka penjelajah lokal.
