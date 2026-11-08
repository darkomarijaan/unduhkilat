---
name: rilis
description: Rilis versi baru UnduhKilat ke GitHub (zip portabel bertanda tangan, pre-release). Pakai saat pengguna minta "rilis", "release", "terbitkan versi X", atau "naikkan versi dan unggah".
---

# Rilis UnduhKilat

Semua langkah mekanis ada di `scripts/release.py`. Jangan mengerjakannya ulang dengan tangan:
jangan mencari kunci, menyusun zip sendiri (`Compress-Archive` menulis `\` di nama berkas dan
merusak zip), menulis program verifikasi, atau mengunduh ulang aset. Skrip sudah menangani semua itu.

## Langkah

1. **Commit perubahan fitur dulu** dengan pesan bergaya repo (`feat(scope): ...`, `fix(scope): ...`,
   bahasa Indonesia, plus baris atribusi dari system reminder). Skrip menolak working tree yang
   belum bersih. Jalankan test hanya bila belum dijalankan setelah perubahan terakhir di sesi ini:
   `cargo test --workspace` dan, bila `extension/` berubah, `node --test extension/tests`.

2. **Tulis catatan rilis** ke scratchpad, misalnya `<scratchpad>/catatan-X.Y.Z.md`. Isinya hanya
   daftar perubahan untuk pengguna dalam bahasa Indonesia, dikelompokkan dengan judul tebal bila
   lebih dari satu topik. Jangan tulis baris "Pembaruan dari ...", SHA-256, atau teks pembaruan
   otomatis; skrip menambahkannya. Bila ekstensi berubah, tulis bahwa ekstensi perlu dimuat ulang
   di `chrome://extensions`.

3. **Jalankan** (satu perintah, ±2–4 menit, hampir semuanya waktu build):

   ```
   python scripts/release.py X.Y.Z --notes "<scratchpad>/catatan-X.Y.Z.md" --trailer "<baris atribusi 1>" --trailer "<baris atribusi 2>"
   ```

   Jalankan di latar belakang (`run_in_background`) dan tunggu notifikasinya; jangan mengecek
   berkas keluarannya berulang kali. Versi baru harus lebih besar dari versi di `Cargo.toml`.
   Bawaannya pre-release, seperti rilis sebelumnya; `--stable` hanya bila pengguna memintanya.
   `--dry-run` membangun dan menandatangani tanpa commit, push, atau rilis.

4. **Laporkan** tautan rilis yang dicetak skrip, SHA-256, dan hal yang perlu dilakukan pengguna
   (misalnya memuat ulang ekstensi).

## Bila gagal

Skrip berhenti dengan pesan `GAGAL: ...`. Pada kegagalan sebelum commit, versi di `Cargo.toml`
dikembalikan otomatis. Perbaiki penyebabnya lalu jalankan ulang. Bila gagal setelah push tetapi
sebelum rilis dibuat, commit versi sudah ada di `main`; buat rilisnya dengan
`gh release create vX.Y.Z -R darkomarijaan/unduhkilat --target main --prerelease --title "UnduhKilat X.Y.Z" --notes-file dist/notes-X.Y.Z.md dist/UnduhKilat-portable-X.Y.Z-x64.zip*`.

## Fakta yang tidak perlu dicari lagi

- Kunci privat: `~/.unduhkilat-signing/release.key` (tidak pernah masuk repo).
- Isi zip: `BACA-SAYA.txt` (dari `packaging/`), `UnduhKilat.exe`, `unduhkilat-host.exe`,
  `unduhkilat-portable.marker` kosong, `extension/` tanpa `tests`.
- `unduhkilat-sign sign ... --expect-public <PUBLIC_KEY_B64>` memverifikasi tanda tangannya sendiri.
  `unduhkilat-sign verify <public> <file>` memeriksa ulang berkas yang sudah ada.
- Updater (`core/src/update.rs`) mengambil aset lewat API GitHub dulu, lalu tautan rilis biasa.
  Uji aset rilis sungguhan dengan
  `cargo test -p unduhkilat-core --test update_live -- --ignored --nocapture`.
