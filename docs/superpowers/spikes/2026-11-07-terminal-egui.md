# Purwarupa: emulator terminal di egui

Tanggal: 2026-11-07. Kode purwarupa dibuang (hidup sebentar di `spikes/term/`). Pengukuran: build release, Windows, tanpa GPU (egui dijalankan tanpa jendela: tata letak teks dan tesselasi, tanpa gambar ke layar).

## Keputusan

**Pakai `alacritty_terminal` 0.26.** (Apache-2.0.) Kedua kandidat cukup cepat; yang menentukan adalah kelengkapan fitur untuk Claude Code.

Fixture: 425 KB aliran byte yang meniru TUI modern: layar alternatif, warna 256 dan truecolor, garis kotak dan blok Unicode, CJK dan emoji, bracketed paste, mouse (klik, seret, SGR), event fokus, protokol papan ketik kitty, OSC 52, OSC 8, pembaruan tersinkron (mode 2026), lalu 5.000 baris keluaran berwarna.

| Hal | `alacritty_terminal` | `vt100` 0.18 |
|---|---|---|
| Kecepatan memproses fixture | 4,5 ms (±94 MB/s) | 7,6 ms (±56 MB/s) |
| Menggambar grid 120x40 di egui, **seluruh baris berubah tiap bingkai** | rata-rata 0,84 ms, p95 1,24 ms, terburuk 1,48 ms | rata-rata 0,76 ms, p95 0,99 ms, terburuk 1,31 ms |
| Layar alternatif, bracketed paste | ya | ya |
| Mouse (klik, seret, SGR 1006) | ya (`TermMode::MOUSE_*`, `SGR_MOUSE`) | ya (`mouse_protocol_mode`, `Sgr`) |
| Event fokus (`?1004`) | ya (`TermMode::FOCUS_IN_OUT`) | **tidak** (lewat `unhandled_csi`) |
| Protokol papan ketik kitty (`ESC[>1u`) | **ya** (`KITTY_KEYBOARD_PROTOCOL`, aktif lewat `Config.kitty_keyboard = true`) | **tidak** (lewat `unhandled_csi`) |
| Pembaruan tersinkron (mode 2026) | ditangani pengurai (vte) | **tidak** (lewat `unhandled_csi`) |
| OSC 52 (salin ke papan klip) | `Event::ClipboardStore(_, "Hello dari server")` sudah ter-decode | callback `copy_to_clipboard` memberi **base64 mentah** (perlu decode sendiri) |
| OSC 8 (tautan) | bagian dari sel (`CellExtra.hyperlink`) | tidak dilacak |
| Karakter lebar (CJK, emoji) | `Flags::WIDE_CHAR` + sel pendamping `WIDE_CHAR_SPACER` | `is_wide()` + sel pendamping kosong |
| Riwayat gulir | `Config.scrolling_history` (diuji 10.000) | argumen konstruktor |

Kedua kandidat jauh di bawah 16 ms per bingkai (60 fps) bahkan pada kasus terburuk tempat setiap baris berubah tiap bingkai, jadi kecepatan bukan pembeda. `vt100` tidak punya fitur yang Claude Code dan spec butuhkan (kitty/fokus/tersinkron); menambalnya sendiri berarti menulis ulang pengurai CSI.

## Yang perlu diketahui Rencana 3

- **Papan ketik diperluas ditangani di sisi pengirim tombol.** Emulator hanya mencatat mode yang diminta program (`Term::mode()` memuat `DISAMBIGUATE_ESC_CODES`, `REPORT_EVENT_TYPES`, dst.). Pengodean tombol (mis. `Shift+Enter` yang terbedakan dari `Enter`) harus ditulis di kode UnduhKilat: bila `KITTY_KEYBOARD_PROTOCOL` aktif kirim `CSI u`, bila tidak kirim urutan klasik. Aktifkan dengan `Config { kitty_keyboard: true, .. }`.
- Pemakaian: `Term::new(Config, &impl Dimensions, impl EventListener)` + `vte::ansi::Processor::default().advance(&mut term, bytes)`; `EventListener::send_event` menerima `ClipboardStore` (OSC 52, isi teks sudah ter-decode), `PtyWrite` (jawaban emulator ke program, harus diteruskan ke kanal SSH), `Title`, `Bell`.
- Baca sel dengan `term.grid()[Point::new(Line(r), Column(c))]` (`Cell { c, fg, bg, flags, extra }`); gabungkan baris menjadi `LayoutJob` per rentang warna sama dan gambar lewat `painter.layout_job` + `painter.galley`.
- `vte` diekspor ulang sebagai `alacritty_terminal::vte`.
- Modul `tty` dan `event_loop` milik `alacritty_terminal` tidak dipakai (itu untuk PTY lokal); hanya `Term`, `grid`, `index`, `selection`, dan `event`.
- API egui 0.37 yang dipakai purwarupa: `Context::run_ui` dan `CentralPanel::show_inside` (yang terakhir ditandai usang; periksa penggantinya di kode aplikasi).
