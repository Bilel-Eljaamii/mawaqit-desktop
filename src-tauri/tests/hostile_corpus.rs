//! In-tree mutation fuzzer for the desktop layer: config files, the alarm
//! engine, and the IPC boundary. Same method as
//! `mawaqit-api/tests/hostile_fuzz.rs` — known-valid seeds, deterministic
//! xorshift mutations, hostile-data invariants:
//!
//! - config files (attacker-writable) always load to a sane `AppConfig`;
//! - hostile time strings can never make the alarm engine fire wrongly;
//! - hostile `confData` that parses must survive IPC serialization — the
//!   Today view gets its entire payload through one `serde_json` pass, and
//!   a serialization failure there means a broken Today view.
//!
//! `cargo test -p mawaqit_desktop --test hostile_corpus` (offline, fast).

use std::path::PathBuf;

use chrono::Local;
use mawaqit_api::parse_page;
use mawaqit_desktop_lib::{
    application::prayer_logic::{is_due, next_prayer},
    domain::models::{AlertsConfig, TodayPayload},
    infrastructure::audio::effective_volume,
    infrastructure::config::load_config_from,
};

// ------------------------------------------------------------- mutation engine

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

const INTERESTING: &[u8] = &[
    b'"', b'\\', b'{', b'}', b'[', b']', b':', b',', b'+', b'-', b'.', b'/', b'?', b'#',
    b'0', b'9', b' ', b'\n', b'\t', 0x00, 0x7F, 0x80, 0xC3, 0xE2, 0xFF, b'e', b't',
];

fn mutate(rng: &mut Rng, data: &[u8]) -> Vec<u8> {
    let mut v = data.to_vec();
    match rng.below(4) {
        0 => {
            let cut = rng.below(v.len() + 1);
            v.truncate(cut);
        }
        1 => {
            let flips = rng.below(8) + 1;
            for _ in 0..flips {
                if v.is_empty() {
                    break;
                }
                let i = rng.below(v.len());
                v[i] = (rng.next() % 256) as u8;
            }
        }
        2 => {
            let at = rng.below(v.len() + 1);
            let n = rng.below(16) + 1;
            for k in 0..n {
                let b = INTERESTING[rng.below(INTERESTING.len())];
                v.insert((at + k).min(v.len()), b);
            }
        }
        _ => {
            if !v.is_empty() {
                let a = rng.below(v.len());
                let b = (a + rng.below(v.len() - a + 1)).min(v.len());
                let slice = v[a..b].to_vec();
                let at = rng.below(v.len() + 1);
                for (k, byte) in slice.iter().enumerate() {
                    v.insert((at + k).min(v.len()), *byte);
                }
            }
        }
    }
    v
}

fn for_each_mutation(seeds: &[&[u8]], iters_per_seed: usize, mut f: impl FnMut(&[u8])) {
    let mut rng = Rng::new(0x9E_37_79_B9_7F_4A_7C_15);
    for seed in seeds {
        for _ in 0..iters_per_seed {
            f(&mutate(&mut rng, seed));
        }
    }
}

fn temp_file(name: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    std::env::temp_dir().join(format!(
        "mawaqit-corpus-{name}-{}-{}.json",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ))
}

// ---------------------------------------------------------------------- seeds

/// A modern config with every feature in use: slug, per-prayer alerts with
/// custom sound, volumes, notify-before, shuruq reminder.
const CONFIG_SEED: &[u8] = r#"{
    "mosque_slug": "grande-mosquee-de-paris",
    "mosque_name": "Grande Mosquée de Paris",
    "sound_enabled": true,
    "iqama_alerts": true,
    "autostart": false,
    "alerts": {
        "fajr":  { "mode": "adhan",  "sound": null,          "volume": 80,  "notify_before_min": 10 },
        "dhuhr": { "mode": "silent", "sound": "/tmp/a.mp3",  "volume": null, "notify_before_min": null },
        "asr":   { "mode": "default","sound": null,          "volume": 42,  "notify_before_min": 5 },
        "maghrib": { "mode": "adhan", "sound": "C:\\music\\athan.flac", "volume": 255, "notify_before_min": 0 },
        "isha":  { "mode": "adhan",  "sound": "",            "volume": -3,  "notify_before_min": 999 },
        "shuruq_notify_before_min": 15
    }
}"#.as_bytes();

/// Alarm entries as the background loop builds them: 5 adhan + 5 iqama.
const ENTRIES_SEED: &[u8] =
    br#"[["Fajr","06:30"],["Fajr iqama","06:45"],["Dhuhr","13:00"],["Dhuhr iqama","13:15"],
         ["Asr","15:30"],["Asr iqama","15:50"],["Maghrib","17:45"],["Maghrib iqama","18:05"],
         ["Isha","19:15"],["Isha iqama","19:30"]]"#;

