use std::{path::PathBuf, sync::Arc, time::Duration};

use chrono::{Local, NaiveDate};

use crate::{
    cache::TtlCache,
    calendar, disk,
    error::{MawaqitError, Result},
    models::{ConfData, MonthIqamaTimes, MonthTimes, Mosque, TodayTimes},
};

const API_URL_BASE: &str = "https://mawaqit.net/api";
const SITE_URL_BASE: &str = "https://mawaqit.net";
const PAGE_LANG: &str = "en";
/// Requests without a browser-ish User-Agent get rejected by the site.
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:132.0) Gecko/20100101 Firefox/132.0";
/// confData carries the whole year; refetching a few times a day is plenty.
const CONF_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const SEARCH_TTL: Duration = Duration::from_secs(30 * 60);
/// Give up rather than hang the caller (the desktop loop shares this client).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A real mosque page is ~60 KB; anything near this cap is hostile.
const MAX_RESPONSE_BYTES: usize = 20 * 1024 * 1024;

/// Keyless client for mawaqit.net — no account required.
///
/// - Mosque search uses the public endpoint `GET
///   /api/2.0/mosque/search?word=…`.
/// - All prayer data comes from the public mosque page `https://mawaqit.net/{lang}/{slug}`,
///   whose embedded `confData` object holds the daily times, the year calendar,
///   the iqama calendar and the mosque metadata. One fetched page is shared by
///   every data method and cached in memory (6h for pages, 30min for searches).
#[derive(Clone)]
pub struct MawaqitClient {
    inner: Arc<Inner>,
    /// Offline snapshot directory (see [`crate::disk`]); shared with clones,
    /// `None` = feature off.
    disk: Option<Arc<PathBuf>>,
}

struct Inner {
    http: reqwest::Client,
    api_base: String,
    site_base: String,
    pages: TtlCache<Arc<ConfData>>,
    searches: TtlCache<Vec<Mosque>>,
}

impl MawaqitClient {
    pub fn new() -> Self {
        Self::with_base_urls(API_URL_BASE.to_string(), SITE_URL_BASE.to_string())
    }

    /// Same client against custom base URLs — the seam the hostile HTTP
    /// tests use to point the client at a local mock server.
    pub fn with_base_urls(api_base: String, site_base: String) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .expect("reqwest client builds without custom TLS config");
        Self {
            inner: Arc::new(Inner {
                http,
                api_base,
                site_base,
                pages: TtlCache::new(CONF_TTL),
                searches: TtlCache::new(SEARCH_TTL),
            }),
            disk: None,
        }
    }

    /// Serve [`Self::conf_data`] from a disk snapshot when the network is
    /// unavailable, and refresh the snapshot on every successful fetch —
    /// the offline layer. See [`crate::disk`].
    pub fn with_disk_cache(mut self, dir: PathBuf) -> Self {
        self.disk = Some(Arc::new(dir));
        self
    }

    /// `GET /api/2.0/mosque/search?word=...` — keyword search, no auth.
    pub async fn search_mosques(&self, word: &str) -> Result<Vec<Mosque>> {
        let word = word.trim();
        if word.is_empty() {
            return Ok(Vec::new());
        }
        let cache_key = word.to_lowercase();
        if let Some(cached) = self.inner.searches.get(&cache_key) {
            return Ok(cached);
        }

        let url = format!("{}/2.0/mosque/search", self.inner.api_base);
        let response = self.inner.http.get(&url).query(&[("word", word)]).send().await?;

        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(MawaqitError::MosqueNotFound(word.to_string()));
        }
        let body = read_capped(response).await?;
        if !status.is_success() {
            return Err(MawaqitError::Api { status: status.as_u16(), url });
        }

        let mosques: Vec<Mosque> = serde_json::from_str(&body)
            .map_err(|e| MawaqitError::Parse(format!("search: {e}")))?;
        self.inner.searches.insert(cache_key, mosques.clone());
        Ok(mosques)
    }

    /// Fetch (or take from cache) the confData of a mosque page. `mosque_id`
    /// is the page slug, e.g. `grande-mosquee-de-paris`. When the network
    /// fails and a disk snapshot exists ([`Self::with_disk_cache`]), the
    /// snapshot is served instead.
    pub async fn conf_data(&self, mosque_id: &str) -> Result<Arc<ConfData>> {
        self.conf_data_dated(mosque_id).await.map(|(conf, _)| conf)
    }

    /// Like [`Self::conf_data`], additionally reporting whether the data
    /// came from the offline disk snapshot: `Some(fetched date)` when it
    /// did, `None` for a fresh (network or in-memory) fetch.
    pub async fn conf_data_dated(
        &self,
        mosque_id: &str,
    ) -> Result<(Arc<ConfData>, Option<NaiveDate>)> {
        if let Some(cached) = self.inner.pages.get(mosque_id) {
            return Ok((cached, None));
        }

        match Self::fetch_conf_data(&self.inner, mosque_id).await {
            Ok(conf) => {
                if let Some(dir) = &self.disk {
                    // Best-effort: a failed snapshot write never breaks an
                    // online fetch.
                    disk::store(dir, mosque_id, &conf);
                }
                Ok((conf, None))
            }
            Err(err) => {
                if let Some(dir) = &self.disk {
                    if let Some((fetched_at, conf)) = disk::load(dir, mosque_id) {
                        return Ok((Arc::new(conf), Some(fetched_at)));
                    }
                }
                Err(err)
            }
        }
    }

    async fn fetch_conf_data(inner: &Inner, mosque_id: &str) -> Result<Arc<ConfData>> {
        let url = page_url(&inner.site_base, mosque_id);
        let response = inner.http.get(&url).send().await?;

        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(MawaqitError::MosqueNotFound(mosque_id.to_string()));
        }
        if !status.is_success() {
            return Err(MawaqitError::Api { status: status.as_u16(), url });
        }
        let html = read_capped(response).await?;
        let conf = Arc::new(crate::scraper::extract_conf_data(&html, mosque_id)?);
        inner.pages.insert(mosque_id.to_string(), conf.clone());
        Ok(conf)
    }

    /// Adhan + resolved iqama times for today (local timezone).
    pub async fn today(&self, mosque_id: &str) -> Result<TodayTimes> {
        let conf = self.conf_data(mosque_id).await?;
        calendar::times_for_date(&conf, Local::now().date_naive())
    }

    /// Adhan times for a month (1-12).
    pub async fn month(&self, mosque_id: &str, month: u32) -> Result<MonthTimes> {
        let conf = self.conf_data(mosque_id).await?;
        calendar::month_times(&conf, month)
    }

    /// Resolved iqama times for a month (1-12).
    pub async fn month_iqama(
        &self,
        mosque_id: &str,
        month: u32,
    ) -> Result<MonthIqamaTimes> {
        let conf = self.conf_data(mosque_id).await?;
        calendar::month_iqama_times(&conf, month)
    }

    /// Drop the cached page for a mosque (or everything when `None`).
    pub fn invalidate(&self, mosque_id: Option<&str>) {
        match mosque_id {
            Some(id) => self.inner.pages.invalidate(id),
            None => self.inner.pages.clear(),
        }
    }
}

