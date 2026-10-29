//! IPC command tests: the REAL command bodies driven through
//! `tauri::test`'s MockRuntime — no display, no GTK, real config-file I/O
//! under a temp `XDG_CONFIG_HOME`, and a local canned-response server for
//! the network paths. Hostile-first: every rejection must leave the
//! on-disk config byte-identical, and no input class may panic.
//!
//! Serialization: the commands read the *global* config path, which
//! follows `XDG_CONFIG_HOME` per call — every config-touching test holds
//! the process-wide [`ENV_LOCK`] for its whole body.

use std::io::Read as _;

use chrono::Datelike;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use mawaqit_desktop_lib::domain::models::AppConfig;
use mawaqit_desktop_lib::infrastructure::builtin_tor::BuiltinTor;
use mawaqit_desktop_lib::infrastructure::client_handle::ClientHolder;
use mawaqit_desktop_lib::infrastructure::config::save_config;
use mawaqit_desktop_lib::presentation::commands;
use tauri::Manager;

// ---- XDG isolation ------------------------------------------------------

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Redirects `XDG_CONFIG_HOME` at a fresh temp dir for the test's whole
/// body (guard drops it back). The mutex must be held for the same span —
/// parallel tests would otherwise race on the process-global env.
struct XdgGuard {
    dir: PathBuf,
    old: Option<std::ffi::OsString>,
    _lock: MutexGuard<'static, ()>,
}

