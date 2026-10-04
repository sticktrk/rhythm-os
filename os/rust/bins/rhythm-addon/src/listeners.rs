//! Network listeners for the add-on's public mobile API.

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

use socket2::SockRef;
use tokio::net::{TcpListener, TcpSocket};

#[derive(Debug)]
pub struct MobileListeners {
    ipv4: TcpListener,
    ipv6: Option<TcpListener>,
}

impl MobileListeners {
    pub async fn serve(self, router: axum::Router) -> io::Result<()> {
        let ipv4 = axum::serve(
            self.ipv4,
            router
                .clone()
                .into_make_service_with_connect_info::<SocketAddr>(),
        );
        match self.ipv6 {
            Some(listener) => {
                let ipv6 = axum::serve(
                    listener,
                    router.into_make_service_with_connect_info::<SocketAddr>(),
                );
                // Both servers belong to this future: shutdown or a serving
                // error drops both listeners, just like the admin server.
                tokio::select! {
                    result = ipv4 => result,
                    result = ipv6 => result,
                }
            }
            None => ipv4.await,
        }
    }
}

pub async fn bind_mobile(port: u16) -> io::Result<MobileListeners> {
    bind_mobile_with_ipv6(port, bind_ipv6).await
}

fn bind_ipv6(port: u16) -> io::Result<TcpListener> {
    let socket = TcpSocket::new_v6()?;
    socket.set_reuseaddr(true)?;
    // Supervisor's dual-stack network forwards IPv6 to the container's IPv6
    // address. A separate IPv6-only socket also preserves IPv4 peer addresses
    // for loopback tunnel provenance checks in the shared auth middleware.
    SockRef::from(&socket).set_only_v6(true)?;
    socket.bind((Ipv6Addr::UNSPECIFIED, port).into())?;
    socket.listen(1024)
}

async fn bind_mobile_with_ipv6(
    port: u16,
    open_ipv6: impl FnOnce(u16) -> io::Result<TcpListener>,
) -> io::Result<MobileListeners> {
    // IPv4 remains mandatory, including on systems with IPv6 disabled.
    let ipv4 = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).await?;
    let ipv6 = match open_ipv6(ipv4.local_addr()?.port()) {
        Ok(listener) => Some(listener),
        // Another service owning IPv6 must not turn startup into a misleading
        // IPv4-only success on the same advertised endpoint.
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => return Err(error),
        Err(error) => {
            log::warn!(target: "sys", "IPv6 mobile listener unavailable ({error}); using IPv4");
            None
        }
    };
    Ok(MobileListeners { ipv4, ipv6 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv6Addr, SocketAddr};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use crate::{http_server, mobile_access::MobileAccess};
    use rhythm_os::state::AppState;

    async fn assert_mobile_auth(listeners: MobileListeners, hosts: &[IpAddr]) {
        let port = listeners.ipv4.local_addr().unwrap().port();
        let mut app_state = AppState::default();
        app_state.platform_context = "ha_addon".into();
        let state = Arc::new(Mutex::new(app_state));
        let access = Arc::new(MobileAccess::new(&"a".repeat(64)).unwrap());
        let router = http_server::create_mobile_router(state, access);
        let server = tokio::spawn(async move {
            listeners.serve(router).await.unwrap();
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        for host in hosts {
            let endpoint = SocketAddr::new(*host, port);
            let health = client
                .get(format!("http://{endpoint}/health"))
                .send()
                .await
                .unwrap();
            assert_eq!(health.status(), reqwest::StatusCode::OK);
            assert_eq!(
                health.json::<serde_json::Value>().await.unwrap()["status"],
                "healthy"
            );
            // cloudflared uses the IPv4 loopback origin. Both real address
            // families must keep forwarding provenance and require a token.
            let status: serde_json::Value = client
                .get(format!("http://{endpoint}/api/auth/status"))
                .header("cf-connecting-ip", "203.0.113.1")
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(status["via_remote_access"], true);
            assert_eq!(status["requires_auth"], true);
            assert_eq!(status["claim_available"], false);
            for path in ["/api/state", "/api/events"] {
                let response = client
                    .get(format!("http://{endpoint}{path}"))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
                assert_eq!(
                    response.json::<serde_json::Value>().await.unwrap()["error"],
                    "Mobile bearer token required"
                );
            }
        }
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn mobile_health_and_auth_work_over_ipv4_and_ipv6() {
        // Hosts without IPv6 exercise the explicit fallback test instead.
        let Ok(ipv6_probe) = std::net::TcpListener::bind((Ipv6Addr::LOCALHOST, 0)) else {
            eprintln!("IPv6 is unavailable on this test host");
            return;
        };
        drop(ipv6_probe);
        let listener = bind_mobile(0).await.unwrap();
        assert_mobile_auth(
            listener,
            &[
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                IpAddr::V6(Ipv6Addr::LOCALHOST),
            ],
        )
        .await;
    }

    #[tokio::test]
    async fn unavailable_ipv6_preserves_ipv4_health_and_auth() {
        for kind in [io::ErrorKind::Unsupported, io::ErrorKind::AddrNotAvailable] {
            // Only the unavailable socket I/O is injected; the fallback binds
            // a real socket and serves the production mobile router.
            let listener = bind_mobile_with_ipv6(0, |_| Err(io::Error::from(kind)))
                .await
                .unwrap();
            assert!(listener.ipv6.is_none());
            assert!(listener.ipv4.local_addr().unwrap().is_ipv4());
            assert_mobile_auth(listener, &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).await;
        }
    }

    #[tokio::test]
    async fn ipv6_port_collision_does_not_silently_bind_only_ipv4() {
        let Ok(socket) = TcpSocket::new_v6() else {
            return;
        };
        SockRef::from(&socket).set_only_v6(true).unwrap();
        let Ok(()) = socket.bind((Ipv6Addr::UNSPECIFIED, 0).into()) else {
            return;
        };
        let occupied = socket.listen(16).unwrap();
        let port = occupied.local_addr().unwrap().port();
        // Prove IPv4 is free, so falling back would incorrectly claim success.
        drop(
            TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))
                .await
                .unwrap(),
        );
        assert_eq!(
            bind_mobile(port).await.unwrap_err().kind(),
            io::ErrorKind::AddrInUse
        );
        // Partial startup must release IPv4 again when IPv6 cannot bind.
        drop(
            TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))
                .await
                .unwrap(),
        );
    }

    #[tokio::test]
    async fn ipv4_port_collision_remains_a_startup_error() {
        let occupied = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).await.unwrap();
        assert_eq!(
            bind_mobile(occupied.local_addr().unwrap().port())
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::AddrInUse
        );
    }
}
