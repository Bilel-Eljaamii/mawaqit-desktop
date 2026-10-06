//! The alarm decision engine: given the config, today's payload and the
//! clock, decides WHICH alerts fire — without touching notifications, audio
//! or the tray. The tick loop in `lib.rs` executes the returned actions.
//! Pure decision logic, exhaustively unit-tested (issue: coverage gate).

use std::collections::HashSet;

use chrono::NaiveDate;

use crate::application::prayer_logic::{
    adhan_entries, iqama_entries, is_due, minutes_before, MAX_NOTIFY_BEFORE_MIN,
};
use crate::domain::models::{AppConfig, AthanMode, PrayerAlerts, TodayPayload};
use crate::infrastructure::audio::AthanSource;

/// One alert action for the tick executor to perform.
#[derive(Debug, Clone, PartialEq)]
pub enum AlarmAction {
    /// Plain popup (heads-up, iqama, Silent/Default adhan).
    Notify { title: String, body: String },
    /// Popup without sound (Silent adhan mode).
    NotifySilent { title: String, body: String },
    /// Popup whose click stops a sounding athan (Adhan mode), plus the
    /// resolved audio source and volume for playback.
    NotifyStoppable { title: String, body: String },
    /// Start adhan playback with the resolved source.
    Play { source: AthanSource, volume: Option<u8> },
    /// Best-effort background download of a catalog voice (uncached voice
    /// selected: this adhan plays builtin, the next one uses the voice).
    DownloadVoice { id: String },
}

/// Evaluate one minute-tick: which alerts fire for each prayer right now.
/// `alerted` deduplicates per `date|kind|name` key and is advanced by this
/// call. Pure decision logic — the caller executes the actions.
pub fn evaluate_tick(
    config: &AppConfig,
    payload: &TodayPayload,
    now: chrono::NaiveTime,
    date: NaiveDate,
    alerted: &mut HashSet<String>,
    voices_dir: &std::path::Path,
) -> Vec<AlarmAction> {
    let mut actions = Vec::new();

    for (index, (name, time)) in
        adhan_entries(&payload.times.adhan).into_iter().enumerate()
    {
        let alerts = config.alerts.prayer(index);

        // Pre-adhan notification, exactly once per prayer and day. Config
        // minutes are attacker-writable: cap before use, and treat 0 as off
        // (it would coincide with the adhan itself).
        if let Some(before) = alerts
            .notify_before_min
            .map(|n| n.min(MAX_NOTIFY_BEFORE_MIN))
            .filter(|n| *n > 0)
        {
            if let Some(target) = minutes_before(&time, before) {
                if is_due(now, &target)
                    && alerted.insert(format!("{date}|pre|{name}"))
                {
                    actions.push(AlarmAction::Notify {
                        title: "Mawaqit".into(),
                        body: minutes_from_now(&name, before),
                    });
                }
            }
        }

        // At the adhan time.
        if is_due(now, &time) && alerted.insert(format!("{date}|adhan|{name}")) {
            let at_time = format!("It is time for the {name} adhan");
            match alerts.mode {
                AthanMode::Silent => actions.push(AlarmAction::NotifySilent {
                    title: "Mawaqit".into(),
                    body: at_time,
                }),
                AthanMode::Default => actions.push(AlarmAction::Notify {
                    title: "Mawaqit".into(),
                    body: at_time,
                }),
                AthanMode::Adhan => {
                    actions.push(AlarmAction::NotifyStoppable {
                        title: "Mawaqit".into(),
                        body: at_time,
                    });
                    let (source, download) =
                        resolve_athan_source(alerts, voices_dir);
                    actions.push(AlarmAction::Play { source, volume: alerts.volume });
                    if let Some(id) = download {
                        actions.push(AlarmAction::DownloadVoice { id });
                    }
                }
            }
        }
    }

    // Optional iqama alerts (notification only).
    if config.iqama_alerts {
        if let Some(iqama) = &payload.times.iqama {
            for (name, time) in iqama_entries(iqama) {
                if is_due(now, &time)
                    && alerted.insert(format!("{date}|iqama|{name}"))
                {
                    actions.push(AlarmAction::Notify {
                        title: "Mawaqit".into(),
                        body: format!("The {name} iqama has started"),
                    });
                }
            }
        }
    }

    // Pre-shurouq notification (notification only — sunrise has no adhan).
    if let Some(before) = config
        .alerts
        .shuruq_notify_before_min
        .map(|n| n.min(MAX_NOTIFY_BEFORE_MIN))
        .filter(|n| *n > 0)
    {
        if let Some(target) = minutes_before(&payload.times.adhan.shurouq, before) {
            if is_due(now, &target)
                && alerted.insert(format!("{date}|pre|Shurouq"))
            {
                actions.push(AlarmAction::Notify {
                    title: "Mawaqit".into(),
                    body: minutes_from_now("Shurouq", before),
                });
            }
        }
    }

    actions
}

