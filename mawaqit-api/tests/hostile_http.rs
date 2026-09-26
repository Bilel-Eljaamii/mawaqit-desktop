//! Hostile HTTP tests: the server on the other end is the attacker.
//!
//! `MawaqitClient` is pointed (via [`MawaqitClient::with_base_urls`]) at a
//! raw-TCP mock that answers with crafted responses: garbage JSON, lies about
//! content length, non-UTF8 bodies, redirects to attacker hosts, oversized
//! bodies. The client must always answer with a clean `Err` or well-formed
//! data — never panic, never hang, never follow the attacker somewhere else.
//!
//! Two kinds of tests live here:
//! - always-run contract tests: behavior that must stay true (green);
//! - `#[ignore = "RED TEAM FINDING F#…"]` tests: secure behavior the client
//!   does *not* have yet. Each documents a finding; un-ignore after fixing. Run
//!   them with `cargo test -p mawaqit-api -- --ignored --nocapture`.

use std::{
    collections::HashMap,
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
};

use mawaqit_api::{MawaqitClient, MawaqitError};

// ---------------------------------------------------------------- mock server

struct MockServer {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
}

/// Serve canned HTTP responses over raw TCP: route path (query stripped) ->
/// exact response bytes. Every request's full target is logged.
fn spawn_mock(routes: Vec<(&str, Vec<u8>)>) -> MockServer {
    let listener =
        TcpListener::bind("127.0.0.1:0").expect("mock binds an ephemeral port");
    let base = format!("http://{}", listener.local_addr().expect("local addr"));
    let routes: HashMap<String, Vec<u8>> =
        routes.into_iter().map(|(p, b)| (p.to_string(), b)).collect();
    let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let log = Arc::clone(&requests);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let target = match read_request_target(&mut stream) {
                Some(t) => t,
                None => continue,
            };
            if let Ok(mut log) = log.lock() {
                log.push(target.clone());
            }
            let path = target.split('?').next().unwrap_or("").to_string();
            let response = routes.get(&path).cloned().unwrap_or_else(http_404);
            let _ = stream.write_all(&response);
            let _ = stream.flush();
        }
    });

    MockServer { base, requests }
}

impl MockServer {
    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("mock log").clone()
    }

    fn client(&self) -> MawaqitClient {
        MawaqitClient::with_base_urls(self.base.clone(), self.base.clone())
    }
}

/// Read one HTTP request head, return the request target (path + query).
fn read_request_target(stream: &mut std::net::TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 64 * 1024 {
            break;
        }
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf);
    head.lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1).map(str::to_string))
}

fn http_bytes(status_line: &str, content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut r = format!("{status_line}\r\nContent-Type: {content_type}\r\n");
    r.push_str(&format!("Content-Length: {}\r\n", body.len()));
    r.push_str("Connection: close\r\n\r\n");
    let mut bytes = r.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

fn ok_json(body: &str) -> Vec<u8> {
    http_bytes("HTTP/1.1 200 OK", "application/json", body.as_bytes())
}

fn ok_html(body: &str) -> Vec<u8> {
    http_bytes("HTTP/1.1 200 OK", "text/html", body.as_bytes())
}

fn status_with_body(status_line: &str, body: &str) -> Vec<u8> {
    http_bytes(status_line, "text/plain", body.as_bytes())
}

fn redirect_to(location: &str) -> Vec<u8> {
    let mut r = String::from("HTTP/1.1 302 Found\r\n");
    r.push_str(&format!("Location: {location}\r\n"));
    r.push_str("Content-Length: 0\r\nConnection: close\r\n\r\n");
    r.into_bytes()
}

fn http_404() -> Vec<u8> {
    status_with_body("HTTP/1.1 404 Not Found", "nope")
}

/// A valid mosque page whose confData is completely attacker-authored.
fn trap_page() -> String {
    let conf = r#"{
        "times": ["00:00", "00:00", "00:00", "00:00", "00:00"],
        "calendar": [{"1": ["00:00","06:00","00:00","00:00","00:00","00:00"]}],
        "name": "EVIL TRAP MOSQUE",
        "image": "https://evil.test/pwn.jpg"
    }"#;
    format!("<html><script>var confData = {conf};</script></html>")
}

/// Minimal valid search result with one clean slug.
fn search_ok() -> String {
    r#"[{"slug":"grande-mosquee-de-paris","name":"Grande Mosquée","locality":"Paris","country":"France"}]"#.to_string()
}