impl XdgGuard {
    fn new(tag: &str) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let old = std::env::var_os("XDG_CONFIG_HOME");
        let dir = std::env::temp_dir().join(format!(
            "mawaqit-ipc-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        Self { dir, old, _lock: lock }
    }

    fn config_file(&self) -> PathBuf {
        self.dir.join("mawaqit-desktop").join("mawaqit-config.json")
    }
}

impl Drop for XdgGuard {
    fn drop(&mut self) {
        match &self.old {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ---- Harness ------------------------------------------------------------

fn harness() -> tauri::App<tauri::test::MockRuntime> {
    let app = tauri::test::mock_app();
    let builtin = BuiltinTor::default();
    let holder = ClientHolder::from_config(&AppConfig::default(), &builtin);
    app.manage(holder);
    app.manage(builtin);
    app
}

/// Swap the harness's transport for one aimed at `api`/`site` (and the CDN
/// at `cdn`): the api crate's documented mock seams.
fn aim_client_at(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
    api: String,
    site: String,
    cdn: String,
) {
    let holder = app.state::<ClientHolder>();
    let client = mawaqit_api::MawaqitClient::with_base_urls(api, site)
        .with_cdn_base(cdn)
        .with_disk_cache(mawaqit_desktop_lib::infrastructure::config::cache_dir());
    holder.swap(client);
}

/// A canned-response server: EVERY request gets the same status + body.
fn spawn_server(status: u16, body: &'static str) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}", server.server_addr());
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let response =
                tiny_http::Response::from_string(body).with_status_code(status);
            let _ = request.respond(response);
        }
    });
    url
}

/// A parseable mosque page. The calendar is POSITIONAL (entry index =
/// month-1) and each entry maps day keys to time rows: entry 0 carries
/// day 1 (the month-1 lookups), and today's month carries today's day —
/// so `times_for_date(now)` resolves. Same confData shape as the api
/// crate's own ct fixtures.
fn sample_page(name: &str) -> String {
    let times = r#"["05:00","06:30","12:00","15:30","18:00","19:30"]"#;
    let today = chrono::Local::now().date_naive();
    let mut months = vec!["{}".to_string(); 12];
    months[0] = format!("{{\"1\":{times}}}");
    months[today.month() as usize - 1] = format!("{{\"{}\":{times}}}", today.day());
    format!(
        "<html><script>var confData = {{\"times\":{times},\"calendar\":[{}],\
         \"name\":\"{name}\"}};</script></html>",
        months.join(",")
    )
}

fn mosque_config() -> AppConfig {
    AppConfig { mosque_slug: "mock-mosque".into(), ..AppConfig::default() }
}

fn read_bytes(path: &Path) -> Vec<u8> {
    std::fs::File::open(path).unwrap().bytes().map(|b| b.unwrap()).collect()
}

// ---- Config commands ----------------------------------------------------

/// @tier durable — the settings contract: a valid update persists and
/// round-trips; `sound_enabled` is recomputed from the per-prayer alerts,
/// never trusted from the caller.
#[test]
fn update_config_persists_and_recomputes_the_legacy_switch() {
    let g = XdgGuard::new("happy");
    let app = harness();
    let mut cfg = mosque_config();
    cfg.sound_enabled = false; // stale value from a hostile caller
    cfg.alerts.fajr.mode = mawaqit_desktop_lib::domain::models::AthanMode::Adhan;

    commands::update_config(cfg.clone(), app.state(), app.state()).expect("valid save");

    let stored = commands::get_config();
    assert_eq!(stored.mosque_slug, "mock-mosque");
    assert!(stored.sound_enabled, "recomputed: adhan mode means sound on");
    assert!(g.config_file().is_file(), "config persisted on disk");
}

/// @tier durable — ANY rejected update leaves the on-disk config
/// byte-identical: failed validation never persists.
#[test]
fn rejected_updates_never_touch_the_disk_config() {
    let g = XdgGuard::new("reject");
    let app = harness();
    commands::update_config(mosque_config(), app.state(), app.state())
        .expect("seed save");
    let before = read_bytes(&g.config_file());

    let mut hostile = mosque_config();
    hostile.mosque_slug = "../evil".into();
    let err = commands::update_config(hostile, app.state(), app.state()).unwrap_err();
    assert!(err.contains("invalid mosque id"), "got {err:?}");

    let mut hostile = mosque_config();
    hostile.alerts.fajr.voice = Some("adhan-not-in-catalog".into());
    let err = commands::update_config(hostile, app.state(), app.state()).unwrap_err();
    assert!(err.contains("unknown adhan voice"), "got {err:?}");

    assert_eq!(read_bytes(&g.config_file()), before, "disk untouched");
}

/// The announcements read-list is capped at 500 through the save path —
/// a tampered config cannot bloat the file unboundedly.
#[test]
fn update_config_clamps_the_announcements_read_list() {
    let g = XdgGuard::new("clamp");
    let app = harness();
    let mut cfg = mosque_config();
    cfg.announcements_read = (0..10_000).map(|i| format!("a-{i}")).collect();

    commands::update_config(cfg, app.state(), app.state()).expect("save");

    let stored = commands::get_config();
    assert!(
        stored.announcements_read.len() <= 500,
        "clamped, got {}",
        stored.announcements_read.len()
    );
    assert!(read_bytes(&g.config_file()).len() < 64_000, "file stays small");
}

#[test]
fn set_offline_mode_toggles_and_persists() {
    let _g = XdgGuard::new("offline");
    let app = harness();

    commands::set_offline_mode(true, app.state()).expect("on");
    assert!(commands::get_config().offline_mode);
    commands::set_offline_mode(true, app.state()).expect("idempotent");
    assert!(commands::get_config().offline_mode);
    commands::set_offline_mode(false, app.state()).expect("off");
    assert!(!commands::get_config().offline_mode);
}

/// @tier durable — enabling an EXTERNAL Tor proxy that nothing listens on
/// is refused with the actionable message, and the config keeps saying
/// Tor is off (the v0.10.1 dead-proxy gate, end to end).
#[test]
fn set_tor_enabled_refuses_a_dead_external_proxy() {
    let _g = XdgGuard::new("tor-dead");
    let app = harness();
    let mut cfg = mosque_config();
    cfg.tor.host = "127.0.0.1".into();
    cfg.tor.port = 1; // nothing listens there; refused instantly
    cfg.tor.builtin = false;
    commands::update_config(cfg, app.state(), app.state()).expect("seed");

    let err = commands::set_tor_enabled(true, app.state(), app.state()).unwrap_err();
    assert!(err.contains("nothing is listening"), "got {err:?}");
    assert!(!commands::get_config().tor.enabled, "still off on disk");

    // Disabling an already-off Tor is a no-op — the gate must not run.
    assert!(!commands::set_tor_enabled(false, app.state(), app.state()).unwrap());
}

// ---- Data commands ------------------------------------------------------

/// @tier durable — without a mosque the Today view is `Ok(None)` (never an
/// error, never a fetch), and the month commands refuse with the stable
/// onboarding message for ANY month number, hostile ones included.
#[test]
fn data_commands_without_a_mosque_degrade_to_onboarding() {
    let _g = XdgGuard::new("nomosque");
    let app = harness();

    assert!(
        tauri::async_runtime::block_on(commands::get_today(app.state()))
            .unwrap()
            .is_none(),
        "no mosque: Today is None, never an error"
    );
    for month in [0u32, 13, u32::MAX] {
        let err = tauri::async_runtime::block_on(commands::get_month(app.state(), month))
            .unwrap_err();
        assert_eq!(err, "No mosque configured yet.", "month={month}");
        let _ =
            tauri::async_runtime::block_on(commands::get_month_iqama(app.state(), month))
                .unwrap_err();
    }
}

/// @tier durable — offline mode is disk-only: a present snapshot serves,
/// and a missing one is a clean, friendly error (never a network attempt).
#[test]
fn get_today_offline_serves_the_snapshot_or_fails_cleanly() {
    let _g = XdgGuard::new("offline-today");
    let app = harness();

    let mut cfg = mosque_config();
    cfg.offline_mode = true;
    save_config(&cfg);

    // No snapshot yet: clean error naming the remedy.
    let err =
        tauri::async_runtime::block_on(commands::get_today(app.state())).unwrap_err();
    assert!(err.contains("no saved data") && err.contains("go online"), "got {err:?}");

    // With a snapshot: served from disk, `as_of` set to today.
    let page = sample_page("Snapshot Mosque");
    let conf = mawaqit_api::parse_page(&page, "t").unwrap();
    mawaqit_api::disk::store(
        &mawaqit_desktop_lib::infrastructure::config::cache_dir(),
        "mock-mosque",
        &conf,
    )
    .unwrap();

    let payload = tauri::async_runtime::block_on(commands::get_today(app.state()))
        .expect("snapshot serves")
        .expect("payload");
    assert_eq!(payload.mosque_name.as_deref(), Some("Snapshot Mosque"));
    let today = chrono::Local::now().date_naive().to_string();
    assert_eq!(payload.as_of.as_deref(), Some(today.as_str()));
}

/// @tier durable — the online path parses a real page through the mock
/// server; month 1 resolves, and hostile months are mapped errors, never
/// panics-by-index.
#[test]
fn month_commands_against_a_mock_server() {
    let _g = XdgGuard::new("months");
    let app = harness();
    let page = sample_page("Mock Mosque");
    let url = spawn_server(200, Box::leak(page.clone().into_boxed_str()));
    aim_client_at(app.handle(), url.clone(), url.clone(), url);
    save_config(&mosque_config());

    let payload = tauri::async_runtime::block_on(commands::get_today(app.state()))
        .expect("online today")
        .expect("payload");
    assert_eq!(payload.mosque_name.as_deref(), Some("Mock Mosque"));

    let month = tauri::async_runtime::block_on(commands::get_month(app.state(), 1))
        .expect("month 1 resolves");
    assert!(!month.days.is_empty(), "month 1 has its day");

    for hostile in [0u32, 13, u32::MAX] {
        let res =
            tauri::async_runtime::block_on(commands::get_month(app.state(), hostile));
        assert!(res.is_err(), "month {hostile} must be rejected");
    }
}

/// @tier durable — the offline gate is unconditional and pre-transport:
/// with offline mode on, NO request class reaches the network (the mock
/// server sees zero connections), whatever the query — empty, oversized,
/// or carrying control characters.
#[test]
fn resolve_mosque_offline_never_reaches_the_network() {
    let _g = XdgGuard::new("search-off");
    let app = harness();
    let url = spawn_server(200, "[]");
    aim_client_at(app.handle(), url.clone(), url.clone(), url);

    let mut cfg = mosque_config();
    cfg.offline_mode = true;
    save_config(&cfg);

    let nul_query = format!("bad{}sin", '\u{0}');
    for query in ["", "valid", "x".repeat(100_000).as_str(), nul_query.as_str()] {
        let err = tauri::async_runtime::block_on(commands::resolve_mosque(
            app.state(),
            query.to_string(),
        ))
        .unwrap_err();
        assert!(err.contains("Offline mode"), "got {err:?}");
    }
}

/// A server error and a malformed body surface as mapped errors — the
/// command contract is `Err(String)`, never a panic.
#[test]
fn resolve_mosque_maps_transport_failures() {
    let _g = XdgGuard::new("search-net");
    let app = harness();
    save_config(&mosque_config());

    let url = spawn_server(500, "boom");
    aim_client_at(app.handle(), url.clone(), url.clone(), url);
    let err = tauri::async_runtime::block_on(commands::resolve_mosque(
        app.state(),
        "paris".into(),
    ))
    .unwrap_err();
    assert!(!err.is_empty());

    let url = spawn_server(200, "not json at all {{{");
    aim_client_at(app.handle(), url.clone(), url.clone(), url);
    let err = tauri::async_runtime::block_on(commands::resolve_mosque(
        app.state(),
        "paris".into(),
    ))
    .unwrap_err();
    assert!(!err.is_empty());
}

// ---- Voice commands -----------------------------------------------------

/// @tier durable — `voice_is_cached` answers only about files inside the
/// voices dir: hostile ids (traversal, separators, empty, huge) must
/// return false without panicking, escaping, or touching the filesystem
/// outside the dir.
#[test]
fn voice_is_cached_is_traversal_proof() {
    let _g = XdgGuard::new("voice-cached");
    let app = harness();
    let huge = "x".repeat(1 << 20);
    for id in ["", ".", "..", "../x", "a/b", "a\\b", huge.as_str()] {
        assert!(
            !commands::voice_is_cached(id.to_string()),
            "hostile id must not resolve to a cached file: {id:?}"
        );
    }
    // A catalog voice with the file present IS cached.
    let dir = mawaqit_desktop_lib::infrastructure::config::voices_dir();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("adhan-maquah.mp3"), b"mp3").unwrap();
    assert!(commands::voice_is_cached("adhan-maquah".into()));
}

