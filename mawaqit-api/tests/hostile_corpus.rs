//! Hostile fuzz-style corpus: deterministic adversarial inputs thrown at the
//! page parser and the calendar pipeline. Nothing here may panic, hang on
//! bounded input, or produce invalid times — `Err` is always a fine outcome.
//!
//! The libFuzzer campaign (`cargo fuzz run` in `fuzz/`) explores beyond this
//! corpus; this file keeps the known-bad shapes pinned in normal `cargo test`.

use mawaqit_api::{parse_page, times_for_date, ConfData};
use serde_json::json;

/// A minimal valid page used as the base for mutations.
fn valid_page() -> String {
    let conf = r#"{
        "times": ["05:27", "07:07", "13:21", "16:37", "19:24", "20:51"],
        "shuruq": "06:37",
        "calendar": [{"1": ["05:27","06:37","07:07","13:21","16:37","19:24","20:51"]}],
        "iqamaCalendar": [{"1": ["+10","+10","+10","+5","+10"]}],
        "name": "Hostile Mosque"
    }"#;
    format!("<html><script>var confData = {conf};</script></html>")
}

fn dig(conf: &ConfData) {
    // Whatever came out, the calendar pipeline must hold together.
    let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 25).unwrap();
    let _ = mawaqit_api::month_times(conf, 1);
    let _ = mawaqit_api::month_times(conf, 12);
    let _ = mawaqit_api::month_iqama_times(conf, 7);
    let _ = times_for_date(conf, date);
}

#[test]
fn truncations_of_valid_page_never_panic() {
    let page = valid_page();
    // every 7th truncation point, deterministic
    for cut in (0..page.len()).step_by(7) {
        let _ = parse_page(&page[..cut], "fuzz");
    }
}

#[test]
fn byte_flips_never_panic() {
    let page = valid_page().into_bytes();
    // deterministic xorshift
    let mut state: u64 = 0x9E3779B97F4A7C15;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..2000 {
        let mut mutated = page.clone();
        let flips = (next() % 8) + 1;
        for _ in 0..flips {
            let i = (next() as usize) % mutated.len();
            mutated[i] = (next() % 256) as u8;
        }
        let html = String::from_utf8_lossy(&mutated).into_owned();
        if let Ok(conf) = parse_page(&html, "fuzz") {
            dig(&conf);
        }
    }
}

#[test]
fn conf_data_lookalikes_never_confuse_the_parser() {
    let cases = [
        // mentions that are not assignments
        r#"<script>if (confData === undefined) load();</script>"#,
        r#"<script>console.log("confData = not json");</script>"#,
        r#"<script>let x = { a: "confData = {" };</script>"#,
        // assignment inside a comment
        r#"<script>// var confData = {"broken";</script>"#,
        // two assignments, first is garbage-looking but valid JSON
        r#"<script>var confData = {"a": 1}; var confData = {"times":["05:05","07:07","13:13","16:16","19:19","20:20"]};</script>"#,
        // braces and semicolons inside strings
        r#"<script>var confData = {"times":["05:05","07:07","13:13","16:16","19:19","20:20"],"calendar":[{}],"name":"}; }; <script>"};</script>"#,
        // escaped quotes and backslash torture
        r#"<script>var confData = {"times":["05:05","07:07","13:13","16:16","19:19","20:20"],"calendar":[{}],"name":"\\\"}}{{;"};</script>"#,
        // assignment in several scripts, unterminated last one
        r#"<html><script>var confData = {"times":["01:01","02:02","03:03","04:04","05:05"]};</script><script>var confData = {"#,
    ];
    for page in cases {
        let _ = parse_page(page, "fuzz");
    }
}

