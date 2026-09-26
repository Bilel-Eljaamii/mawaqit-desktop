use chrono::{Datelike, Duration, NaiveDate, NaiveTime};

use crate::{
    error::{MawaqitError, Result},
    models::{
        ConfData, DailyIqamaTimes, DailyPrayerTimes, DayIqamaTimes, DayTimes,
        MonthIqamaTimes, MonthTimes, RawCalendar, TodayTimes,
    },
};

pub(crate) fn parse_hhmm(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s.trim(), "%H:%M").ok()
}

fn time_string(t: NaiveTime) -> String {
    t.format("%H:%M").to_string()
}

/// Build one day's times from a calendar day row.
///
/// Calendar rows carry a shuruq (sunrise) column at index 1:
/// `[first_prayer, shuruq, rest…]`. The number of prayers varies by mosque:
/// - normal mode (5 prayers): `[Fajr, Shuruq, Dhuhr, Asr, Maghrib, Isha]`
/// - imsak mode (6 prayers, `displayingSabahImsak`, e.g. DİTİB mosques):
///   `[İmsak, Sabah, Shurûq, Dhuhr, Asr, Maghrib, Isha]` — the displayed Shurûq
///   is the third column (Diyanet Güneş); "Sabah" (index 1) is an extra chip
///   value not used as a prayer time.
/// Rows without the shuruq column are treated as plain prayer lists, with
/// sunrise taken from the page-level `shuruq` field.
pub(crate) fn daily_from_row(
    row: &[String],
    conf_shuruq: Option<&str>,
) -> Result<DailyPrayerTimes> {
    let has_shuruq_column =
        row.len() >= 6 && row.get(1).is_some_and(|v| parse_hhmm(v).is_some());

    if has_shuruq_column {
        let normal_shurouq = row[1].clone();
        let mut prayers = vec![row[0].clone()];
        prayers.extend_from_slice(&row[2..]);
        return match prayers.len() {
            // imsak mode: [İmsak, Shurûq, Dhuhr, Asr, Maghrib, Isha]
            6 => Ok(DailyPrayerTimes {
                fajr: prayers[0].clone(),
                shurouq: prayers[1].clone(),
                dhuhr: prayers[2].clone(),
                asr: prayers[3].clone(),
                maghrib: prayers[4].clone(),
                isha: prayers[5].clone(),
            }),
            // normal mode: [Fajr, Dhuhr, Asr, Maghrib, Isha]
            5 => Ok(DailyPrayerTimes {
                fajr: prayers[0].clone(),
                shurouq: normal_shurouq,
                dhuhr: prayers[1].clone(),
                asr: prayers[2].clone(),
                maghrib: prayers[3].clone(),
                isha: prayers[4].clone(),
            }),
            n => {
                Err(MawaqitError::Parse(format!("expected 5 or 6 prayer times, got {n}")))
            }
        };
    }

    // Plain prayer list without a shuruq column.
    match row.len() {
        6 => Ok(DailyPrayerTimes {
            fajr: row[0].clone(),
            shurouq: row[1].clone(),
            dhuhr: row[2].clone(),
            asr: row[3].clone(),
            maghrib: row[4].clone(),
            isha: row[5].clone(),
        }),
        5 => {
            let shurouq = conf_shuruq
                .ok_or_else(|| MawaqitError::Parse("no shuruq value for day".into()))?
                .to_string();
            Ok(DailyPrayerTimes {
                fajr: row[0].clone(),
                shurouq,
                dhuhr: row[1].clone(),
                asr: row[2].clone(),
                maghrib: row[3].clone(),
                isha: row[4].clone(),
            })
        }
        n => Err(MawaqitError::Parse(format!("expected 5 or 6 prayer times, got {n}"))),
    }
}

/// Resolve one raw iqama entry: "HH:MM" stays as-is, "+N" becomes
/// `adhan + N minutes`. Anything else falls back to the adhan time itself,
/// like the official integrations do. N is clamped to one day: real offsets
/// are minutes, and huge hostile values must not overflow the time math.
pub(crate) fn resolve_iqama(raw: &str, adhan: &str) -> String {
    let adhan_t = parse_hhmm(adhan);
    if let Some(mins) = raw.trim().strip_prefix('+') {
        if let (Ok(n), Some(t)) = (mins.trim().parse::<i64>(), adhan_t) {
            if let Some(delta) = Duration::try_minutes(n.clamp(0, 24 * 60)) {
                return time_string(t + delta);
            }
        }
    }
    if parse_hhmm(raw).is_some() {
        return raw.trim().to_string();
    }
    adhan.trim().to_string()
}

