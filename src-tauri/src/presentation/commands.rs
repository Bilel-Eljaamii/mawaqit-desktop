use std::path::PathBuf;

use mawaqit_api::{MonthIqamaTimes, MonthTimes, Mosque};
use tauri::State;

use crate::{
    domain::models::{AppConfig, TodayPayload},
    infrastructure::{
        audio,
        builtin_tor::BuiltinTor,
        client_handle::ClientHolder,
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
    client: State<ClientHolder>,
    builtin: State<BuiltinTor>,
) -> Result<(), String> {
    let previous = load_config();
    let validated =
        crate::application::settings::validate_and_apply_update(&client, &builtin, &previous, config)?;
    save_config(&validated);
    Ok(())
}

#[tauri::command]
pub async fn search_mosques(
    client: State<'_, ClientHolder>,
    query: String,
) -> Result<Vec<Mosque>, String> {
    if load_config().offline_mode {
        return Err("Offline mode is on — turn it off to search for mosques.".into());
    }
    let client = client.current();
    client.search_mosques(&query).await.map_err(|e| e.to_string())
}

/// The conf-data source for the configured mosque, honoring offline mode:
/// online = network (snapshot fallback on failure, client-managed); offline
/// = the disk snapshot only, never the network.
async fn conf_for_view(
    client: &ClientHolder,
    config: &AppConfig,
) -> Result<(std::sync::Arc<mawaqit_api::ConfData>, Option<chrono::NaiveDate>), String> {
    let client = client.current();
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
pub fn set_offline_mode(on: bool, client: State<ClientHolder>) -> Result<(), String> {
    let previous = load_config();
    let config = crate::application::settings::set_offline(&client, &previous, on);
    save_config(&config);
    Ok(())
}

/// Topbar Tor toggle: flips the policy, rebuilds and swaps the transport so
/// the change applies immediately (alarms and UI data paths), and persists
/// it. Built-in mode starts the embedded stack (sub-second, the network
/// connection continues in the background); external mode fails when the
/// configured proxy is not listening.
#[tauri::command]
pub fn set_tor_enabled(
    on: bool,
    client: State<ClientHolder>,
    builtin: State<BuiltinTor>,
) -> Result<bool, String> {
    let previous = load_config();
    let config = crate::application::settings::set_tor(&client, &builtin, &previous, on)?;
    save_config(&config);
    Ok(config.tor.enabled)
}

/// Today's times + mosque metadata for the Today view.
#[tauri::command]
pub async fn get_today(
    client: State<'_, ClientHolder>,
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
    client: State<'_, ClientHolder>,
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
    client: State<'_, ClientHolder>,
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

/// Play the athan on demand (notification panel's preview button). An
/// uncached catalog voice is **downloaded first** (issue #2: previews of
/// not-yet-downloaded voices used to all fall back to the builtin); on
/// download failure it falls back to the builtin. `sound` is a custom
/// audio file path used when no voice is set; `volume` is 0–100 percent.
#[tauri::command]
pub async fn preview_athan(
    client: State<'_, ClientHolder>,
    sound: Option<String>,
    voice_id: Option<String>,
    volume: Option<u8>,
) -> Result<bool, String> {
    let voice_file = match voice_id.as_deref() {
        Some(id) => {
            let dir = crate::infrastructure::config::voices_dir();
            match mawaqit_api::voices::download_voice(&client.current(), id, &dir).await
            {
                Ok(path) => Some(path),
                // Download failed (offline/CDN down): builtin fallback.
                Err(e) => {
                    eprintln!("voice preview download failed ({id}): {e}");
                    None
                }
            }
        }
        None => None,
    };
    let source = match voice_file {
        Some(path) => audio::AthanSource::File(path),
        None => match sound.as_deref() {
            Some(path) if !path.trim().is_empty() => {
                audio::AthanSource::File(PathBuf::from(path))
            }
            _ => audio::AthanSource::Builtin,
        },
    };
    Ok(audio::play_athan(source, volume))
}

/// Whether a catalog voice is already downloaded (drives the sheet's
/// "needs download" marker).
#[tauri::command]
pub fn voice_is_cached(voice_id: String) -> bool {
    crate::infrastructure::config::voices_dir()
        .join(format!("{voice_id}.mp3"))
        .is_file()
}

/// Download a catalog voice into the voices cache. Used by the panel's
/// picker (with a spinner) and at startup for the selected voice.
/// The adhan voice catalog for the notifications panel's voice sheet
/// (drift-proof: the UI renders what Rust exposes, no TS mirror).
#[tauri::command]
pub fn adhan_voices() -> Vec<mawaqit_api::AdhanVoice> {
    mawaqit_api::voices::ADHAN_VOICES.to_vec()
}

#[tauri::command]
pub async fn download_voice(
    client: State<'_, ClientHolder>,
    voice_id: String,
) -> Result<(), String> {
    let dir = crate::infrastructure::config::voices_dir();
    let client = client.current();
    mawaqit_api::voices::download_voice(&client, &voice_id, &dir)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
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
