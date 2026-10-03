//! Disk snapshot of a mosque's `ConfData` — the offline layer.
//!
//! One fetched mosque page carries the whole year (adhan calendar, iqama
//! calendar, metadata), so a single snapshot file per mosque serves the
//! Today view, the tray alarms and the month view with no network. The
//! snapshot directory lives next to the app config and is exactly as
//! attacker-writable as the config file: loading must be total — missing,
//! truncated or hostile files yield `None`, never a panic.

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

use chrono::{Local, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::models::ConfData;

/// Bump when the envelope layout changes; older versions are ignored.
const SNAPSHOT_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct SnapshotEnvelope {
    version: u32,
    /// The slug this snapshot belongs to — verified on load so a file can
    /// never be served for a different mosque.
    mosque_slug: String,
    fetched_at: NaiveDate,
    /// The original confData object (`ConfData::raw`). Storing the raw
    /// object — not the struct — avoids the duplicate-key corruption
    /// `#[serde(flatten)]` produces on serialize (calendar/times/name would
    /// appear twice, and a flattened struct refuses to parse that back).
    conf: serde_json::Value,
}

/// The JSON stored in the envelope: the tolerant struct's serialization,
/// overlaid with the unmodeled extras from `raw`. Storing the struct (not
/// the raw object) is what makes messy mosques work (FINDING F10): the
/// scraper tolerates wire shapes strict serde rejects (nulls inside iqama
/// rows, numeric names) — serializing raw verbatim would write those shapes
/// back and the snapshot could never load. Modeled keys always win over raw
/// keys, so hostile shapes inside them can never reach the file.
fn conf_to_storage(conf: &ConfData) -> Option<serde_json::Value> {
    let mut stripped = conf.clone();
    stripped.raw = serde_json::Value::Null;
    let mut value = serde_json::to_value(&stripped).ok()?;
    if let (serde_json::Value::Object(base), serde_json::Value::Object(extras)) =
        (&mut value, &conf.raw)
    {
        for (key, extra) in extras {
            base.entry(key.clone()).or_insert_with(|| extra.clone());
        }
    }
    Some(value)
}

/// The snapshot file for `slug` inside `dir`. The name is derived from a
/// hash of the slug, so a hostile slug (`../`, `?`, `#`, 4 MB of Unicode)
/// can never escape the directory or forge another mosque's file; the real
/// slug travels inside the envelope.
pub fn snapshot_path(dir: &Path, slug: &str) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    slug.hash(&mut hasher);
    dir.join(format!("{:016x}.json", hasher.finish()))
}

/// Load the snapshot for `slug`, or `None` when absent, stale-schema, or
/// hostile (any parse failure is "no snapshot" — same contract as the app
/// config).
pub fn load(dir: &Path, slug: &str) -> Option<(NaiveDate, ConfData)> {
    let content = std::fs::read_to_string(snapshot_path(dir, slug)).ok()?;
    let envelope: SnapshotEnvelope = serde_json::from_str(&content).ok()?;
    if envelope.version != SNAPSHOT_VERSION || envelope.mosque_slug != slug {
        return None;
    }
    let conf = serde_json::from_value(envelope.conf).ok()?;
    Some((envelope.fetched_at, conf))
}

/// Atomically store a snapshot for `slug` (write to a temp file, then
/// rename). IO and serialization failures are swallowed: the snapshot is an
/// optimization, a failed write must never break an online fetch. Returns
/// the recorded fetch date on success.
pub fn store(dir: &Path, slug: &str, conf: &ConfData) -> Option<NaiveDate> {
    let fetched_at = Local::now().date_naive();
    let envelope = SnapshotEnvelope {
        version: SNAPSHOT_VERSION,
        mosque_slug: slug.to_string(),
        fetched_at,
        conf: conf_to_storage(conf)?,
    };
    let json = serde_json::to_string(&envelope).ok()?;
    let path = snapshot_path(dir, slug);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).ok()?;
    std::fs::rename(&tmp, &path).ok()?;
    Some(fetched_at)
}

#[cfg(test)]
mod tests {
    //! Hostile snapshot-file tests: the cache directory is attacker-writable
    //! (same user), so a hostile file must degrade to "no snapshot" and a
    //! hostile slug must never escape the directory.

    use super::*;

