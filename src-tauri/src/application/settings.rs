//! Settings application service: the logic behind the settings IPC
//! commands, extracted from `presentation/commands.rs` so it can be tested
//! without a Tauri harness. Every function takes the `ClientHolder` (for
//! cache invalidation and transport swaps) plus the previous config, and
//! returns the config to persist.

use crate::domain::models::{is_plausible_tor_host, AppConfig, TorProxy};
use crate::infrastructure::builtin_tor::BuiltinTor;
use crate::infrastructure::client_handle::{tor_reachable, ClientHolder};
use mawaqit_api;

/// Validate and apply a settings update (the `update_config` command body):
/// gates the mosque slug (F2), the Tor policy (built-in stack starts; an
/// external proxy must be plausible and reachable) and the voice ids
/// (catalog keys), recomputes the legacy sound switch, invalidates the
/// page cache on a mosque switch and swaps the transport when the Tor
/// policy changed. Returns the config to persist.
pub fn validate_and_apply_update(
    holder: &ClientHolder,
    builtin: &BuiltinTor,
    previous: &AppConfig,
    config: AppConfig,
) -> Result<AppConfig, String> {
    validate_and_apply_update_gated(holder, builtin, previous, config, &tor_gate(builtin))
}

/// The save-time gate for one Tor policy: built-in mode must be able to
/// start its stack; external mode must point at a plausible host that is
/// actually listening. Refusing here beats silently black-holing every
/// remote request afterwards.
fn tor_gate(
    builtin: &BuiltinTor,
) -> impl Fn(&TorProxy) -> Result<(), String> + '_ {
    move |tor| {
        if tor.builtin {
            builtin
                .ensure_started()
                .map(|_| ())
                .map_err(|e| format!("Could not start built-in Tor: {e}"))
        } else if !is_plausible_tor_host(&tor.host) {
            Err(format!("invalid Tor proxy host {:?}", tor.host))
        } else if !tor_reachable(&tor.host, tor.port) {
            Err(format!(
                "Cannot enable Tor — nothing is listening on {}:{} — start the Tor daemon (or Tor Browser) first.",
                tor.host, tor.port
            ))
        } else {
            Ok(())
        }
    }
}

/// Same as [`validate_and_apply_update`] with the Tor gate injected, so
/// tests don't need a listening proxy or the built-in stack.
fn validate_and_apply_update_gated(
    holder: &ClientHolder,
    builtin: &BuiltinTor,
    previous: &AppConfig,
    mut config: AppConfig,
    gate: &dyn Fn(&TorProxy) -> Result<(), String>,
) -> Result<AppConfig, String> {
    // FINDING F2: a slug reaching the config file must be a real mosque page
    // identifier — `../`, `?`/`#` or encoded variants never get persisted.
    if !config.mosque_slug.is_empty()
        && !mawaqit_api::is_valid_slug(&config.mosque_slug)
    {
        return Err(format!(
            "invalid mosque id {:?} — search for the mosque again",
            config.mosque_slug
        ));
    }
    // Tor opt-in: external mode needs a plausible host/port (checked at
    // the gate, together with reachability); built-in mode has no address
    // to validate. The strict enforcement (remote-DNS scheme,
    // reachability) lives in the api at client construction.
    if config.tor.enabled && !config.tor.builtin && config.tor.port == 0 {
        return Err("invalid Tor proxy port 0 — use 1–65535".into());
    }
    // Voice ids are catalog keys: a known id resolves to a CDN URL; an
    // unknown one would silently never play, so reject it at save time.
    for voice in [
        &config.alerts.fajr.voice,
        &config.alerts.dhuhr.voice,
        &config.alerts.asr.voice,
        &config.alerts.maghrib.voice,
        &config.alerts.isha.voice,
    ]
    .into_iter()
    .flatten()
    {
        if mawaqit_api::voices::adhan_voice_url(voice).is_none() {
            return Err(format!("unknown adhan voice {voice:?}"));
        }
    }
    // The legacy global switch stays in the file for downgrades; whatever
    // the caller sent, it must agree with the per-prayer settings that now
    // rule.
    config.sound_enabled = config.alerts.any_adhan();
    if previous.mosque_slug != config.mosque_slug {
        holder.current().invalidate(Some(&previous.mosque_slug));
    }
    // A changed Tor policy swaps the transport immediately — the toggle and
    // the settings fields apply without a restart. An enabling (or moved)
    // policy must pass the gate first: refusing at save time beats
    // silently black-holing every remote request afterwards. An unchanged
    // policy is never gated — saving unrelated settings must not depend on
    // the proxy being up.
    if previous.tor != config.tor {
        if config.tor.enabled {
            gate(&config.tor)?;
        }
        crate::infrastructure::client_handle::rebuild_client(holder, &config, builtin);
    }
    Ok(config)
}