// ------------------------------------------------------------ contract tests

#[tokio::test]
async fn search_garbage_bodies_are_errors_never_panics() {
    let bodies = [
        "[1,2,",                        // truncated
        "{\"mosques\": []}",            // object, not array
        "null",                         // wrong kind
        "\"array?\"",                   // wrong kind
        "12345",                        // wrong kind
        "",                             // empty body
        "<html>blocked</html>",         // WAF page
        "{\"slug\": 1}, {\"slug\": 2}", // half object
    ];
    for body in bodies {
        let mock = spawn_mock(vec![("/2.0/mosque/search", ok_json(body))]);
        let result = mock.client().search_mosques("paris").await;
        assert!(result.is_err(), "body {body:?} must not deserialize into mosques");
    }
}

#[tokio::test]
async fn search_error_statuses_surface_as_errors() {
    let cases = [
        ("HTTP/1.1 404 Not Found", true), // 404 is special: MosqueNotFound
        ("HTTP/1.1 403 Forbidden", false),
        ("HTTP/1.1 429 Too Many Requests", false),
        ("HTTP/1.1 500 Internal Server Error", false),
        ("HTTP/1.1 503 Service Unavailable", false),
    ];
    for (status_line, is_404) in cases {
        let mock =
            spawn_mock(vec![("/2.0/mosque/search", status_with_body(status_line, "no"))]);
        let err = mock.client().search_mosques("paris").await.unwrap_err();
        if is_404 {
            assert!(matches!(err, MawaqitError::MosqueNotFound(_)), "{err}");
        } else {
            assert!(matches!(err, MawaqitError::Api { .. }), "{err}");
        }
    }
}

#[tokio::test]
async fn search_bounded_input_stays_bounded() {
    // 10k entries of minimal (all-optional) mosques: large but legal — must
    // parse, since the 20 MB cap is what bounds hostility, not entry count.
    let body = format!("[{}]", vec!["{}"; 10_000].join(","));
    let mock = spawn_mock(vec![("/2.0/mosque/search", ok_json(&body))]);
    let mosques = mock.client().search_mosques("x").await.expect("10k entries parse");
    assert_eq!(mosques.len(), 10_000);

    // One hostile element anywhere in the array poisons the whole response —
    // that must be an Err, not a partial result.
    let poisoned = format!("[{},null]", "{}".repeat(100));
    let mock = spawn_mock(vec![("/2.0/mosque/search", ok_json(&poisoned))]);
    assert!(mock.client().search_mosques("x").await.is_err());
}

#[tokio::test]
async fn search_non_utf8_and_bom_bodies_are_rejected() {
    let mock = spawn_mock(vec![(
        "/2.0/mosque/search",
        http_bytes("HTTP/1.1 200 OK", "application/json", &[0xFF, 0xFE, 0x5B, 0x5D]),
    )]);
    let err = mock.client().search_mosques("x").await.unwrap_err();
    assert!(matches!(err, MawaqitError::Parse(_)), "{err}");

    let mock = spawn_mock(vec![(
        "/2.0/mosque/search",
        http_bytes("HTTP/1.1 200 OK", "application/json", b"\xEF\xBB\xBF[]"),
    )]);
    assert!(mock.client().search_mosques("x").await.is_err(), "BOM is not JSON");
}

#[tokio::test]
async fn search_query_is_percent_encoded_crlf_never_reach_the_wire() {
    // Classic request-smuggling probe: CRLF in the search word must never
    // terminate the request line early.
    let evil = "paris\r\nX-Injected: 1\r\n\r\nGET /admin HTTP/1.1";
    let mock = spawn_mock(vec![("/2.0/mosque/search", ok_json(&search_ok()))]);
    let _ = mock.client().search_mosques(evil).await;

    let requests = mock.requests();
    assert_eq!(requests.len(), 1, "one request, no smuggled second one: {requests:?}");
    let line = &requests[0];
    assert_eq!(line.lines().count(), 1, "target is a single line: {line:?}");
}

