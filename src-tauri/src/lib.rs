pub mod application;
pub mod domain;
pub mod infrastructure;
pub mod presentation;

use std::{collections::HashSet, path::PathBuf, time::Duration};

use application::prayer_logic::{
    adhan_entries, iqama_entries, is_due, minutes_before, next_prayer,
    MAX_NOTIFY_BEFORE_MIN,
};
use chrono::Local;
use domain::models::{AppConfig, AthanMode, TodayPayload};
use infrastructure::audio::AthanSource;
use mawaqit_api::MawaqitClient;
use presentation::tray::{
    prayer_menu_rows, set_tray_status, setup_tray, update_tray, update_tray_prayers,
    RefreshFlag,
};
use tauri::{Emitter, Manager};
#[cfg(not(target_os = "linux"))]
use tauri_plugin_notification::NotificationExt;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // The disk snapshot makes prayer times (and the alarms) survive a dead
    // network: fetched pages are stored, failures fall back to the store.
    let client =
        MawaqitClient::new().with_disk_cache(infrastructure::config::cache_dir());

    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(client.clone())
        .manage(RefreshFlag::new())
        .invoke_handler(tauri::generate_handler![
            presentation::commands::get_config,
            presentation::commands::update_config,
            presentation::commands::search_mosques,
            presentation::commands::get_today,
            presentation::commands::get_month,
            presentation::commands::get_month_iqama,
            presentation::commands::stop_athan,
            presentation::commands::athan_playing,
            presentation::commands::preview_athan,
        ])
        .setup(|app| {
            // Make the mawaqit icon show up in the application menu, dock
            // and taskbar for the portable binary too, and keep the
            // autostart entry pointing at this binary.
            infrastructure::desktop::ensure_menu_entry();
            infrastructure::desktop::ensure_autostart_entry();

            if let Some(window) = app.get_webview_window("main") {
                if let Ok(img) =
                    image::load_from_memory(include_bytes!("../icons/icon.png"))
                {
                    let rgba = img.into_rgba8();
                    let (width, height) = rgba.dimensions();
                    let _ = window.set_icon(tauri::image::Image::new_owned(
                        rgba.into_raw(),
                        width,
                        height,
                    ));
                }
            }

            // Launched at login ("Run at startup" passes --minimized):
            // start quietly in the tray instead of opening the window.
            if std::env::args().any(|arg| arg == "--minimized") {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }

            setup_tray(app.handle())?;

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                background_loop(handle, client).await;
            });

            Ok(())
        })
        // Closing the window keeps the app alive in the tray; quitting is
        // done from the tray menu.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
            // The quick-glance overlay dismisses itself on focus loss.
            if window.label() == "glance" {
                if let tauri::WindowEvent::Focused(false) = event {
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Show the quick-glance overlay when hidden, hide it when shown.
fn toggle_glance(app: &tauri::AppHandle) {
    if let Some(glance) = app.get_webview_window("glance") {
        if glance.is_visible().unwrap_or(false) {
            let _ = glance.hide();
        } else {
            let _ = glance.show();
            let _ = glance.set_focus();
        }
    }
}

/// Pin the glance overlay to the right screen edge: top-right on macOS
/// (tray lives in the menu bar), bottom-right everywhere else.
fn position_glance(glance: &tauri::WebviewWindow) {
    let Ok(Some(monitor)) = glance.primary_monitor() else {
        return;
    };
    let Ok(size) = glance.outer_size() else {
        return;
    };
    let mp = monitor.position();
    let ms = monitor.size();
    let margin = 16;
    let x = mp.x + ms.width as i32 - size.width as i32 - margin;
    #[cfg(target_os = "macos")]
    let y = mp.y + margin * 3;
    #[cfg(not(target_os = "macos"))]
    let y = mp.y + ms.height as i32 - size.height as i32 - margin * 3;
    let _ = glance.set_position(tauri::PhysicalPosition::new(x, y));
}

async fn background_loop(handle: tauri::AppHandle, client: MawaqitClient) {
    let mut last_slug = String::new();
    let mut last_date = chrono::Local::now().date_naive();
    let mut today: Option<TodayPayload> = None;
    // "{date}|adhan|Fajr" keys; reset whenever the date rolls over.
    let mut alerted: HashSet<String> = HashSet::new();

    loop {
        let config = infrastructure::config::load_config();

        if config.mosque_slug != last_slug {
            client.invalidate(None);
            today = None;
            last_slug = config.mosque_slug.clone();
        }

        let now_date = Local::now().date_naive();
        if now_date != last_date {
            alerted.clear();
            today = None;
            last_date = now_date;
        }

        if !config.has_mosque() {
            set_tray_status(&handle, "Mawaqit: right-click to choose your mosque");
        } else if today.is_none() {
            match client.conf_data_dated(&config.mosque_slug).await {
                Ok((conf, as_of)) => {
                    match mawaqit_api::times_for_date(&conf, Local::now().date_naive()) {
                        Ok(times) => {
                            today = Some(TodayPayload::from_conf(&conf, times, as_of))
                        }
                        Err(e) => set_tray_status(&handle, &format!("Mawaqit: {e}")),
                    }
                }
                Err(e) => set_tray_status(&handle, &format!("Mawaqit: {e}")),
            }
        }

        if let Some(payload) = &today {
            tick(&handle, &config, payload, &mut alerted);
        }

        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

/// One minute-tick of tray update + alert dispatch.
fn tick(
    handle: &tauri::AppHandle,
    config: &AppConfig,
    payload: &TodayPayload,
    alerted: &mut HashSet<String>,
) {
    let now = Local::now().time();
    let date = Local::now().date_naive();

    // Tray counts down to the next relevant event: iqama when iqama alerts
    // are on, otherwise the next adhan.
    let mut upcoming: Vec<(String, String)> = adhan_entries(&payload.times.adhan);
    if config.iqama_alerts {
        if let Some(iqama) = &payload.times.iqama {
            upcoming.extend(
                iqama_entries(iqama)
                    .into_iter()
                    .map(|(name, time)| (format!("{name} iqama"), time)),
            );
        }
    }
    if let Some(next) = next_prayer(&upcoming) {
        update_tray(handle, next.minutes_remaining, &next.name, payload.as_of.is_some());
    }
    // The tray menu mirrors today's times (next one marked), refreshed each
    // tick so the marker follows the passing prayers.
    update_tray_prayers(handle, &prayer_menu_rows(payload, now));

    // Per-prayer alerts: each prayer's heads-up and adhan behavior are
    // configured independently.
    for (index, (name, time)) in
        adhan_entries(&payload.times.adhan).into_iter().enumerate()
    {
        let alerts = config.alerts.prayer(index);

        // Pre-adhan notification, exactly once per prayer and day. Config
        // minutes are attacker-writable: cap before use, and treat 0 as off
        // (it would coincide with the adhan itself).
        if let Some(before) = alerts
            .notify_before_min
            .map(|n| n.min(MAX_NOTIFY_BEFORE_MIN))
            .filter(|n| *n > 0)
        {
            if let Some(target) = minutes_before(&time, before) {
                if is_due(now, &target) && alerted.insert(format!("{date}|pre|{name}")) {
                    notify(handle, "Mawaqit", &minutes_from_now(&name, before));
                }
            }
        }

        // At the adhan time.
        if is_due(now, &time) && alerted.insert(format!("{date}|adhan|{name}")) {
            let at_time = format!("It is time for the {name} adhan");
            match alerts.mode {
                AthanMode::Silent => notify_silent(handle, "Mawaqit", &at_time),
                AthanMode::Default => notify(handle, "Mawaqit", &at_time),
                AthanMode::Adhan => {
                    notify_stoppable(handle, "Mawaqit", &at_time);
                    let source = match &alerts.sound {
                        Some(path) => AthanSource::File(PathBuf::from(path)),
                        None => AthanSource::Builtin,
                    };
                    if infrastructure::audio::play_athan(source, alerts.volume) {
                        // The UI shows its stop button while the athan sounds.
                        let _ = handle.emit("athan-started", ());
                    }
                }
            }
        }
    }

    // Optional iqama alerts (notification only).
    if config.iqama_alerts {
        if let Some(iqama) = &payload.times.iqama {
            for (name, time) in iqama_entries(iqama) {
                if is_due(now, &time) && alerted.insert(format!("{date}|iqama|{name}")) {
                    notify(handle, "Mawaqit", &format!("The {name} iqama has started"));
                }
            }
        }
    }

    // Pre-shurouq notification (notification only — sunrise has no adhan).
    if let Some(before) = config
        .alerts
        .shuruq_notify_before_min
        .map(|n| n.min(MAX_NOTIFY_BEFORE_MIN))
        .filter(|n| *n > 0)
    {
        if let Some(target) = minutes_before(&payload.times.adhan.shurouq, before) {
            if is_due(now, &target) && alerted.insert(format!("{date}|pre|Shurouq")) {
                notify(handle, "Mawaqit", &minutes_from_now("Shurouq", before));
            }
        }
    }
}

/// "Fajr adhan in 5 minutes" / "…in 1 minute" — singular kept grammatical.
fn minutes_from_now(event: &str, n: u16) -> String {
    let unit = if n == 1 { "minute" } else { "minutes" };
    format!("{event} in {n} {unit}")
}

/// A plain popup notification (heads-up, iqama, Silent/Default adhan).
fn notify(handle: &tauri::AppHandle, title: &str, body: &str) {
    #[cfg(target_os = "linux")]
    {
        use notify_rust::Notification;
        let _ = Notification::new()
            .summary(title)
            .body(body)
            .icon(infrastructure::desktop::APP_ICON_NAME)
            .timeout(notify_rust::Timeout::Milliseconds(60_000))
            .show();
        let _ = handle;
    }

    #[cfg(not(target_os = "linux"))]
    let _ = handle.notification().builder().title(title).body(body).show();
}

/// The adhan notification, with a click/button that stops a sounding athan.
/// Linux: notify-rust "default" action (as before). Windows: a toast button
/// (tauri-winrt-notification). macOS: a plain popup — action buttons need a
/// signed UNUserNotificationCenter delegate, so the tray's "Stop athan
/// sound" item stays the stop path there.
fn notify_stoppable(handle: &tauri::AppHandle, title: &str, body: &str) {
    #[cfg(target_os = "linux")]
    {
        use notify_rust::Notification;
        // Created and awaited inside one thread: clicking the notification
        // (the "default" action) must stop a sounding athan.
        let title = title.to_string();
        let body = body.to_string();
        std::thread::spawn(move || {
            let shown = Notification::new()
                .summary(&title)
                .body(&body)
                .icon(infrastructure::desktop::APP_ICON_NAME)
                .action("default", "Stop athan")
                .timeout(notify_rust::Timeout::Milliseconds(60_000))
                .show();
            if let Ok(n) = shown {
                n.wait_for_action(|action| {
                    if action == "default" {
                        infrastructure::audio::stop_athan();
                    }
                });
            }
        });
        let _ = handle;
    }

    #[cfg(target_os = "windows")]
    {
        let app_id = handle.config().identifier.clone();
        let _ = tauri_winrt_notification::Toast::new(&app_id)
            .title(title)
            .text(body)
            .add_button("Stop athan", "stop")
            .on_activated(|action| {
                if action.as_deref() == Some("stop") {
                    infrastructure::audio::stop_athan();
                }
                Ok(())
            })
            .show();
        let _ = handle;
    }

    #[cfg(target_os = "macos")]
    notify(handle, title, body);
}

/// A "Silent" alert: popup notification without audio. A sound-suppression
/// hint is sent where the platform supports one (freedesktop); elsewhere a
/// Silent alert looks like a Default one — there is no reachable mute switch.
fn notify_silent(handle: &tauri::AppHandle, title: &str, body: &str) {
    #[cfg(target_os = "linux")]
    {
        use notify_rust::{Hint, Notification};
        let _ = Notification::new()
            .summary(title)
            .body(body)
            .icon(infrastructure::desktop::APP_ICON_NAME)
            .hint(Hint::SuppressSound(true))
            .timeout(notify_rust::Timeout::Milliseconds(60_000))
            .show();
        let _ = handle;
    }

    #[cfg(not(target_os = "linux"))]
    let _ = handle.notification().builder().title(title).body(body).show();
}
