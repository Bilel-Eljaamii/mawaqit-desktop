//! Hostile semantics: attacks that use *valid* JSON and *valid* structures to
//! corrupt behavior instead of crashing it. The panic-safety layer
//! (`hostile_corpus.rs`, `cargo fuzz`) already proves garbage can't break the
//! parser; this suite proves garbage can't lie either.
//!
//! Everything goes through [`mawaqit_api::parse_page`] — the same entry point
//! network data takes — never through a direct serde deserialize, which is
//! not what a hostile response hits.
//!
//! Always-run tests pin contracts that hold today. `#[ignore = "RED TEAM
//! FINDING F#…"]` tests pin secure contracts that do NOT hold yet — each is
//! a finding; run them with `cargo test -p mawaqit-api -- --ignored`.

use chrono::Datelike;
use mawaqit_api::{month_times, page_url, parse_page, ConfData};
use serde_json::json;

fn conf(value: serde_json::Value) -> ConfData {
    let page = format!("<html><script>var confData = {value};</script></html>");
    parse_page(&page, "redteam").expect("test builds structurally valid confData")
}

/// 2026-01-01 — the calendar fixtures below define day 1 of month 1.
fn the_date() -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()
}

fn valid_row() -> Vec<String> {
    ["06:30", "08:00", "13:00", "15:30", "17:45", "19:15"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn is_valid_hhmm(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 5
        && b[2] == b':'
        && b[..2].iter().all(|c| c.is_ascii_digit())
        && b[3..].iter().all(|c| c.is_ascii_digit())
        && s[..2].parse::<u8>().map(|h| h < 24).unwrap_or(false)
        && s[3..].parse::<u8>().map(|m| m < 60).unwrap_or(false)
}

// ------------------------------------------------------------ contract tests

/// Documented inference: imsak mode is decided by `times.len() == 6` alone,
/// at the scraper boundary. The calendar row shape is an independent,
/// inherently ambiguous signal — this pins the single-source rule so it
/// cannot drift silently.
#[test]
fn imsak_mode_is_decided_by_times_count_alone() {
    for (times_len, imsak) in [(5usize, false), (6, true), (7, false)] {
        let c = conf(json!({
            "times": vec!["06:30"; times_len],
            "calendar": [ { "1": valid_row() } ],
        }));
        assert_eq!(c.imsak_mode, imsak, "times.len() == {times_len}");
        // Whatever the mode, the calendar pipeline still works.
        let _ = month_times(&c, the_date().month()).unwrap();
        let _ = mawaqit_api::times_for_date(&c, the_date()).unwrap();
    }

    // Fewer than 5 time strings: the real boundary refuses the page outright.
    let page = format!(
        "<script>var confData = {}; </script>",
        json!({ "times": ["06:30", "07:00"], "calendar": [ { "1": valid_row() } ] })
    );
    assert!(parse_page(&page, "redteam").is_err(), "short `times` is a hard error");
}

/// "+N" offsets that roll into the next day still resolve to a valid HH:MM
/// string (next-day semantics are inherent to HH:MM display; the frontend
/// handles the rollover via its own event builder).
#[test]
fn iqama_offset_rollover_stays_valid_hhmm() {
    let c = conf(json!({
        "times": ["23:30", "00:00", "00:00", "00:00", "00:00"],
        "calendar": [ { "1": ["23:30","23:45","23:50","23:55","23:58","23:59"] } ],
        "iqamaCalendar": [ { "1": ["+600", "+600", "+600", "+600", "+600"] } ],
    }));
    let today = mawaqit_api::times_for_date(&c, the_date()).unwrap();
    let iq = today.iqama.expect("iqama present");
    for t in [&iq.fajr, &iq.dhuhr, &iq.asr, &iq.maghrib, &iq.isha] {
        assert!(is_valid_hhmm(t), "{t} is not valid HH:MM");
    }
    assert_eq!(iq.fajr, "09:30"); // 23:30 + 600 min, next-day wall clock
}

/// Day keys are strings from the wire: exotic keys are skipped, never parsed
/// as something else, never counted as days.
#[test]
fn exotic_month_keys_are_skipped() {
    let mut month = std::collections::BTreeMap::new();
    for key in ["١", "٣١", "1e2", "4294967296", " 1", "1 ", "1.0", "0x1", "٣"] {
        month.insert(key.to_string(), valid_row());
    }
    let c = conf(json!({
        "times": ["06:30", "08:00", "13:00", "15:30", "17:45"],
        "calendar": [month],
    }));
    let days = month_times(&c, 1).unwrap().days;
    assert!(days.is_empty(), "no exotic key counts as a day, got {days:?}");
}

/// A calendar full of structurally-broken rows fails safe: empty month for
/// `month_times`, hard `NoCalendar` error for `times_for_date`.
#[test]
fn all_broken_rows_fail_safe() {
    let row = vec!["garbage".to_string(); 3];
    let month: std::collections::BTreeMap<String, Vec<String>> =
        (1..=31).map(|d| (d.to_string(), row.clone())).collect();
    let c = conf(json!({
        "times": ["06:30", "08:00", "13:00", "15:30", "17:45"],
        "calendar": [month],
    }));
    assert!(month_times(&c, 1).unwrap().days.is_empty());
    assert!(mawaqit_api::times_for_date(&c, the_date()).is_err());
}

/// Non-string scalars in display fields collapse to None at the scraper
/// boundary; they never fail the page and never surface as "null"/"[object]"
/// strings.
#[test]
fn non_string_display_fields_become_none() {
    let c = conf(json!({
        "times": ["06:30", "08:00", "13:00", "15:30", "17:45"],
        "calendar": [ { "1": valid_row() } ],
        "name": 12345,
        "jumua": { "x": 1 },
        "jumua2": [1, 2],
        "image": true,
        "shuruq": 7.5,
    }));
    assert!(c.name.is_none());
    assert!(c.jumua.is_none());
    assert!(c.jumua2.is_none());
    assert!(c.image.is_none());
    assert!(c.shuruq.is_none());
}

/// Every combination of times-count × calendar-row-shape survives the whole
/// pipeline without panicking (redundant with the fuzz corpus, but stated as
/// an explicit matrix so a regression in one cell is named, not discovered).
#[test]
fn layout_confusion_matrix_never_panics() {
    let rows: Vec<Vec<String>> = vec![
        vec![],                                            // 0
        ["06:30"].iter().map(|s| s.to_string()).collect(), // 1
        valid_row(),                                       // 6 (normal)
        ["05:27", "06:37", "07:07", "13:21", "16:37", "19:24", "20:51"]
            .iter()
            .map(|s| s.to_string())
            .collect(), // 7 (imsak)
        vec![
            "06:30".into(),
            "xx:yy".into(),
            "13:00".into(),
            "15:30".into(),
            "17:45".into(),
            "19:15".into(),
        ], // broken middle
    ];
    for times_len in [5usize, 6, 7] {
        for row in &rows {
            let page = format!(
                "<script>var confData = {}; </script>",
                json!({
                    "times": vec!["06:30"; times_len],
                    "calendar": [ { "1": row } ],
                    "iqamaCalendar": [ { "1": row } ],
                })
            );
            if let Ok(c) = parse_page(&page, "redteam") {
                let _ = month_times(&c, 1);
                let _ = mawaqit_api::month_iqama_times(&c, 1);
                let _ = mawaqit_api::times_for_date(&c, the_date());
            }
        }
    }
}

/// The URL builder keeps benign slugs inside the mosque page namespace.
#[test]
fn page_url_is_well_formed_for_benign_slugs() {
    let url = page_url("https://mawaqit.net", "grande-mosquee-de-paris");
    assert_eq!(url, "https://mawaqit.net/en/grande-mosquee-de-paris");
}

// ------------------------------------------------- findings (currently red)

/// FINDING F4 — surfaced times are never validated. `daily_from_row` passes
/// adhan strings through verbatim; "25:70" at the fajr position reaches the
/// UI, where the frontend's `setHours(25, 70)` silently rolls into another
/// day and the countdown points at a made-up time. FIXED: a row surfacing
/// any non-strict-HH:MM value is rejected whole, so the day drops out of the
/// month (the same "the day errors out instead of showing fabricated times"
/// semantic as layout-mismatched rows). The original probe expected the
/// hostile day to resolve garbage-free — impossible without fabricating a
/// fajr — so it now asserts the day refuses to resolve while its valid
/// neighbor still surfaces.
#[test]
fn finding_f4_surfaced_times_are_always_valid_hhmm() {
    let c = conf(json!({
        "times": ["25:70", "99:99", "ab:cd", "7:5", "+30"],
        "calendar": [ { "1": ["25:70","06:37","13:21","16:37","19:24","20:51"],
                         "2": ["06:30","08:00","13:00","15:30","17:45","19:15"] } ],
    }));

    // The hostile day must not resolve at all.
    assert!(
        mawaqit_api::times_for_date(&c, the_date()).is_err(),
        "a day surfacing an invalid time must not resolve"
    );

    // Its valid neighbor is untouched and surfaces only valid times.
    let day2 = chrono::NaiveDate::from_ymd_opt(2026, 1, 2).unwrap();
    let today = mawaqit_api::times_for_date(&c, day2).expect("valid day resolves");
    for t in [
        today.adhan.fajr.as_str(),
        today.adhan.shurouq.as_str(),
        today.adhan.dhuhr.as_str(),
        today.adhan.asr.as_str(),
        today.adhan.maghrib.as_str(),
        today.adhan.isha.as_str(),
    ] {
        assert!(is_valid_hhmm(t), "surfaced {t:?} is not a valid HH:MM time");
    }

    // The month view carries exactly the surviving day.
    let days = month_times(&c, 1).unwrap().days;
    assert_eq!(days.iter().map(|d| d.day).collect::<Vec<_>>(), vec![2]);

    // The iqama passthrough cannot smuggle non-display times either: garbage
    // falls back to the adhan value like it always has.
    let hostile_iq = conf(json!({
        "times": ["06:30", "08:00", "13:00", "15:30", "17:45", "19:15"],
        "calendar": [ { "1": ["06:30","08:00","13:00","15:30","17:45","19:15"] } ],
        "iqamaCalendar": [ { "1": ["25:70","06:45","13:20","+20","18:00"] } ],
    }));
    let today = mawaqit_api::times_for_date(&hostile_iq, the_date()).expect("resolves");
    let iq = today.iqama.expect("iqama present");
    for t in [iq.fajr.as_str(), iq.dhuhr.as_str(), iq.asr.as_str(), iq.maghrib.as_str(), iq.isha.as_str()] {
        assert!(is_valid_hhmm(t), "surfaced iqama {t:?} is not a valid HH:MM time");
    }
    assert_eq!(iq.fajr, today.adhan.fajr, "hostile iqama fell back to adhan");
}

/// FINDING F5 — lenient day keys duplicate days. Rust's integer FromStr
/// accepts "01" and "+1", so a hostile month carrying "1", "01" and "+1"
/// yields three entries for day 1; `times_for_date` then silently picks
/// whichever sorts first (BTreeMap order, not data quality).
/// FIX: dedupe by parsed day in `month_times` (skip duplicates), then
/// un-ignore.
#[test]
#[ignore = "RED TEAM FINDING F5: duplicate day keys are not deduplicated"]
fn finding_f5_duplicate_day_keys_yield_one_day() {
    let mut month = std::collections::BTreeMap::new();
    month.insert("1".to_string(), valid_row());
    month.insert("01".to_string(), vec!["00:00".to_string(); 6]); // attacker's
                                                                  // row
    month.insert("+1".to_string(), vec!["12:34".to_string(); 6]); // attacker's
                                                                  // row
    let c = conf(json!({
        "times": ["06:30", "08:00", "13:00", "15:30", "17:45"],
        "calendar": [month],
    }));
    let days = month_times(&c, 1).unwrap().days;
    assert_eq!(days.len(), 1, "day 1 appears {} times", days.len());
}

/// FINDING F6 — control and bidi-override characters flow verbatim into
/// strings that end up in the window title, the tray tooltip and OS
/// notifications (`Mawaqit: <name>`). U+202E can visually reverse that text
/// (spoofed tray state), C0 controls corrupt terminal logs.
/// FIX: strip C0/C1 + bidi controls at the `ConfData` boundary, then
/// un-ignore.
#[test]
#[ignore = "RED TEAM FINDING F6: control/bidi characters reach display strings"]
fn finding_f6_display_strings_carry_no_control_or_bidi_characters() {
    let evil = "\u{202E}esreveR\u{202D}\u{0000}\u{007F}\n\t";
    let c = conf(json!({
        "times": ["06:30", "08:00", "13:00", "15:30", "17:45"],
        "calendar": [ { "1": valid_row() } ],
        "name": evil,
        "jumua": evil,
    }));
    let today = mawaqit_api::times_for_date(&c, the_date()).unwrap();
    let strings: Vec<&str> =
        [c.name.as_deref(), c.jumua.as_deref(), Some(today.adhan.fajr.as_str())]
            .into_iter()
            .flatten()
            .collect();
    for s in strings {
        assert!(
            !s.chars().any(|ch| ch.is_control() || matches!(ch,
                '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}')),
            "display string carries hostile control/bidi characters: {s:?}"
        );
    }
}
