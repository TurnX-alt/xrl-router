//! 上游错误响应（status >= 400）的透传转发 + 失败 usage_log 记录。

use std::time::Instant;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use crate::db::Database;
use tracing::warn;

/// Forward an upstream error response (status >= 400) to the client as-is,
/// rather than attempting to stream a non-SSE body. Also records a failed
/// usage_log row (success=false, zero tokens).
pub(super) async fn forward_upstream_error(
    database: &Database,
    provider_id: &str,
    provider_name: &str,
    model_id: &str,
    model_display_name: &str,
    key_id: Option<&str>,
    key_name: Option<&str>,
    key_masked: Option<&str>,
    service_key_id: Option<&str>,
    service_key_name: &str,
    service_key_masked: &str,
    request_type: &str,
    response: reqwest::Response,
    upstream_status: u16,
    trace_id: &str,
    start_time: &Instant,
) -> Response {
    let duration_ms = start_time.elapsed().as_millis();
    warn!(
        trace_id = %trace_id,
        upstream_status = upstream_status,
        duration_ms = duration_ms,
        "Upstream returned error, forwarding to client"
    );
    let err_body: Value = response
        .json()
        .await
        .unwrap_or_else(|_| json!({"error": {"type": "api_error", "message": "upstream error"}}));
    let code = StatusCode::from_u16(upstream_status).unwrap_or(StatusCode::BAD_GATEWAY);
    let _ = database.insert_usage_log(
        chrono::Utc::now().timestamp(),
        provider_id,
        provider_name,
        model_id,
        model_display_name,
        key_id,
        key_name.unwrap_or(""),
        key_masked.unwrap_or(""),
        service_key_id,
        service_key_name,
        service_key_masked,
        request_type,
        0,
        0,
        duration_ms as i64,
        false,
        Some(&format!("upstream status {}", upstream_status)),
        0,
    );
    (code, Json(err_body)).into_response()
}
