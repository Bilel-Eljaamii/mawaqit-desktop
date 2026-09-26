pub mod application;
pub mod domain;
pub mod infrastructure;
pub mod presentation;

use std::{collections::HashSet, time::Duration};

use application::prayer_logic::{adhan_entries, iqama_entries, is_due, next_prayer};
use chrono::Local;
use domain::models::{AppConfig, TodayPayload};
use mawaqit_api::MawaqitClient;
use presentation::tray::{set_tray_status, setup_tray, update_tray, RefreshFlag};
use tauri::{Emitter, Manager};
#[cfg(not(target_os = "linux"))]
use tauri_plugin_notification::NotificationExt;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let client = MawaqitClient::new();

    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(tauri_plugin_notification::init())
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
        ])
        .setup(|app| {
            // Make the mawaqit icon show up in the application menu, dock
            // and taskbar for the portable binary too.
            infrastructure::desktop::ensure_menu_entry();

            if let Some(window) = app.get_webview_window("main") {
                if let Ok(img) = image::load_from_memory(include_bytes!("../icons/icon.png")) {
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
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
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
            match client.conf_data(&config.mosque_slug).await {
                Ok(conf) => {
                    match mawaqit_api::times_for_date(&conf, Local::now().date_naive()) {
                        Ok(times) => today = Some(TodayPayload::from_conf(&conf, times)),
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
        update_tray(handle, next.minutes_remaining, &next.name);
    }

    // Adhan alerts (notification + athan).
    for (name, time) in adhan_entries(&payload.times.adhan) {
        if is_due(now, &time) && alerted.insert(format!("{date}|adhan|{name}")) {
            notify(handle, "Mawaqit", &format!("It is time for the {name} adhan"));
            if config.sound_enabled && infrastructure::audio::play_athan() {
                // The UI shows its stop button while the athan sounds.
                let _ = handle.emit("athan-started", ());
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
}

fn notify(handle: &tauri::AppHandle, title: &str, body: &str) {
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

    #[cfg(not(target_os = "linux"))]
    let _ = handle.notification().builder().title(title).body(body).show();
}
