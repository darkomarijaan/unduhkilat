---
name: UnduhKilat
description: Manajer unduhan yang dilacak seperti paket ekspres: setiap unduhan punya resi dan lintasan lima langkah, di bawah pita jingga kurir.
colors:
  kurir-orange: "#FF5A1F"
  ink: "#1B1918"
  paper: "#F3F1EE"
  card: "#FFFFFF"
  card-soft: "#FAF8F6"
  line: "#E2DDD6"
  ink-mute: "#5B554E"
  track-off: "#DAD3CA"
  ok: "#127A48"
  bad: "#BE2A1D"
  cat-programs: "#6D4AFF"
  cat-video: "#E5484D"
  cat-documents: "#0E7AE6"
  cat-compressed: "#C77D0A"
  cat-music: "#12A594"
  cat-images: "#D6409F"
  cat-general: "#6B7280"
typography:
  brand:
    fontFamily: "Segoe UI Variable Display, Segoe UI, sans-serif"
    fontSize: "22px"
    fontWeight: 800
  title:
    fontFamily: "Segoe UI Variable Text, Segoe UI, sans-serif"
    fontSize: "14px"
    fontWeight: 700
  body:
    fontFamily: "Segoe UI Variable Text, Segoe UI, sans-serif"
    fontSize: "13px"
    fontWeight: 400
  numeric:
    fontFamily: "Segoe UI Variable Text, Segoe UI, sans-serif"
    fontSize: "17px"
    fontWeight: 800
  waybill:
    fontFamily: "Cascadia Mono, Consolas, monospace"
    fontSize: "11.5px"
    fontWeight: 400
rounded:
  sm: "6px"
  md: "8px"
spacing:
  sm: "8px"
  md: "16px"
  lg: "24px"
components:
  button-primary:
    backgroundColor: "{colors.ink}"
    textColor: "#FFFFFF"
    rounded: "{rounded.md}"
    height: "44px"
  package-card:
    backgroundColor: "{colors.card}"
    rounded: "{rounded.md}"
    height: "92px"
  header-band:
    backgroundColor: "{colors.kurir-orange}"
    textColor: "{colors.ink}"
    height: "72px"
---

# Design System: UnduhKilat

## Overview

**Creative North Star: "Paket Ekspres"**

Setiap unduhan diperlakukan seperti paket yang dilacak dari antre sampai sampai. Pengguna umum Windows di Indonesia sudah paham metafora ini: nomor resi, tahapan perjalanan, cap "Sampai". Kemajuan terbaca dari posisi di lintasan, bukan dari membaca angka. Satu warna kuat (jingga kurir) memenuhi pita atas; sisanya putih hangat dan tinta. Mode: Operate. Ekspresi tidak boleh menutupi tugas, status, atau aksi yang familiar.

Ditolak secara eksplisit: tampilan gelap datar dengan semua area sewarna (versi lama), neon, bayangan kaca, dan ikon-dalam-kartu yang seragam.

## Colors

Strategi: Committed. Jingga `kurir-orange` memegang pita atas (sekitar seperempat tinggi jendela di layar utama); tinta `ink` memegang teks, tombol utama, dan item navigasi aktif.

- Kurir Orange `#FF5A1F`: pita atas dan penanda langkah aktif. Teks di atasnya selalu tinta (kontras 6,6:1), tidak pernah putih.
- Ink `#1B1918`: teks utama, tombol utama, lintasan yang sudah dilewati, bayangan keras pada kartu terpilih.
- Paper `#F3F1EE` latar jendela; Card `#FFFFFF` permukaan kartu; Line `#E2DDD6` garis tipis dan garis putus pemisah.
- Ok `#127A48` dan Bad `#BE2A1D` hanya untuk hasil akhir (Sampai, Berbagi, Tertahan) dan selalu disertai teks/ikon.
- Warna kategori hanya untuk titik kecil di navigasi dan ubin ikon berkas; tidak pernah untuk status.

