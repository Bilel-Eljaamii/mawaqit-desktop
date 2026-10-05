use chrono::NaiveDate;
use mawaqit_api::{Announcement, ConfData, TodayTimes};
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

/// Per-prayer alert behavior, mirroring the Mawaqit mobile app's
/// Silent / Default / Adhan choice.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AthanMode {
    /// Popup notification, no athan audio (a sound-suppression hint is sent
    /// where the platform supports one).
    Silent,
    /// Popup notification, system-default sound behavior.
    Default,
    /// Popup notification plus the full athan sound.
    #[default]
    Adhan,
}

/// One prayer's alert settings. Every field is independently defaulted so a
/// partially-written config never fails to parse (the F9 class of bug).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct PrayerAlerts {
    #[serde(default)]
    pub mode: AthanMode,
    /// `None` plays the embedded athan; `Some(path)` plays a local audio
    /// file (mp3/wav). The path comes from a config file any process running
    /// as the user can write — it is treated as audio input only.
    #[serde(default)]
    pub sound: Option<String>,
    /// 0–100; `None` follows the system volume.
    #[serde(default)]
    pub volume: Option<u8>,
    /// Minutes before the adhan for a heads-up notification.
    #[serde(default)]
    pub notify_before_min: Option<u16>,
}

/// Per-prayer alerts plus the global pre-shurouq reminder. Field order and
/// names match `prayer_logic::PRAYERS`; `prayer()` indexes into them.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct AlertsConfig {
    #[serde(default)]
    pub fajr: PrayerAlerts,
    #[serde(default)]
    pub dhuhr: PrayerAlerts,
    #[serde(default)]
    pub asr: PrayerAlerts,
    #[serde(default)]
    pub maghrib: PrayerAlerts,
    #[serde(default)]
    pub isha: PrayerAlerts,
    /// Notification-only reminder before sunrise (sunrise has no adhan).
    #[serde(default)]
    pub shuruq_notify_before_min: Option<u16>,
}

impl AlertsConfig {
    /// The settings for the prayer at `index` in `PRAYERS` order.
    pub fn prayer(&self, index: usize) -> &PrayerAlerts {
        let all = [&self.fajr, &self.dhuhr, &self.asr, &self.maghrib, &self.isha];
        all.get(index).copied().unwrap_or(&self.fajr)
    }

    /// The settings for a prayer by its (lowercase) key, e.g. `"fajr"`.
    pub fn prayer_alert(&self, name: &str) -> Option<&PrayerAlerts> {
        match name {
            "fajr" => Some(&self.fajr),
            "dhuhr" => Some(&self.dhuhr),
            "asr" => Some(&self.asr),
            "maghrib" => Some(&self.maghrib),
            "isha" => Some(&self.isha),
            _ => None,
        }
    }

    /// Legacy single-switch behavior: any prayer set to play the athan.
    pub fn any_adhan(&self) -> bool {
        [&self.fajr, &self.dhuhr, &self.asr, &self.maghrib, &self.isha]
            .iter()
            .any(|p| p.mode == AthanMode::Adhan)
    }
}

/// Persisted app configuration. Old config files (with `masjid_id`, or the
/// brief `email`/`password` era) load cleanly: unknown fields are ignored
/// and missing ones get defaults. No account data is stored — mawaqit.net
/// is used without login.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AppConfig {
    /// Mosque page slug from the search endpoint,
    /// e.g. `grande-mosquee-de-paris`.
    #[serde(default)]
    pub mosque_slug: String,
    #[serde(default)]
    pub mosque_name: Option<String>,
    /// Legacy global athan switch, kept for downgrade compatibility. It is
    /// recomputed from `alerts` on save; loading seeds `alerts` from it when
    /// the file predates per-prayer settings.
    #[serde(default = "default_true")]
    pub sound_enabled: bool,
    /// Also notify when each iqama time starts (adhan alerts are always on).
    #[serde(default)]
    pub iqama_alerts: bool,
    #[serde(default = "default_true")]
    pub autostart: bool,
    /// Offline mode: prayer data is served from the disk snapshot only —
    /// the network is never touched for conf data (search is refused too).
    #[serde(default)]
    pub offline_mode: bool,
    #[serde(default)]
    pub alerts: AlertsConfig,
    /// Ids of mosque announcements the user has read. Capped on save.
    #[serde(default)]
    pub announcements_read: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            mosque_slug: String::new(),
            mosque_name: None,
            sound_enabled: true,
            iqama_alerts: false,
            autostart: true,
            offline_mode: false,
            alerts: AlertsConfig::default(),
            announcements_read: Vec::new(),
        }
    }
}

impl AppConfig {
    pub fn has_mosque(&self) -> bool {
        !self.mosque_slug.is_empty()
    }
}

/// One mosque announcement, normalized for the inbox UI. The id is a
/// stable string — the wire id when the mosque publishes one, otherwise a
/// content hash — so per-item read state survives refetches.
#[derive(Debug, Clone, Serialize)]
pub struct AnnouncementDto {
    pub id: String,
    pub title: Option<String>,
    pub content: Option<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
}

/// Stable read-state key for an announcement: the wire id when present
/// (number or string), otherwise a hash of its content.
pub fn announcement_key(a: &Announcement) -> String {
    match &a.id {
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => s.clone(),
        _ => {
            #[derive(Hash)]
            struct Key<'a>(&'a Option<String>, &'a Option<String>, &'a Option<String>);
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            Key(&a.title, &a.content, &a.start_date).hash(&mut hasher);
            format!("hash-{:016x}", hasher.finish())
        }
    }
}

/// Everything the Today view needs, in one IPC call.
#[derive(Debug, Clone, Serialize)]
pub struct TodayPayload {
    pub mosque_name: Option<String>,
    /// Jumu'a times (some mosques have two).
    pub jumua: Option<String>,
    pub jumua2: Option<String>,
    /// Mosque picture — used as the background, like mawaqit.net does.
    pub image: Option<String>,
    /// True for "Sabah Imsak" mosques (DİTİB): the first prayer is labeled
    /// "Imsak", not "Fajr".
    pub imsak_mode: bool,
    pub times: TodayTimes,
    /// Fetch date of the offline snapshot when this data was served from
    /// disk (no network); `None` for live data.
    pub as_of: Option<String>,
    /// The mosque's announcements (wire order), ids normalized.
    pub announcements: Vec<AnnouncementDto>,
}

impl TodayPayload {
    pub fn from_conf(
        conf: &ConfData,
        times: TodayTimes,
        as_of: Option<NaiveDate>,
    ) -> Self {
        Self {
            mosque_name: conf.name.clone(),
            jumua: conf.jumua.clone(),
            jumua2: conf.jumua2.clone(),
            image: conf.image.clone(),
            imsak_mode: conf.imsak_mode,
            times,
            as_of: as_of.map(|d| d.to_string()),
            announcements: conf
                .announcements
                .iter()
                .map(|a| AnnouncementDto {
                    id: announcement_key(a),
                    title: a.title.clone(),
                    content: a.content.clone(),
                    start_date: a.start_date.clone(),
                    end_date: a.end_date.clone(),
                })
                .collect(),
        }
    }
}
