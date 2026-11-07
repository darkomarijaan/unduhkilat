//! "Paket Ekspres" design tokens (DESIGN.md). Every color used by the UI comes from here;
//! no raw hex in widgets. Orange is for the header band, the active tracker step and
//! downloaded segments; ink carries text, the primary button and the selected nav item.

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle};
use std::sync::Arc;

/// Window ground (warm paper).
pub const BG: Color32 = Color32::from_rgb(0xF3, 0xF1, 0xEE);
/// Cards, dialogs, panels.
pub const SURFACE: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
/// Fields, bar troughs, hover fills.
pub const SURFACE_RAISED: Color32 = Color32::from_rgb(0xED, 0xE8, 0xE1);
pub const BORDER: Color32 = Color32::from_rgb(0xE2, 0xDD, 0xD6);
/// Ink.
pub const FG: Color32 = Color32::from_rgb(0x1B, 0x19, 0x18);
pub const FG_SECONDARY: Color32 = Color32::from_rgb(0x5B, 0x55, 0x4E);
pub const MUTED: Color32 = Color32::from_rgb(0x76, 0x6F, 0x67);
/// Kurir orange: fills only. Never for small text (3.2:1 on white).
pub const ACCENT: Color32 = Color32::from_rgb(0xFF, 0x5A, 0x1F);
/// Orange darkened enough to read as text on white (5.2:1).
pub const ACCENT_TEXT: Color32 = Color32::from_rgb(0xC2, 0x41, 0x0C);
/// Text on orange: ink, not white (6.6:1).
pub const ACCENT_INK: Color32 = Color32::from_rgb(0x1B, 0x19, 0x18);
pub const FOCUS: Color32 = Color32::from_rgb(0x1B, 0x19, 0x18);
pub const SUCCESS: Color32 = Color32::from_rgb(0x12, 0x7A, 0x48);
pub const WARNING: Color32 = Color32::from_rgb(0x9A, 0x5B, 0x00);
pub const DANGER: Color32 = Color32::from_rgb(0xBE, 0x2A, 0x1D);
/// Tracker line before the active step.
pub const TRACK_OFF: Color32 = Color32::from_rgb(0xDA, 0xD3, 0xCA);

pub const RADIUS: u8 = 8;
pub const RADIUS_SM: u8 = 6;

/// Tile color per category (nav dots and file tiles only; never status).
pub fn category_color(name: &str) -> Color32 {
    match name.to_ascii_lowercase().as_str() {
        "programs" => Color32::from_rgb(0x6D, 0x4A, 0xFF),
        "video" => Color32::from_rgb(0xE5, 0x48, 0x4D),
        "documents" => Color32::from_rgb(0x0E, 0x7A, 0xE6),
        "compressed" => Color32::from_rgb(0xC7, 0x7D, 0x0A),
        "music" => Color32::from_rgb(0x12, 0xA5, 0x94),
        "images" => Color32::from_rgb(0xD6, 0x40, 0x9F),
        _ => Color32::from_rgb(0x6B, 0x72, 0x80),
    }
}

/// Numbers, waybill ids, paths.
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("telemetry".into()))
}

/// Segoe UI Bold (egui has no synthetic bold): brand, card titles, big numbers.
pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

fn load_system_font(name: &str, files: &[&str]) -> Option<(String, Arc<FontData>)> {
    let dir = std::env::var_os("WINDIR").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Windows".into()).join("Fonts");
    files.iter().find_map(|f| std::fs::read(dir.join(f)).ok()).map(|b| (name.to_string(), Arc::new(FontData::from_owned(b))))
}

