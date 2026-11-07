# Verifikasi berkas server, pratinjau, dan Alt+V (2026-11-08)

Yang **diuji otomatis** (tanpa internet; server SSH/SFTP dan FTP/FTPS dalam-proses di loopback):
SFTP (daftar, folder, ganti nama, hapus, baca rentang, tulis lanjut, nama Unicode, koneksi putus),
FTP dan FTPS (format daftar Unix dan DOS, TLS eksplisit dan implisit, sertifikat dikonfirmasi sekali
lalu dipin, sertifikat berubah diblokir, password tidak dikirim ke sertifikat yang ditolak),
antrean (unduh, unggah, jeda lalu lanjut tanpa berkas rusak, tabrakan nama, batas kecepatan global,
berkas 0 byte, nama berbahaya, menghapus unggahan tidak menghapus berkas lokal, kunci host belum
dikonfirmasi gagal dengan petunjuk), pratinjau (teks sebagian dan "muat lebih banyak", biner ditolak,
Markdown, gambar, PDF satu halaman, docx, odt, xlsx, ods, ZIP bom, berkas melebihi batas tidak diambil).

Yang **dilihat langsung** (server fixture SFTP di loopback, `crates/unduhkilat-core/examples/fixture_server.rs`):
aplikasi tersambung dengan kunci, menampilkan isi server di panel kanan dan folder Unduhan di panel kiri,
pratinjau Markdown dan gambar tampil benar.

Yang **tidak** diuji dan perlu dicoba Eko:
- Tombol Unduh, Unggah, Ganti nama, Hapus, dan dialog timpa di UI (hanya logika inti yang teruji).
- Pratinjau PDF berhalaman-halaman, lembar xlsx/ods, docx/odt di UI (inti teruji; tampilan tidak dilihat).
- `Alt+V` dari papan klip sungguhan ke server sungguhan (bagian murni teruji; jalur penuh tidak).
- FTP/FTPS terhadap server nyata (hanya server uji), dan SFTP lewat jump host nyata.
- PDF terenkripsi (pesan jelas dipetakan, tetapi tidak ada berkas uji).
- Folder dengan ribuan entri di panel (daftar dibatasi, tetapi kinerja tidak diukur).

Belum dibuat: impor `~/.ssh/config`, SSH agent (butir tambahan spec bagian 13).
