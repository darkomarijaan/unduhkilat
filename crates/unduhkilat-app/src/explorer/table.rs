//! The file table every panel shares: columns Nama, Ukuran, Jenis and Diubah under a header that
//! sorts when clicked. A panel narrower than the columns need scrolls sideways.

use super::*;
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, Vec2};
use unduhkilat_core::server::fs::extension_of;

pub const ROW_H: f32 = 24.0;
const HEADER_H: f32 = 26.0;
const SIZE_W: f32 = 84.0;
const TYPE_W: f32 = 132.0;
const DATE_W: f32 = 128.0;
const NAME_MIN: f32 = 200.0;
/// The table is never narrower than this; a narrower panel scrolls sideways instead.
pub const MIN_W: f32 = NAME_MIN + SIZE_W + TYPE_W + DATE_W;
pub const SELECTED: Color32 = Color32::from_rgb(0xF8, 0xC9, 0xB6);

/// Widths of Nama, Ukuran, Jenis, Diubah for a table `width` wide: the three data columns are fixed
/// and the name takes the rest.
pub fn widths(width: f32) -> [f32; 4] {
    [(width - SIZE_W - TYPE_W - DATE_W).max(NAME_MIN), SIZE_W, TYPE_W, DATE_W]
}

/// Run `add` in a horizontal scroll area `height` tall. The content is at least `MIN_W` wide, so all
/// four columns (and every sort) are always there; a narrow panel scrolls sideways.
pub fn scrolled_x(ui: &mut Ui, salt: &'static str, height: f32, add: impl FnOnce(&mut Ui)) {
    let w = ui.available_width().max(MIN_W);
    egui::ScrollArea::horizontal().id_salt(("file-table-x", salt)).max_height(height).auto_shrink([false, false]).show(ui, |ui| {
        ui.set_width(w);
        add(ui);
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Folder,
    Pdf,
    Doc,
    Sheet,
    Archive,
    App,
    Video,
    Audio,
    Image,
    Data,
    Text,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindInfo {
    pub label: String,
    pub badge: String,
    pub tone: Tone,
}

/// The "Jenis" text and the small badge for a name, from its extension.
pub fn file_kind(name: &str, is_dir: bool) -> KindInfo {
    if is_dir {
        return KindInfo { label: "Folder".into(), badge: "DIR".into(), tone: Tone::Folder };
    }
    let ext = extension_of(name);
    let up = ext.to_uppercase();
    let badge = if ext.is_empty() { "FILE".to_string() } else { up.chars().take(4).collect() };
    let (label, tone): (String, Tone) = match ext.as_str() {
        "" => ("Berkas".into(), Tone::Other),
        "pdf" => ("Dokumen PDF".into(), Tone::Pdf),
        "doc" | "docx" | "odt" | "rtf" => (format!("Dokumen {up}"), Tone::Doc),
        "ppt" | "pptx" | "odp" => (format!("Presentasi {up}"), Tone::Doc),
        "xls" | "xlsx" | "ods" | "csv" => (format!("Lembar {up}"), Tone::Sheet),
        "zip" | "rar" | "7z" | "tar" | "gz" | "tgz" | "bz2" | "xz" => (format!("Arsip {up}"), Tone::Archive),
        "iso" => ("Citra disk ISO".into(), Tone::Archive),
        "apk" => ("Paket Android".into(), Tone::App),
        "exe" | "msi" => ("Program Windows".into(), Tone::App),
        "mp4" | "mkv" | "avi" | "mov" | "webm" => (format!("Video {up}"), Tone::Video),
        "mp3" | "wav" | "flac" | "m4a" | "ogg" => (format!("Audio {up}"), Tone::Audio),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" => (format!("Gambar {up}"), Tone::Image),
        "json" | "xml" | "yaml" | "yml" | "toml" | "ini" | "conf" | "cfg" | "sql" | "db" => (format!("Berkas {up}"), Tone::Data),
        "txt" | "log" => ("Berkas teks".into(), Tone::Text),
        "md" => ("Dokumen Markdown".into(), Tone::Text),
        "sh" | "bat" | "ps1" | "py" | "js" | "ts" | "rs" | "go" | "java" | "c" | "h" | "cpp" | "cs" | "php" | "rb" | "html" | "css" => (format!("Kode {up}"), Tone::Text),
        _ => (format!("Berkas {up}"), Tone::Other),
    };
    KindInfo { label, badge, tone }
}

/// Badge background and text colours (text 4.5:1 or better on its background).
fn tone_colors(t: Tone) -> (Color32, Color32) {
    let c = |r, g, b| Color32::from_rgb(r, g, b);
    match t {
        Tone::Folder => (c(0xF6, 0xE2, 0xA6), c(0x6B, 0x4E, 0x00)),
        Tone::Pdf => (c(0xF7, 0xD3, 0xD0), c(0x8A, 0x1C, 0x13)),
        Tone::Doc => (c(0xD6, 0xE6, 0xF7), c(0x17, 0x4A, 0x82)),
        Tone::Sheet | Tone::App => (c(0xD3, 0xEB, 0xD9), c(0x17, 0x58, 0x3A)),
        Tone::Archive => (c(0xDD, 0xD6, 0xF7), c(0x3B, 0x2A, 0x99)),
        Tone::Video => (c(0xF5, 0xD9, 0xEA), c(0x7A, 0x1F, 0x58)),
        Tone::Audio => (c(0xFB, 0xE3, 0xCC), c(0x7A, 0x3B, 0x00)),
        Tone::Image => (c(0xCF, 0xEE, 0xEE), c(0x0F, 0x5A, 0x5A)),
        Tone::Data => (c(0xD6, 0xE6, 0xF7), c(0x17, 0x4A, 0x82)),
        Tone::Text | Tone::Other => (c(0xE6, 0xE2, 0xDA), c(0x3A, 0x38, 0x33)),
    }
}

/// `2026-11-08 09:12` in this computer's time zone; empty when the server does not say.
pub fn format_modified(secs: Option<i64>) -> String {
    secs.and_then(|s| chrono::DateTime::from_timestamp(s, 0)).map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default()
}

fn cell_rects(rect: Rect) -> [Rect; 4] {
    let w = widths(rect.width());
    let mut x = rect.left();
    let mut out = [Rect::NOTHING; 4];
    for (i, cw) in w.iter().enumerate() {
        out[i] = Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(x + cw, rect.bottom()));
        x += cw;
    }
    out
}

/// The sortable header. Returns true when the sort changed.
/// `salt` keeps the header's widget ids apart from another panel's header.
pub fn header(ui: &mut Ui, salt: &'static str, sort: &mut (SortKey, bool)) -> bool {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), HEADER_H), Sense::hover());
    let cells = cell_rects(rect);
    ui.painter().rect_filled(rect, 0.0, SURFACE_RAISED);
    ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, BORDER));
    let mut changed = false;
    for (i, (key, label)) in [(SortKey::Name, "Nama"), (SortKey::Size, "Ukuran"), (SortKey::Type, "Jenis"), (SortKey::Modified, "Diubah")].into_iter().enumerate() {
        let r = cells[i];
        let resp = ui.interact(r, egui::Id::new(("file-table-header", salt, label)), Sense::click());
        let on = sort.0 == key;
        if on {
            ui.painter().rect_filled(r, 0.0, SELECTED);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 0.0, BORDER);
        }
        if i > 0 {
            ui.painter().vline(r.left(), r.y_range(), Stroke::new(1.0, BORDER));
        }
        let arrow = if on { if sort.1 { " \u{25B2}" } else { " \u{25BC}" } } else { "" };
        let text = format!("{label}{arrow}");
        let font = theme::bold(12.0);
        if i == 1 {
            ui.painter().text(egui::pos2(r.right() - 10.0, r.center().y), Align2::RIGHT_CENTER, text, font, FG);
        } else {
            ui.painter().text(egui::pos2(r.left() + 10.0, r.center().y), Align2::LEFT_CENTER, text, font, FG);
        }
        if resp.on_hover_text("Klik untuk mengurutkan").clicked() {
            *sort = (key, if on { !sort.1 } else { true });
            changed = true;
        }
    }
    changed
}

