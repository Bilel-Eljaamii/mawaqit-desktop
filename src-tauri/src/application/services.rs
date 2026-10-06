//! Settings + data-path application services: the logic behind the IPC
//! commands, extracted from `presentation/commands.rs` so it runs in tests
//! without a Tauri harness. Commands are thin adapters over these fns.

use std::path::{Path, PathBuf};

use tauri::State;

use crate::domain::models::{is_plausible_tor_host, AppConfig, TodayPayload};
use crate::infrastructure::audio::AthanSource;
use crate::infrastructure::client_handle::ClientHolder;
use crate::infrastructure::config::{cache_dir, voices_dir};
use crate::{
    mawaqit_api,
    mawaqit_api::{MonthIqamaTimes, MonthTimes},
};

fn is_plausible_socks_addr(addr: &str) -> bool {
    let Some(rest) = addr.trim().strip_prefix("socks5h://") else {
        return false;
    };
    // A proxy address is scheme://host[:port] — a path, query or fragment
    // means the string is not one.
    if rest.contains(['/', '?', '#']) {
        return false;
    }
    let authority = rest;
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    let host_ok =
        !host.is_empty() && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    let port_ok = port.is_none_or(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    host_ok && port_ok
}

/// Validate and apply a settings update (the `update_config` command body):
/// gates the mosque slug (F2), the Tor proxy (host plausibility, port
/// bounds) and the voice ids (catalog keys), recomputes the legacy sound
/// switch, invalidates the page cache on a mosque switch and swaps the
/// transport when the Tor policy changed. Returns the config to persist.
pub fn validate_and_apply_update(
    holder: &ClientHolder,
    previous: &AppConfig,
    mut config: AppConfig,
) -> Result<AppConfig, String> {
    // FINDING F2: a slug reaching the config file must be a real mosque page
    // identifier — `../`, `?`/`#` or encoded variants never get persisted.
    if !config.mosque_slug.is_empty() && !mawaqit_api::is_valid_slug(&config.mosque_slug)
    {
        return Err(format!(
            "invalid mosque id {:?} — search for the mosque again",
            config.mosque_slug
        ));
    }
    // Tor opt-in: the host and port must be plausible for the socks5h URL
    // the domain value object builds; the strict enforcement (remote-DNS
    // scheme, reachability) lives in the api at client construction.
    if config.tor.enabled {
        if !is_plausible_tor_host(&config.tor.host) {
            return Err(format!("invalid Tor proxy host {:?}", config.tor.host));
        }
        if config.tor.port == 0 {
            return Err("invalid Tor proxy port 0 — use 1–65535".into());
        }
    }
    // Voice ids are catalog keys: a known id resolves to a CDN URL; an
    // unknown one would silently never play, so reject it at save time.
    for voice in [
        &config.alerts.fajr.voice,
        &config.alerts.dhuhr.voice,
        &config.alerts.asr.voice,
        &config.alerts.maghrib.voice,
        &config.alerts.isha.voice,
    ]
    .into_iter()
    .flatten()
    {
        if mawaqit_api::voices::adhan_voice_url(voice).is_none() {
            return Err(format!("unknown adhan voice {voice:?}"));
        }
    }
    // The legacy global switch stays in the file for downgrades; whatever the
    // caller sent, it must agree with the per-prayer settings that now rule.
    config.sound_enabled = config.alerts.any_adhan();
    if previous.mosque_slug != config.mosque_slug {
        holder.current().invalidate(Some(&previous.mosque_slug));
    }
    // A changed Tor policy swaps the transport immediately — the toggle and
    // the settings fields apply without a restart.
    if previous.tor != config.tor {
        crate::infrastructure::client_handle::rebuild_client(holder, &config);
    }
    Ok(config)
}

/// Topbar Tor toggle: flips the policy, rebuilds and swaps the transport so
/// the change applies immediately (alarms and UI data paths). Fails when
/// Tor is switched on without a plausible host.
pub fn set_tor(
    holder: &ClientHolder,
    previous: &AppConfig,
    on: bool,
) -> Result<AppConfig, String> {
    if on && !is_plausible_tor_host(&previous.tor.host) {
        return Err("Cannot enable Tor — set a valid proxy host in Settings first.".into());
    }
    let mut config = previous.clone();
    if config.tor.enabled != on {
        config.tor.enabled = on;
        crate::infrastructure::client_handle::rebuild_client(holder, &config);
    }
    Ok(config)
}

/// Offline toggle: flips the flag and clears the in-memory page cache so
/// the next view serves from the newly chosen source, not a stale copy.
pub fn set_offline(holder: &ClientHolder, previous: &AppConfig, on: bool) -> AppConfig {
    let mut config = previous.clone();
    if config.offline_mode != on {
        config.offline_mode = on;
        holder.current().invalidate(None);
    }
    config
}

/// Conf-data source for the configured mosque, honoring offline mode:
/// online = network (snapshot fallback on failure, client-managed); offline
/// = the disk snapshot only, never the network.
pub async fn conf_for_view(
    holder: &ClientHolder,
    config: &AppConfig,
) -> Result<(std::sync::Arc<mawaqit_api::ConfData>, Option<chrono::NaiveDate>), String> {
    let client = holder.current();
    if config.offline_mode {
        let (conf, date) =
            offline_conf_from(&cache_dir(), &config.mosque_slug)?;
        Ok((std::sync::Arc::new(conf), Some(date)))
    } else {
        client
            .conf_data_dated(&config.mosque_slug)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Snapshot-only load: sync, disk-only, testable without Tauri.
fn offline_conf_from(
    dir: &Path,
    slug: &str,
) -> Result<(mawaqit_api::ConfData, chrono::NaiveDate), String> {
    mawaqit_api::disk::load(dir, slug)
        .map(|(date, conf)| (conf, date))
        .ok_or_else(|| {
            "Offline mode is on, but there is no saved data for this mosque yet — go online once to fetch it.".to_string()
        })
}

/// Today's times + mosque metadata (the `get_today` command body).
pub async fn today_payload(
    holder: &ClientHolder,
    config: &AppConfig,
) -> Result<Option<TodayPayload>, String> {
    if !config.has_mosque() {
        return Ok(None);
    }
    let (conf, as_of) = conf_for_view(holder, config).await?;
    let times = mawaqit_api::times_for_date(&conf, chrono::Local::now().date_naive())
        .map_err(|e| e.to_string())?;
    Ok(Some(TodayPayload::from_conf(&conf, times, as_of)))
}

/// Adhan times for a month (the `get_month` command body).
pub async fn month_times_for(
    holder: &ClientHolder,
    config: &AppConfig,
    month: u32,
) -> Result<MonthTimes, String> {
    if !config.has_mosque() {
        return Err("No mosque configured yet.".into());
    }
    let (conf, _) = conf_for_view(holder, config).await?;
    mawaqit_api::month_times(&conf, month).map_err(|e| e.to_string())
}

/// Resolved iqama times for a month (the `get_month_iqama` command body).
pub async fn month_iqama_times_for(
    holder: &ClientHolder,
    config: &AppConfig,
    month: u32,
) -> Result<MonthIqamaTimes, String> {
    if !config.has_mosque() {
        return Err("No mosque configured yet.".into());
    }
    let (conf, _) = conf_for_view(holder, config).await?;
    mawaqit_api::month_iqama_times(&conf, month).map_err(|e| e.to_string())
}

/// The audio source for a preview: a cached catalog voice beats a custom
/// file path; an uncached voice falls back to the custom path or the
/// built-in athan. Pure, no download (previews must be instant).
pub fn resolve_preview_source(
    sound: Option<String>,
    voice_id: Option<String>,
) -> AthanSource {
    let voice_file = voice_id
        .as_deref()
        .map(|id| voices_dir().join(format!("{id}.mp3")))
        .filter(|p| p.is_file());
    match voice_file {
        Some(path) => AthanSource::File(path),
        None => match sound.as_deref() {
            Some(path) if !path.trim().is_empty() => {
                AthanSource::File(PathBuf::from(path))
            }
            _ => AthanSource::Builtin,
        },
    }
}

/// Whether a catalog voice is already downloaded (drives the sheet's
/// "needs download" marker).
pub fn voice_is_cached(voice_id: String) -> bool {
    voices_dir().join(format!("{voice_id}.mp3")).is_file()
}
