//! Settings application service: the logic behind the settings IPC
//! commands, extracted from `presentation/commands.rs` so it can be tested
//! without a Tauri harness. Every function takes the `ClientHolder` (for
//! cache invalidation and transport swaps) plus the previous config, and
//! returns the config to persist.

use crate::domain::models::{is_plausible_tor_host, AppConfig};
use crate::infrastructure::client_handle::ClientHolder;
use mawaqit_api;

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
    if !config.mosque_slug.is_empty()
        && !mawaqit_api::is_valid_slug(&config.mosque_slug)
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
    // The legacy global switch stays in the file for downgrades; whatever
    // the caller sent, it must agree with the per-prayer settings that now
    // rule.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::{PrayerAlerts, TorProxy};

    fn config_with(slug: &str) -> AppConfig {
        AppConfig {
            mosque_slug: slug.into(),
            ..AppConfig::default()
        }
    }

    fn tor_on(host: &str, port: u16) -> TorProxy {
        TorProxy { enabled: true, host: host.into(), port }
    }

    #[test]
    fn update_rejects_hostile_slug() {
        let holder = ClientHolder::from_config(&AppConfig::default());
        let previous = config_with("test-mosque");
        let mut config = config_with("../evil");
        config.sound_enabled = false;

        let err = validate_and_apply_update(&holder, &previous, config).unwrap_err();
        assert!(err.contains("invalid mosque id"), "got {err:?}");
    }

    #[test]
    fn update_rejects_disabled_tor_with_implausible_host() {
        let holder = ClientHolder::from_config(&AppConfig::default());
        let previous = config_with("test-mosque");
        let mut config = config_with("test-mosque");
        config.tor.enabled = true;
        config.tor.host = "not a host!".into();

        let err = validate_and_apply_update(&holder, &previous, config).unwrap_err();
        assert!(err.contains("invalid Tor proxy host"), "got {err:?}");
    }

    #[test]
    fn update_rejects_zero_tor_port_when_enabled() {
        let holder = ClientHolder::from_config(&AppConfig::default());
        let previous = config_with("test-mosque");
        let mut config = config_with("test-mosque");
        config.tor.enabled = true;
        config.tor.port = 0;

        let err = validate_and_apply_update(&holder, &previous, config).unwrap_err();
        assert!(err.contains("port 0"), "got {err:?}");
    }

    #[test]
    fn update_rejects_unknown_voice_id() {
        let holder = ClientHolder::from_config(&AppConfig::default());
        let previous = config_with("test-mosque");
        let mut config = config_with("test-mosque");
        config.alerts.fajr.voice = Some("adhan-afassy".into()); // not in catalog

        let err = validate_and_apply_update(&holder, &previous, config).unwrap_err();
        assert!(err.contains("unknown adhan voice"), "got {err:?}");
    }

    #[test]
    fn update_accepts_a_clean_config_and_recomputes_legacy_switch() {
        let holder = ClientHolder::from_config(&AppConfig::default());
        let previous = config_with("test-mosque");
        let mut config = config_with("test-mosque");
        config.sound_enabled = false; // stale legacy value from the caller
        config.alerts.fajr.mode = crate::domain::models::AthanMode::Adhan;

        let validated =
            validate_and_apply_update(&holder, &previous, config).expect("clean config");
        // Recomputed from the per-prayer alerts (any adhan mode ⇒ on).
        assert!(validated.sound_enabled);
    }

    #[test]
    fn tor_toggle_swaps_the_transport() {
        let holder = ClientHolder::from_config(&AppConfig::default());
        let previous = config_with("test-mosque");

        let on = set_tor(&holder, &previous, true).expect("enable");
        assert!(on.tor.enabled);

        let off = set_tor(&holder, &on, false).expect("disable");
        assert!(!off.tor.enabled);
    }

    #[test]
    fn tor_enable_fails_without_a_plausible_host() {
        let holder = ClientHolder::from_config(&AppConfig::default());
        let previous = AppConfig {
            tor: tor_on("not a host!", 9050),
            ..config_with("test-mosque")
        };
        let err = set_tor(&holder, &previous, true).unwrap_err();
        assert!(err.contains("valid proxy host"), "got {err:?}");
    }

    #[test]
    fn tor_toggle_is_idempotent() {
        let holder = ClientHolder::from_config(&AppConfig::default());
        let previous = config_with("test-mosque");

        let once = set_tor(&holder, &previous, true).expect("first");
        let twice = set_tor(&holder, &once, true).expect("second");
        assert!(twice.tor.enabled);
    }

    #[test]
    fn offline_toggle_is_idempotent_and_clears_the_cache() {
        let holder = ClientHolder::from_config(&AppConfig::default());
        let previous = config_with("test-mosque");

        let on = set_offline(&holder, &previous, true);
        assert!(on.offline_mode);
        let off = set_offline(&holder, &on, false);
        assert!(!off.offline_mode);
    }
}
