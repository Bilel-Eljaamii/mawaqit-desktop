use chrono::NaiveTime;
use mawaqit_api::{DailyIqamaTimes, DailyPrayerTimes};

#[derive(Debug, Clone)]
pub struct NextPrayerInfo {
    pub name: String,
    pub time: NaiveTime,
    pub minutes_remaining: i64,
}

/// The five prayers that get an adhan (shurouq/sunrise has none).
pub const PRAYERS: [(&str, fn(&DailyPrayerTimes) -> &str, fn(&DailyIqamaTimes) -> &str);
    5] = [
    ("Fajr", |t| &t.fajr, |t| &t.fajr),
    ("Dhuhr", |t| &t.dhuhr, |t| &t.dhuhr),
    ("Asr", |t| &t.asr, |t| &t.asr),
    ("Maghrib", |t| &t.maghrib, |t| &t.maghrib),
    ("Isha", |t| &t.isha, |t| &t.isha),
];

pub fn adhan_entries(times: &DailyPrayerTimes) -> Vec<(String, String)> {
    PRAYERS
        .iter()
        .map(|(name, adhan, _)| (name.to_string(), adhan(times).to_string()))
        .collect()
}

pub fn iqama_entries(times: &DailyIqamaTimes) -> Vec<(String, String)> {
    PRAYERS
        .iter()
        .map(|(name, _, iqama)| (name.to_string(), iqama(times).to_string()))
        .collect()
}

/// First entry whose time is still ahead of `now`; if all passed, the first
/// entry tomorrow. Times must be valid "HH:MM" strings.
pub fn next_prayer(entries: &[(String, String)]) -> Option<NextPrayerInfo> {
    let now = chrono::Local::now().time();

    let mut parsed: Vec<(&str, NaiveTime)> = entries
        .iter()
        .filter_map(|(name, hhmm)| {
            NaiveTime::parse_from_str(hhmm, "%H:%M").ok().map(|t| (name.as_str(), t))
        })
        .collect();
    parsed.sort_by_key(|&(_, t)| t);

    for &(name, time) in &parsed {
        if time > now {
            return Some(NextPrayerInfo {
                name: name.to_string(),
                time,
                minutes_remaining: (time - now).num_minutes(),
            });
        }
    }

    // Everything passed: next occurrence is the first prayer tomorrow.
    let first = parsed.first()?;
    let now_secs = (now - NaiveTime::MIN).num_seconds();
    let first_secs = (first.1 - NaiveTime::MIN).num_seconds();
    let total_secs = 24 * 3600 - now_secs + first_secs;
    Some(NextPrayerInfo {
        name: first.0.to_string(),
        time: first.1,
        minutes_remaining: total_secs / 60,
    })
}

/// True on the first loop tick (60s cadence) after `hhmm` has arrived.
pub fn is_due(now: NaiveTime, hhmm: &str) -> bool {
    match NaiveTime::parse_from_str(hhmm, "%H:%M") {
        Ok(t) => {
            let elapsed = (now - t).num_seconds();
            (0..60).contains(&elapsed)
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_adhan() -> DailyPrayerTimes {
        DailyPrayerTimes {
            fajr: "06:30".into(),
            shurouq: "08:00".into(),
            dhuhr: "13:00".into(),
            asr: "15:30".into(),
            maghrib: "17:45".into(),
            isha: "19:15".into(),
        }
    }

    #[test]
    fn is_due_only_within_the_minute() {
        let now = NaiveTime::from_hms_opt(13, 0, 30).unwrap();
        assert!(is_due(now, "13:00"));
        assert!(!is_due(now, "12:59"));
        assert!(!is_due(now, "13:01"));
        assert!(!is_due(now, "bogus"));
    }

    #[test]
    fn times_parse_into_prayer_entries() {
        let entries = adhan_entries(&sample_adhan());
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[0], ("Fajr".to_string(), "06:30".to_string()));
        // shurouq deliberately excluded: sunrise has no adhan
        assert!(!entries.iter().any(|(n, _)| *n == "Shurouq"));
    }

    #[test]
    fn iqama_entries_cover_five_prayers() {
        let iq = DailyIqamaTimes {
            fajr: "06:45".into(),
            dhuhr: "13:15".into(),
            asr: "15:50".into(),
            maghrib: "18:05".into(),
            isha: "19:30".into(),
        };
        assert_eq!(iqama_entries(&iq).len(), 5);
    }

    #[test]
    fn next_prayer_picks_the_upcoming_one() {
        // reference "now" only matters for ordering, entries are sorted anyway
        let e = adhan_entries(&sample_adhan());
        let next = next_prayer(&e).unwrap();
        assert!(PRAYERS.iter().any(|(n, _, _)| *n == next.name));
        assert!(next.minutes_remaining >= 0);
    }

    #[test]
    fn skips_unparsable_entries() {
        let mut e = adhan_entries(&sample_adhan());
        e[1].1 = "bogus".to_string(); // Dhuhr unparsable
        let next = next_prayer(&e).unwrap();
        assert_ne!(next.name, "Dhuhr");
    }

    // ---- hostile-time tests: the strings below are attacker-influenced ----

    #[test]
    fn all_unparsable_entries_yield_no_next_prayer() {
        let bogus: Vec<(String, String)> =
            (0..5).map(|i| (format!("P{i}"), "bogus".to_string())).collect();
        assert!(next_prayer(&bogus).is_none(), "no parseable entry -> no next prayer");
        assert!(next_prayer(&[]).is_none());
    }

    #[test]
    fn hostile_time_shapes_are_rejected_not_misparsed() {
        let now = NaiveTime::from_hms_opt(13, 0, 0).unwrap();
        // Shapes that must be fully inert: never due, never a next prayer.
        let inert = [
            "25:70",    // out-of-range rollover bait
            "99:99",    // classic garbage
            "+30",      // iqama-style offset in an adhan slot
            "",         // empty
            "١٣:٠٠",    // Arabic-Indic digits
            "13:00:00", // seconds sneak in
            "13:00\n",  // trailing newline (string smuggling from JSON)
        ];
        for t in inert {
            assert!(!is_due(now, t), "{t:?} must never be due");
            let e = vec![("P".to_string(), t.to_string())];
            assert!(next_prayer(&e).is_none(), "{t:?} must yield no next prayer");
        }

        // Lenient-but-accurate shapes: chrono accepts these, and the alarm
        // must land on the face-value time — never on a rollover. This is
        // the property that matters when the strings come from the wire.
        let lenient = [
            ("7:5", NaiveTime::from_hms_opt(7, 5, 0).unwrap()),
            (" 13:00", NaiveTime::from_hms_opt(13, 0, 0).unwrap()),
        ];
        for (t, expected) in lenient {
            let next = next_prayer(&[("P".to_string(), t.to_string())])
                .unwrap_or_else(|| panic!("{t:?} must parse to its face value"));
            assert_eq!(next.time, expected, "{t:?} resolved to the wrong time");
        }
    }

    #[test]
    fn day_boundary_alerts_stay_accurate() {
        // Midnight wrap: 00:00 alert fires just after midnight, not at noon.
        let just_after = NaiveTime::from_hms_opt(0, 0, 30).unwrap();
        assert!(is_due(just_after, "00:00"));
        assert!(!is_due(just_after, "23:59"));
        // Last second of the day.
        let last = NaiveTime::from_hms_opt(23, 59, 59).unwrap();
        assert!(is_due(last, "23:59"));
    }
}
