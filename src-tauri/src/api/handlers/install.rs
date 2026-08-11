//! 本机局域网 IP 查询。
//!
//! `get_local_ip` 供主机 Tauri UI 拼装分发链接，走管理 listener（127.0.0.1:19068）。

use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde_json::json;

use crate::gateway::server::AppState;

/// GET /api/install/local-ip — 返回本机非 loopback 出口 IP 和端口（管理端口）。
/// UDP socket 连 8.8.8.8:80（不发数据）取本机出口地址，过滤回环。
pub(crate) async fn get_local_ip(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let ip = detect_local_ip();
    let port = state.config.port;
    Json(json!({ "ip": ip, "port": port }))
}

fn detect_local_ip() -> Option<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    sock.local_addr()
        .ok()
        .map(|a| a.ip().to_string())
        .filter(|s| !s.starts_with("127."))
}