fn clipped_text(ui: &Ui, cell: Rect, left_pad: f32, right_pad: f32, text: String, color: Color32, align_right: bool) {
    let area = Rect::from_min_max(egui::pos2(cell.left() + left_pad, cell.top()), egui::pos2(cell.right() - right_pad, cell.bottom()));
    let p = ui.painter().with_clip_rect(area);
    let font = FontId::proportional(13.0);
    if align_right {
        p.text(egui::pos2(area.right(), area.center().y), Align2::RIGHT_CENTER, text, font, color);
    } else {
        p.text(egui::pos2(area.left(), area.center().y), Align2::LEFT_CENTER, text, font, color);
    }
}

/// One file or folder row.
pub fn row(ui: &mut Ui, e: &Entry, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let bg = if selected { SELECTED } else if resp.hovered() { Color32::from_rgb(0xF4, 0xF1, 0xEC) } else { Color32::TRANSPARENT };
    ui.painter().rect_filled(rect, 0.0, bg);
    let cells = cell_rects(rect);
    let kind = file_kind(&e.name, e.is_dir);
    let (badge_bg, badge_fg) = tone_colors(kind.tone);
    let badge = Rect::from_min_size(egui::pos2(rect.left() + 8.0, rect.center().y - 8.0), Vec2::new(30.0, 16.0));
    ui.painter().rect_filled(badge, 3.0, badge_bg);
    ui.painter().text(badge.center(), Align2::CENTER_CENTER, &kind.badge, theme::bold(9.0), badge_fg);
    clipped_text(ui, cells[0], 46.0, 8.0, e.name.clone(), FG, false);
    if !e.is_dir {
        clipped_text(ui, cells[1], 4.0, 10.0, format::bytes(e.size as i64), FG, true);
    }
    clipped_text(ui, cells[2], 10.0, 6.0, kind.label, FG_SECONDARY, false);
    clipped_text(ui, cells[3], 10.0, 6.0, format_modified(e.modified), FG_SECONDARY, false);
    resp
}

