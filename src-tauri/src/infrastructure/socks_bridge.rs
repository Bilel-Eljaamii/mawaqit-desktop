//! Minimal SOCKS5 CONNECT bridge between a local TCP listener and an
//! embedded transport (production: the built-in Tor client). The api
//! crate's reqwest client (`socks` feature) is the only client that ever
//! talks to this listener — it binds to loopback only — so the protocol
//! surface is deliberately tiny: no auth methods beyond "none", no BIND,
//! no UDP ASSOCIATE.
//!
//! Hostnames are passed through **unresolved** (socks5h semantics): DNS
//! never happens on this side of the bridge, so nothing leaks before the
//! traffic enters the tunnel.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A relay-able stream: what a connector hands back for one CONNECT
/// request. A trait object because the two implementations (Tor
/// `DataStream`, test `TcpStream`) are unrelated types, and Rust trait
/// objects cannot combine two non-auto traits directly.
pub trait RelayStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> RelayStream for T {}

/// Opens `(hostname, port)` through the transport behind the bridge. The
/// future must be `Send` — each CONNECT is served on its own task.
pub type Connector = Arc<
    dyn Fn(String, u16) -> Pin<Box<dyn Future<Output = std::io::Result<Box<dyn RelayStream>>> + Send>>
        + Send
        + Sync,
>;

/// Accept forever, serving each connection on its own task. Accept errors
/// are logged and retried — a transient listener hiccup must not take the
/// bridge down.
pub async fn serve(listener: TcpListener, connect: Connector) {
    loop {
        match listener.accept().await {
            Ok((sock, _peer)) => {
                let connect = Arc::clone(&connect);
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(sock, connect).await {
                        eprintln!("socks bridge connection ended: {e}");
                    }
                });
            }
            Err(e) => eprintln!("socks bridge accept failed: {e}"),
        }
    }
}

