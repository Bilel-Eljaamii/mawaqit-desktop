use mawaqit_api::{ConfData, TodayTimes};
use serde::{Deserialize, Serialize};

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
    pub sound_enabled: bool,
    /// Also notify when each iqama time starts (adhan alerts are always on).
    #[serde(default)]
    pub iqama_alerts: bool,
    #[serde(default = "default_true")]
    pub autostart: bool,
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
        }
    }
}

impl AppConfig {
    pub fn has_mosque(&self) -> bool {
        !self.mosque_slug.is_empty()
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
}

impl TodayPayload {
    pub fn from_conf(conf: &ConfData, times: TodayTimes) -> Self {
        Self {
            mosque_name: conf.name.clone(),
            jumua: conf.jumua.clone(),
            jumua2: conf.jumua2.clone(),
            image: conf.image.clone(),
            imsak_mode: conf.imsak_mode,
            times,
        }
    }
}