#[tokio::test]
async fn conf_page_hostile_xss_content_parses_structurally() {
    // The attacker fully controls confData. The parser's job is structure,
    // not sanitation: it may accept hostile strings, and the render layer
    // (frontend tests) must neutralize them. What the parser must NEVER do
    // is panic or mis-shape the data.
    let conf = r#"{
        "times": ["05:27", "06:37", "13:21", "16:37", "19:24"],
        "shuruq": "06:37",
        "calendar": [{"1": ["05:27","06:37","13:21","16:37","19:24","20:51"]}],
        "name": "<script>alert('xss')</script>",
        "jumua": "<img src=x onerror=alert(1)>",
        "image": "javascript:alert(document.cookie)",
        "announcements": [{"title": "</textarea><script>alert(1)</script>"}]
    }"#;
    let page = format!("<html><script>var confData = {conf};</script></html>");
    let mock = spawn_mock(vec![("/en/victim", ok_html(&page))]);
    let conf = mock.client().conf_data("victim").await.expect("parses");
    assert_eq!(conf.name.as_deref(), Some("<script>alert('xss')</script>"));
}

#[tokio::test]
async fn conf_page_garbage_html_is_conf_data_not_found() {
    for body in [
        "<html>no confData here</html>",
        "",
        "confData = not json;",
        "<script>var confData = [1,2,3];</script>",
    ] {
        let mock = spawn_mock(vec![("/en/victim", ok_html(body))]);
        let err = mock.client().conf_data("victim").await.unwrap_err();
        assert!(matches!(err, MawaqitError::ConfDataNotFound(_)), "{body:?}: {err}");
    }
}

#[tokio::test]
async fn conf_page_lax_content_type_is_documented_behavior() {
    // The parser does not check Content-Type — a text/plain or
    // application/octet-stream body with valid confData still parses. Not a
    // finding by itself (the payload is what matters), but pinned here so the
    // decision stays deliberate.
    let page = trap_page();
    for ctype in ["text/plain", "application/octet-stream", "weird/vendor-type"] {
        let mock = spawn_mock(vec![(
            "/en/victim",
            http_bytes("HTTP/1.1 200 OK", ctype, page.as_bytes()),
        )]);
        assert!(
            mock.client().conf_data("victim").await.is_ok(),
            "{ctype} with valid confData parses"
        );
    }
}

#[tokio::test]
async fn truncated_body_and_connection_reset_are_errors() {
    // Headers promise 10 000 bytes, 50 arrive, socket dies.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let head =
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 10000\r\n\r\n";
        let _ = stream.write_all(head.as_bytes());
        let partial: &[u8] = b"<html><script>var confData";
        let _ = stream.write_all(&partial[..partial.len().min(27)]);
        let _ = stream.flush();
        drop(stream);
    });
    let client = MawaqitClient::with_base_urls(base.clone(), base.clone());
    assert!(client.conf_data("victim").await.is_err(), "truncated body is an Err");
}

#[tokio::test]
async fn oversized_body_is_rejected_by_the_cap() {
    let over_cap = "A".repeat(20 * 1024 * 1024 + 1);
    let mock = spawn_mock(vec![(
        "/en/victim",
        http_bytes("HTTP/1.1 200 OK", "text/html", over_cap.as_bytes()),
    )]);
    let err = mock.client().conf_data("victim").await.unwrap_err();
    assert!(err.to_string().contains("cap"), "{err}");
}

#[tokio::test]
async fn redirect_loop_stays_bounded_by_the_http_client() {
    let mock = spawn_mock(vec![("/en/loop", redirect_to("/en/loop"))]);
    assert!(mock.client().conf_data("loop").await.is_err(), "redirect loop must end");
}

#[tokio::test]
async fn cache_never_confuses_two_slugs() {
    let mock = spawn_mock(vec![
        ("/en/mosque-a", ok_html(&trap_page())),
        ("/en/mosque-b", ok_html("<html><script>var confData = {\"times\":[\"06:00\",\"07:00\",\"13:00\",\"16:00\",\"19:00\",\"21:00\"],\"calendar\":[{\"1\":[\"06:00\",\"07:00\",\"13:00\",\"16:00\",\"19:00\",\"21:00\"]}]};</script></html>")),
    ]);
    let client = mock.client();
    let a = client.conf_data("mosque-a").await.unwrap();
    let b = client.conf_data("mosque-b").await.unwrap();
    assert_ne!(a.name, b.name, "two slugs never share a cache entry");
    assert_eq!(mock.requests().len(), 2, "each slug fetched exactly once");
    // Same slug again is served from cache: still exactly 2 requests.
    let _ = client.conf_data("mosque-a").await.unwrap();
    assert_eq!(mock.requests().len(), 2);
}

