//! Print the times the client fetches for a mosque, to compare by eye with
//! the mawaqit.net page of the same mosque.
//!
//! Usage: cargo run -p mawaqit-api --example times -- <slug> [month]
//! Example: cargo run -p mawaqit-api --example times -- grande-mosquee-de-paris

use chrono::Datelike;
use mawaqit_api::MawaqitClient;

#[tokio::main]
async fn main() {
    let slug =
        std::env::args().nth(1).unwrap_or_else(|| "grande-mosquee-de-paris".to_string());
    let month: u32 = std::env::args().nth(2).and_then(|m| m.parse().ok()).unwrap_or(9);

    let client = MawaqitClient::new();

    let conf = client.conf_data(&slug).await.expect("failed to fetch mosque page");
    println!("mosque: {}  ({slug})", conf.name.as_deref().unwrap_or("?"));
    println!();

    let today = mawaqit_api::times_for_date(&conf, chrono::Local::now().date_naive())
        .expect("no times for today");
    println!("TODAY (from page confData, what the site displays):",);
    println!(
        "  Fajr {}  Shuruq {}  Dhuhr {}  Asr {}  Maghrib {}  Isha {}",
        today.adhan.fajr,
        today.adhan.shurouq,
        today.adhan.dhuhr,
        today.adhan.asr,
        today.adhan.maghrib,
        today.adhan.isha,
    );
    if let Some(iq) = &today.iqama {
        println!(
            "  iqama: Fajr {}  Dhuhr {}  Asr {}  Maghrib {}  Isha {}",
            iq.fajr, iq.dhuhr, iq.asr, iq.maghrib, iq.isha,
        );
    }
    println!(
        "  jumua: {} {}",
        conf.jumua.as_deref().unwrap_or("-"),
        conf.jumua2.as_deref().unwrap_or("")
    );
    println!();

    // Raw confData for the same day — the exact values embedded in the page.
    println!("RAW confData.times (site's today):  {:?}", conf.times);
    println!("RAW confData.shuruq:                {:?}", conf.shuruq);
    let d = chrono::Local::now();
    let day = conf.calendar[(d.month() - 1) as usize].get(&d.day().to_string()).cloned();
    println!("RAW calendar[{}][{}]:          {:?}", d.month(), d.day(), day);
    println!();

    match client.month(&slug, month).await {
        Ok(m) => {
            println!("MONTH {} — first 5 days:", month);
            for day in m.days.iter().take(5) {
                println!(
                    "  {:>2}: {} {} {} {} {} {}",
                    day.day,
                    day.times.fajr,
                    day.times.shurouq,
                    day.times.dhuhr,
                    day.times.asr,
                    day.times.maghrib,
                    day.times.isha,
                );
            }
        }
        Err(e) => println!("month fetch failed: {e}"),
    }
}