    fn sample_conf() -> ConfData {
        ConfData {
            times: vec![
                "05:00".into(),
                "06:30".into(),
                "12:00".into(),
                "15:30".into(),
                "18:00".into(),
                "19:30".into(),
            ],
            ..Default::default()
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mawaqit-snapshot-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn store_then_load_roundtrips() {
        let dir = temp_dir("roundtrip");
        let stored =
            store(&dir, "grande-mosquee-de-paris", &sample_conf()).expect("store");
        let (fetched_at, conf) = load(&dir, "grande-mosquee-de-paris").expect("load");
        assert_eq!(fetched_at, stored);
        assert_eq!(conf.times.len(), 6);
        assert_eq!(conf.times[0], "05:00");
    }

    #[test]
    fn roundtrip_survives_a_populated_raw_object() {
        // The scraped ConfData carries the whole original confData object in
        // `raw`; a naive struct serialization duplicates calendar/times/name
        // and the file can never be read back. This is a regression guard.
        let dir = temp_dir("raw-roundtrip");
        let mut conf = sample_conf();
        conf.raw = serde_json::json!({
            "times": ["05:00", "06:30", "12:00", "15:30", "18:00", "19:30"],
            "calendar": [{"1": ["05:00","06:30","12:00","15:30","18:00","19:30"]}],
            "name": "Snapshot Test Mosque",
            "something_unmodeled": {"deep": true}
        });
        conf.name = Some("Snapshot Test Mosque".into());

        store(&dir, "some-mosque", &conf).expect("store");
        let (_, loaded) = load(&dir, "some-mosque").expect("load");
        assert_eq!(loaded.times.len(), 6);
        assert_eq!(loaded.name.as_deref(), Some("Snapshot Test Mosque"));
        assert_eq!(
            loaded.raw.get("something_unmodeled").and_then(|v| v.get("deep")),
            Some(&serde_json::Value::Bool(true)),
            "unmodeled extras must survive the roundtrip"
        );
    }

    #[test]
    fn missing_file_is_no_snapshot() {
        let dir = temp_dir("missing");
        assert!(load(&dir, "never-stored").is_none());
    }

    #[test]
    fn hostile_file_contents_degrade_to_none() {
        let dir = temp_dir("hostile");
        for content in [
            "",
            "not json",
            "42",
            "[1, 2, 3]",
            r#"{"version":1,"mosque_slug":"x"}"#, // truncated envelope
            r#"{"version":1,"mosque_slug":"x","fetched_at":"nope","conf":{}}"#, // bad date
            r#"{"version":99,"mosque_slug":"x","fetched_at":"2026-10-01","conf":{}}"#, // future version
        ] {
            std::fs::write(snapshot_path(&dir, "some-mosque"), content).unwrap();
            assert!(load(&dir, "some-mosque").is_none(), "{content:?} must not load");
        }
    }

    #[test]
    fn messy_wire_shapes_in_raw_never_break_loading() {
        // F10: the scraper tolerates wire shapes strict serde rejects (nulls
        // inside iqama rows, numeric names) and collapses them — the stored
        // file must carry the collapsed values, never the hostile raw ones.
        let dir = temp_dir("f10");
        let mut conf = sample_conf();
        conf.raw = serde_json::json!({
            "times": ["06:30", "08:00", "13:00", "15:30", "17:45"],
            "calendar": [{"1": ["06:30","08:00","13:00","15:30","17:45","19:15"]}],
            "iqamaCalendar": [{"1": ["+10", null, "13:20", "+20", "18:00"]}],
            "name": 12345
        });
        conf.name = Some("Messy Mosque".into());

        store(&dir, "messy-mosque", &conf).expect("store");
        let (_, loaded) = load(&dir, "messy-mosque").expect("messy snapshot reloads");
        // The struct's tolerant values win over the hostile raw shapes.
        assert_eq!(loaded.times.len(), 6);
        assert_eq!(loaded.name.as_deref(), Some("Messy Mosque"));
        assert!(loaded.iqama_calendar.is_none(), "scraper dropped the broken iqama calendar");
    }

    #[test]
    fn snapshot_never_serves_a_different_mosque() {
        let dir = temp_dir("slug-mismatch");
        store(&dir, "mosque-a", &sample_conf());
        assert!(load(&dir, "mosque-b").is_none(), "slug mismatch must not load");
    }

    #[test]
    fn hostile_slugs_stay_inside_the_directory() {
        let dir = temp_dir("traversal");
        for slug in [
            "../../etc/passwd",
            "..\\..\\windows\\win.ini",
            "a/b/c",
            "",
            "\u{0000}\u{202E}",
            &"x".repeat(100_000),
        ] {
            let path = snapshot_path(&dir, slug);
            assert!(
                path.parent() == Some(dir.as_path()),
                "slug {slug:?} escaped the directory: {path:?}"
            );
            assert_eq!(path.extension().and_then(|e| e.to_str()), Some("json"));
        }
    }

    #[test]
    fn atomic_write_leaves_no_tmp_behind() {
        let dir = temp_dir("atomic");
        store(&dir, "some-mosque", &sample_conf());
        let entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(entries.len(), 1, "only the snapshot file: {entries:?}");
        assert!(entries[0].ends_with(".json") && !entries[0].ends_with(".tmp"));
    }
}