Aturan: jingga tidak dipakai di luar pita atas, penanda langkah aktif, dan segmen terunduh. Status tidak pernah hanya warna.

## Typography

Segoe UI Variable (dimuat dari C:\Windows\Fonts; jatuh ke Segoe UI) untuk semua teks, Cascadia Mono hanya untuk nomor resi dan ID. Skala: brand 22/800, angka kecepatan 17-26/800, judul kartu 14/700, isi 13/400, keterangan 11,5-12/400. Angka selalu rata (tabular). Nama berkas dipotong di tengah baris dengan elipsis dan tooltip penuh.

## Layout

Jendela utama 1280x800 (DPI 125% = ~1024x640 logis, harus tetap muat; di bawah 1000px lebar sidebar disembunyikan). Urutan dari atas: bilah judul jingga, pita atas jingga (merek, "Tambah unduhan", tab Semua/Aktif/Selesai/Terjeda dengan hitungan, kecepatan total di kanan), lalu badan dengan sidebar 200px (Semua paket, Kategori, Alat, Pengaturan, Diagnostik) dan daftar paket, strip peta segmen untuk paket terpilih, dan bilah status. Ritme spasi: 8 / 16 / 24; jarak antar kartu 10px.

## Elevation & Depth

Datar. Satu-satunya kedalaman: kartu terpilih mendapat bayangan keras tanpa blur (offset 3px, warna tinta) dan bingkai tinta; dialog memakai bayangan lembut gelap. Tidak ada blur latar.

## Shapes

Radius 8px untuk kartu dan tombol, 6px untuk kontrol kecil, lingkaran penuh untuk penanda langkah. Garis putus 1,5px memisahkan "label resi" (kiri) dari lintasan (tengah), seperti sobekan resi. Cap hasil akhir: bingkai 1,5px, huruf kapital tebal berjarak, miring -3 derajat.

## Components

- **Paket (baris unduhan):** tinggi 92px. Kiri: ubin ikon berkas bergaris tinta, nama berkas, nomor resi mono (UK-XXXX-NNNN), sumber dan protokol. Tengah: lintasan lima langkah (Antre, Terhubung, Mengunduh, Verifikasi, Selesai); langkah lewat = titik tinta berisi centang, langkah aktif = lingkaran jingga 26px berisi petir (mengunduh), jeda (terjeda), jam (antre), atau X merah (gagal); label aktif menampilkan persen. Kanan: kecepatan dan sisa waktu, atau ukuran dan keterangan; cap hasil akhir.
- **Tombol utama "Tambah unduhan":** tinta dengan teks putih, 44px, satu per layar. Tombol sekunder: kontur tipis.
- **Tab pita atas:** teks tinta; tab aktif berlatar tinta 14% di atas jingga, dengan hitungan.
- **Navigasi:** item aktif tinta penuh dengan teks putih; titik kategori bulat 9px.
- **Peta segmen:** satu irisan per koneksi/potongan, terunduh jingga di atas dasar krem, di bawah daftar untuk paket terpilih.
- **Dialog Tambah:** putih, radius 8px, satu aksi utama (tinta); lencana "Tangkap Otomatis" bertinta jingga untuk tangkapan browser.
- **Keadaan kosong:** lingkaran jingga berisi petir, judul, satu kalimat arahan, tombol Tambah.

## Do's and Don'ts

Do:
- Tampilkan tahap, bukan hanya persen.
- Pertahankan satu aksi utama per layar dan letakkan di pita atas.
- Beri setiap status ikon dan teks; gagal menyebut kode galat dan cara pulih.

Don't:
- Jangan pakai jingga sebagai warna status atau latar kartu.
- Jangan tumpuk kartu di dalam kartu atau seragamkan semua blok jadi kartu ber-ikon.
- Jangan pakai teks putih di atas jingga, bayangan berblur lebar, atau efek kaca.
- Jangan menyembunyikan aksi penting hanya pada hover; aksi baris muncul di kartu terpilih dan juga lewat keyboard.