/// Serve one SOCKS5 client connection end to end: handshake, CONNECT
/// request, relay. Every failure path closes the socket — the client (our
/// own reqwest) surfaces a connect error, nothing more.
async fn handle_connection(
    mut sock: TcpStream,
    connect: Connector,
) -> Result<(), std::io::Error> {
    let (host, port) = negotiate(&mut sock).await?;
    let mut target = match (connect)(host, port).await {
        Ok(stream) => stream,
        Err(e) => {
            // 01 = general SOCKS server failure (the transport refused or
            // is not ready — e.g. Tor still bootstrapping).
            let _ = sock
                .write_all(&[0x05, 0x01, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await;
            return Err(std::io::Error::other(format!("connect failed: {e}")));
        }
    };
    // Success: BND.ADDR/BND.PORT are meaningless for our client, zeroed.
    sock.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await?;
    let _ = tokio::io::copy_bidirectional(&mut sock, &mut target).await;
    Ok(())
}

/// SOCKS5 greeting + CONNECT request. Returns the (unresolved) target.
async fn negotiate(sock: &mut TcpStream) -> Result<(String, u16), std::io::Error> {
    // Greeting: VER NMETHODS METHODS...
    let mut head = [0u8; 2];
    sock.read_exact(&mut head).await?;
    if head[0] != 0x05 {
        return Err(std::io::Error::other(format!(
            "bad socks version {:#x}",
            head[0]
        )));
    }
    let mut methods = vec![0u8; head[1] as usize];
    sock.read_exact(&mut methods).await?;
    if !methods.contains(&0x00) {
        // 0xFF = no acceptable methods.
        let _ = sock.write_all(&[0x05, 0xFF]).await;
        return Err(std::io::Error::other("client offered no no-auth method"));
    }
    sock.write_all(&[0x05, 0x00]).await?;

    // Request: VER CMD RSV ATYP ADDR PORT
    let mut req = [0u8; 4];
    sock.read_exact(&mut req).await?;
    if req[0] != 0x05 {
        return Err(std::io::Error::other(format!(
            "bad socks version {:#x} in request",
            req[0]
        )));
    }
    if req[1] != 0x01 {
        // 07 = command not supported (we only implement CONNECT).
        let _ = sock
            .write_all(&[0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
            .await;
        return Err(std::io::Error::other(format!(
            "unsupported socks command {:#x}",
            req[1]
        )));
    }
    let host = match req[3] {
        0x01 => {
            let mut octets = [0u8; 4];
            sock.read_exact(&mut octets).await?;
            std::net::Ipv4Addr::from(octets).to_string()
        }
        0x03 => {
            let mut len = [0u8; 1];
            sock.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            sock.read_exact(&mut name).await?;
            String::from_utf8(name)
                .map_err(|_| std::io::Error::other("non-utf8 socks hostname"))?
        }
        0x04 => {
            let mut octets = [0u8; 16];
            sock.read_exact(&mut octets).await?;
            std::net::Ipv6Addr::from(octets).to_string()
        }
        other => {
            // 08 = address type not supported.
            let _ = sock
                .write_all(&[0x05, 0x08, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await;
            return Err(std::io::Error::other(format!(
                "unsupported socks address type {other:#x}"
            )));
        }
    };
    let mut port = [0u8; 2];
    sock.read_exact(&mut port).await?;
    Ok((host, u16::from_be_bytes(port)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    /// A connector that tunnels to a local "upstream" echo server, so the
    /// relay is exercised in both directions without any real network.
    /// Returns the upstream's address (where the connector always lands,
    /// whatever host the socks client asked for).
    async fn echo_upstream() -> (std::net::SocketAddr, Connector) {
        let upstream = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = upstream.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = upstream.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let (mut reader, mut writer) = sock.split();
                    let _ = tokio::io::copy(&mut reader, &mut writer).await;
                });
            }
        });
        let connector: Connector = Arc::new(move |_host, _port| {
            let addr = addr;
            Box::pin(async move {
                Ok(Box::new(TcpStream::connect(addr).await?) as Box<dyn RelayStream>)
            })
        });
        (addr, connector)
    }

    /// Drive a raw SOCKS5 client against the bridge and return the reply
    /// to the CONNECT request plus the relayed socket (None when the
    /// bridge closed without replying).
    async fn socks_client(
        bridge: std::net::SocketAddr,
        target: &[u8],
        cmd: u8,
    ) -> (Option<Vec<u8>>, Option<TcpStream>) {
        let mut sock = TcpStream::connect(bridge).await.unwrap();
        sock.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
        let mut reply = [0u8; 2];
        sock.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply, [0x05, 0x00], "no-auth must be accepted");

        let mut req = vec![0x05, cmd, 0x00];
        req.extend_from_slice(target);
        req.extend_from_slice(&80u16.to_be_bytes());
        sock.write_all(&req).await.unwrap();

        let mut connect_reply = vec![0u8; 10];
        match sock.read_exact(&mut connect_reply).await {
            Ok(_) => (Some(connect_reply), Some(sock)),
            Err(_) => (None, None),
        }
    }

    #[tokio::test]
    async fn relays_both_directions_after_connect() {
        let (_guard, connector) = echo_upstream().await;
        let bridge = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let bridge_addr = bridge.local_addr().unwrap();
        tokio::spawn(serve(bridge, connector));

        // Domain target — the bridge must pass it through unresolved.
        let mut target = vec![0x03u8, 7];
        target.extend_from_slice(b"example");
        let (reply, sock) = socks_client(bridge_addr, &target, 0x01).await;
        let reply = reply.expect("success reply expected");
        assert_eq!(&reply[..3], &[0x05, 0x00, 0x00]);
        let mut sock = sock.unwrap();

        sock.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        sock.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping", "echo round-trip through the bridge");
    }

    #[tokio::test]
    async fn rejects_bind_with_command_not_supported() {
        let (_guard, connector) = echo_upstream().await;
        let bridge = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let bridge_addr = bridge.local_addr().unwrap();
        tokio::spawn(serve(bridge, connector));

        let target = [0x01u8, 127, 0, 0, 1];
        let (reply, sock) = socks_client(bridge_addr, &target, 0x02).await; // BIND
        let reply = reply.expect("BIND must still get a reply before close");
        assert_eq!(&reply[..2], &[0x05, 0x07], "07 = command not supported");
        drop(sock);
    }

    #[tokio::test]
    async fn rejects_unknown_address_type() {
        let (_guard, connector) = echo_upstream().await;
        let bridge = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let bridge_addr = bridge.local_addr().unwrap();
        tokio::spawn(serve(bridge, connector));

        let (reply, _) = socks_client(bridge_addr, &[0x09u8, 1, 2], 0x01).await;
        let reply = reply.expect("bad ATYP must still get a reply");
        assert_eq!(&reply[..2], &[0x05, 0x08], "08 = address type not supported");
    }

    #[tokio::test]
    async fn connector_failure_replies_general_failure() {
        let failing: Connector = Arc::new(|_host, _port| {
            Box::pin(async {
                Err(std::io::Error::other("transport not ready"))
            })
        });
        let bridge = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let bridge_addr = bridge.local_addr().unwrap();
        tokio::spawn(serve(bridge, failing));

        let target = [0x01u8, 127, 0, 0, 1];
        let (reply, _) = socks_client(bridge_addr, &target, 0x01).await;
        let reply = reply.expect("connector failure must still get a reply");
        assert_eq!(&reply[..2], &[0x05, 0x01], "01 = general failure");
    }

    #[tokio::test]
    async fn closes_on_bad_version_without_reply() {
        let (_guard, connector) = echo_upstream().await;
        let bridge = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let bridge_addr = bridge.local_addr().unwrap();
        tokio::spawn(serve(bridge, connector));

        let mut sock = TcpStream::connect(bridge_addr).await.unwrap();
        sock.write_all(&[0x04, 0x01, 0x00]).await.unwrap(); // SOCKS4 greeting
        let mut buf = [0u8; 2];
        let res = sock.read_exact(&mut buf).await;
        assert!(res.is_err(), "bad version must close without a reply");
    }
}