/// A real mosque page (normal layout, iqama offsets, display metadata).
const PAGE_SEED: &[u8] = r#"<html><script>var confData = {
    "times": ["06:30", "08:00", "13:00", "15:30", "17:45"],
    "shuruq": "08:00",
    "calendar": [{"1": ["06:30","08:00","13:00","15:30","17:45","19:15"],
                  "15": ["06:20","07:50","12:50","15:10","17:30","19:00"]}],
    "iqamaCalendar": [{"1": ["+10","+15","+15","+20","+10"]}],
    "name": "Seed Mosque",
    "jumua": "13:50",
    "jumua2": "14:30",
    "image": "https://pics.test/seed.jpg"
};</script></html>"#
    .as_bytes();

// --------------------------------------------------------------------- tests

#[test]
fn fuzz_config_files_load_to_sane_state() {
    for_each_mutation(&[CONFIG_SEED], 2000, |bytes| {
        let path = temp_file("cfg");
        std::fs::write(&path, bytes).expect("write config");
        let cfg = load_config_from(&path);
        let _ = std::fs::remove_file(&path);

        // Whatever loaded must be internally sane: the accessor surface the
        // app uses must be total, and the config must re-serialize (the
        // save path runs on every settings save).
        for i in 0..8 {
            let _ = cfg.alerts.prayer(i).mode;
        }
        for name in ["fajr", "dhuhr", "asr", "maghrib", "isha", "", "\u{0000}", "FAJR"] {
            let _ = cfg.alerts.prayer_alert(name);
        }
        let _ = cfg.alerts.any_adhan();
        assert!(
            serde_json::to_string(&cfg).is_ok(),
            "a loaded config must always re-serialize"
        );
        assert_eq!(cfg.has_mosque(), !cfg.mosque_slug.is_empty());
    });
}

#[test]
fn fuzz_alert_entries_never_break_the_alarm_engine() {
    let now = chrono::Local::now().time();
    for_each_mutation(&[ENTRIES_SEED], 2000, |bytes| {
        let Ok(entries) = serde_json::from_slice::<Vec<(String, String)>>(bytes) else {
            return;
        };
        if let Some(next) = next_prayer(&entries) {
            // The countdown the tray shows is never negative — a hostile
            // entry list must not make time run backwards.
            assert!(
                next.minutes_remaining >= 0,
                "negative countdown {next:?} from entries {entries:?}"
            );
            let _ = is_due(now, &next.time.to_string());
        }
        for (name, hhmm) in &entries {
            let _ = is_due(now, hhmm);
            let _ = name.len();
        }
    });
}

#[test]
fn fuzz_today_payload_ipc_serialization_never_fails() {
    let today = Local::now().date_naive();
    for_each_mutation(&[PAGE_SEED], 2000, |bytes| {
        let html = String::from_utf8_lossy(bytes);
        let Ok(conf) = parse_page(&html, "corpus") else { return };
        let Ok(times) = mawaqit_api::times_for_date(&conf, today) else { return };

        // The entire Today view crosses IPC as one JSON payload. Whatever
        // parsed from a hostile page must serialize — a failure here means
        // a mosque that breaks the app's main screen.
        let payload = TodayPayload::from_conf(&conf, times, Some(today));
        let json = serde_json::to_string(&payload)
            .expect("hostile-but-parsed confData must survive IPC serialization");
        assert!(json.contains("mosque_name"), "payload must carry its fields");
    });
}

#[test]
fn fuzz_alerts_struct_and_volume_clamping() {
    const ALERTS_SEED: &[u8] = br#"{"fajr":{"mode":"adhan","sound":null,"volume":80,"notify_before_min":10},"isha":{"volume":300},"shuruq_notify_before_min":null}"#;
    for_each_mutation(&[ALERTS_SEED], 2000, |bytes| {
        if let Ok(alerts) = serde_json::from_slice::<AlertsConfig>(bytes) {
            for i in 0..8 {
                let _ = alerts.prayer(i);
            }
            let _ = alerts.any_adhan();
        }
    });

    // effective_volume takes the raw config byte: every possible value must
    // land in [0, 1], never amplify past full volume.
    for p in 0..=u8::MAX {
        let v = effective_volume(Some(p));
        assert!((0.0..=1.0).contains(&v), "volume {p} -> {v} out of range");
    }
    assert_eq!(effective_volume(None), 1.0);
}
