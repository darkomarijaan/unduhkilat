# Klien Server: Peta Rencana

**Spec:** `docs/superpowers/specs/2026-11-07-server-client-design.md` (disetujui Eko, 2026-11-07).

Spec ini terlalu besar untuk satu rencana yang masih bisa dipercaya. Rencana dipecah menjadi lima bagian berurutan, mengikuti spec bagian 9. Setiap bagian menghasilkan perangkat lunak yang bisa diuji sendiri. **Rilis hanya keluar setelah kelima bagian selesai** (keputusan Eko: semua fitur di rilis pertama).

| # | Rencana | Menghasilkan | Bergantung pada |
|---|---|---|---|
| 1 | `…-01-spikes-and-foundation.md` | Tiga purwarupa pembuang (terminal, PDF, rantai jump) dengan catatan keputusan; penyimpanan server, kredensial, kunci host; sambungan SSH dengan jump; layar Server + "Uji sambungan" | spec |
| 2 | `…-02-files-and-queue.md` | SFTP, FTP, FTPS: daftar direktori, operasi berkas, unduh dan **unggah** lewat antrean | 1 |
| 3 | `…-03-terminal.md` | Terminal (emulasi, tab, mouse, papan klip, Claude Code, `Alt+V` tempel gambar, tata letak A/B) | 1, 2 (untuk `Alt+V`) |
| 4 | `…-04-preview.md` | Pratinjau teks, Markdown, gambar, PDF, Office (teks/tabel), buka di aplikasi | 2 |
| 5 | `…-05-ui-and-release.md` | Penjelajah dua panel, pengaturan, dokumentasi, pembaruan otomatis untuk berkas tambahan bila perlu, rilis | 1-4 |

**Rencana 2 sampai 5 sengaja belum ditulis rinci.** Pilihan pustaka (emulator terminal, perender PDF, cara FTPS) ditentukan oleh hasil purwarupa di Rencana 1 (spec bagian 12 butir 1). Menulis tugas rinci sebelum itu berarti menebak nama fungsi pustaka. Setiap rencana berikutnya ditulis setelah rencana sebelumnya selesai dan catatan keputusannya ada.

## Cakupan spec per rencana (agar tidak ada yang terlewat)

- Bagian 2 butir 1, 2, 3, 8 (server, kredensial, jump, kunci host): Rencana 1.
- Bagian 2 butir 5, 6 (penjelajah, antrean unggah) dan FTPS (bagian 8 butir 7): Rencana 2; layar penjelajahnya di Rencana 5.
- Bagian 2 butir 4, bagian 8a, 8b, tata letak A/B: Rencana 3.
- Bagian 2 butir 7, bagian 6 (pratinjau): Rencana 4.
- Bagian 7 (keamanan) tersebar: kunci host dan kredensial di Rencana 1; isi sesi tidak masuk log di Rencana 3; pratinjau tanpa makro di Rencana 4.

## Hal yang sengaja belum diputuskan

- SOCKS5 untuk sambungan pertama: Rencana 1 memakai proxy HTTP (`ProxyConfig::ftp_proxy`) dan menolak mode SOCKS5 dengan pesan jelas. Dukungan SOCKS5 untuk SSH dicatat sebagai tindak lanjut di Rencana 5 bila Eko memakainya.
- SSH agent dan impor `~/.ssh/config`: butir tambahan (spec bagian 13); dikerjakan di Rencana 5 hanya bila murah.
