use std::{fs, path::PathBuf};

use crate::domain::models::AppConfig;

pub fn get_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mawaqit-desktop")
        .join("mawaqit-config.json")
}

/// Offline prayer-time snapshot directory (one file per mosque slug, see
/// `mawaqit_api::disk`).
pub fn cache_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mawaqit-desktop")
        .join("times-cache")
}


/// Downloaded adhan-voice files (one mp3 per catalog voice id, see
/// `mawaqit_api::voices`).
pub fn voices_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mawaqit-desktop")
        .join("voices")
}

pub fn load_config() -> AppConfig {
    load_config_from(&get_config_path())
}

/// `load_config` at an arbitrary path — the seam the hostile config-file
/// tests use (they must never touch the real user config).
pub fn load_config_from(path: &std::path::Path) -> AppConfig {
    let Ok(content) = fs::read_to_string(path) else {
        return AppConfig::default();
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&content) else {
        return AppConfig::default();
    };
    // Upgrade migration: prayers the (possibly absent) alerts block does not
    // mention inherit the legacy global sound switch, so a config written
    // before per-prayer settings existed — or a partially-written one —
    // changes nothing observable for the user.
    let sound_switch = legacy_sound_switch(&value);
    seed_missing_prayer_alerts(&mut value, sound_switch);
    let Ok(config) = serde_json::from_value::<AppConfig>(value) else {
        return AppConfig::default();
    };
    config
}

/// The legacy `sound_enabled` field, read raw (defaulted true) before the
/// typed parse, so prayer seeding can use it.
fn legacy_sound_switch(value: &serde_json::Value) -> bool {
    value.get("sound_enabled").and_then(|v| v.as_bool()).unwrap_or(true)
}

/// Fill in the five per-prayer entries that a valid-or-absent alerts block
/// does not mention. A wrong-typed alerts block is left alone: the typed
/// parse then fails and the whole config falls back to defaults (the pinned
/// wrong-type rule — the mosque goes, not the sanity).
fn seed_missing_prayer_alerts(value: &mut serde_json::Value, sound_enabled: bool) {
    let mode = if sound_enabled { "adhan" } else { "silent" };
    let seeded = || serde_json::json!({ "mode": mode });
    match value.get_mut("alerts") {
        None => {
            if let Some(root) = value.as_object_mut() {
                let mut block = serde_json::Map::new();
                for name in ["fajr", "dhuhr", "asr", "maghrib", "isha"] {
                    block.insert(name.to_string(), seeded());
                }
                root.insert("alerts".to_string(), serde_json::Value::Object(block));
            }
        }
        Some(serde_json::Value::Object(block)) => {
            for name in ["fajr", "dhuhr", "asr", "maghrib", "isha"] {
                block.entry(name.to_string()).or_insert_with(seeded);
            }
        }
        Some(_) => {}
    }
}

/// Bound for the announcement read-list: a hostile or huge config must not
/// balloon memory, and the newest marks are the ones worth keeping.
pub const MAX_READ_ANNOUNCEMENTS: usize = 500;

/// Keep at most the newest [`MAX_READ_ANNOUNCEMENTS`] read marks.
pub fn clamp_announcements_read(mut read: Vec<String>) -> Vec<String> {
    if read.len() > MAX_READ_ANNOUNCEMENTS {
        let drop = read.len() - MAX_READ_ANNOUNCEMENTS;
        read.drain(..drop);
    }
    read
}

pub fn save_config(config: &AppConfig) {
    let path = get_config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut trimmed = config.clone();
    trimmed.announcements_read =
        clamp_announcements_read(std::mem::take(&mut trimmed.announcements_read));
    if let Ok(content) = serde_json::to_string_pretty(&trimmed) {
        let _ = fs::write(path, content);
    }
}

#[cfg(test)]
mod tests {
    //! Hostile config-file tests: the config JSON is attacker-writable by a
    //! local process (same user). Loading must always yield a sane
    //! AppConfig — never panic, never half-initialized state.

