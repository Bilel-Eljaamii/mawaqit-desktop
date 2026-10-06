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
    /// Adhan voice from the catalog (`mawaqit_api::ADHAN_VOICES`) — takes
    /// precedence over `sound`. `None` = builtin embedded athan (also the
    /// fallback when the voice file is not cached and cannot be downloaded).
    #[serde(default)]
    pub voice: Option<String>,
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

/// The Tor transport policy (value object): whether mawaqit.net traffic is
/// routed through Tor, and by which transport. `builtin: true` (the
/// default for configs that predate the field) runs Tor inside the app via
/// the embedded Arti client — no external daemon needed; `host`/`port` are
/// then unused. `builtin: false` routes through an external SOCKS5 proxy
/// at `host:port` (system tor, Tor Browser, …). The `socks5h://` scheme is
/// fixed by this type — callers only ever supply host and port, so a
/// DNS-leaking plain `socks5://` can never be stored.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct TorProxy {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_tor_host")]
    pub host: String,
    #[serde(default = "default_tor_port")]
    pub port: u16,
    /// Built-in (embedded Arti) Tor — see `infrastructure::builtin_tor`.
    /// Defaults to true so pre-v0.11 configs switch to the zero-setup
    /// transport on load.
    #[serde(default = "default_true")]
    pub builtin: bool,
}

pub const DEFAULT_TOR_HOST: &str = "127.0.0.1";

fn default_tor_host() -> String {
    DEFAULT_TOR_HOST.into()
}

fn default_tor_port() -> u16 {
    9050
}

impl Default for TorProxy {
    fn default() -> Self {
        Self {
            enabled: false,
            host: default_tor_host(),
            port: default_tor_port(),
            builtin: default_true(),
        }
    }
}