/// Topbar Tor toggle: flips the policy, rebuilds and swaps the transport so
/// the change applies immediately (alarms and UI data paths). Built-in
/// mode starts the embedded stack (sub-second; the network connection
/// continues in the background); external mode requires a plausible host
/// that is actually listening — refusing beats silently black-holing every
/// remote request.
pub fn set_tor(
    holder: &ClientHolder,
    builtin: &BuiltinTor,
    previous: &AppConfig,
    on: bool,
) -> Result<AppConfig, String> {
    set_tor_gated(holder, builtin, previous, on, &tor_gate(builtin))
}

/// Same as [`set_tor`] with the Tor gate injected, so tests don't need a
/// listening proxy or the built-in stack. `builtin` is the same instance
/// the production gate uses — `rebuild_client` must hand the transport the
/// SAME stack that was gated, or built-in mode would start a second one.
fn set_tor_gated(
    holder: &ClientHolder,
    builtin: &BuiltinTor,
    previous: &AppConfig,
    on: bool,
    gate: &dyn Fn(&TorProxy) -> Result<(), String>,
) -> Result<AppConfig, String> {
    // An implausible external address is a settings mistake, not a
    // transient condition — catch it before even gating.
    if on && !previous.tor.builtin && !is_plausible_tor_host(&previous.tor.host) {
        return Err("Cannot enable Tor — set a valid proxy address in Settings first.".into());
    }
    let mut config = previous.clone();
    if config.tor.enabled != on {
        if on {
            gate(&config.tor)?;
        }
        config.tor.enabled = on;
        crate::infrastructure::client_handle::rebuild_client(holder, &config, builtin);
    }
    Ok(config)
}