pub fn install(ctx: &egui::Context) {
    // Segoe UI for text, Segoe UI Bold for emphasis, Cascadia/Consolas for ids and numbers.
    // Each falls back to egui's built-in fonts when a system font is missing.
    let mut fonts = FontDefinitions::default();
    if let Some((n, d)) = load_system_font("segoe", &["segoeui.ttf"]) {
        fonts.font_data.insert(n.clone(), d);
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, n);
    }
    let mut bold_names: Vec<String> = Vec::new();
    if let Some((n, d)) = load_system_font("segoe-bold", &["segoeuib.ttf", "seguisb.ttf"]) {
        fonts.font_data.insert(n.clone(), d);
        bold_names.push(n);
    }
    bold_names.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(FontFamily::Name("bold".into()), bold_names);
    let mut mono_names: Vec<String> = Vec::new();
    if let Some((n, d)) = load_system_font("cascadia", &["CascadiaMono.ttf", "CascadiaCode.ttf", "consola.ttf"]) {
        fonts.font_data.insert(n.clone(), d);
        mono_names.push(n);
    }
    mono_names.extend(fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default());
    fonts.families.insert(FontFamily::Name("telemetry".into()), mono_names);
    ctx.set_fonts(fonts);

    let mut style = (*ctx.global_style()).clone();
    style.text_styles = [
        (TextStyle::Heading, bold(16.0)),
        (TextStyle::Body, FontId::proportional(13.5)),
        (TextStyle::Button, FontId::proportional(13.5)),
        (TextStyle::Small, FontId::proportional(11.5)),
        (TextStyle::Monospace, mono(12.0)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(14.0, 7.0);
    style.spacing.interact_size.y = 32.0;

    let v = &mut style.visuals;
    v.dark_mode = false;
    v.override_text_color = Some(FG);
    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.extreme_bg_color = SURFACE_RAISED;
    v.faint_bg_color = SURFACE_RAISED;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = RADIUS.into();
    v.menu_corner_radius = RADIUS_SM.into();
    v.window_shadow = egui::epaint::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(70) };
    v.hyperlink_color = ACCENT_TEXT;
    v.selection.bg_fill = ACCENT.gamma_multiply(0.28);
    v.selection.stroke = Stroke::new(2.0, FOCUS);
    let r = egui::CornerRadius::same(RADIUS_SM);
    for (w, fill, stroke_col) in [
        (&mut v.widgets.noninteractive, SURFACE, BORDER),
        (&mut v.widgets.inactive, SURFACE, BORDER),
        (&mut v.widgets.hovered, SURFACE_RAISED, FG_SECONDARY),
        (&mut v.widgets.active, SURFACE_RAISED, FG),
        (&mut v.widgets.open, SURFACE_RAISED, BORDER),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = Stroke::new(1.0, stroke_col);
        w.corner_radius = r;
        w.fg_stroke = Stroke::new(1.0, FG);
    }
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, FG_SECONDARY);
    ctx.set_global_style(style);
}

/// Level-1 action: solid ink with white text (one per screen / dialog).
pub fn primary_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(text.to_string()).color(Color32::WHITE).font(bold(13.5))).fill(FG).stroke(Stroke::NONE).corner_radius(RADIUS)
}

/// The lightning bolt (same silhouette as the icon sprite in the mockups).
pub fn paint_bolt(p: &egui::Painter, center: egui::Pos2, size: f32, color: Color32) {
    let k = size / 24.0;
    let pt = |x: f32, y: f32| egui::pos2(center.x + (x - 12.0) * k, center.y + (y - 12.0) * k);
    let (a, b, c, d, e, f) = (pt(13.5, 2.0), pt(4.5, 13.5), pt(11.0, 13.5), pt(10.0, 22.0), pt(19.0, 10.5), pt(12.5, 10.5));
    let mut mesh = egui::epaint::Mesh::default();
    for t in [[a, b, f], [b, c, f], [c, e, f], [c, d, e]] {
        let i = mesh.vertices.len() as u32;
        for q in t {
            mesh.colored_vertex(q, color);
        }
        mesh.add_triangle(i, i + 1, i + 2);
    }
    p.add(egui::Shape::mesh(mesh));
}
