use serde_json::Value;

use crate::{
    error::{MawaqitError, Result},
    models::{Announcement, ConfData, RawCalendar},
};

/// Extract the `confData` JavaScript object embedded in a mosque page.
///
/// The public page (`https://mawaqit.net/{lang}/{slug}`) ships its whole
/// configuration — daily times, year calendar, iqama calendar, mosque
/// metadata — as one JSON literal assigned to a `confData` variable.
pub fn extract_conf_data(page_html: &str, mosque_id: &str) -> Result<ConfData> {
    let json_str = find_conf_data_json(page_html)
        .ok_or_else(|| MawaqitError::ConfDataNotFound(mosque_id.to_string()))?;

    let value: Value = serde_json::from_str(json_str)
        .map_err(|e| MawaqitError::Parse(format!("confData: {e}")))?;

    let times: Vec<String> = value["times"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    if times.len() < 5 {
        return Err(MawaqitError::Parse(format!(
            "confData.times has {} entries, expected 5",
            times.len()
        )));
    }

    let calendar: RawCalendar =
        serde_json::from_value(value["calendar"].clone()).unwrap_or_default();
    if calendar.is_empty() {
        return Err(MawaqitError::NoCalendar);
    }
    let iqama_calendar = parse_calendar(&value, "iqamaCalendar").ok();

    let announcements: Vec<Announcement> = value["announcements"]
        .as_array()
        .map(|a| {
            a.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect()
        })
        .unwrap_or_default();

    Ok(ConfData {
        name: value["name"].as_str().map(str::to_string),
        jumua: value["jumua"].as_str().map(str::to_string),
        jumua2: value["jumua2"].as_str().map(str::to_string),
        image: value["image"].as_str().map(str::to_string),
        shuruq: value["shuruq"].as_str().map(str::to_string),
        imsak_mode: times.len() == 6,
        times,
        calendar,
        iqama_calendar,
        announcements,
        raw: value,
    })
}

/// Find the JSON literal assigned to `confData` in any script of the page,
/// scanning from the opening `{` to its balanced closing brace (string- and
/// escape-aware, so `;` or braces inside JSON strings don't break it).
/// Every `confData` mention is tried until one is an actual assignment.
fn find_conf_data_json(html: &str) -> Option<&str> {
    let mut cursor = 0;
    while let Some(offset) = html[cursor..].find("confData") {
        let start = cursor + offset + "confData".len();
        // The assignment operator and whitespace between the marker and the
        // literal; anything else means this is some other `confData` mention.
        let rest = html[start..].trim_start();
        if let Some(rest) = rest.strip_prefix('=') {
            let rest = rest.trim_start();
            if let Some(json) = balanced_json(rest) {
                return Some(json);
            }
        }
        cursor = start;
    }
    None
}

/// Length of the balanced `{...}` JSON literal at the start of `s`.
fn balanced_json(s: &str) -> Option<&str> {
    if !s.starts_with('{') {
        return None;
    }
    let bytes = s.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_calendar(value: &Value, key: &str) -> Result<RawCalendar> {
    serde_json::from_value::<RawCalendar>(value[key].clone())
        .map_err(|e| MawaqitError::Parse(format!("{key}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_page() -> String {
        let conf = r#"{"times":["06:09","07:41","13:47","16:58","19:45"],"shuruq":"07:41","calendar":[{"1":["07:05","08:44","12:59","14:48","17:08","18:35"]}],"iqamaCalendar":[{"1":["+8","+8","+8","+0","+8"]}],"name":"GRANDE MOSQUÉE DE PARIS","jumua":"13:50","jumua2":"14:30","announcements":[{"id":1,"title":"Hello"}]}"#;
        format!(
            "<html><head><script>var a=1;</script></head><body>\
             <script>var confData = {conf};</script></body></html>"
        )
    }

    #[test]
    fn extracts_conf_data_from_page() {
        let conf = extract_conf_data(&sample_page(), "grande-mosquee-de-paris").unwrap();
        assert_eq!(conf.name.as_deref(), Some("GRANDE MOSQUÉE DE PARIS"));
        assert_eq!(conf.times.len(), 5);
        assert_eq!(conf.shuruq.as_deref(), Some("07:41"));
        assert_eq!(conf.calendar.len(), 1);
        assert_eq!(conf.iqama_calendar.as_ref().unwrap().len(), 1);
        assert_eq!(conf.jumua.as_deref(), Some("13:50"));
        assert_eq!(conf.announcements.len(), 1);
    }

    #[test]
    fn handles_multiline_and_semicolons_in_strings() {
        let conf = "{\n  \"times\": [\"06:09\", \"07:41\", \"13:47\", \"16:58\", \"19:45\"],\n  \"calendar\": [{\"1\": [\"06:09\", \"07:41\", \"13:47\", \"16:58\", \"19:45\", \"21:12\"]}],\n  \"image\": \"https://x.test/a.jpg?q=1;s=2\"\n}";
        let page = format!("<script>\n  var x = 0;\n  let confData =\n    {conf};\n  alert(x);\n</script>");
        let conf = extract_conf_data(&page, "x").unwrap();
        assert_eq!(conf.times.len(), 5);
        assert_eq!(conf.calendar.len(), 1);
        assert!(conf.raw["image"].as_str().unwrap().contains(';'));
    }

    #[test]
    fn ignores_conf_data_mentions_that_are_not_assignments() {
        let times = r#""times":["01:01","01:01","01:01","01:01","01:01"]"#;
        let calendar =
            r#""calendar":[{"1":["01:01","01:01","01:01","01:01","01:01","01:01"]}]"#;
        let page = format!(
            r#"<script>if (confData === undefined) {{}} var confData = {{{times},{calendar}}};</script>"#
        );
        let conf = extract_conf_data(&page, "x").unwrap();
        assert_eq!(conf.times.len(), 5);
    }

    #[test]
    fn missing_conf_data_is_an_error() {
        let page = "<html><script>var other = 1;</script></html>".to_string();
        let err = extract_conf_data(&page, "some-mosque").unwrap_err();
        assert!(matches!(err, MawaqitError::ConfDataNotFound(_)));
    }
}