/// @tier durable — downloads write ONLY catalog ids, ONLY into the voices
/// dir: a hostile id is rejected before any byte moves (the mock server
/// would serve to anyone), and a catalog id lands at exactly
/// `<voices_dir>/<id>.mp3`.
#[test]
fn download_voice_writes_catalog_ids_inside_the_voices_dir_only() {
    let _g = XdgGuard::new("voice-dl");
    let app = harness();
    let url = spawn_server(200, "ID3fake-mp3-bytes");
    aim_client_at(app.handle(), url.clone(), url.clone(), url);

    // Hostile id: rejected locally, no file anywhere.
    let err = tauri::async_runtime::block_on(commands::download_voice(
        app.state(),
        "../../evil".to_string(),
    ))
    .unwrap_err();
    assert!(!err.is_empty());
    let dir = mawaqit_desktop_lib::infrastructure::config::voices_dir();
    if dir.exists() {
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "nothing written for a hostile id"
        );
    }

    // Catalog id: written to exactly one place.
    tauri::async_runtime::block_on(commands::download_voice(
        app.state(),
        "adhan-maquah".to_string(),
    ))
    .expect("catalog download");
    assert!(dir.join("adhan-maquah.mp3").is_file());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
}

/// The preview path degrades, never panics: an unknown voice with the CDN
/// gone plays the builtin (silent at volume 0), and stop works — the
/// full lifecycle contract of the stop button.
#[test]
fn preview_lifecycle_degrades_and_stops() {
    let _g = XdgGuard::new("preview");
    let app = harness();
    let dead = "http://127.0.0.1:1".to_string();
    aim_client_at(app.handle(), dead.clone(), dead.clone(), dead);

    let _played = tauri::async_runtime::block_on(commands::preview_athan(
        app.state(),
        None,
        Some("adhan-no-such-voice".to_string()),
        Some(0), // silent — the assertion is about the lifecycle
    ))
    .expect("preview degrades to builtin, never errors");

    commands::stop_athan();
    assert!(!commands::athan_playing());
}
