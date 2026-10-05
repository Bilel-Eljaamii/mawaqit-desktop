//! Wiring contract for the quick-glance overlay. The v0.5.3 release shipped
//! with `toggle_glance`/`position_glance` orphaned — a refactor dropped their
//! call sites, the compiler only warned (dead_code), and Ctrl+Alt+P was
//! silently dead. These greps pin the wiring so a silent drop fails CI.

use serde_json::Value;

const LIB: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"));

fn strip(src: &str) -> String {
    src.replace(char::is_whitespace, "")
}

#[test]
fn glance_shortcut_is_registered_and_wired() {
    let lib = strip(LIB);
    // Runtime registration of the shortcut, with the toggle wired in.
    assert!(
        lib.contains(r#"on_shortcut("ctrl+alt+p""#),
        "the ctrl+alt+p shortcut registration is missing"
    );
    assert!(
        lib.contains("toggle_glance(app);"),
        "the shortcut handler must call toggle_glance"
    );
    // Non-fatal: a registration failure logs, never panics.
    assert!(
        lib.contains("quick-glanceshortcutunavailable"),
        "registration failure must degrade, not abort"
    );
    assert!(
        !lib.contains("expect(\"the built-in glance shortcut"),
        "shortcut registration must not use expect() at startup"
    );
}

#[test]
fn glance_overlay_is_positioned_and_dismissable() {
    let lib = strip(LIB);
    assert!(
        lib.contains("position_glance(&glance);"),
        "setup must position the glance overlay at the screen edge"
    );
    assert!(
        lib.contains("fntoggle_glance") && lib.contains("fnposition_glance"),
        "the glance helpers must exist"
    );
}

#[test]
fn glance_window_is_declared_in_the_conf() {
    let conf: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tauri.conf.json"
    )))
    .expect("tauri.conf.json parses");
    let glance = conf["app"]["windows"]
        .as_array()
        .expect("windows list")
        .iter()
        .find(|w| w["label"] == "glance")
        .expect("glance window declared");
    assert_eq!(glance["decorations"], Value::Bool(false));
    assert_eq!(glance["alwaysOnTop"], Value::Bool(true));
    assert_eq!(glance["visible"], Value::Bool(false));
}
