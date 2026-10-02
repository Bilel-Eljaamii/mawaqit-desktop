use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use image::{Rgba, RgbaImage};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Wry,
};

/// Set by the tray menu's "Refresh" item, consumed by the background loop.
#[derive(Clone)]
pub struct RefreshFlag(pub Arc<AtomicBool>);

impl RefreshFlag {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    /// Take the flag: true if a refresh was requested since the last call.
    pub fn take(&self) -> bool {
        self.0.swap(false, Ordering::SeqCst)
    }
}

impl Default for RefreshFlag {
    fn default() -> Self {
        Self::new()
    }
}

pub struct TrayState {
    #[allow(dead_code)]
    tray: TrayIcon,
    next_item: MenuItem<Wry>,
}

pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn hide_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

pub fn setup_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    // Clean icon (no countdown badge) until the first tick updates it.
    let tauri_icon = generate_tray_icon(i64::MAX);

    let next_item =
        MenuItem::with_id(app, "next-prayer", "Next prayer: -", false, None::<&str>)?;
    let open_item = MenuItem::with_id(app, "open", "Open Mawaqit", true, None::<&str>)?;
    let refresh_item =
        MenuItem::with_id(app, "refresh", "Refresh prayer times", true, None::<&str>)?;
    let stop_athan_item =
        MenuItem::with_id(app, "stop-athan", "Stop athan sound", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&next_item, &open_item, &refresh_item, &stop_athan_item, &quit_item],
    )?;

    let tray = TrayIconBuilder::with_id("mawaqit-tray")
        .tooltip("Mawaqit")
        .icon(tauri_icon)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main_window(app),
            "refresh" => {
                if let Some(flag) = app.try_state::<RefreshFlag>() {
                    flag.0.store(true, Ordering::SeqCst);
                }
            }
            // No-op when nothing is playing; always available so no state
            // sync between the playback thread and the menu is needed.
            "stop-athan" => crate::infrastructure::audio::stop_athan(),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if app
                    .get_webview_window("main")
                    .map(|w| w.is_visible().unwrap_or(false))
                    .unwrap_or(false)
                {
                    hide_main_window(app);
                } else {
                    show_main_window(app);
                }
            }
        })
        .build(app)?;

    app.manage(TrayState { tray, next_item });
    Ok(())
}

fn draw_digit(
    image: &mut RgbaImage,
    x: u32,
    y: u32,
    digit: u8,
    color: Rgba<u8>,
    scale: u32,
) {
    let font: [[[bool; 3]; 5]; 10] = [
        [
            [true, true, true],
            [true, false, true],
            [true, false, true],
            [true, false, true],
            [true, true, true],
        ], // 0
        [
            [false, true, false],
            [true, true, false],
            [false, true, false],
            [false, true, false],
            [true, true, true],
        ], // 1
        [
            [true, true, true],
            [false, false, true],
            [true, true, true],
            [true, false, false],
            [true, true, true],
        ], // 2
        [
            [true, true, true],
            [false, false, true],
            [true, true, true],
            [false, false, true],
            [true, true, true],
        ], // 3
        [
            [true, false, true],
            [true, false, true],
            [true, true, true],
            [false, false, true],
            [false, false, true],
        ], // 4
        [
            [true, true, true],
            [true, false, false],
            [true, true, true],
            [false, false, true],
            [true, true, true],
        ], // 5
        [
            [true, true, true],
            [true, false, false],
            [true, true, true],
            [true, false, true],
            [true, true, true],
        ], // 6
        [
            [true, true, true],
            [false, false, true],
            [false, true, false],
            [true, false, false],
            [true, false, false],
        ], // 7
        [
            [true, true, true],
            [true, false, true],
            [true, true, true],
            [true, false, true],
            [true, true, true],
        ], // 8
        [
            [true, true, true],
            [true, false, true],
            [true, true, true],
            [false, false, true],
            [true, true, true],
        ], // 9
    ];

    let pattern = &font[(digit % 10) as usize];
    for (py, row) in pattern.iter().enumerate() {
        for (px, &pixel) in row.iter().enumerate() {
            if !pixel {
                continue;
            }
            for dy in 0..scale {
                for dx in 0..scale {
                    let x = x + px as u32 * scale + dx;
                    let y = y + py as u32 * scale + dy;
                    if x < image.width() && y < image.height() {
                        image.put_pixel(x, y, color);
                    }
                }
            }
        }
    }
}

const TRAY_ICON_PNG: &[u8] = include_bytes!("../../icons/tray-icon.png");
const TRAY_SIZE: u32 = 32;

fn urgency_color(minutes: i64) -> Rgba<u8> {
    if minutes <= 5 {
        Rgba([228, 50, 50, 255]) // red
    } else if minutes <= 10 {
        Rgba([240, 150, 40, 255]) // orange
    } else {
        Rgba([32, 160, 90, 255]) // green
    }
}

/// The mawaqit tray icon with a small countdown badge in the bottom-right
/// corner. The icon itself stays visible: the badge only covers a corner,
/// and it disappears entirely when more than 99 minutes remain (the tooltip
/// carries the exact countdown anyway).
pub fn generate_tray_icon(minutes: i64) -> Image<'static> {
    let mut img = image::load_from_memory(TRAY_ICON_PNG)
        .expect("embedded tray icon decodes")
        .resize_exact(TRAY_SIZE, TRAY_SIZE, image::imageops::FilterType::Lanczos3)
        .into_rgba8();

    if minutes < 100 {
        // Badge pill on x 12..=30, y 19..=30 of the 32x32 canvas, corners
        // rounded off.
        let color = urgency_color(minutes);
        for y in 19..=30 {
            for x in 12..=30 {
                let corner = (x < 14 || x > 28) && (y < 21 || y > 28);
                if !corner {
                    img.put_pixel(x, y, color);
                }
            }
        }

        let digits = minutes.clamp(0, 99).to_string();
        let ink = Rgba([25, 25, 25, 255]);
        let bytes = digits.as_bytes();
        if bytes.len() == 1 {
            draw_digit(&mut img, 18, 21, bytes[0] - b'0', ink, 2);
        } else {
            draw_digit(&mut img, 14, 21, bytes[0] - b'0', ink, 2);
            draw_digit(&mut img, 22, 21, bytes[1] - b'0', ink, 2);
        }
    }

    let (width, height) = img.dimensions();
    Image::new_owned(img.into_raw(), width, height)
}

/// Update the countdown icon, tooltip and the tray menu's info line. When
/// `offline` is set the times come from the disk snapshot, and the tray
/// says so.
pub fn update_tray(app: &AppHandle, minutes: i64, event_label: &str, offline: bool) {
    if let Some(state) = app.try_state::<TrayState>() {
        let suffix = if offline { " (offline)" } else { "" };
        let _ = state.tray.set_icon(Some(generate_tray_icon(minutes)));
        let _ = state.tray.set_tooltip(Some(&format!(
            "Next: {} in {} min{}",
            event_label, minutes, suffix
        )));
        let _ = state
            .next_item
            .set_text(format!("Next: {} in {} min{}", event_label, minutes, suffix));
    }
}

/// Status line without a countdown (not configured, no connection, ...).
pub fn set_tray_status(app: &AppHandle, text: &str) {
    if let Some(state) = app.try_state::<TrayState>() {
        let _ = state.tray.set_tooltip(Some(text));
        let _ = state.next_item.set_text(text.to_string());
    }
}