pub(crate) fn daily_iqama_from(
    raw: &[String],
    adhan: &DailyPrayerTimes,
) -> Result<DailyIqamaTimes> {
    if raw.len() < 5 {
        return Err(MawaqitError::Parse(format!(
            "expected 5 iqama times, got {}",
            raw.len()
        )));
    }
    Ok(DailyIqamaTimes {
        fajr: resolve_iqama(&raw[0], &adhan.fajr),
        dhuhr: resolve_iqama(&raw[1], &adhan.dhuhr),
        asr: resolve_iqama(&raw[2], &adhan.asr),
        maghrib: resolve_iqama(&raw[3], &adhan.maghrib),
        isha: resolve_iqama(&raw[4], &adhan.isha),
    })
}

fn raw_month(calendar: &RawCalendar, month: u32) -> Result<&crate::models::RawMonth> {
    if !(1..=12).contains(&month) {
        return Err(MawaqitError::InvalidMonth(month));
    }
    calendar.get((month - 1) as usize).ok_or(MawaqitError::NoCalendar)
}

/// Adhan times for every day of a month.
pub fn month_times(conf: &ConfData, month: u32) -> Result<MonthTimes> {
    let raw = raw_month(&conf.calendar, month)?;
    let mut days = Vec::with_capacity(raw.len());
    for (key, values) in raw {
        let Ok(day) = key.parse::<u32>() else {
            continue;
        };
        if let Ok(times) = daily_from_row(values, conf.shuruq.as_deref()) {
            days.push(DayTimes { day, times });
        }
    }
    days.sort_by_key(|d| d.day);
    Ok(MonthTimes { month, days })
}

/// Resolved iqama times for every day of a month (uses the adhan calendar
/// to expand "+N" entries).
pub fn month_iqama_times(conf: &ConfData, month: u32) -> Result<MonthIqamaTimes> {
    let iqama_calendar = conf.iqama_calendar.as_ref().ok_or(MawaqitError::NoCalendar)?;
    let raw_iqama = raw_month(iqama_calendar, month)?;
    let adhan_month = month_times(conf, month)?;
    let adhan_by_day: std::collections::HashMap<u32, &DailyPrayerTimes> =
        adhan_month.days.iter().map(|d| (d.day, &d.times)).collect();

    let mut days = Vec::with_capacity(raw_iqama.len());
    for (key, values) in raw_iqama {
        let Ok(day) = key.parse::<u32>() else {
            continue;
        };
        let Some(adhan) = adhan_by_day.get(&day) else {
            continue;
        };
        if let Ok(times) = daily_iqama_from(values, adhan) {
            days.push(DayIqamaTimes { day, times });
        }
    }
    days.sort_by_key(|d| d.day);
    Ok(MonthIqamaTimes { month, days })
}

