use std::{fs, path::PathBuf};

use crate::domain::models::AppConfig;

pub fn get_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mawaqit-desktop")
        .join("mawaqit-config.json")
}

pub fn load_config() -> AppConfig {
    load_config_from(&get_config_path())
}

/// `load_config` at an arbitrary path — the seam the hostile config-file
/// tests use (they must never touch the real user config).
pub fn load_config_from(path: &std::path::Path) -> AppConfig {
    if let Ok(content) = fs::read_to_string(path) {
        if let Ok(config) = serde_json::from_str(&content) {
            return config;
        }
    }
    AppConfig::default()
}

pub fn save_config(config: &AppConfig) {
    let path = get_config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(content) = serde_json::to_string_pretty(config) {
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

    /// FINDING F9 — `sound_enabled` is the only AppConfig field without a
    /// `#[serde(default)]`, so a config file that carries the user's mosque
    /// but is missing (or corrupted in) that single bool fails to parse
    /// ENTIRELY and silently resets the app to "no mosque". Any partial
    /// write/truncation/tamper that drops one field wipes the whole
    /// configuration. All other fields already default; make sound_enabled
    /// `#[serde(default = "default_true")]` too, then un-ignore.
    #[test]
    #[ignore = "RED TEAM FINDING F9: one missing field discards the whole config"]
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
}