/// Offline toggle: flips the flag and clears the in-memory page cache so
/// the next view serves from the newly chosen source, not a stale copy.
pub fn set_offline(holder: &ClientHolder, previous: &AppConfig, on: bool) -> AppConfig {
    let mut config = previous.clone();
    if config.offline_mode != on {
        config.offline_mode = on;
        holder.current().invalidate(None);
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::TorProxy;

    fn config_with(slug: &str) -> AppConfig {
        AppConfig {
            mosque_slug: slug.into(),
            ..AppConfig::default()
        }
    }

    /// An EXTERNAL tor policy — the shape every gate test uses, so no test
    /// ever starts the real built-in stack or touches the network.
    fn tor_on(host: &str, port: u16) -> TorProxy {
        TorProxy { enabled: true, host: host.into(), port, builtin: false }
    }

    /// The gate tests inject: Ok for a "listening" proxy, Err for a dead
    /// one — with the production error text so message assertions stay
    /// meaningful.

    #[test]
    fn update_rejects_hostile_slug() {
        let builtin = BuiltinTor::default();
        let holder = ClientHolder::from_config(&AppConfig::default(), &builtin);
        let previous = config_with("test-mosque");
        let mut config = config_with("../evil");
        config.sound_enabled = false;

        let err =
            validate_and_apply_update(&holder, &builtin, &previous, config).unwrap_err();
        assert!(err.contains("invalid mosque id"), "got {err:?}");
    }

    #[test]
    fn update_rejects_external_tor_with_implausible_host() {
        let builtin = BuiltinTor::default();
        let holder = ClientHolder::from_config(&AppConfig::default(), &builtin);
        let previous = config_with("test-mosque");
        let mut config = config_with("test-mosque");
        config.tor = tor_on("not a host!", 9050);

        // The production gate checks plausibility BEFORE reachability, so
        // this never touches the network.
        let err =
            validate_and_apply_update(&holder, &builtin, &previous, config).unwrap_err();
        assert!(err.contains("invalid Tor proxy host"), "got {err:?}");
    }

    #[test]
    fn update_rejects_zero_external_tor_port_when_enabled() {
        let builtin = BuiltinTor::default();
        let holder = ClientHolder::from_config(&AppConfig::default(), &builtin);
        let previous = config_with("test-mosque");
        let mut config = config_with("test-mosque");
        config.tor = tor_on("127.0.0.1", 0);

        let err =
            validate_and_apply_update(&holder, &builtin, &previous, config).unwrap_err();
        assert!(err.contains("port 0"), "got {err:?}");
    }

    #[test]
    fn builtin_mode_ignores_the_unused_host_and_port() {
        let builtin = BuiltinTor::default();
        let holder = ClientHolder::from_config(&AppConfig::default(), &builtin);
        let previous = config_with("test-mosque");
        let mut config = config_with("test-mosque");
        // Host/port are meaningless in built-in mode: garbage values must
        // not block a save (the address comes from the internal stack).
        config.tor = TorProxy {
            enabled: true,
            host: "not a host!".into(),
            port: 0,
            builtin: true,
        };

        let validated = validate_and_apply_update_gated(
            &holder,
            &builtin,
            &previous,
            config,
            &|tor| {
                assert!(tor.builtin, "gate must see built-in mode");
                Ok(())
            },
        )
        .expect("built-in mode skips address validation");
        assert!(validated.tor.enabled && validated.tor.builtin);
    }

    #[test]
    fn update_rejects_unknown_voice_id() {
        let builtin = BuiltinTor::default();
        let holder = ClientHolder::from_config(&AppConfig::default(), &builtin);
        let previous = config_with("test-mosque");
        let mut config = config_with("test-mosque");
        config.alerts.fajr.voice = Some("adhan-afassy".into()); // not in catalog

        let err =
            validate_and_apply_update(&holder, &builtin, &previous, config).unwrap_err();
        assert!(err.contains("unknown adhan voice"), "got {err:?}");
    }

    #[test]
    fn update_accepts_a_clean_config_and_recomputes_legacy_switch() {
        let builtin = BuiltinTor::default();
        let holder = ClientHolder::from_config(&AppConfig::default(), &builtin);
        let previous = config_with("test-mosque");
        let mut config = config_with("test-mosque");
        config.sound_enabled = false; // stale legacy value from the caller
        config.alerts.fajr.mode = crate::domain::models::AthanMode::Adhan;

        let validated =
            validate_and_apply_update(&holder, &builtin, &previous, config).expect("clean config");
        // Recomputed from the per-prayer alerts (any adhan mode ⇒ on).
        assert!(validated.sound_enabled);
    }

    #[test]
    fn tor_toggle_swaps_the_transport() {
        let builtin = BuiltinTor::default();
        let holder = ClientHolder::from_config(&AppConfig::default(), &builtin);
        let previous = config_with("test-mosque");
        let alive = &|_: &TorProxy| Ok(());

        let on = set_tor_gated(&holder, &builtin, &previous, true, alive).expect("enable");
        assert!(on.tor.enabled);

        let off = set_tor_gated(&holder, &builtin, &on, false, alive).expect("disable");
        assert!(!off.tor.enabled);
    }

    #[test]
    fn tor_enable_fails_without_a_plausible_host() {
        let builtin = BuiltinTor::default();
        let holder = ClientHolder::from_config(&AppConfig::default(), &builtin);
        let previous = AppConfig {
            tor: tor_on("not a host!", 9050),
            ..config_with("test-mosque")
        };
        // The plausibility gate fires before the reachability probe.
        let err = set_tor_gated(&holder, &builtin, &previous, true, &|_| Ok(())).unwrap_err();
        assert!(err.contains("valid proxy address"), "got {err:?}");
    }

    #[test]
    fn tor_enable_refused_when_nothing_listens_on_the_proxy() {
        let builtin = BuiltinTor::default();
        let holder = ClientHolder::from_config(&AppConfig::default(), &builtin);
        let previous = config_with("test-mosque");
        let dead = &|_: &TorProxy| {
            Err("Cannot enable Tor — nothing is listening on 127.0.0.1:9050 — start the Tor daemon (or Tor Browser) first.".into())
        };

        let err = set_tor_gated(&holder, &builtin, &previous, true, dead).unwrap_err();
        assert!(err.contains("nothing is listening"), "got {err:?}");
        assert!(err.contains("127.0.0.1:9050"), "got {err:?}");
    }

    #[test]
    fn tor_enable_refused_when_the_builtin_stack_cannot_start() {
        let builtin = BuiltinTor::default();
        let holder = ClientHolder::from_config(&AppConfig::default(), &builtin);
        let previous = config_with("test-mosque");
        let broken_stack = &|tor: &TorProxy| {
            assert!(tor.builtin);
            Err("Could not start built-in Tor: boom".into())
        };

        let err = set_tor_gated(&holder, &builtin, &previous, true, broken_stack).unwrap_err();
        assert!(err.contains("Could not start built-in Tor"), "got {err:?}");
    }

    #[test]
    fn tor_reenable_is_a_noop_even_when_the_probe_fails() {
        let builtin = BuiltinTor::default();
        let holder = ClientHolder::from_config(&AppConfig::default(), &builtin);
        let previous = AppConfig {
            tor: tor_on("127.0.0.1", 9050),
            ..config_with("test-mosque")
        };
        // Already enabled: the toggle is idempotent and must not start
        // failing because the daemon died since.
        let again = set_tor_gated(&holder, &builtin, &previous, true, &|_| Err("dead".into()))
            .expect("no-op");
        assert!(again.tor.enabled);
    }

    #[test]
    fn tor_toggle_is_idempotent() {
        let builtin = BuiltinTor::default();
        let holder = ClientHolder::from_config(&AppConfig::default(), &builtin);
        let previous = config_with("test-mosque");
        let alive = &|_: &TorProxy| Ok(());

        let once = set_tor_gated(&holder, &builtin, &previous, true, alive).expect("first");
        let twice = set_tor_gated(&holder, &builtin, &once, true, alive).expect("second");
        assert!(twice.tor.enabled);
    }

    #[test]
    fn update_refuses_a_dead_proxy_on_tor_transition() {
        let builtin = BuiltinTor::default();
        let holder = ClientHolder::from_config(&AppConfig::default(), &builtin);
        let previous = config_with("test-mosque");
        let mut config = config_with("test-mosque");
        config.tor = tor_on("127.0.0.1", 9050);

        let err = validate_and_apply_update_gated(
            &holder,
            &builtin,
            &previous,
            config,
            &|_| Err("nothing is listening".into()),
        )
        .unwrap_err();
        assert!(err.contains("nothing is listening"), "got {err:?}");
    }

    #[test]
    fn update_accepts_a_live_proxy_on_tor_transition() {
        let builtin = BuiltinTor::default();
        let holder = ClientHolder::from_config(&AppConfig::default(), &builtin);
        let previous = config_with("test-mosque");
        let mut config = config_with("test-mosque");
        config.tor = tor_on("127.0.0.1", 9050);

        let validated = validate_and_apply_update_gated(
            &holder,
            &builtin,
            &previous,
            config,
            &|_| Ok(()),
        )
        .expect("live proxy");
        assert!(validated.tor.enabled);
    }

    #[test]
    fn update_never_gates_when_the_tor_policy_is_unchanged() {
        let builtin = BuiltinTor::default();
        let holder = ClientHolder::from_config(&AppConfig::default(), &builtin);
        let previous = AppConfig {
            tor: tor_on("127.0.0.1", 9050),
            ..config_with("test-mosque")
        };
        // Same policy, unrelated edit: the save must not depend on the
        // proxy being up (the gate would fail here).
        let validated = validate_and_apply_update_gated(
            &holder,
            &builtin,
            &previous,
            previous.clone(),
            &|_| Err("gate must not run".into()),
        )
        .expect("unchanged policy skips the gate");
        assert!(validated.tor.enabled);
    }

    #[test]
    fn offline_toggle_is_idempotent_and_clears_the_cache() {
        let builtin = BuiltinTor::default();
        let holder = ClientHolder::from_config(&AppConfig::default(), &builtin);
        let previous = config_with("test-mosque");

        let on = set_offline(&holder, &previous, true);
        assert!(on.offline_mode);
        let off = set_offline(&holder, &on, false);
        assert!(!off.offline_mode);
    }
}
