//! Prevent a WebSocket upstream from resolving back to the proxy listener.
use anyhow::{Context, Result};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

fn normalized(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(ip) => ip.to_ipv4_mapped().map_or(IpAddr::V6(ip), IpAddr::V4),
        ip @ IpAddr::V4(_) => ip,
    }
}

pub(super) fn reject_self(listener: SocketAddr, peer: SocketAddr) -> Result<()> {
    if listener.port() != peer.port() {
        return Ok(());
    }
    let bound = normalized(listener.ip());
    let target = normalized(peer.ip());
    let same = bound == target
        || (bound.is_unspecified() && {
            let compatible = bound.is_ipv6() || target.is_ipv4();
            compatible
                && (target.is_loopback()
                    || target.is_unspecified()
                    || nix::ifaddrs::getifaddrs()?.any(|interface| {
                        interface.address.is_some_and(|address| {
                            address
                                .as_sockaddr_in()
                                .is_some_and(|a| IpAddr::V4(a.ip()) == target)
                                || address
                                    .as_sockaddr_in6()
                                    .is_some_and(|a| normalized(IpAddr::V6(a.ip())) == target)
                        })
                    }))
        });
    if same {
        return Err(crate::ProxyConfigError(
            "proxy upstream resolves to its own TCP listener".into(),
        )
        .into());
    }
    Ok(())
}

pub(super) async fn validate(listener: SocketAddr, upstream: &str) -> Result<()> {
    if !upstream.starts_with("ws://") {
        return Ok(());
    }
    let url = url::Url::parse(upstream)?;
    let port = url
        .port_or_known_default()
        .context("missing upstream port")?;
    if port != listener.port() {
        return Ok(());
    }
    let host = crate::proxy::socket_host(&url)?;
    let addresses = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::net::lookup_host((host.as_str(), port)),
    )
    .await??;
    for address in addresses {
        reject_self(listener, address)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn listener_identity_handles_wildcards_mapped_ipv4_and_distinct_ports() {
        for (listen, peer) in [
            ("0.0.0.0:4500", "127.0.0.1:4500"),
            ("[::]:4500", "[::1]:4500"),
            ("[::]:4500", "127.0.0.1:4500"),
            ("127.0.0.1:4500", "[::ffff:127.0.0.1]:4500"),
        ] {
            assert!(reject_self(listen.parse().unwrap(), peer.parse().unwrap()).is_err());
        }
        for (listen, peer) in [
            ("127.0.0.1:4500", "127.0.0.1:4501"),
            ("127.0.0.1:4500", "[::1]:4500"),
            ("0.0.0.0:4500", "192.0.2.1:4500"),
        ] {
            assert!(reject_self(listen.parse().unwrap(), peer.parse().unwrap()).is_ok());
        }
    }
    #[tokio::test]
    async fn dns_alias_is_checked_against_actual_listener() {
        validate(
            "127.0.0.1:4500".parse().unwrap(),
            "ws://localhost:4500/other?x=1",
        )
        .await
        .expect_err("DNS alias to listener");
    }
    #[tokio::test]
    async fn actual_peer_check_runs_before_websocket_handshake() {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let error =
            crate::proxy::open_stream_checked(&format!("ws://{address}/other"), Some(address))
                .await
                .err()
                .unwrap();
        assert!(error.is::<crate::ProxyConfigError>());
        let (mut peer, _) = listener.accept().await.unwrap();
        assert_eq!(peer.read(&mut [0]).await.unwrap(), 0);
    }
}
