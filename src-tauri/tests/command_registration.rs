//! Command-registration contract: every `#[tauri::command]` defined in
//! `presentation/commands.rs` must appear in the `invoke_handler` list in
//! `lib.rs`. Regression guard for issue #1 — `set_tor_enabled` shipped
//! defined but unregistered, so the topbar Tor toggle failed with
//! "Command set_tor_enabled not found" while `cargo check` stayed green
//! (an unregistered command is a valid plain function).

use std::collections::HashSet;

const COMMANDS_RS: &str = include_str!("../src/presentation/commands.rs");
const LIB_RS: &str = include_str!("../src/lib.rs");

/// Command names defined via `#[tauri::command]` + `pub fn name(`.
fn defined_commands() -> HashSet<String> {
    let mut out = HashSet::new();
    let mut rest = COMMANDS_RS;
    while let Some(idx) = rest.find("#[tauri::command]") {
        let after = &rest[idx..];
        let Some(fn_idx) = after.find("fn ") else { break };
        let name_start = fn_idx + 3;
        let name: String = after[name_start..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            out.insert(name);
        }
        rest = &after[fn_idx..];
    }
    out
}

/// Command names passed to `generate_handler![...]`: every
/// `presentation::commands::<name>` entry in the handler list.
fn registered_commands() -> HashSet<String> {
    let Some(start) = LIB_RS.find("generate_handler![") else {
        return HashSet::new();
    };
    let Some(end) = LIB_RS[start..].find("])") else {
        return HashSet::new();
    };
    let list = &LIB_RS[start..start + end];
    list.split("presentation::commands::")
        .skip(1)
        .filter_map(|segment| {
            let name: String = segment
                .trim()
                .trim_end_matches(',')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if name.is_empty() {
                None
            } else {
                Some(name)
            }
        })
        .collect()
}

#[test]
fn every_defined_command_is_registered() {
    let defined = defined_commands();
    assert!(
        !defined.is_empty(),
        "parser found no commands — the parse is broken"
    );
    let registered = registered_commands();
    for name in &defined {
        assert!(
            registered.contains(name),
            "command `{name}` is defined in commands.rs but NOT registered in \
             invoke_handler — Tauri will reject IPC calls for it at runtime \
             (issue #1: set_tor_enabled shipped unregistered)"
        );
    }
}

#[test]
fn set_tor_enabled_is_registered() {
    assert!(
        registered_commands().contains("set_tor_enabled"),
        "the Tor toggle IPC must be registered (issue #1)"
    );
}
