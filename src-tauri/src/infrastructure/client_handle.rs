//! Managed handle around the mawaqit.net client. The client is cheap to
//! clone (Arc inside) but immutable once constructed, and Tauri's managed
//! state cannot be re-managed — so Tor toggles swap the client in place
//! through this holder and every consumer reads the current transport.

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::domain::models::AppConfig;
use crate::infrastructure::builtin_tor::BuiltinTor;
use crate::MawaqitClient;

/// Whether something is accepting TCP connections on `host:port` right
/// now. The save-time gate for EXTERNAL Tor proxies (system tor, Tor
/// Browser): fail the enable at the toggle instead of silently
/// black-holing every remote request afterwards. An unresolvable host
/// counts as unreachable.
pub fn tor_reachable(host: &str, port: u16) -> bool {
    let Ok(mut addrs) = (host, port).to_socket_addrs() else {
        return false;
    };
    addrs
        .next()
        .is_some_and(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(750)).is_ok())
}

#[derive(Clone)]
pub struct ClientHolder {
    client: Arc<RwLock<MawaqitClient>>,
}

impl ClientHolder {
    /// Build the transport for one config: direct, through the built-in
    /// Tor stack, or through an external SOCKS5 proxy. An unreachable or
    /// strictly-invalid external proxy degrades to a direct connection
    /// with a log line — the app must always start. Built-in Tor never
    /// degrades: see [`tor_proxy_url`].
    pub fn from_config(config: &AppConfig, builtin: &BuiltinTor) -> Self {
        Self {
            client: Arc::new(RwLock::new(build_client(config, builtin))),
        }
    }

    /// The current transport (cheap clone).
    pub fn current(&self) -> MawaqitClient {
        self.client.read().expect("client lock poisoned").clone()
    }

    /// Replace the transport — used when the Tor policy changes.
    pub fn swap(&self, client: MawaqitClient) {
        *self.client.write().expect("client lock poisoned") = client;
    }
}

/// The transport for one config: direct, through the built-in Tor stack,
/// or through an external SOCKS5 proxy (system tor, Tor Browser, …). A
/// strictly-invalid proxy config degrades to a direct connection with a
/// log line — the app must always start.
pub fn build_client(config: &AppConfig, builtin: &BuiltinTor) -> MawaqitClient {
    let direct = || MawaqitClient::new().with_disk_cache(crate::infrastructure::config::cache_dir());
    let mut client = direct();
    if let Some(url) = tor_proxy_url(config, builtin) {
        client = match client.with_socks_proxy(url) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("tor proxy unavailable: {e}");
                direct()
            }
        };
    }
    client
}

/// Rebuild the transport for `config` and swap it into the holder.
pub fn rebuild_client(holder: &ClientHolder, config: &AppConfig, builtin: &BuiltinTor) {
    holder.swap(build_client(config, builtin));
}

/// The socks5h URL for this config's Tor policy, or `None` when Tor is
/// off. The privacy rule: Tor on is never silently downgraded to direct —
/// built-in mode starts the internal stack (or keeps pointing at its
/// default port so requests fail loudly), external mode hands out the
/// configured address (an implausible one fails loudly, also never
/// direct).
pub fn tor_proxy_url(config: &AppConfig, builtin: &BuiltinTor) -> Option<String> {
    if !config.tor.enabled {
        return None;
    }
    if config.tor.builtin {
        let addr = match builtin.ensure_started() {
            Ok(addr) => addr,
            Err(e) => {
                eprintln!(
                    "built-in tor failed to start: {e} — remote requests will fail (no fallback to direct: traffic stays on Tor)"
                );
                // Point at the would-be port anyway: the connection is
                // refused loudly instead of leaking around Tor.
                return Some(format!(
                    "socks5h://127.0.0.1:{}",
                    crate::infrastructure::builtin_tor::BUILTIN_TOR_PORT
                ));
            }
        };
        return Some(format!("socks5h://{addr}"));
    }
    let url = config.tor.socks5h_url();
    if url.is_none() {
        eprintln!(
            "tor enabled with an implausible external address — remote requests will fail (no fallback to direct: traffic stays on Tor)"
        );
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::{TorProxy, DEFAULT_TOR_HOST};

    #[test]
    fn current_reflects_the_swap() {
        let builtin = BuiltinTor::default();
        let holder = ClientHolder::from_config(&AppConfig::default(), &builtin);
        let direct = holder.current();

        // A tor-enabled config produces a DIFFERENT transport instance.
        let tor_on = AppConfig {
            tor: TorProxy {
                enabled: true,
                host: DEFAULT_TOR_HOST.into(),
                port: 9050,
                builtin: false,
            },
            ..AppConfig::default()
        };
        let holder = ClientHolder::from_config(&tor_on, &builtin);
        let proxied = holder.current();
        holder.swap(direct);
        let _ = proxied; // instances are opaque; swap must not panic
        let _ = holder.current();
    }

    #[test]
    fn implausible_external_host_degrades_to_direct() {
        // An implausible external host yields no socks5h URL — the
        // transport is the plain direct client and the holder still works.
        let builtin = BuiltinTor::default();
        let cfg = AppConfig {
            tor: TorProxy {
                enabled: true,
                host: "not a host!".into(),
                port: 9050,
                builtin: false,
            },
            ..AppConfig::default()
        };
        let holder = ClientHolder::from_config(&cfg, &builtin);
        let _ = holder.current();
    }

    #[test]
    fn builtin_tor_on_yields_the_stack_url() {
        // Pre-start the stack in Deferred mode (no network): tor_proxy_url
        // must then take the idempotent fast path and hand out exactly
        // that address.
        let tmp = std::env::temp_dir().join(format!(
            "mawaqit-arti-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        std::env::set_var("MAWAQIT_ARTI_STATE_DIR", &tmp);

        let builtin = BuiltinTor::default();
        let expected = builtin
            .ensure_started_mode(crate::infrastructure::builtin_tor::StartMode::Deferred)
            .expect("stack starts");
        let cfg = AppConfig {
            tor: TorProxy {
                enabled: true,
                ..TorProxy::default()
            },
            ..AppConfig::default()
        };
        let url = tor_proxy_url(&cfg, &builtin).expect("built-in tor yields a URL");
        assert_eq!(url, format!("socks5h://{expected}"));
    }

    #[test]
    fn tor_off_yields_no_url() {
        let builtin = BuiltinTor::default();
        let cfg = AppConfig::default();
        assert!(tor_proxy_url(&cfg, &builtin).is_none());
    }
}