// ------------------------------------------------- findings (currently red)

/// FINDING F1 — the client follows redirects anywhere, including cross-origin.
/// A single open redirect (or a compromise) on mawaqit.net turns
/// `conf_data` into "parse whatever evil.test serves" — attacker-authored
/// prayer times, mosque name and image URL on the user's screen.
/// FIX: pin `redirect::Policy` to same-host (or `Policy::none`) in
/// `MawaqitClient::with_base_urls`, then un-ignore.
#[tokio::test]
#[ignore = "RED TEAM FINDING F1: cross-origin redirects are followed"]
async fn finding_f1_cross_origin_redirect_is_not_followed() {
    let mock = spawn_mock(vec![
        ("/en/victim", redirect_to("http://attacker.invalid/en/trap")),
        ("/en/trap", ok_html(&trap_page())),
    ]);
    // Not following means the 302 itself surfaces as an Api error.
    let err = mock.client().conf_data("victim").await.unwrap_err();
    assert!(
        matches!(err, MawaqitError::Api { status: 302, .. }),
        "client followed the redirect instead of surfacing the 302: {err}"
    );
}

/// FINDING F2 — slugs are never validated between the search response and
/// the URL: `update_config` persists anything, and `conf_data` interpolates
/// it verbatim into `…/{lang}/{slug}`. A hostile/compromised search response
/// (or a tampered config file) can point the fetch at arbitrary mawaqit.net
/// paths: `../` escapes the mosque namespace, `?`/`#` swap the page under a
/// legit-looking slug.
/// FIX: validate the slug once (e.g. `^[a-z0-9]+(-[a-z0-9]+)*$`) at the
/// `Mosque::mosque_id` boundary and in `update_config`, then un-ignore.
#[tokio::test]
#[ignore = "RED TEAM FINDING F2: hostile slugs are fetched verbatim"]
async fn finding_f2_hostile_slug_never_leaves_the_mosque_namespace() {
    let hostile_slugs = [
        "../trap",      // dot-segment traversal
        "..%2Ftrap",    // encoded traversal
        "%2e%2e/trap",  // encoded dot-segment
        "victim?x=1",   // query injection swaps nothing visible…
        "victim#frag",  // fragment injection
        "victim/extra", // path extension
        "victim%00",    // NUL byte
    ];
    let mock = spawn_mock(vec![
        ("/en/victim", ok_html("<html><script>var confData = {\"times\":[\"06:00\",\"07:00\",\"13:00\",\"16:00\",\"19:00\",\"21:00\"],\"calendar\":[{\"1\":[\"06:00\",\"07:00\",\"13:00\",\"16:00\",\"19:00\",\"21:00\"]}]};</script></html>")),
        ("/trap", ok_html(&trap_page())),
    ]);
    for slug in hostile_slugs {
        let result = mock.client().conf_data(slug).await;
        let path = mock.requests().last().cloned().unwrap_or_default();
        // Secure contract: the request stays a single page under /en/, and a
        // hostile slug never yields attacker content.
        let decoded = path.replace("%2F", "/").replace("%2e", ".").replace("%2E", ".");
        assert!(
            decoded.starts_with("/en/") && !decoded.contains(".."),
            "slug {slug:?} escaped the mosque namespace, request path {path:?}"
        );
        assert!(result.is_err(), "slug {slug:?} must be rejected, got parsed data");
    }
}

/// FINDING F3 — the 20 MB cap is applied *after* the whole body is buffered
/// (`response.bytes()`): a hostile server streaming gigabytes OOMs the app
/// before the cap trips. This test only proves the cap rejects oversized
/// responses (it does); the memory-blowup during download is the finding.
/// FIX: stream the body through `bytes_stream()` and abort once the running
/// total exceeds the cap, then delete this comment.
#[tokio::test]
async fn finding_f3_documented_cap_rejects_oversized_response() {
    let over_cap = "A".repeat(20 * 1024 * 1024 + 1);
    let mock = spawn_mock(vec![(
        "/en/victim",
        http_bytes("HTTP/1.1 200 OK", "text/html", over_cap.as_bytes()),
    )]);
    assert!(mock.client().conf_data("victim").await.is_err());
}
