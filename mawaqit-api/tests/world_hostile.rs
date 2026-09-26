//! Hostile integration test: hammer the client against real mosques across
//! five continents and assert structural invariants hold everywhere.
//!
//! Real-world mosque data is hostile: layouts differ (normal / imsak /
//! legacy), fields are missing, some search slugs point to pages that no
//! longer exist, and some mosques enter malformed times. This test hard-fails
//! on anything that would break the app (fetch/parse errors, invalid values,
//! panics) and reports data-quality anomalies without failing on them.
//!
//! The fixture is hand-curated: only mosques with a working public page are
//! included, and already-covered developer mosques are left out.
//!
//! Run (hits the live site, takes a few minutes):
//!   cargo test -p mawaqit-api --test world_hostile -- --ignored --nocapture

use mawaqit_api::MawaqitClient;
use serde::Deserialize;

#[derive(Deserialize)]
struct WorldMosque {
    continent: String,
    city: String,
    name: String,
    slug: String,
}

const FIXTURE: &str = include_str!("fixtures/world_mosques.json");

fn valid_hhmm(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 5 || bytes[2] != b':' {
        return false;
    }
    let (h, m) = (s[..2].parse::<u8>(), s[3..].parse::<u8>());
    matches!((h, m), (Ok(h), Ok(m)) if h < 24 && m < 60)
}

fn to_minutes(s: &str) -> Option<i64> {
    let h: i64 = s[..2].parse().ok()?;
    let m: i64 = s[3..].parse().ok()?;
    Some(h * 60 + m)
}

#[tokio::test]
#[ignore = "hits the live mawaqit.net site for 100+ mosques"]
async fn hostile_world_tour() {
    let mosques: Vec<WorldMosque> =
        serde_json::from_str(FIXTURE).expect("world fixture parses");
    assert!(mosques.len() >= 100, "fixture should hold 100+ mosques");
    assert!(
        mosques.iter().all(|m| !m.slug.is_empty()),
        "every fixture entry must carry a slug"
    );

    let client = MawaqitClient::new();
    let mut dead_slugs = Vec::new();
    let mut failures = Vec::new();
    let mut anomalies = Vec::new();
    let mut parsed_count = 0usize;
    let mut iqama_count = 0usize;

    for (i, m) in mosques.iter().enumerate() {
        // One shared client; sleep a little to stay polite with the site.
        if i % 10 == 9 {
            std::thread::sleep(std::time::Duration::from_millis(300));
        }

        let conf = match client.conf_data(&m.slug).await {
            Ok(c) => c,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("not found") || msg.contains("404") {
                    dead_slugs.push(format!("{} ({})", m.slug, m.city));
                } else {
                    failures.push(format!("{} [{}]: fetch: {e}", m.slug, m.city));
                }
                continue;
            }
        };

        // 1. The year calendar must have all 12 months.
        if conf.calendar.len() != 12 {
            failures.push(format!(
                "{}: calendar has {} months",
                m.slug,
                conf.calendar.len()
            ));
            continue;
        }

        // 2. Today must parse with fully valid, ordered times.
        let today =
            match mawaqit_api::times_for_date(&conf, chrono::Local::now().date_naive()) {
                Ok(t) => t,
                Err(e) => {
                    failures.push(format!("{} [{}]: today: {e}", m.slug, m.city));
                    continue;
                }
            };
        let a = &today.adhan;
        let times = [
            (&a.fajr, "fajr"),
            (&a.shurouq, "shurouq"),
            (&a.dhuhr, "dhuhr"),
            (&a.asr, "asr"),
            (&a.maghrib, "maghrib"),
            (&a.isha, "isha"),
        ];
        if let Some((name, v)) = times.iter().find(|(v, _)| !valid_hhmm(v)) {
            failures.push(format!("{}: today {name} is not HH:MM: {v:?}", m.slug));
            continue;
        }

        // 3. Soft sanity: sunrise between fajr and dhuhr, prayers ascending.
        let (f, sh, dh, asr, mg, is) = (
            to_minutes(&a.fajr),
            to_minutes(&a.shurouq),
            to_minutes(&a.dhuhr),
            to_minutes(&a.asr),
            to_minutes(&a.maghrib),
            to_minutes(&a.isha),
        );
        if let (Some(f), Some(sh), Some(dh)) = (f, sh, dh) {
            if sh < f || sh > dh {
                anomalies
                    .push(format!("{}: sunrise {sh} min outside [{f},{dh}]", m.slug));
            }
        }
        for (label, early, late) in
            [("dhuhr<asr", dh, asr), ("asr<maghrib", asr, mg), ("maghrib<isha", mg, is)]
        {
            if let (Some(e), Some(l)) = (early, late) {
                if e >= l {
                    anomalies.push(format!("{}: {label} violated", m.slug));
                }
            }
        }

        // 4. Iqama, when present, must resolve to five valid HH:MM times.
        if let Some(iq) = &today.iqama {
            let iq_times = [
                (&iq.fajr, "fajr"),
                (&iq.dhuhr, "dhuhr"),
                (&iq.asr, "asr"),
                (&iq.maghrib, "maghrib"),
                (&iq.isha, "isha"),
            ];
            if let Some((name, v)) = iq_times.iter().find(|(v, _)| !valid_hhmm(v)) {
                failures.push(format!("{}: iqama {name} is not HH:MM: {v:?}", m.slug));
                continue;
            }
            iqama_count += 1;
        }

        // 5. Month extraction must work on both ends of the year.
        for month in [1u32, 12] {
            if let Err(e) = client.month(&m.slug, month).await {
                failures.push(format!("{}: month {month}: {e}", m.slug));
            }
        }

        parsed_count += 1;
        let label = format!(
            "{} — {} [{}, {}]",
            conf.name.as_deref().unwrap_or("?"),
            m.slug,
            m.city,
            m.continent
        );
        if i % 20 == 0 || i == mosques.len() - 1 {
            println!("{:>3}/{} ✓ {label}", i + 1, mosques.len());
        }
    }

    println!();
    println!("================ hostile world tour summary ================");
    println!("mosques in fixture : {}", mosques.len());
    println!("fully parsed       : {parsed_count}");
    println!("with iqama data    : {iqama_count}");
    println!("dead slugs (404)   : {}", dead_slugs.len());
    for d in &dead_slugs {
        println!("   dead: {d}");
    }
    println!("data anomalies     : {}", anomalies.len());
    for a in anomalies.iter().take(10) {
        println!("   odd : {a}");
    }
    println!("HARD failures      : {}", failures.len());
    for f in &failures {
        println!("   FAIL: {f}");
    }

    // Dead slugs are an upstream data-quality issue (search indexes pages
    // that no longer exist); tolerate a few, but the app must never fail to
    // parse or crash on real mosque data.
    assert!(
        dead_slugs.len() * 10 <= mosques.len(),
        "more than 10% of search slugs are dead"
    );
    assert!(
        failures.is_empty(),
        "{} mosques failed hostile parsing (see FAIL lines above)",
        failures.len()
    );
}
