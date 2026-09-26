use mawaqit_api::{MawaqitClient, MonthIqamaTimes, MonthTimes, Mosque};
use tauri::State;

use crate::{
    domain::models::{AppConfig, TodayPayload},
    infrastructure::{audio, config::{load_config, save_config}},
};

#[tauri::command]
pub fn get_config() -> AppConfig {
    load_config()
}

#[tauri::command]
pub fn update_config(config: AppConfig, client: State<MawaqitClient>) {
    let previous = load_config();
    if previous.mosque_slug != config.mosque_slug {
        client.invalidate(Some(&previous.mosque_slug));
    }
    save_config(&config);
}

#[tauri::command]
pub async fn search_mosques(
    client: State<'_, MawaqitClient>,
    query: String,
) -> Result<Vec<Mosque>, String> {
    client.search_mosques(&query).await.map_err(|e| e.to_string())
}

/// Today's times + mosque metadata for the Today view.
#[tauri::command]
pub async fn get_today(
    client: State<'_, MawaqitClient>,
) -> Result<Option<TodayPayload>, String> {
    let config = load_config();
    if !config.has_mosque() {
        return Ok(None);
    }
    let conf = client.conf_data(&config.mosque_slug).await.map_err(|e| e.to_string())?;
    let times = mawaqit_api::times_for_date(&conf, chrono::Local::now().date_naive())
        .map_err(|e| e.to_string())?;
    Ok(Some(TodayPayload::from_conf(&conf, times)))
}

#[tauri::command]
pub async fn get_month(
    client: State<'_, MawaqitClient>,
    month: u32,
) -> Result<MonthTimes, String> {
    let config = load_config();
    if !config.has_mosque() {
        return Err("No mosque configured yet.".into());
    }
    client.month(&config.mosque_slug, month).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_month_iqama(
    client: State<'_, MawaqitClient>,
    month: u32,
) -> Result<MonthIqamaTimes, String> {
    let config = load_config();
    if !config.has_mosque() {
        return Err("No mosque configured yet.".into());
    }
    client.month_iqama(&config.mosque_slug, month).await.map_err(|e| e.to_string())
}

/// Manually stop a sounding athan (UI stop button). No-op when silent.
#[tauri::command]
pub fn stop_athan() {
    audio::stop_athan();
}

/// True while the athan is playing — drives the stop button's visibility.
#[tauri::command]
pub fn athan_playing() -> bool {
    audio::is_playing()
}