/// The ".." row that goes up one folder.
pub fn up_row(ui: &mut Ui) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
    if ui.is_rect_visible(rect) {
        if resp.hovered() {
            ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(0xF4, 0xF1, 0xEC));
        }
        ui.painter().text(egui::pos2(rect.left() + 14.0, rect.center().y), Align2::LEFT_CENTER, "\u{2191}  ..", FontId::proportional(13.0), FG_SECONDARY);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_four_columns_are_always_there_and_the_name_takes_the_rest() {
        for w in [300.0f32, MIN_W, 900.0, 1400.0] {
            let c = widths(w);
            assert!(c.iter().all(|x| *x > 0.0), "{w}: {c:?}");
            assert!(c[0] >= NAME_MIN);
        }
        let wide = widths(1000.0);
        assert!((wide.iter().sum::<f32>() - 1000.0).abs() < 0.01, "a wide panel is filled exactly");
        assert_eq!(widths(300.0).iter().sum::<f32>(), MIN_W, "a narrow one is as wide as the columns need, and scrolls");
    }

    #[test]
    fn kinds_read_naturally() {
        assert_eq!(file_kind("a.pdf", false), KindInfo { label: "Dokumen PDF".into(), badge: "PDF".into(), tone: Tone::Pdf });
        assert_eq!(file_kind("Proyek", true).label, "Folder");
        assert_eq!(file_kind("x.APK", false).label, "Paket Android");
        assert_eq!(file_kind("laya.zip", false).label, "Arsip ZIP");
        assert_eq!(file_kind("data.xyz", false).label, "Berkas XYZ");
        assert_eq!(file_kind(".bashrc", false).badge, "FILE", "a dot file has no extension");
        assert_eq!(file_kind("README", false).label, "Berkas");
        assert_eq!(file_kind("a.verylongext", false).badge, "VERY", "the badge is at most four letters");
    }

    #[test]
    fn modified_time_is_shown_to_the_minute_or_left_blank() {
        assert_eq!(format_modified(None), "");
        let s = format_modified(Some(1_700_000_000));
        assert_eq!(s.len(), 16, "{s}");
        assert!(s.chars().enumerate().all(|(i, c)| match i {
            4 | 7 => c == '-',
            10 => c == ' ',
            13 => c == ':',
            _ => c.is_ascii_digit(),
        }));
    }
}
