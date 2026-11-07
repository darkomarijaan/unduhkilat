//! System tray (DEC-004, FR-051) and the lightning-bolt brand mark.

use crate::boot::exit_flow;
use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::runtime::Handle;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use unduhkilat_core::engine::manager::Manager;

/// RGBA lightning bolt drawn procedurally (no bitmap asset, per §7).
pub fn bolt_rgba(size: u32) -> Vec<u8> {
    let poly = [(0.58, 0.04), (0.20, 0.56), (0.46, 0.56), (0.38, 0.96), (0.80, 0.40), (0.54, 0.40)];
    let inside = |x: f32, y: f32| {
        let mut c = false;
        let mut j = poly.len() - 1;
        for i in 0..poly.len() {
            let (xi, yi) = poly[i];
            let (xj, yj) = poly[j];
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                c = !c;
            }
            j = i;
        }
        c
    };
    let mut px = vec![0u8; (size * size * 4) as usize];
    for py in 0..size {
        for pxl in 0..size {
            // 2x2 supersampling for soft edges.
            let mut hit = 0;
            for (dx, dy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                if inside((pxl as f32 + dx) / size as f32, (py as f32 + dy) / size as f32) {
                    hit += 1;
                }
            }
            let i = ((py * size + pxl) * 4) as usize;
            px[i..i + 4].copy_from_slice(&[0x00, 0xD2, 0xFF, (hit * 255 / 4) as u8]);
        }
    }
    px
}

pub fn window_icon() -> egui::IconData {
    egui::IconData { rgba: bolt_rgba(64), width: 64, height: 64 }
}

/// Flag set by the tray's "Pengaturan..." item; the UI thread consumes it.
pub type OpenSettings = Arc<AtomicBool>;

pub struct Tray {
    _icon: TrayIcon,
    pub open_settings: OpenSettings,
}

pub fn show_window(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    ctx.request_repaint();
}

/// Must be called on the thread that runs the window's message loop.
pub fn create(ctx: egui::Context, rt: Handle, manager: Manager) -> Result<Tray, Box<dyn std::error::Error>> {
    let menu = Menu::new();
    let show = MenuItem::new("Tampilkan UnduhKilat", true, None);
    let start = MenuItem::new("Mulai Antrian", true, None);
    let stop = MenuItem::new("Jeda Antrian", true, None);
    let settings = MenuItem::new("Pengaturan...", true, None);
    let quit = MenuItem::new("Keluar dari UnduhKilat", true, None);
    menu.append_items(&[&show, &start, &stop, &settings, &PredefinedMenuItem::separator(), &quit])?;

    let icon = tray_icon::Icon::from_rgba(bolt_rgba(32), 32, 32)?;
    let tray = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("UnduhKilat").with_icon(icon).build()?;

    let open_settings: OpenSettings = Arc::new(AtomicBool::new(false));
    let flag = open_settings.clone();
    let (show_id, start_id, stop_id, set_id, quit_id) = (show.id().clone(), start.id().clone(), stop.id().clone(), settings.id().clone(), quit.id().clone());

    // The window may be hidden (and therefore not repainting), so tray actions are
    // handled right here instead of waiting for the UI thread.
    let (c1, rt1, m1) = (ctx.clone(), rt.clone(), manager.clone());
    std::thread::spawn(move || {
        for ev in MenuEvent::receiver() {
            if ev.id == show_id {
                show_window(&c1);
            } else if ev.id == start_id {
                let m = m1.clone();
                rt1.spawn(async move {
                    let _ = m.start_all().await;
                });
            } else if ev.id == stop_id {
                let m = m1.clone();
                rt1.spawn(async move {
                    let _ = m.stop_all().await;
                });
            } else if ev.id == set_id {
                flag.store(true, Ordering::SeqCst);
                show_window(&c1);
            } else if ev.id == quit_id {
                let (rt, m) = (rt1.clone(), m1.clone());
                std::thread::spawn(move || exit_flow(rt, m));
            }
        }
    });
    let c2 = ctx;
    std::thread::spawn(move || {
        for ev in TrayIconEvent::receiver() {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                show_window(&c2);
            }
        }
    });
    Ok(Tray { _icon: tray, open_settings })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bolt_has_opaque_center_and_transparent_corners() {
        let s = 32;
        let px = bolt_rgba(s);
        let a = |x: u32, y: u32| px[((y * s + x) * 4 + 3) as usize];
        assert_eq!(a(0, 0), 0);
        assert_eq!(a(s - 1, s - 1), 0);
        assert!(a((0.48 * s as f32) as u32, (0.5 * s as f32) as u32) > 200, "bolt body is opaque");
    }
}
