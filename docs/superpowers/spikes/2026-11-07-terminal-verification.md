# Verifikasi terminal (2026-11-07)

Catatan jujur: yang **terukur otomatis** diberi angka; yang butuh mata dan tangan manusia ditandai
**tidak terlihat** (tidak dicoba), bukan dianggap lulus.

## Kinerja

| Butir | Hasil | Bukti |
|---|---|---|
| UI tidak macet saat banjir keluaran 50 MB | lulus: tunggu kunci terburuk < 50 ms (syarat: < 200 ms) | tes `session::tests` (banjir 50 MB, utas OS memotret kunci) |
| Tempel 1 MB | terpotong 4 KB per bagian, tidak memblokir UI | tes `paste::tests::one_megabyte_is_chunked` |
| `Get-Content` berkas 20 MB di tab nyata, ukuran bingkai | **tidak terlihat** | butuh pengamatan manusia; tes banjir di atas adalah pengganti terukurnya |
| Ubah ukuran jendela | **tidak terlihat** | tidak ada otomasi mouse yang aman (jendela pernah terminimalkan oleh otomasi) |

## Daftar periksa Claude Code

Semuanya **tidak terlihat** (saya tidak menjalankan Claude Code di dalam tab dan tidak bisa mengetik
atau mengeklik di jendela). Yang sudah dibuktikan lewat tes unit tanpa Claude Code:

- Warna truecolor, karakter lebar, mode mouse, bracketed paste, OSC 52, kunci kitty (`Shift+Enter`
  mengirim `CSI 13;2u` bila program meminta): tes `term::engine`, `term::keys`, `term::mouse`, `term::paste`.
- Jawaban DSR/DA dari emulator (ConPTY menunggu `ESC[6n`): tes sesi dan `tests/term_local.rs`.
- Layar nyata: `UNDUHKILAT_START_SCREEN=terminal` menampilkan prompt PowerShell sungguhan di tab lokal
  (tangkapan layar via `PrintWindow`).

Yang harus dicoba Eko sendiri (tiap butir: terlihat / tidak terlihat / gagal):

1. Claude Code di tab lokal: warna dan karakter lebar benar.
2. `Shift+Enter` membuat baris baru; `Alt+Enter`.
3. Roda mouse menggulir; klik di dalam aplikasi bekerja.
4. `Ctrl+V` teks panjang tidak dieksekusi per baris.
5. Teks yang disalin Claude Code (OSC 52) masuk ke papan klip Windows.
6. Ubah ukuran jendela menyusun ulang tampilan; `Esc` dua kali cepat.
7. Semua di atas lewat tab SSH (mis. "Tunnel NAS2"), lalu putuskan server dan tekan "Sambung ulang".

Butir yang gagal menjadi tugas perbaikan di sini.

## Ditunda

- `Alt+V` (tempel gambar): menunggu Rencana 2 (SFTP).
- Penjelajah berkas server di tab SSH: "Segera hadir" sampai Rencana 2.