impl Default for MawaqitClient {
    fn default() -> Self {
        Self::new()
    }
}

/// The exact URL [`MawaqitClient::conf_data`] fetches for a slug. Exposed as
/// a pure function so hostile-slug handling (`../`, `?`, `#`, giant or
/// non-ASCII slugs) can be asserted without touching the network.
pub fn page_url(site_base: &str, mosque_id: &str) -> String {
    format!("{site_base}/{PAGE_LANG}/{mosque_id}")
}

/// Read a response body with a hard size cap so a hostile/huge response can
/// never balloon memory.
async fn read_capped(response: reqwest::Response) -> Result<String> {
    let bytes = response.bytes().await?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(MawaqitError::Parse(format!(
            "response of {} bytes exceeds the {} byte cap",
            bytes.len(),
            MAX_RESPONSE_BYTES
        )));
    }
    String::from_utf8(bytes.to_vec())
        .map_err(|e| MawaqitError::Parse(format!("response is not UTF-8: {e}")))
}

/// Convenience: minutes between two "HH:MM" times (b-a), handling midnight
/// wrap.
pub fn minutes_between(a: &str, b: &str) -> Option<i64> {
    let a = calendar::parse_hhmm(a)?;
    let b = calendar::parse_hhmm(b)?;
    let diff = (b - a).num_minutes();
    Some(if diff < 0 { diff + 24 * 60 } else { diff })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minutes_between_handles_wrap() {
        assert_eq!(minutes_between("10:00", "10:30"), Some(30));
        assert_eq!(minutes_between("23:30", "00:10"), Some(40));
    }

    /// Live end-to-end check against the real site (no account needed):
    /// `cargo test -p mawaqit-api -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "hits the live mawaqit.net site"]
    async fn live_search_and_calendar() {
        let client = MawaqitClient::new();
        let mosques = client.search_mosques("Paris").await.expect("search");
        println!("search hits: {}", mosques.len());
        assert!(!mosques.is_empty(), "expected at least one mosque");

        let slug = mosques
            .iter()
            .find_map(|m| m.mosque_id())
            .expect("a mosque with a slug")
            .to_string();
        println!("mosque: {} ({slug})", mosques[0].display_name());

        let today = client.today(&slug).await.expect("today");
        println!(
            "fajr={} shurouq={} isha={}",
            today.adhan.fajr, today.adhan.shurouq, today.adhan.isha
        );
        if let Some(iq) = &today.iqama {
            println!("iqama: {:?}", iq);
        }

        let month = client.month(&slug, 1).await.expect("month");
        assert!(!month.days.is_empty());
    }
}