/// "Fajr adhan in 5 minutes" / "…in 1 minute" — singular kept grammatical.
pub fn minutes_from_now(event: &str, n: u16) -> String {
    let unit = if n == 1 { "minute" } else { "minutes" };
    format!("{event} in {n} {unit}")
}

/// The audio source for one adhan, honoring the per-prayer config:
/// catalog voice (if cached; an uncached catalog voice yields the builtin
/// plus a download action for next time) — otherwise the custom file,
/// otherwise the builtin.
fn resolve_athan_source(
    alerts: &PrayerAlerts,
    voices_dir: &std::path::Path,
) -> (AthanSource, Option<String>) {
    if let Some(voice) = &alerts.voice {
        let path = voices_dir.join(format!("{voice}.mp3"));
        if path.is_file() {
            return (AthanSource::File(path.into()), None);
        }
        // Download best-effort in the background so the NEXT adhan uses the
        // voice; this one plays the builtin instead of blocking the alert.
        if mawaqit_api::voices::adhan_voice_url(voice).is_some() {
            return (AthanSource::Builtin, Some(voice.clone()));
        }
    }
    match &alerts.sound {
        Some(path) => (AthanSource::File(path.clone().into()), None),
        None => (AthanSource::Builtin, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::{AlertsConfig, TodayPayload};
use mawaqit_api::{DailyIqamaTimes, DailyPrayerTimes, TodayTimes};
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn sample_payload() -> TodayPayload {
        TodayPayload {
            mosque_name: Some("Test Mosque".into()),
            jumua: None,
            jumua2: None,
            image: None,
            imsak_mode: false,
            times: TodayTimes {
                date: chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap(),
                adhan: DailyPrayerTimes {
                    fajr: "05:30".into(),
                    shurouq: "07:07".into(),
                    dhuhr: "13:21".into(),
                    asr: "16:37".into(),
                    maghrib: "19:24".into(),
                    isha: "21:05".into(),
                },
                iqama: Some(DailyIqamaTimes {
                    fajr: "05:47".into(),
                    dhuhr: "13:35".into(),
                    asr: "16:55".into(),
                    maghrib: "19:40".into(),
                    isha: "21:05".into(),
                }),
                iqama_at: None,
            },
            as_of: None,
            announcements: Vec::new(),
        }
    }

    fn config() -> AppConfig {
        AppConfig::default()
    }

    fn now(h: u32, m: u32) -> chrono::NaiveTime {
        chrono::NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    fn date() -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap()
    }

    fn temp_voices_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "alarm-engine-{tag}-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn silent_mode_notifies_without_audio() {
        let mut cfg = config();
        cfg.alerts.fajr.mode = AthanMode::Silent;
        let mut alerted = HashSet::new();
        let actions = evaluate_tick(&cfg, &sample_payload(), now(5, 30), date(), &mut alerted, &temp_voices_dir("t"));
        let silent: Vec<_> = actions.iter().filter(|a| matches!(a, AlarmAction::NotifySilent { .. })).collect();
        assert_eq!(silent.len(), 1, "got {actions:?}");
        assert!(!actions.iter().any(|a| matches!(a, AlarmAction::Play { .. })));
    }

    #[test]
    fn default_mode_notifies_without_audio_or_download() {
        let mut cfg = config();
        cfg.alerts.fajr.mode = AthanMode::Default;
        let mut alerted = HashSet::new();
        let actions = evaluate_tick(&cfg, &sample_payload(), now(5, 30), date(), &mut alerted, &temp_voices_dir("t"));
        assert!(actions.iter().any(|a| matches!(a, AlarmAction::Notify { .. })));
        assert!(!actions.iter().any(|a| matches!(a, AlarmAction::Play { .. })));
        assert!(!actions.iter().any(|a| matches!(a, AlarmAction::DownloadVoice { .. })));
    }

    #[test]
    fn adhan_mode_plays_and_requests_voice_download_when_uncached() {
        let dir = std::env::temp_dir().join(format!(
            "alarm-engine-uncached-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = config();
        cfg.alerts.fajr.mode = AthanMode::Adhan;
        cfg.alerts.fajr.voice = Some("adhan-quds".into());
        let mut alerted = HashSet::new();
        let actions = evaluate_tick(&cfg, &sample_payload(), now(5, 30), date(), &mut alerted, &temp_voices_dir("t"));
        let play = actions.iter().find(|a| matches!(a, AlarmAction::Play { .. })).expect("play action");
        assert!(matches!(&play, AlarmAction::Play { source: AthanSource::Builtin, .. }));
        assert!(actions.iter().any(|a| matches!(a, AlarmAction::DownloadVoice { id } if id == "adhan-quds")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cached_voice_file_plays_without_download() {
        let dir = temp_voices_dir("cached");
        std::fs::write(dir.join("adhan-quds.mp3"), b"cached").unwrap();
        let mut cfg = config();
        cfg.alerts.fajr.mode = AthanMode::Adhan;
        cfg.alerts.fajr.voice = Some("adhan-quds".into());
        let mut alerted = HashSet::new();
        let actions = evaluate_tick(&cfg, &sample_payload(), now(5, 30), date(), &mut alerted, &dir);
        let play = actions.iter().find(|a| matches!(a, AlarmAction::Play { .. })).expect("play");
        assert!(matches!(&play, AlarmAction::Play { source: AthanSource::File(_), .. }));
    }

    #[test]
    fn notify_before_fires_once_per_prayer_per_day() {
        let mut cfg = config();
        cfg.alerts.fajr.notify_before_min = Some(5);
        cfg.alerts.fajr.mode = AthanMode::Default;
        let mut alerted = HashSet::new();

        let actions = evaluate_tick(&cfg, &sample_payload(), now(5, 25), date(), &mut alerted, &temp_voices_dir("t"));
        assert!(actions.iter().any(|a| matches!(a, AlarmAction::Notify { body, .. } if body.contains("5 minutes"))));

        // Same minute again: deduplicated — no second heads-up.
        let actions = evaluate_tick(&cfg, &sample_payload(), now(5, 25), date(), &mut alerted, &temp_voices_dir("t"));
        assert!(actions.is_empty(), "dedup failed: {actions:?}");
    }

    #[test]
    fn iqama_alerts_fire_only_when_enabled() {
        let mut cfg = config();
        cfg.iqama_alerts = true;
        let mut alerted = HashSet::new();
        let actions = evaluate_tick(&cfg, &sample_payload(), now(13, 35), date(), &mut alerted, &temp_voices_dir("t"));
        assert!(actions.iter().any(|a| matches!(a, AlarmAction::Notify { body, .. } if body.contains("iqama has started"))));

        // Disabled: nothing at the iqama minute.
        cfg.iqama_alerts = false;
        let actions = evaluate_tick(&cfg, &sample_payload(), now(13, 35), date(), &mut alerted, &temp_voices_dir("t"));
        assert!(actions.is_empty());
    }

    #[test]
    fn hostile_adhan_time_is_inert() {
        let mut cfg = config();
        cfg.alerts.fajr.mode = AthanMode::Adhan;
        cfg.alerts.fajr.voice = Some("adhan-quds".into());
        let mut payload = sample_payload();
        payload.times.adhan.fajr = "25:70".into(); // invalid: F4-class
        let dir = temp_voices_dir("hostile");
        let mut alerted = HashSet::new();

        let actions = evaluate_tick(&cfg, &payload, now(5, 30), date(), &mut alerted, &dir);
        // The hostile field must produce no notify, no play, no download.
        assert!(actions.is_empty(), "got {actions:?}");
    }
}
