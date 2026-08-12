//! Admin IP guard middleware — restricts endpoints to loopback (localhost) access only.

use axum::extract::connect_info::ConnectInfo;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::net::SocketAddr;
use tracing::warn;

/// Middleware that rejects non-loopback clients with 403 Forbidden.
///
/// Applied to `/api/*` management endpoints so that only the local machine
/// (Tauri WebView, localhost CLI tools) can reach admin APIs.
/// Requires the server to use `into_make_service_with_connect_info::<SocketAddr>()`.
pub async fn admin_ip_guard(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    // 双栈 listener（v6only=false）上 IPv4 连接以 ::ffff:127.0.0.1 形式出现，
    // 需 to_canonical() 归一化后再判 loopback，否则本机请求被误拒 403
    let ip = addr.ip();
    if !(ip.is_loopback() || ip.to_canonical().is_loopback()) {
        warn!(
            path = %request.uri().path(),
            client_ip = %addr.ip(),
            "admin endpoint rejected non-loopback request"
        );
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    fn app_with_guard() -> Router {
        Router::new()
            .route("/api/test", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(admin_ip_guard))
    }

    #[tokio::test]
    async fn test_loopback_allowed() {
        use axum::extract::connect_info::MockConnectInfo;

        let app = app_with_guard().layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_non_loopback_forbidden() {
        use axum::extract::connect_info::MockConnectInfo;

        let app = app_with_guard().layer(MockConnectInfo(SocketAddr::from(([192, 168, 1, 50], 0))));

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    /// 双栈 listener 下 IPv4 回环连接以 ::ffff:127.0.0.1 呈现，必须放行。
    #[tokio::test]
    async fn test_ipv4_mapped_loopback_allowed() {
        use axum::extract::connect_info::MockConnectInfo;

        // ::ffff:127.0.0.1
        let ipv4_mapped_loopback = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 127, 0, 0, 1];
        let app = app_with_guard().layer(MockConnectInfo(SocketAddr::from((ipv4_mapped_loopback, 0))));

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// 双栈下非回环 IPv4-mapped 地址仍应 403。
    #[tokio::test]
    async fn test_ipv4_mapped_non_loopback_forbidden() {
        use axum::extract::connect_info::MockConnectInfo;

        // ::ffff:192.168.1.50
        let ipv4_mapped = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 192, 168, 1, 50];
        let app = app_with_guard().layer(MockConnectInfo(SocketAddr::from((ipv4_mapped, 0))));

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    /// 纯 IPv6 回环 ::1 也应放行。
    #[tokio::test]
    async fn test_ipv6_loopback_allowed() {
        use axum::extract::connect_info::MockConnectInfo;

        let app = app_with_guard().layer(MockConnectInfo(SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 0))));

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
