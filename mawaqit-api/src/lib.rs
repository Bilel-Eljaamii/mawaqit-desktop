//! # mawaqit-api
//!
//! Keyless Rust client for [mawaqit.net](https://mawaqit.net) prayer times —
//! no account required.
//!
//! - Mosque search: public endpoint `GET /api/2.0/mosque/search?word=…`
//! - Prayer data: the public mosque page `https://mawaqit.net/{lang}/{slug}`
//!   embeds a `confData` object with the daily times, the year calendar, the
//!   iqama calendar and the mosque metadata; one fetch serves every method.
//!
//! ```no_run
//! # async fn example() -> Result<(), mawaqit_api::MawaqitError> {
//! let client = mawaqit_api::MawaqitClient::new();
//!
//! let mosques = client.search_mosques("Paris").await?;
//! let slug = mosques[0].mosque_id().unwrap().to_string();
//!
//! let today = client.today(&slug).await?;
//! println!("{} — Fajr at {} (iqama {})", mosques[0].display_name(),
//!     today.adhan.fajr,
//!     today.iqama.as_ref().map(|i| i.fajr.as_str()).unwrap_or("?"));
//! # Ok(())
//! # }
//! ```
//!
//! Iqama entries of the form `"+15"` (minutes after the adhan) are resolved
//! to absolute `HH:MM` times automatically.

mod cache;
mod calendar;
mod client;
pub mod disk;
mod error;
mod models;
mod scraper;
pub use calendar::{month_iqama_times, month_times, times_for_date};
pub use client::{is_valid_slug, minutes_between, page_url, MawaqitClient};
pub use error::{MawaqitError, Result};
pub use models::{
    Announcement, ConfData, DailyIqamaTimes, DailyPrayerTimes, DayIqamaTimes, DayTimes,
    MonthIqamaTimes, MonthTimes, Mosque, RawCalendar, RawMonth, TodayTimes,
};
/// Parse a mosque page's HTML into its [`ConfData`] — exposed for tests and
/// fuzzing; [`MawaqitClient::conf_data`] is the network-backed wrapper.
pub use scraper::extract_conf_data as parse_page;
