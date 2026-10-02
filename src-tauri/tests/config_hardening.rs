//! Build-configuration hardening tests: the webview is attack surface
//! because it renders attacker-controlled strings (mosque names, times,
//! jumua strings) from mawaqit.net. These tests read `tauri.conf.json` and
//! pin the hardening posture so it cannot silently regress — or stay open.
//!
//! `#[ignore = "RED TEAM FINDING F#…"]` = hardening the config does not
//! have yet. Un-ignore after fixing `tauri.conf.json`.

use serde_json::Value;

const CONF: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json"));

fn conf() -> Value {
    serde_json::from_str(CONF).expect("tauri.conf.json is valid JSON")
}

#[test]
fn config_is_structurally_sane() {
    let c = conf();
    assert!(c["productName"].is_string());
    assert!(
        !c["app"]["windows"].as_array().expect("windows list").is_empty(),
        "at least one window must be configured"
    );
    assert!(c["build"]["frontendDist"].is_string());
}

/// CONTRACT (was FINDING F7, fixed mid-engagement): a real CSP must stay
/// configured. The webview renders attacker-controlled strings (mosque
/// names, times, jumua strings) from mawaqit.net; the CSP is the second
/// line of defense behind textContent. If this test fails, someone set
/// `security.csp` back to null — restore a policy:
///   "default-src 'self'; img-src 'self' https: data: blob:;
///    style-src 'self' 'unsafe-inline'; font-src 'self' data:;
///    script-src 'self'"
/// (style-src inline: the app sets CSS custom properties from JS; img-src
/// https: the mosque backdrop image.)
#[test]
fn csp_is_configured() {
    let csp = &conf()["app"]["security"]["csp"];
    assert!(
        csp.is_string(),
        "app.security.csp must be a policy, got: {csp} — a null CSP removes the \
         second line of defense for a webview that renders hostile data"
    );
    let policy = csp.as_str().unwrap();
    assert!(
        policy.contains("script-src") || policy.contains("default-src"),
        "the CSP must at least constrain scripts, got: {policy}"
    );
}

/// FINDING F8 — `withGlobalTauri: true` exposes `window.__TAURI__` to any
/// script running in the webview. The frontend never uses it (it imports
/// `@tauri-apps/api` modules, which are bundled), so it is pure attack
/// surface: combined with any future injection it hands the attacker every
/// IPC command, including `update_config`.
/// FIX: set `"withGlobalTauri": false` in `src-tauri/tauri.conf.json`, then
/// un-ignore.
#[test]
#[ignore = "RED TEAM FINDING F8: window.__TAURI__ is exposed for no reason"]
fn finding_f8_global_tauri_is_disabled() {
    assert_eq!(
        conf()["app"]["withGlobalTauri"],
        Value::Bool(false),
        "withGlobalTauri must be false; the app uses bundled @tauri-apps/api imports"
    );
}

/// The capability file grants the webview only what it uses. If this test
/// fails, someone added a permission — justify it or remove it.
#[test]
fn capabilities_stay_minimal() {
    let caps: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/capabilities/default.json"
    )))
    .expect("capabilities/default.json is valid JSON");
    let permissions: Vec<&str> = caps["permissions"]
        .as_array()
        .expect("permissions list")
        .iter()
        .filter_map(|p| p.as_str())
        .collect();
    for allowed in [
        "core:default",
        "opener:default",
        "autostart:allow-enable",
        "autostart:allow-disable",
        "autostart:allow-is-enabled",
        // Native file picker for the per-prayer custom athan sound: read-only
        // open dialog, returns the chosen path. No write access granted.
        "dialog:allow-open",
    ] {
        assert!(
            permissions.contains(&allowed),
            "expected permission {allowed} missing (was it intentionally removed? update this test)"
        );
    }
    assert!(
        permissions.len() <= 6,
        "new permissions appeared: {permissions:?} — justify each one against hostile webview content"
    );
}
