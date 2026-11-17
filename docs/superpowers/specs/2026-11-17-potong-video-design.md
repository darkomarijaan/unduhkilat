# Desain: Potong Video/Audio Saat Unduh (rentang Dari–Sampai)

Tanggal: 2026-11-17. Status: disetujui pengguna. Melanjutkan spec
`2026-11-14-unduh-video-ytdlp-design.md` (DEC-015), yang sebelumnya mencantumkan "potong video" sebagai
bukan tujuan.

## 1. Maksud dan alasan

Pengguna ingin menentukan menit/detik **Dari** dan **Sampai** untuk link video, supaya hanya bagian yang
ingin ditonton atau didengar yang diunduh. Bawaannya tetap seluruh video. Ketepatan potongan sampai ke
detik tidak penting; yang penting cepat dan kualitas utuh.

## 2. Tujuan, bukan tujuan, anti-tujuan

**Tujuan**
- Dialog Tambah untuk link video punya isian Dari dan Sampai; kosong = seluruh video.
- Berlaku untuk Video MP4 dan Audio MP3.
- Item potongan berjalan di antrian seperti item video lain.

**Bukan tujuan**
- Potongan tepat di detik (encode ulang). Awal potongan boleh mundur ke keyframe terdekat.
- Lebih dari satu rentang per item.
- Potong dari ekstensi browser atau mode CLI.
- Melanjutkan (`--continue`) potongan yang dijeda.

**Anti-tujuan**
- Item video tanpa potongan berubah perilaku (argumen, nama, resume).
- Berkas yang sudah ada tertimpa (`ERR_NAME_COLLISION`).
- Seluruh video diunduh lalu dipotong di lokal.

## 3. Pendekatan

| | Pendekatan | Untung | Rugi |
|---|---|---|---|
| **A (dipilih)** | `yt-dlp --download-sections "*A-B"` tanpa `--force-keyframes-at-cuts` | Hanya rentang itu yang diambil; cepat; kualitas utuh | Awal potongan mengikuti keyframe; resume mulai ulang; progres kasar |
| B | Unduh utuh lalu potong dengan ffmpeg | Progres dan resume normal | Membuang kuota dan waktu |
| C | A + `--force-keyframes-at-cuts` | Tepat di detik | Encode ulang, lambat, berat CPU; pengguna tidak butuh |

Pengguna memilih tujuan "sekadar mengambil bagian yang ingin ditonton", jadi A.

## 4. Desain

### 4.1 Dialog Tambah
Di bawah baris Format/Kualitas, hanya untuk link video:

```
Potong:  Dari [ 1:30    ]  Sampai [ 2:45    ]   kosong = seluruh video
```

- Format isian: `ss`, `m:ss`, atau `h:mm:ss` (menit/detik dua digit setelah `:` tidak wajib, tetapi
  nilai menit/detik setelah `:` harus < 60).
- Salah satu boleh kosong: hanya Dari = sampai akhir; hanya Sampai = dari awal.
- Tidak valid, atau Dari ≥ Sampai, atau (setelah Periksa) Dari ≥ durasi: catatan merah, tombol Mulai
  dinonaktifkan. Sampai yang melebihi durasi diperlakukan sebagai "sampai akhir" (catatan, bukan error).

### 4.2 Bentuk URL item
`video:<opsi>[;t=<dari>-<sampai>]|<url>`, detik bulat, ujung terbuka dikosongkan:
`video:v=1080;t=90-165|…`, `video:a=mp3-192;t=90-|…`, `video:v=best;t=-165|…`.
Bentuk lama tanpa `;t=` tetap terbaca (tanpa migrasi). `t=-` (kedua ujung kosong) ditolak oleh `parse`;
dialog tidak pernah membuatnya.

### 4.3 Argumen yt-dlp
`--download-sections "*<dari>-<sampai>"` dengan `0` untuk awal terbuka dan `inf` untuk akhir terbuka.
Tanpa `--force-keyframes-at-cuts`. Potongan butuh ffmpeg: tanpa ffmpeg → `ERR_VIDEO_TOOLS_MISSING`.

### 4.4 Nama berkas
Nama dari judul diberi akhiran rentang: `Judul [01.30-02.45].mp4`, jam ditambahkan bila ≥ 1 jam
(`[1.02.03-1.05.00]`), ujung terbuka ditulis `awal`/`akhir` (`[01.30-akhir]`). `:` tidak boleh di nama
berkas Windows, maka `.`. Nama yang diketik pengguna tidak diubah. Ini panggilan Claude.

### 4.5 Resume dan progres
- Item potongan yang dijeda lalu dijalankan lagi membuang folder kerjanya dulu dan mengunduh dari awal
  (ffmpeg tidak bisa meneruskan `.part`). Panggilan Claude.
- Progres memakai template yang sama; kalau total tidak diketahui (`-1`), UI menampilkan progres tanpa
  persen. Pembangun memverifikasi keluaran progres yt-dlp saat memakai downloader ffmpeg.

## 5. Tes
- Urai format waktu (valid/tidak valid, batas 60).
- `parse`/`wrap` bolak-balik dengan dan tanpa `;t=`, bentuk lama tetap terbaca, `t=-` ditolak.
- Argumen memuat `--download-sections` hanya bila ada potongan.
- `final_name` dengan akhiran rentang.

## 6. Diserahkan ke pembangun
Letak persis isian di dialog, teks catatan validasi, dan cara item potongan ditandai di daftar unduhan
(opsional).
