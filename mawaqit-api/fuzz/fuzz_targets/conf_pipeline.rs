#![no_main]
//! Fuzz the typed model + calendar pipeline directly: arbitrary bytes are
//! interpreted as JSON confData (serde's parse path) and, when valid, pushed
//! through every calendar extraction.

use libfuzzer_sys::fuzz_target;
use mawaqit_api::{times_for_date, ConfData};

fn dig(conf: &ConfData) {
    let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 25).unwrap();
    let _ = times_for_date(conf, date);
    for month in 1..=12u32 {
        let _ = mawaqit_api::month_times(conf, month);
        let _ = mawaqit_api::month_iqama_times(conf, month);
    }
}

fuzz_target!(|data: &[u8]| {
    // serde_json::from_str keeps the app's 128-level recursion limit,
    // exactly like the networked client does.
    if let Ok(conf) = serde_json::from_slice::<ConfData>(data) {
        dig(&conf);
    }
});