/// A plausible Tor proxy host: letters/digits/dots/dashes, no scheme, no
/// path, no whitespace. The api crate enforces the strict socks5h URL rules
/// at client construction; this is the save-time guard.
pub fn is_plausible_tor_host(host: &str) -> bool {
    !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

impl TorProxy {
    /// The proxy address for the api client (`socks5h://host:port`), or
    /// `None` when Tor is off, the mode is built-in (the address comes
    /// from the running internal stack, see `infrastructure::builtin_tor`),
    /// or the host is implausible — callers never fall back to a direct
    /// connection while Tor is on.
    pub fn socks5h_url(&self) -> Option<String> {
        if !self.enabled || self.builtin || !is_plausible_tor_host(&self.host) {
            return None;
        }
        Some(format!("socks5h://{}:{}", self.host, self.port))
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
    /// Tor/SOCKS5 proxy address (`socks5h://host[:port]`) — when set, all
    /// mawaqit.net traffic is routed through it. Applied at startup; an
    /// unreachable proxy degrades to a direct connection with a log line.
    #[serde(default)]
    pub tor: TorProxy,
    /// Legacy single-string Tor address (`socks5h://host:port`), migrated
    /// into `tor` on load and no longer written after the first save.
    #[serde(default)]
    pub tor_socks_addr: Option<String>,
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
            tor: TorProxy::default(),
            tor_socks_addr: None,
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
    /// Banner image URL (mosque-uploaded). Loaded by the webview under the
    /// CSP's img-src allowlist, never fetched by the backend.
    pub image: Option<String>,
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
                    image: a.image.clone(),
                    start_date: a.start_date.clone(),
                    end_date: a.end_date.clone(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tor_proxy_tests {
    use super::*;

    #[test]
    fn url_built_only_when_enabled_and_plausible() {
        let on = TorProxy { enabled: true, host: "127.0.0.1".into(), port: 9050, builtin: false };
        assert_eq!(on.socks5h_url().as_deref(), Some("socks5h://127.0.0.1:9050"));

        // Disabled: no URL, whatever the address.
        let off = TorProxy { enabled: false, ..on };
        assert!(off.socks5h_url().is_none());

        // Implausible host: no URL.
        let bad = TorProxy { enabled: true, host: "not a host!".into(), port: 9050, builtin: false };
        assert!(bad.socks5h_url().is_none());
        let empty = TorProxy { enabled: true, host: String::new(), port: 9050, builtin: false };
        assert!(empty.socks5h_url().is_none());
    }

    #[test]
    fn port_bounds_and_host_charset() {
        assert!(is_plausible_tor_host("tor.internal.lan"));
        assert!(is_plausible_tor_host("127.0.0.1"));
        assert!(!is_plausible_tor_host(""));
        assert!(!is_plausible_tor_host("bad host"));
        assert!(!is_plausible_tor_host("socks5h://evil"));

        let bad_port = TorProxy { enabled: true, host: "127.0.0.1".into(), port: 0, builtin: false };
        // Port 0 is technically stored but the URL keeps it explicit — the
        // api rejects unreachable proxies by failing to connect, and the
        // settings UI bounds the input to 1..=65535.
        assert_eq!(
            bad_port.socks5h_url().as_deref(),
            Some("socks5h://127.0.0.1:0")
        );
    }

    #[test]
    fn serde_shape_is_stable() {
        let json = r#"{"enabled":true,"host":"127.0.0.1","port":9150}"#;
        let proxy: TorProxy = serde_json::from_str(json).unwrap();
        assert!(proxy.enabled);
        assert_eq!(proxy.host, "127.0.0.1");
        assert_eq!(proxy.port, 9150);
        // Missing fields take defaults (disabled, system tor).
        let minimal: TorProxy = serde_json::from_str("{}").unwrap();
        assert!(!minimal.enabled);
        assert_eq!(minimal.host, "127.0.0.1");
        assert_eq!(minimal.port, 9050);
    }

    #[test]
    fn configs_predating_builtin_migrate_to_the_builtin_stack() {
        // A v0.10-era config block carries no `builtin` field: it loads as
        // built-in mode so the Tor toggle keeps working with zero external
        // setup (the stored host/port stay but are unused).
        let legacy: TorProxy =
            serde_json::from_str(r#"{"enabled":true,"host":"127.0.0.1","port":9050}"#).unwrap();
        assert!(legacy.builtin);
        // The built-in address comes from the running stack, never from
        // host/port — so the value object hands out no URL here.
        assert!(legacy.socks5h_url().is_none());

        // Explicit external mode still resolves the URL.
        let external: TorProxy = serde_json::from_str(
            r#"{"enabled":true,"host":"127.0.0.1","port":9150,"builtin":false}"#,
        )
        .unwrap();
        assert_eq!(
            external.socks5h_url().as_deref(),
            Some("socks5h://127.0.0.1:9150")
        );
    }
}

#[cfg(test)]
mod announcement_tests {
    use super::*;

    fn payload_with_announcement(image_json: &str) -> TodayPayload {
        let page = format!(
            concat!(
                r#"<script>var confData = {{"times":["06:30","08:00","13:00","15:30","17:45"],"#,
                r#""calendar":[{{"1":["06:30","08:00","13:00","15:30","17:45","19:15"]}}],"#,
                r#""announcements":[{{"id":7,"title":"Iftar","content":"Bring a plate","image":{image_json}}}]}};"#,
                r#"</script>"#
            ),
            image_json = image_json
        );
        let conf = mawaqit_api::parse_page(&page, "t").expect("page parses");
        let date = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let times = mawaqit_api::times_for_date(&conf, date).expect("times");
        TodayPayload::from_conf(&conf, times, None)
    }

    #[test]
    fn announcement_image_flows_to_the_dto() {
        let payload = payload_with_announcement("\"https://pics.test/iftar.jpg\"");
        let a = &payload.announcements[0];
        assert_eq!(a.image.as_deref(), Some("https://pics.test/iftar.jpg"));
        assert_eq!(a.title.as_deref(), Some("Iftar"));
    }

    #[test]
    fn announcement_without_image_is_none_not_a_broken_field() {
        let payload = payload_with_announcement("null");
        assert_eq!(payload.announcements[0].image, None);
    }
}
