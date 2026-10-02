//! Disk-snapshot (offline) contract tests: a successful fetch must refresh
//! the snapshot, a failed fetch must serve it, and a hostile snapshot file
//! must degrade to a plain error — never a panic.

use std::{
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    thread,
};

use mawaqit_api::{disk, MawaqitClient};

// ---------------------------------------------------------------- mock server

/// Serve one canned response for every request (raw TCP, like the hostile
/// HTTP suite). Requests are counted.
fn spawn_mock(response: String) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock binds an ephemeral port");
    let base = format!("http://{}", listener.local_addr().expect("local addr"));
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let head = String::from_utf8_lossy(&buf);
            if !head.starts_with("GET ") {
                continue;
            }
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    (base, handle)
}

fn ok_html(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// A minimal valid mosque page: 6 daily times + a one-month calendar.
fn mosque_page() -> String {
    let conf = r#"{
        "times": ["05:00", "06:30", "12:00", "15:30", "18:00", "19:30"],
        "calendar": [{"1": ["05:00","06:30","12:00","15:30","18:00","19:30"]}],
        "name": "Snapshot Test Mosque"
    }"#;
    ok_html(&format!("<html><script>var confData = {conf};</script></html>"))
}

/// A base URL on port 1 — connection refused instantly.
const DEAD_BASE: &str = "http://127.0.0.1:1";

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mawaqit-disk-cache-{name}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const SLUG: &str = "grande-mosquee-de-paris";

// ------------------------------------------------------------------- tests

#[tokio::test]
async fn successful_fetch_writes_a_snapshot_and_serves_memory_afterwards() {
    let dir = temp_dir("store");
    let (base, server) = spawn_mock(mosque_page());
    let client = MawaqitClient::with_base_urls(base.clone(), base.clone())
        .with_disk_cache(dir.clone());

    let (conf, as_of) = client.conf_data_dated(SLUG).await.expect("first fetch");
    assert_eq!(conf.name.as_deref(), Some("Snapshot Test Mosque"));
    assert_eq!(as_of, None, "a live fetch is not 'from disk'");

    // The snapshot landed on disk with today's date.
    let (fetched_at, stored) = disk::load(&dir, SLUG).expect("snapshot stored");
    assert_eq!(fetched_at, chrono::Local::now().date_naive());
    assert_eq!(stored.name.as_deref(), Some("Snapshot Test Mosque"));

    // Second call is the in-memory cache: still not 'from disk'.
    let (_, as_of) = client.conf_data_dated(SLUG).await.expect("second fetch");
    assert_eq!(as_of, None);
    drop(server);
}

#[tokio::test]
async fn failed_fetch_falls_back_to_the_snapshot() {
    let dir = temp_dir("fallback");
    // Seed the snapshot through the public API (a previous online session).
    let (base, _server) = spawn_mock(mosque_page());
    let online = MawaqitClient::with_base_urls(base.clone(), base)
        .with_disk_cache(dir.clone());
    online.conf_data(SLUG).await.expect("seed fetch");
    drop(_server);

    // Now the network is gone; the snapshot must serve.
    let offline = MawaqitClient::with_base_urls(DEAD_BASE.to_string(), DEAD_BASE.to_string())
        .with_disk_cache(dir);
    let (conf, as_of) = offline.conf_data_dated(SLUG).await.expect("offline fallback");
    assert_eq!(conf.name.as_deref(), Some("Snapshot Test Mosque"));
    assert!(as_of.is_some(), "served-from-snapshot must carry its date");

    // And derived data paths work offline too.
    let month = offline.month(SLUG, 1).await.expect("offline month");
    assert!(!month.days.is_empty());
}

#[tokio::test]
async fn offline_without_a_snapshot_is_an_error() {
    let dir = temp_dir("empty");
    let offline = MawaqitClient::with_base_urls(DEAD_BASE.to_string(), DEAD_BASE.to_string())
        .with_disk_cache(dir);
    assert!(offline.conf_data_dated(SLUG).await.is_err());
}

#[tokio::test]
async fn hostile_snapshot_file_degrades_to_an_error() {
    let dir = temp_dir("hostile");
    std::fs::write(disk::snapshot_path(&dir, SLUG), "{\"version\":1,\"mos").unwrap();
    let offline = MawaqitClient::with_base_urls(DEAD_BASE.to_string(), DEAD_BASE.to_string())
        .with_disk_cache(dir);
    assert!(
        offline.conf_data_dated(SLUG).await.is_err(),
        "a truncated snapshot must not serve, and must not panic"
    );
}

#[tokio::test]
async fn without_disk_cache_the_client_behaves_as_before() {
    let client = MawaqitClient::with_base_urls(DEAD_BASE.to_string(), DEAD_BASE.to_string());
    assert!(client.conf_data_dated(SLUG).await.is_err(), "no cache, no fallback");
}