    use std::{
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().subsec_nanos();
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("mawaqit-redteam-{name}-{nanos}-{id}.json"))
    }

    fn write_temp(name: &str, content: &str) -> PathBuf {
        let path = temp_file(name);
        fs::write(&path, content).expect("write temp config");
        path
    }

    fn cleanup(path: &PathBuf) {
        let _ = fs::remove_file(path);
    }

    #[test]
    fn missing_empty_and_garbage_files_fall_back_to_defaults() {
        // Absent file.
        assert_eq!(load_config_from(&temp_file("absent")), AppConfig::default());

        for content in ["", "   \n\t ", "not json", "[1,2,3]", "\"str\"", "12345", "null"]
        {
            let path = write_temp("garbage", content);
            let cfg = load_config_from(&path);
            cleanup(&path);
            assert_eq!(
                cfg,
                AppConfig::default(),
                "content {content:?} must yield defaults"
            );
        }
    }

    #[test]
    fn offline_mode_defaults_to_off_and_roundtrips() {
        // Absent field (every pre-toggle config) → offline mode off.
        let path = write_temp(
            "no-offline",
            r#"{"mosque_slug":"grande-mosquee-de-paris","sound_enabled":true}"#,
        );
        let cfg = load_config_from(&path);
        cleanup(&path);
        assert!(!cfg.offline_mode, "absent offline_mode must default to false");

        // Persisted state round-trips.
        let path = write_temp(
            "offline-on",
            r#"{"mosque_slug":"grande-mosquee-de-paris","sound_enabled":true,"offline_mode":true}"#,
        );
        let cfg = load_config_from(&path);
        cleanup(&path);
        assert!(cfg.offline_mode, "offline_mode true must survive the roundtrip");
        assert_eq!(cfg.mosque_slug, "grande-mosquee-de-paris");
    }

    #[test]
    fn binary_junk_falls_back_to_defaults() {
        let path = temp_file("binary");
        fs::write(&path, [0xFFu8, 0xFE, 0x00, 0x01, 0x02]).unwrap();
        let cfg = load_config_from(&path);
        cleanup(&path);
        assert_eq!(cfg, AppConfig::default());
    }

    #[test]
    fn wrong_typed_fields_fall_back_to_defaults() {
        let cases = [
            r#"{"mosque_slug":"x","sound_enabled":"yes"}"#, // string bool
            r#"{"mosque_slug":"x","iqama_alerts":1}"#,      // int bool
            r#"{"mosque_slug":42}"#,                        // int slug
            r#"{"mosque_slug":null}"#,                      /* null slug (no default?
                                                             * slug has default ->
                                                             * null fails) */
            r#"{"mosque_slug":{"deep":true}}"#, // object slug
            r#"{"sound_enabled":true,"autostart":"always"}"#, // string bool 2
        ];
        for content in cases {
            let path = write_temp("wrong-type", content);
            let cfg = load_config_from(&path);
            cleanup(&path);
            assert_eq!(
                cfg,
                AppConfig::default(),
                "content {content:?} must yield defaults"
            );
        }
    }

    #[test]
    fn hostile_slug_values_load_verbatim_validation_is_missing() {
        // FINDING F2 (config side): any string is accepted as mosque_slug.
        // Until slug validation exists at the boundary, loading must at
        // least be panic-free and lossless — pinned here.
        let hostile = [
            r#"{"mosque_slug":"../../etc/passwd","sound_enabled":true}"#,
            r#"{"mosque_slug":"x?y=1#z","sound_enabled":true}"#,
        ];
        for content in hostile {
            let path = write_temp("slug", content);
            let cfg = load_config_from(&path);
            cleanup(&path);
            assert!(cfg.has_mosque(), "hostile slug loads verbatim: {content:?}");
        }

        // A 4 MB slug loads without panicking (memory is bounded by the file
        // the attacker already controls).
        let big = format!(
            r#"{{"mosque_slug":"{}","sound_enabled":true}}"#,
            "a".repeat(4 * 1024 * 1024)
        );
        let path = write_temp("big-slug", &big);
        let cfg = load_config_from(&path);
        cleanup(&path);
        assert_eq!(cfg.mosque_slug.len(), 4 * 1024 * 1024);
    }

    #[test]
    fn unknown_fields_and_legacy_formats_load_cleanly() {
        // Unknown fields must stay ignored (forward compat).
        let path = write_temp(
            "unknown",
            r#"{"mosque_slug":"paris","sound_enabled":true,"some_future_field":true}"#,
        );
        let cfg = load_config_from(&path);
        cleanup(&path);
        assert_eq!(cfg.mosque_slug, "paris");

        // Legacy eras (masjid_id, credentials) load as unconfigured.
        for legacy in [
            r#"{"masjid_id":"abc","email":"a@b.c","password":"x"}"#,
            r#"{"masjid_id":"abc"}"#,
        ] {
            let path = write_temp("legacy", legacy);
            let cfg = load_config_from(&path);
            cleanup(&path);
            assert!(!cfg.has_mosque(), "legacy config {legacy:?} is unconfigured");
            assert_eq!(cfg, AppConfig::default());
        }
    }

    #[test]
    fn nul_and_unicode_slugs_do_not_break_loading() {
        let path = write_temp(
            "nul",
            r#"{"mosque_slug":"a\u0000b\u202ec","sound_enabled":true}"#,
        );
        let cfg = load_config_from(&path);
        cleanup(&path);
        assert_eq!(cfg.mosque_slug, "a\u{0000}b\u{202E}c");
    }

    /// FIXED (was FINDING F9) — `sound_enabled` now carries a
    /// `#[serde(default)]` like every other field, so a config that carries
    /// the user's mosque but is missing (or corrupted in) that single bool
    /// no longer resets the app to "no mosque". Regression guard.
    #[test]
    fn finding_f9_partial_config_preserves_the_mosque() {
        let path = write_temp(
            "partial",
            r#"{"mosque_slug":"grande-mosquee-de-paris","mosque_name":"Grande Mosquée"}"#,
        );
        let cfg = load_config_from(&path);
        cleanup(&path);
        assert_eq!(
            cfg.mosque_slug, "grande-mosquee-de-paris",
            "a missing optional bool must not wipe the mosque selection"
        );
    }

    #[test]
    fn legacy_config_without_alerts_seeds_from_the_sound_switch() {
        // sound_enabled=true (defaulted) -> every prayer plays the adhan.
        let path =
            write_temp("legacy-on", r#"{"mosque_slug":"grande-mosquee-de-paris"}"#);
        let cfg = load_config_from(&path);
        cleanup(&path);
        for name in ["fajr", "dhuhr", "asr", "maghrib", "isha"] {
            assert_eq!(
                cfg.alerts.prayer_alert(name).unwrap().mode,
                crate::domain::models::AthanMode::Adhan,
                "{name} must inherit the legacy sound_enabled=true"
            );
        }

        // sound_enabled=false -> every prayer is muted.
        let path = write_temp(
            "legacy-off",
            r#"{"mosque_slug":"grande-mosquee-de-paris","sound_enabled":false}"#,
        );
        let cfg = load_config_from(&path);
        cleanup(&path);
        for name in ["fajr", "dhuhr", "asr", "maghrib", "isha"] {
            assert_eq!(
                cfg.alerts.prayer_alert(name).unwrap().mode,
                crate::domain::models::AthanMode::Silent,
                "{name} must inherit the legacy sound_enabled=false"
            );
        }
    }

    #[test]
    fn config_with_alerts_key_keeps_its_explicit_settings() {
        let path = write_temp(
            "explicit",
            r#"{"mosque_slug":"paris","sound_enabled":false,
                "alerts":{"isha":{"mode":"default"}}}"#,
        );
        let cfg = load_config_from(&path);
        cleanup(&path);
        // The explicit isha setting wins even though sound_enabled is false;
        // the other prayers still seed from the legacy switch.
        assert_eq!(cfg.alerts.isha.mode, crate::domain::models::AthanMode::Default);
        assert_eq!(cfg.alerts.fajr.mode, crate::domain::models::AthanMode::Silent);
    }

    #[test]
    fn wrong_typed_alerts_block_falls_back_to_defaults() {
        let cases = [
            r#"{"mosque_slug":"x","alerts":42}"#, // int block
            r#"{"mosque_slug":"x","alerts":"silent"}"#, // string block
            r#"{"mosque_slug":"x","alerts":{"fajr":{"mode":"LOUD"}}}"#, // bad enum string
            r#"{"mosque_slug":"x","alerts":{"fajr":{"mode":3}}}"#, // int mode
            r#"{"mosque_slug":"x","alerts":{"asr":{"volume":9999}}}"#, // out-of-u8 volume
            r#"{"mosque_slug":"x","alerts":{"isha":{"notify_before_min":"5"}}}"#, // string minutes
        ];
        for content in cases {
            let path = write_temp("bad-alerts", content);
            let cfg = load_config_from(&path);
            cleanup(&path);
            assert_eq!(
                cfg,
                AppConfig::default(),
                "content {content:?} must yield defaults"
            );
        }
    }

    #[test]
    fn removed_alerts_fields_are_ignored_not_fatal() {
        // `live_timer` existed in the brief v0.2.0 era and was removed; old
        // config files still carry it. Unknown fields must stay ignored
        // (forward compat) — the mosque survives and the field is dropped.
        let path = write_temp(
            "legacy-live-timer",
            r#"{"mosque_slug":"paris","alerts":{"fajr":{
                "mode":"default","notify_before_min":5,"live_timer":true
            }}}"#,
        );
        let cfg = load_config_from(&path);
        cleanup(&path);
        assert_eq!(cfg.mosque_slug, "paris");
        assert_eq!(cfg.alerts.fajr.notify_before_min, Some(5));
        let expected = crate::domain::models::PrayerAlerts {
            mode: crate::domain::models::AthanMode::Default,
            notify_before_min: Some(5),
            ..Default::default()
        };
        assert_eq!(cfg.alerts.fajr, expected);
    }

    #[test]
    fn partial_alerts_block_defaults_the_rest() {
        // A valid but partial alerts block must not wipe anything: missing
        // prayers get their serde defaults, the mosque stays.
        let path = write_temp(
            "partial-alerts",
            r#"{"mosque_slug":"paris","alerts":{"dhuhr":{"volume":40}}}"#,
        );
        let cfg = load_config_from(&path);
        cleanup(&path);
        assert_eq!(cfg.mosque_slug, "paris");
        assert_eq!(cfg.alerts.dhuhr.volume, Some(40));
        assert_eq!(cfg.alerts.fajr, crate::domain::models::PrayerAlerts::default());
    }

    #[test]
    fn wrong_typed_tor_socks_addr_falls_back_to_defaults() {
        for content in [
            r#"{"mosque_slug":"x","tor_socks_addr":42}"#,
            r#"{"mosque_slug":"x","tor_socks_addr":["socks5h://1.2.3.4"]}"#,
            r#"{"mosque_slug":"x","tor_socks_addr":{"scheme":"socks5h"}}"#,
        ] {
            let path = write_temp("bad-tor-addr", content);
            let cfg = load_config_from(&path);
            cleanup(&path);
            assert_eq!(
                cfg,
                AppConfig::default(),
                "content {content:?} must yield defaults"
            );
        }
    }

    #[test]
    fn hostile_tor_address_string_loads_losslessly() {
        // The config layer never editorializes: a hostile address loads
        // verbatim and is rejected/degraded at use time instead. (The raw
        // string carries `\\evil`; JSON unescapes it to a single backslash.)
        let path = write_temp(
            "tor-addr-verbatim",
            r#"{"mosque_slug":"x","tor_socks_addr":"socks5h://..\\evil"}"#,
        );
        let cfg = load_config_from(&path);
        cleanup(&path);
        assert_eq!(
            cfg.tor_socks_addr.as_deref(),
            Some("socks5h://..\\evil")
        );
    }

    #[test]
    fn wrong_typed_announcements_read_falls_back_to_defaults() {
        for content in [
            r#"{"mosque_slug":"x","announcements_read":"yes"}"#,
            r#"{"mosque_slug":"x","announcements_read":42}"#,
            r#"{"mosque_slug":"x","announcements_read":[1,2,3]}"#,
        ] {
            let path = write_temp("bad-read-list", content);
            let cfg = load_config_from(&path);
            cleanup(&path);
            assert_eq!(
                cfg,
                AppConfig::default(),
                "content {content:?} must yield defaults"
            );
        }
    }

    #[test]
    fn read_list_clamp_keeps_the_newest_marks() {
        let read: Vec<String> = (0..600).map(|i| i.to_string()).collect();
        let clamped = clamp_announcements_read(read);
        assert_eq!(clamped.len(), MAX_READ_ANNOUNCEMENTS);
        assert_eq!(clamped.first().unwrap(), "100");
        assert_eq!(clamped.last().unwrap(), "599");
    }

    #[test]
    fn extreme_but_in_range_alerts_values_load_losslessly() {
        // u16::MAX minutes and volume 255 parse fine — clamping to sane
        // behavior happens at use time, the config layer must stay lossless
        // (the mosque must survive a weird-but-typed value).
        let path = write_temp(
            "extreme",
            r#"{"mosque_slug":"paris","alerts":{
                "fajr":{"notify_before_min":65535,"volume":255},
                "asr":{"notify_before_min":0}
            }}"#,
        );
        let cfg = load_config_from(&path);
        cleanup(&path);
        assert_eq!(cfg.alerts.fajr.notify_before_min, Some(65535));
        assert_eq!(cfg.alerts.fajr.volume, Some(255));
        assert_eq!(cfg.alerts.asr.notify_before_min, Some(0));
        assert_eq!(cfg.mosque_slug, "paris");
    }
}
