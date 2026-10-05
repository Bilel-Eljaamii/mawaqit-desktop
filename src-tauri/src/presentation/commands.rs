use std::path::PathBuf;

use mawaqit_api::{MawaqitClient, MonthIqamaTimes, MonthTimes, Mosque};
use tauri::State;

use crate::{
    domain::models::{AppConfig, TodayPayload},
    infrastructure::{
        audio,
        config::{cache_dir, load_config, save_config},
    },
};

#[tauri::command]
pub fn get_config() -> AppConfig {
    load_config()
}

#[tauri::command]
pub fn update_config(
    config: AppConfig,
    client: State<MawaqitClient>,
) -> Result<(), String> {
    // FINDING F2: a slug reaching the config file must be a real mosque page
    // identifier — `../`, `?`/`#` or encoded variants never get persisted.
    if !config.mosque_slug.is_empty() && !mawaqit_api::is_valid_slug(&config.mosque_slug)
    {
        return Err(format!(
            "invalid mosque id {:?} — search for the mosque again",
            config.mosque_slug
        ));
    }
    // The legacy global switch stays in the file for downgrades; whatever the
    // caller sent, it must agree with the per-prayer settings that now rule.
    let mut config = config;
    config.sound_enabled = config.alerts.any_adhan();
    let previous = load_config();
    if previous.mosque_slug != config.mosque_slug {
        client.invalidate(Some(&previous.mosque_slug));
    }
    save_config(&config);
    Ok(())
}

#[tauri::command]
pub async fn search_mosques(
    client: State<'_, MawaqitClient>,
    query: String,
) -> Result<Vec<Mosque>, String> {
    if load_config().offline_mode {
        return Err("Offline mode is on — turn it off to search for mosques.".into());
    }
    client.search_mosques(&query).await.map_err(|e| e.to_string())
}

/// The conf-data source for the configured mosque, honoring offline mode:
/// online = network (snapshot fallback on failure, client-managed); offline
/// = the disk snapshot only, never the network.
async fn conf_for_view(
    client: &State<'_, MawaqitClient>,
    config: &AppConfig,
) -> Result<(std::sync::Arc<mawaqit_api::ConfData>, Option<chrono::NaiveDate>), String> {
    if config.offline_mode {
        let (conf, date) = offline_conf_from(&cache_dir(), &config.mosque_slug)?;
        Ok((std::sync::Arc::new(conf), Some(date)))
    } else {
        client.conf_data_dated(&config.mosque_slug).await.map_err(|e| e.to_string())
    }
}

/// Snapshot-only load: sync, disk-only, testable without Tauri.
fn offline_conf_from(
    dir: &std::path::Path,
    slug: &str,
) -> Result<(mawaqit_api::ConfData, chrono::NaiveDate), String> {
    mawaqit_api::disk::load(dir, slug)
        .map(|(date, conf)| (conf, date))
        .ok_or_else(|| {
            "Offline mode is on, but there is no saved data for this mosque yet — go online once to fetch it.".to_string()
        })
}

/// Toggle offline mode. Both directions clear the in-memory page cache so
/// the next view serves from the newly chosen source, not a stale copy.
#[tauri::command]
pub fn set_offline_mode(on: bool, client: State<MawaqitClient>) -> Result<(), String> {
    let mut config = load_config();
    if config.offline_mode != on {
        config.offline_mode = on;
        client.invalidate(None);
        save_config(&config);
    }
    Ok(())
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
    let (conf, as_of) = conf_for_view(&client, &config).await?;
    let times = mawaqit_api::times_for_date(&conf, chrono::Local::now().date_naive())
        .map_err(|e| e.to_string())?;
    Ok(Some(TodayPayload::from_conf(&conf, times, as_of)))
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
    let (conf, _) = conf_for_view(&client, &config).await?;
    mawaqit_api::month_times(&conf, month).map_err(|e| e.to_string())
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
    let (conf, _) = conf_for_view(&client, &config).await?;
    mawaqit_api::month_iqama_times(&conf, month).map_err(|e| e.to_string())
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

/// Play the athan on demand (notification panel's preview button). `sound`
/// is a custom audio file path, or empty/null for the built-in athan;
/// `volume` is 0–100 percent.
#[tauri::command]
pub fn preview_athan(sound: Option<String>, volume: Option<u8>) -> bool {
    let source = match sound.as_deref() {
        Some(path) if !path.trim().is_empty() => {
            audio::AthanSource::File(PathBuf::from(path))
        }
        _ => audio::AthanSource::Builtin,
    };
    audio::play_athan(source, volume)
}

#[cfg(test)]
mod tests {
    //! Offline-mode tests: in offline mode the conf data comes from the disk
    //! snapshot and nothing else — a missing snapshot is a clean, friendly
    //! error, never a network attempt or a panic.

    use super::*;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mawaqit-offline-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_page(name: &str) -> String {
        format!(
            r#"<script>var confData = {{"times":["06:30","08:00","13:00","15:30","17:45"],
               "calendar":[{{"1":["06:30","08:00","13:00","15:30","17:45","19:15"]}}],
               "name":"{name}"}};</script>"#
        )
    }

    #[test]
    fn offline_serves_the_snapshot_when_present() {
        let dir = temp_dir("hit");
        let conf = mawaqit_api::parse_page(&sample_page("Snapshot Mosque"), "t").unwrap();
        mawaqit_api::disk::store(&dir, "my-mosque", &conf).unwrap();

        let (conf, date) = offline_conf_from(&dir, "my-mosque").expect("snapshot serves");
        assert_eq!(conf.name.as_deref(), Some("Snapshot Mosque"));
        assert_eq!(date, chrono::Local::now().date_naive());
    }

    #[test]
    fn offline_without_a_snapshot_is_a_clean_error() {
        let dir = temp_dir("miss");
        let err = offline_conf_from(&dir, "my-mosque").unwrap_err();
        assert!(err.contains("no saved data"), "unfriendly error: {err}");
        assert!(err.contains("go online"), "must tell the user the way out: {err}");
    }

    #[test]
    fn offline_never_serves_another_mosque() {
        let dir = temp_dir("isolation");
        let conf = mawaqit_api::parse_page(&sample_page("A"), "t").unwrap();
        mawaqit_api::disk::store(&dir, "mosque-a", &conf).unwrap();
        assert!(offline_conf_from(&dir, "mosque-b").is_err());
    }
}
