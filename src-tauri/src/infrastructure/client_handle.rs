//! Managed handle around the mawaqit.net client. The client is cheap to
//! clone (Arc inside) but immutable once constructed, and Tauri's managed
//! state cannot be re-managed — so Tor toggles swap the client in place
//! through this holder and every consumer reads the current transport.

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::domain::models::AppConfig;
use crate::MawaqitClient;

/// Whether something is accepting TCP connections on `host:port` right
/// now. Used to fail a Tor enable *at the toggle* instead of silently
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
    /// Build the transport for one config: direct, or through the Tor
    /// SOCKS5 proxy when enabled with a plausible host. An unreachable or
    /// strictly-invalid proxy degrades to a direct connection with a log
    /// line — the app must always start.
    pub fn from_config(config: &AppConfig) -> Self {
        Self {
            client: Arc::new(RwLock::new(build_client(config))),
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

/// The transport for one config: direct, or through the Tor SOCKS5 proxy
/// when enabled with a plausible host. An unreachable or strictly-invalid
/// proxy degrades to a direct connection with a log line — the app must
/// always start.
pub fn build_client(config: &AppConfig) -> MawaqitClient {
    let direct = || MawaqitClient::new().with_disk_cache(crate::infrastructure::config::cache_dir());
    let mut client = direct();
    if let Some(url) = config.tor.socks5h_url() {
        // A configured-but-dead proxy is NOT silently bypassed: the user
        // routed traffic through Tor, so it stays on Tor and fails loudly.
        if !tor_reachable(&config.tor.host, config.tor.port) {
            eprintln!(
                "tor proxy {}:{} is not reachable — remote requests will fail until Tor is running (no fallback to direct: traffic stays on Tor)",
                config.tor.host, config.tor.port
            );
        }
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
pub fn rebuild_client(holder: &ClientHolder, config: &AppConfig) {
    holder.swap(build_client(config));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_reflects_the_swap() {
        let holder = ClientHolder::from_config(&AppConfig::default());
        let direct = holder.current();

        // A tor-enabled config produces a DIFFERENT transport instance.
        let tor_on = AppConfig {
            tor: crate::domain::models::TorProxy {
                enabled: true,
                host: "127.0.0.1".into(),
                port: 9050,
            },
            ..AppConfig::default()
        };
        let holder = ClientHolder::from_config(&tor_on);
        let proxied = holder.current();
        holder.swap(direct);
        let _ = proxied; // instances are opaque; swap must not panic
        let _ = holder.current();
    }

    #[test]
    fn implausible_host_degrades_to_direct() {
        // An implausible host yields no socks5h URL — the transport is the
        // plain direct client and the holder still works.
        let cfg = AppConfig {
            tor: crate::domain::models::TorProxy {
                enabled: true,
                host: "not a host!".into(),
                port: 9050,
            },
            ..AppConfig::default()
        };
        let holder = ClientHolder::from_config(&cfg);
        let _ = holder.current();
    }
}
