//! Built-in Tor: the app embeds the Tor Project's Rust client (Arti) and
//! exposes it to the api crate's HTTP stack as a local SOCKS5 listener —
//! so the Tor toggle needs zero external setup (no system tor, no Tor
//! Browser). One stack lives for the whole app lifetime: the toggle just
//! points the transport at it (or away), and a re-enable is instant.
//!
//! Bootstrap is non-blocking by design: `ensure_started` returns as soon
//! as the listener is bound (sub-second), while the client connects to the
//! Tor network in the background. Until it is ready, requests through the
//! bridge fail with a clear error — they never fall back to a direct
//! connection (privacy: the user routed this traffic through Tor).
//!
//! Only compiled with the `built-in-tor` feature; without it the type
//! still exists (call sites stay cfg-free) but enabling built-in Tor is a
//! clean error.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

#[cfg(feature = "built-in-tor")]
use {
    arti_client::TorClient,
    std::future::Future,
    std::pin::Pin,
};

/// Preferred loopback port for the internal SOCKS5 listener. Chosen to
/// avoid the system tor (9050) and Tor Browser (9150); if taken, the OS
/// assigns a free port instead.
pub const BUILTIN_TOR_PORT: u16 = 9058;

/// Whether the stack should connect to the Tor network on start. `Auto`
/// for the app; `Deferred` exists so tests can exercise the listener and
/// idempotence without touching the network (only test builds construct
/// it, hence the allow).
#[cfg(feature = "built-in-tor")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum StartMode {
    Auto,
    Deferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinTorStatus {
    /// No stack has been started (Tor never enabled this session).
    NotStarted,
    /// The listener is up; the client is still connecting to the Tor
    /// network (first run downloads the directory: can take a minute).
    Bootstrapping,
    /// Connected to the Tor network.
    Ready,
}

pub struct BuiltinTor {
    /// The live stack, if Tor was enabled at least once this session.
    stack: Mutex<Option<Stack>>,
}

struct Stack {
    socks_addr: SocketAddr,
    bootstrapped: Arc<std::sync::atomic::AtomicBool>,
}

impl Default for BuiltinTor {
    fn default() -> Self {
        Self {
            stack: Mutex::new(None),
        }
    }
}

impl BuiltinTor {
    /// Start the stack if not running and return the local SOCKS5 address
    /// the transport should use (`socks5h://<here>`). Sub-second: binding
    /// the listener is synchronous, Arti construction and the network
    /// bootstrap happen on a background task. Idempotent — an existing
    /// stack is reused (and stays warm across Tor off/on toggles).
    #[cfg(feature = "built-in-tor")]
    pub fn ensure_started(&self) -> Result<SocketAddr, String> {
        self.ensure_started_mode(StartMode::Auto)
    }

    /// [`Self::ensure_started`] with the bootstrap behavior injectable
    /// (`Deferred` never touches the network — the seam the tests use).
    #[cfg(feature = "built-in-tor")]
    pub(crate) fn ensure_started_mode(&self, mode: StartMode) -> Result<SocketAddr, String> {
        let mut guard = self.stack.lock().expect("builtin-tor lock poisoned");
        if let Some(stack) = guard.as_ref() {
            return Ok(stack.socks_addr);
        }

        // Bind synchronously (std, no runtime needed) so the caller gets
        // a real address before the background task has done anything: a
        // socks client connecting early just waits in the accept backlog
        // until the bridge serves.
        let std_listener = std::net::TcpListener::bind((host(), BUILTIN_TOR_PORT))
            .or_else(|_| std::net::TcpListener::bind((host(), 0)))
            .map_err(|e| format!("could not bind the built-in Tor listener: {e}"))?;
        let socks_addr = std_listener
            .local_addr()
            .map_err(|e| format!("built-in Tor listener has no address: {e}"))?;
        std_listener
            .set_nonblocking(true)
            .map_err(|e| format!("built-in Tor listener: {e}"))?;

        let bootstrapped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&bootstrapped);
        tauri::async_runtime::spawn(async move {
            // Runtime context: now the std listener can join the reactor.
            let listener = match tokio::net::TcpListener::from_std(std_listener) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("built-in tor stopped: could not register listener: {e}");
                    return;
                }
            };
            if let Err(e) = run_stack(listener, flag, mode).await {
                // The stack is dead; the listener is closed. Remote
                // requests fail until the next enable/restart — loud, not
                // silent, and never a fallback to direct.
                eprintln!("built-in tor stopped: {e}");
            }
        });

        if mode == StartMode::Auto {
            eprintln!(
                "built-in tor: listener on {socks_addr}, connecting to the Tor network in the background"
            );
        }
        *guard = Some(Stack {
            socks_addr,
            bootstrapped,
        });
        Ok(socks_addr)
    }

    /// Without the feature there is no embedded Tor: enabling built-in
    /// mode is a clean error, external-proxy mode still works.
    #[cfg(not(feature = "built-in-tor"))]
    pub fn ensure_started(&self) -> Result<SocketAddr, String> {
        Err("built-in Tor is not included in this build".into())
    }

    /// Where the transport should point for built-in mode, without
    /// starting anything. `None` when no stack is running.
    pub fn socks_addr(&self) -> Option<SocketAddr> {
        self.stack
            .lock()
            .expect("builtin-tor lock poisoned")
            .as_ref()
            .map(|s| s.socks_addr)
    }

    /// Connect-to-Tor-network progress, for logs and (future) UI.
    pub fn status(&self) -> BuiltinTorStatus {
        let guard = self.stack.lock().expect("builtin-tor lock poisoned");
        match guard.as_ref() {
            None => BuiltinTorStatus::NotStarted,
            Some(s) if s.bootstrapped.load(std::sync::atomic::Ordering::Relaxed) => {
                BuiltinTorStatus::Ready
            }
            Some(_) => BuiltinTorStatus::Bootstrapping,
        }
    }
}

