use serde::{Deserialize, Serialize};

/// A mosque as returned by the keyless search endpoint
/// (`GET /api/2.0/mosque/search?word=...`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mosque {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    /// URL slug used to fetch the public page (`/{lang}/{slug}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locality: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Mosque {
    /// Best human-readable name: label, then name, then the slug.
    pub fn display_name(&self) -> &str {
        self.label
            .as_deref()
            .or(self.name.as_deref())
            .or(self.slug.as_deref())
            .unwrap_or("?")
    }

    /// The slug identifier used by [`crate::MawaqitClient`] data methods.
    pub fn mosque_id(&self) -> Option<&str> {
        self.slug.as_deref()
    }

    /// Short place description ("locality, country") when available.
    pub fn place(&self) -> Option<String> {
        match (self.locality.as_deref(), self.country.as_deref()) {
            (Some(l), Some(c)) => Some(format!("{l}, {c}")),
            (Some(l), None) => Some(l.to_string()),
            (None, Some(c)) => Some(c.to_string()),
            (None, None) => None,
        }
    }
}

/// The parts of the page's `confData` object the client exposes.
/// Anything else stays accessible through [`ConfData::raw`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfData {
    /// Today's adhan times, [Fajr, Shurouq, Dhuhr, Asr, Maghrib, Isha].
    #[serde(default)]
    pub times: Vec<String>,
    /// Today's sunrise.
    #[serde(default)]
    pub shuruq: Option<String>,
    /// Year calendar: 12 months, each an object day -> [Fajr, Shurouq,
    /// Dhuhr, Asr, Maghrib, Isha] as "HH:MM".
    #[serde(default)]
    pub calendar: RawCalendar,
    /// Iqama calendar: 12 months, each an object day -> 5 values (no
    /// shurouq); values are "HH:MM" or "+N" minutes after the adhan.
    #[serde(default, rename = "iqamaCalendar")]
    pub iqama_calendar: Option<RawCalendar>,
    /// Mosque display name.
    #[serde(default)]
    pub name: Option<String>,
    /// True when the mosque uses the 6-prayer "Sabah Imsak" layout
    /// (`displayingSabahImsak`, common on DİTİB mosques): the first prayer
    /// time is the imsak and is usually displayed as "Imsak", not "Fajr".
    #[serde(default)]
    pub imsak_mode: bool,
    /// Jumu'a times (some mosques have two).
    #[serde(default)]
    pub jumua: Option<String>,
    #[serde(default)]
    pub jumua2: Option<String>,
    /// Mosque picture shown as the background on mawaqit.net.
    #[serde(default)]
    pub image: Option<String>,
    /// Announcements configured by the mosque.
    #[serde(default)]
    pub announcements: Vec<Announcement>,
    /// The complete raw confData for anything not modeled above.
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Announcement {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_date: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Raw API shape: a month is an object keyed by day-of-month ("1".."31"),
/// each day an ordered list of "HH:MM" strings.
pub type RawMonth = std::collections::BTreeMap<String, Vec<String>>;
pub type RawCalendar = Vec<RawMonth>;

/// The six adhan times of one day, in API order.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DailyPrayerTimes {
    pub fajr: String,
    pub shurouq: String,
    pub dhuhr: String,
    pub asr: String,
    pub maghrib: String,
    pub isha: String,
}

/// The five iqama times of one day (no iqama for shurouq), already resolved
/// to absolute "HH:MM" times (relative "+N" entries are applied to the adhan).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DailyIqamaTimes {
    pub fajr: String,
    pub dhuhr: String,
    pub asr: String,
    pub maghrib: String,
    pub isha: String,
}

/// Adhan + resolved iqama times for one calendar day.
#[derive(Debug, Clone, Serialize)]
pub struct TodayTimes {
    pub date: chrono::NaiveDate,
    pub adhan: DailyPrayerTimes,
    pub iqama: Option<DailyIqamaTimes>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DayTimes {
    pub day: u32,
    pub times: DailyPrayerTimes,
}

#[derive(Debug, Clone, Serialize)]
pub struct MonthTimes {
    /// 1-12
    pub month: u32,
    pub days: Vec<DayTimes>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DayIqamaTimes {
    pub day: u32,
    pub times: DailyIqamaTimes,
}

#[derive(Debug, Clone, Serialize)]
pub struct MonthIqamaTimes {
    /// 1-12
    pub month: u32,
    pub days: Vec<DayIqamaTimes>,
}