/// Adhan (+ iqama) times for a specific date.
pub fn times_for_date(conf: &ConfData, date: NaiveDate) -> Result<TodayTimes> {
    let month = date.month() as u32;
    let day = date.day();
    let adhan = month_times(conf, month)?
        .days
        .into_iter()
        .find(|d| d.day == day)
        .ok_or(MawaqitError::NoCalendar)?
        .times;
    let iqama = conf
        .iqama_calendar
        .as_ref()
        .and_then(|_| month_iqama_times(conf, month).ok())
        .and_then(|m| m.days.into_iter().find(|d| d.day == day))
        .map(|d| d.times);
    Ok(TodayTimes { date, adhan, iqama })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::models::RawMonth;

    fn month_map(pairs: &[(&str, Vec<&str>)]) -> RawMonth {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
            .collect()
    }

    fn sample_conf() -> ConfData {
        serde_json::from_value(json!({
            "calendar": [
                month_map(&[("1", vec!["06:30","08:00","13:00","15:30","17:45","19:15"])]),
                month_map(&[("1", vec!["06:20","07:50","12:50","15:10","17:30","19:00"])])
            ],
            "iqamaCalendar": [
                month_map(&[("1", vec!["06:45","+15","13:20","+20","18:00"])])
            ],
            "name": "Test Mosque"
        }))
        .unwrap()
    }

    #[test]
    fn parses_normal_mode_row() {
        // [Fajr, Shuruq, Dhuhr, Asr, Maghrib, Isha]
        let row: Vec<String> = ["06:30", "08:00", "13:00", "15:30", "17:45", "19:15"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let t = daily_from_row(&row, None).unwrap();
        assert_eq!(t.fajr, "06:30");
        assert_eq!(t.shurouq, "08:00");
        assert_eq!(t.dhuhr, "13:00");
        assert_eq!(t.isha, "19:15");
    }

    #[test]
    fn parses_imsak_mode_row() {
        let row: Vec<String> =
            ["05:27", "06:37", "07:07", "13:21", "16:37", "19:24", "20:51"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        let t = daily_from_row(&row, Some("06:37")).unwrap();
        assert_eq!(t.fajr, "05:27"); // displayed as "Imsak"
        assert_eq!(t.shurouq, "07:07"); // Diyanet Güneş
        assert_eq!(t.dhuhr, "13:21");
        assert_eq!(t.asr, "16:37");
        assert_eq!(t.maghrib, "19:24");
        assert_eq!(t.isha, "20:51");
    }

    #[test]
    fn parses_row_without_shuruq_column() {
        let row: Vec<String> = ["06:09", "13:47", "16:58", "19:45", "21:12"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let t = daily_from_row(&row, Some("07:41")).unwrap();
        assert_eq!(t.fajr, "06:09");
        assert_eq!(t.shurouq, "07:41");
        assert_eq!(t.dhuhr, "13:47");
        assert_eq!(t.isha, "21:12");
    }

    #[test]
    fn rejects_short_day() {
        let raw: Vec<String> = vec!["06:30".into(), "08:00".into()];
        assert!(matches!(daily_from_row(&raw, None), Err(MawaqitError::Parse(_))));
    }

    #[test]
    fn resolves_relative_and_absolute_iqama() {
        let adhan = DailyPrayerTimes {
            fajr: "06:30".into(),
            shurouq: "08:00".into(),
            dhuhr: "13:00".into(),
            asr: "15:30".into(),
            maghrib: "17:45".into(),
            isha: "19:15".into(),
        };
        let raw: Vec<String> = ["06:45", "+15", "13:20", "+20", "18:00"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let iq = daily_iqama_from(&raw, &adhan).unwrap();
        assert_eq!(iq.fajr, "06:45");
        assert_eq!(iq.dhuhr, "13:15");
        assert_eq!(iq.asr, "13:20"); // absolute value kept
        assert_eq!(iq.maghrib, "18:05"); // 17:45 + 20 minutes
        assert_eq!(iq.isha, "18:00");
    }

    #[test]
    fn invalid_iqama_falls_back_to_adhan() {
        assert_eq!(resolve_iqama("garbage", "17:45"), "17:45");
        assert_eq!(resolve_iqama("+abc", "17:45"), "17:45");
    }

    #[test]
    fn hostile_iqama_offsets_do_not_panic() {
        // Red-team finding: TimeDelta::minutes(i64::MAX) used to panic.
        let big = "+9223372036854775807";
        assert_eq!(resolve_iqama(big, "17:45"), "17:45"); // clamped to +24h ->
                                                          // next-day 17:45
        assert_eq!(resolve_iqama("+999999999999999", "17:45"), "17:45");
        assert_eq!(resolve_iqama("+0", "17:45"), "17:45");
        assert_eq!(resolve_iqama("+1440", "17:45"), "17:45");
        assert_eq!(resolve_iqama("+", "17:45"), "17:45");
        // one '+' is stripped and "+5" parses, so this resolves to +5 minutes
        assert_eq!(resolve_iqama("++5", "17:45"), "17:50");
    }

    #[test]
    fn extracts_month_and_iqama() {
        let r = sample_conf();
        let m = month_times(&r, 1).unwrap();
        assert_eq!(m.days.len(), 1);
        assert_eq!(m.days[0].times.dhuhr, "13:00");

        let mi = month_iqama_times(&r, 1).unwrap();
        assert_eq!(mi.days[0].times.dhuhr, "13:15");
    }

    #[test]
    fn rejects_invalid_month() {
        let r = sample_conf();
        assert!(matches!(month_times(&r, 0), Err(MawaqitError::InvalidMonth(0))));
        assert!(matches!(month_times(&r, 13), Err(MawaqitError::InvalidMonth(13))));
    }

    #[test]
    fn finds_today() {
        let r = sample_conf();
        let today =
            times_for_date(&r, NaiveDate::from_ymd_opt(2026, 2, 1).unwrap()).unwrap();
        assert_eq!(today.adhan.fajr, "06:20");
        // no iqama calendar entry for February -> iqama is None, adhan still
        // works
        assert!(today.iqama.is_none());

        let first =
            times_for_date(&r, NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()).unwrap();
        assert_eq!(first.adhan.fajr, "06:30");
        assert_eq!(first.iqama.unwrap().dhuhr, "13:15");
    }
}
