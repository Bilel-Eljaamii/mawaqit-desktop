#![no_main]
//! Fuzz the HTML page parser end-to-end: arbitrary bytes in, and when the
//! parse succeeds the full calendar pipeline must run without panicking.

use libfuzzer_sys::fuzz_target;
use mawaqit_api::{parse_page, times_for_date, ConfData};

fn dig(conf: &ConfData) {
    let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 25).unwrap();
    let _ = times_for_date(conf, date);
    for month in 1..=12u32 {
        let _ = mawaqit_api::month_times(conf, month);
        let _ = mawaqit_api::month_iqama_times(conf, month);
    }
}

fuzz_target!(|data: &[u8]| {
    let html = String::from_utf8_lossy(data);
    if let Ok(conf) = parse_page(&html, "fuzz") {
        dig(&conf);
    }
});