#[cfg(feature = "built-in-tor")]
async fn run_stack(
    listener: tokio::net::TcpListener,
    bootstrapped: Arc<std::sync::atomic::AtomicBool>,
    mode: StartMode,
) -> Result<(), String> {
    use crate::infrastructure::socks_bridge::{self, RelayStream};

    let config_dir = arti_state_root();
    let (state_dir, cache_dir) = (config_dir.join("arti-state"), config_dir.join("arti-cache"));
    for dir in [&state_dir, &cache_dir] {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not create Tor state dir {}: {e}", dir.display()))?;
    }

    let config = arti_client::config::TorClientConfigBuilder::from_directories(
            state_dir, cache_dir,
        )
        .build()
        .map_err(|e| format!("built-in tor config: {e}"))?;

    // Unbootstrapped first: the listener must start accepting now; the
    // network connection continues in the background.
    let client = TorClient::builder()
        .config(config)
        .create_unbootstrapped_async()
        .await
        .map_err(|e| format!("built-in tor client: {e}"))?;

    if mode == StartMode::Auto {
        let client = Arc::clone(&client);
        tokio::spawn(async move {
            // Safe to retry later (per arti docs); a failed bootstrap
            // keeps the bridge up so the next request attempt surfaces a
            // transport error rather than a dead socket.
            match client.bootstrap().await {
                Ok(()) => {
                    bootstrapped.store(true, std::sync::atomic::Ordering::Relaxed);
                    eprintln!("built-in tor: connected to the Tor network");
                }
                Err(e) => eprintln!("built-in tor: bootstrap failed: {e}"),
            }
        });
    }

    // Fresh isolation per connection: two requests never share a circuit,
    // so concurrent UI data paths and voice downloads are not linkable.
    let connect = Arc::new(move |host: String, port: u16| {
        let client = client.isolated_client();
        Box::pin(async move {
            let stream = client
                .connect((host.as_str(), port))
                .await
                .map_err(|e| std::io::Error::other(format!("tor: {e}")))?;
            Ok(Box::new(stream) as Box<dyn RelayStream>)
        }) as Pin<Box<dyn Future<Output = std::io::Result<Box<dyn RelayStream>>> + Send>>
    });

    socks_bridge::serve(listener, connect).await;
    Ok(())
}

/// Root directory for Arti's persistent state (directory cache, client
/// state). Lives under the app config dir so uninstalling/clearing the
/// app config clears Tor's cache too. `MAWAQIT_ARTI_STATE_DIR` overrides
/// it (tests point it at a temp dir).
#[cfg(feature = "built-in-tor")]
fn arti_state_root() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("MAWAQIT_ARTI_STATE_DIR") {
        if !dir.trim().is_empty() {
            return std::path::PathBuf::from(dir);
        }
    }
    crate::infrastructure::config::config_dir()
}

/// Loopback host for the internal listener.
#[cfg(feature = "built-in-tor")]
fn host() -> std::net::IpAddr {
    use std::net::Ipv4Addr;
    Ipv4Addr::LOCALHOST.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_state_is_not_started() {
        let tor = BuiltinTor::default();
        assert_eq!(tor.status(), BuiltinTorStatus::NotStarted);
        assert!(tor.socks_addr().is_none());
    }

    #[cfg(not(feature = "built-in-tor"))]
    #[test]
    fn ensure_started_is_a_clean_error_without_the_feature() {
        let tor = BuiltinTor::default();
        let err = tor.ensure_started().unwrap_err();
        assert!(err.contains("not included in this build"), "got {err:?}");
    }

    #[cfg(feature = "built-in-tor")]
    #[test]
    fn ensure_started_binds_and_is_idempotent() {
        // Deferred start: listener + Arti construction only, never the
        // network. State lands in a temp dir, not the user's config.
        let tmp = std::env::temp_dir().join(format!(
            "mawaqit-arti-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        std::env::set_var("MAWAQIT_ARTI_STATE_DIR", &tmp);

        let tor = BuiltinTor::default();
        let addr = tor
            .ensure_started_mode(StartMode::Deferred)
            .expect("stack starts");
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        // Second call reuses the stack: same address, no second listener.
        let again = tor
            .ensure_started_mode(StartMode::Deferred)
            .expect("stack persists");
        assert_eq!(addr, again);
        assert!(matches!(tor.status(), BuiltinTorStatus::Bootstrapping));
        assert!(tor.socks_addr().is_some());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