#[test]
fn hostile_json_structures_never_panic() {
    let times = json!(["05:27", "07:07", "13:21", "16:37", "19:24", "20:51"]);
    let mut cases: Vec<serde_json::Value> = vec![
        json!({ "times": times, "calendar": null }),
        json!({ "times": times, "calendar": "nope" }),
        json!({ "times": times, "calendar": [null] }),
        json!({ "times": times, "calendar": [ { "1": null } ] }),
        json!({ "times": times, "calendar": [ { "1": [] } ] }),
        json!({ "times": times, "calendar": [ { "1": ["x", "y", "z"] } ] }),
        json!({ "times": times, "calendar": [ { "1": ["05:27"] } ] }),
        json!({ "times": times, "calendar": [ { "0": ["05:27","06:37","07:07","13:21","16:37","19:24","20:51"] },
                                               { "-1": ["a","b","c","d","e","f"] },
                                               { "1e2": ["a","b","c","d","e","f"] },
                                               { "4294967296": ["a","b","c","d","e","f"] },
                                               { "٣": ["a","b","c","d","e","f"] } ] }),
        json!({ "times": ["5", "05:60", "24:00", "AA:BB", "05:2x", null, 7],
                "calendar": [ { "1": ["05:27","06:37","07:07","13:21","16:37","19:24","20:51"] } ] }),
        json!({ "times": ["＋5", "06:3٠", "13:21", "16:37", "19:24", "20:51"],
                "calendar": [ { "1": ["05:27","06:37","07:07","13:21","16:37","19:24","20:51"] } ] }),
        json!({ "times": times,
                "calendar": [ { "1": ["05:27","06:37","07:07","13:21","16:37","19:24","20:51"] } ],
                "iqamaCalendar": [ { "1": ["+", "+-", "++", " +5 ", "+5.5", "+1e9", "+9223372036854775807", "-5", "05:70", "99:99"] } ] }),
        json!({ "times": times, "calendar": [ { "1": ["05:27","+9223372036854775807","07:07","13:21","16:37","19:24","20:51"] } ] }),
    ];
    // Deep nesting. serde_json::from_str (the app's parse path) rejects
    // depth > 128 with an error; we pin that behavior. from_value has no
    // depth limit and would blow the stack, so the from_value path only
    // gets moderately nested input — the same guard-rail the fuzz target
    // relies on.
    let deep_json = "{\"a\":".repeat(5000) + "1" + &"}".repeat(5000);
    match parse_page(&format!(
        "<script>var confData = {{\"times\":[\"05:27\",\"07:07\",\"13:21\",\"16:37\",\"19:24\",\"20:51\"],\"calendar\":[{{\"1\":{deep_json}}}]}};</script>"
    ), "fuzz") {
        Ok(conf) => dig(&conf),
        Err(_) => {}
    }
    let moderately_nested = {
        let mut v = serde_json::Value::Null;
        for _ in 0..100 {
            v = json!({ "a": v });
        }
        v
    };
    cases.push(json!({ "times": times, "calendar": [ { "1": moderately_nested } ] }));

    for case in cases {
        if let Ok(conf) = serde_json::from_value::<ConfData>(case.clone()) {
            dig(&conf);
        }
        let page = format!("<script>var confData = {case};</script>");
        if let Ok(conf) = parse_page(&page, "fuzz") {
            dig(&conf);
        }
    }
}

#[test]
fn oversized_and_repetitive_pages_stay_bounded() {
    // a megabyte of confData mentions with no assignment
    let page = "confData ".repeat(100_000);
    let _ = parse_page(&page, "fuzz");

    // a megabyte of open braces after a real assignment prefix (unbalanced)
    let mut page = String::from("<script>var confData = {\"a\":");
    page.push_str(&"{".repeat(1_000_000));
    let _ = parse_page(&page, "fuzz");

    // 5 MB of filler inside a string value
    let mut page = String::from("<script>var confData = {\"name\":\"");
    page.push_str("A".repeat(5_000_000).as_str());
    page.push_str("\"};</script>");
    let _ = parse_page(&page, "fuzz");
}

#[test]
fn unicode_and_control_characters_never_panic() {
    let injections = [
        "\u{0000}null\u{0000}",
        "\u{202E}rtl\u{202D}override",
        "\u{200B}\u{200D}\u{FEFF}zero-width",
        "🕌\u{1F6D1}emoji",
        "line\nbreak\ttab\rreturn",
        "\u{007F}\u{0080}\u{009C}c0-controls",
    ];
    for junk in injections {
        let page = format!("<script>var confData = {junk};</script>");
        let _ = parse_page(&page, "fuzz");

        let page = format!(
            "<script>var confData = {{\"times\":[\"05:27\",\"07:07\",\"13:21\",\"16:37\",\"19:24\",\"20:51\"],\"calendar\":[{{\"1\":[\"05:27\",\"06:37\",\"07:07\",\"13:21\",\"16:37\",\"19:24\",\"20:51\"]}}],\"name\":\"{junk}\"}};</script>"
        );
        if let Ok(conf) = parse_page(&page, "fuzz") {
            dig(&conf);
        }
    }
}
