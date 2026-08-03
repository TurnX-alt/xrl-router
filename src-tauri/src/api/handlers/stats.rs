//! 统计聚合 + 应用设置（websearch_hijack 开关）handler。

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use chrono::Utc;
use serde::Deserialize;

use crate::gateway::server::AppState;

#[derive(Deserialize)]
pub(crate) struct StatsQuery {
    from: Option<i64>,
    to: Option<i64>,
    /// "hour" -> hourly buckets, anything else -> daily buckets.
    granularity: Option<String>,
    /// Local timezone offset in seconds (e.g. UTC+8 = 28800), so buckets align
    /// to local day/hour boundaries instead of UTC.
    tz_offset: Option<i64>,
}

#[derive(Deserialize)]
pub(crate) struct LogQuery {
    page: Option<i64>,
    page_size: Option<i64>,
}

/// GET /api/stats/requests — 请求日志分页（时间逆序），每页默认 10 条。
pub(crate) async fn get_stats_requests(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LogQuery>,
) -> impl IntoResponse {
    let page = params.page.unwrap_or(1).max(1);
    let page_size = params.page_size.unwrap_or(10).clamp(1, 100);
    let (total, data) = state
        .database
        .get_usage_log_page(page, page_size)
        .unwrap_or_else(|_| (0, Vec::new()));
    Json(serde_json::json!({
        "total": total,
        "page": page,
        "page_size": page_size,
        "data": data,
    }))
}

pub(crate) async fn get_stats(
    State(state): State<Arc<AppState>>,
    Query(params): Query<StatsQuery>,
) -> impl IntoResponse {
    let now = Utc::now().timestamp();
    // Defaults to the last 24h when the client omits the range.
    let from = params.from.unwrap_or(now - 86400);
    let to = params.to.unwrap_or(now);
    let bucket_seconds: i64 = match params.granularity.as_deref() {
        Some("hour") => 3600,
        _ => 86400,
    };
    let tz_offset = params.tz_offset.unwrap_or(0);
    let data = state
        .database
        .get_usage_by_day_and_key(from, to, bucket_seconds, tz_offset)
        .unwrap_or_default();
    let model_usage = state
        .database
        .get_usage_by_model(from, to)
        .unwrap_or_default();
    let top_model = model_usage.first().cloned();

    Json(serde_json::json!({ "data": data, "top_model": top_model }))
}

pub(crate) async fn get_settings(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(serde_json::json!({
        "websearch_hijack": state.websearch_hijack.load(std::sync::atomic::Ordering::Relaxed),
        "failover_enabled": state.failover_enabled.load(std::sync::atomic::Ordering::Relaxed),
    }))
}

#[derive(Deserialize)]
pub(crate) struct UpdateSettingsRequest {
    websearch_hijack: Option<bool>,
    failover_enabled: Option<bool>,
}

pub(crate) async fn update_settings(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateSettingsRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if let Some(v) = req.websearch_hijack {
        state.websearch_hijack.store(v, std::sync::atomic::Ordering::Relaxed);
        let val = if v { "true" } else { "false" };
        if let Err(e) = state.database.set_setting("websearch_hijack", val) {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            ));
        }
    }
    if let Some(v) = req.failover_enabled {
        state.failover_enabled.store(v, std::sync::atomic::Ordering::Relaxed);
        let val = if v { "true" } else { "false" };
        if let Err(e) = state.database.set_setting("failover_enabled", val) {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            ));
        }
    }
    Ok(Json(serde_json::json!({"status": "ok"})))
}
