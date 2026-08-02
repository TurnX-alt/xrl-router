//! Service Key 滚动窗口 token 配额：限额检查（429）与 /v1/user/balance 余额查询。
//!
//! 配额只持久化上限（service_keys.quota_5h / quota_7d），已用量按需从
//! usage_log 聚合，不维护额外计数器。

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde_json::{json, Value};

use crate::gateway::server::AppState;

use super::auth::verify_service_key;

/// 窗口常量（秒），与 usage::get_service_key_usage 保持一致。
pub(super) const FIVE_HOURS: i64 = 5 * 3600;
pub(super) const SEVEN_DAYS: i64 = 7 * 86400;

/// 把剩余秒数格式化为 `XdYh` / `XhYm` / `Ym`（近似读数，用于 UI 与余额端点）。
fn format_resets_in(remaining: i64) -> String {
    let r = remaining.max(0);
    let days = r / 86400;
    let hours = (r % 86400) / 3600;
    let mins = (r % 3600) / 60;
    if days > 0 {
        format!("{}d{}h", days, hours)
    } else if hours > 0 {
        format!("{}h{}m", hours, mins)
    } else {
        format!("{}m", mins.max(1))
    }
}

/// 检查 5h / 7d 滚动窗口配额。任一窗口 used >= limit（limit > 0）即超限。
/// 超限返回 Err(429 + retry-after 头 + quota_error JSON 体)，与代理层错误签名一致。
pub(super) async fn check_quota(
    state: &AppState,
    info: &crate::api::proxy::auth::ServiceKeyInfo,
) -> Result<(), (StatusCode, HeaderMap, Json<Value>)> {
    let now = chrono::Utc::now().timestamp();
    let (used_5h, used_7d) = state
        .database
        .get_service_key_usage(&info.id, now)
        .unwrap_or((0, 0));

    // (窗口标签, 窗口秒数)；7d 优先（重置更晚，客户端 retry 参考更保守）。
    let exceeded: Option<(&str, i64)> = if info.quota_7d > 0 && used_7d >= info.quota_7d {
        Some(("7d", SEVEN_DAYS))
    } else if info.quota_5h > 0 && used_5h >= info.quota_5h {
        Some(("5h", FIVE_HOURS))
    } else {
        None
    };

    if let Some((label, window_secs)) = exceeded {
        // 滚动窗口按「从现在往回数 window_secs」定义，重置发生在 now 之后
        // 的下一个整窗口边界，剩余秒数与窗口周期同余。
        let remaining = window_secs - (now % window_secs);
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", remaining.to_string().parse().unwrap());
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            headers,
            Json(json!({
                "error": {
                    "type": "quota_error",
                    "message": format!(
                        "Quota exceeded for this key ({} window). Resets in {}.",
                        label,
                        format_resets_in(remaining)
                    ),
                }
            })),
        ));
    }
    Ok(())
}

/// 当前滚动窗口的下一个重置边界（unix 秒）。窗口按 `now % window_secs` 对齐，
/// 与 check_quota 的 retry-after 口径一致。
fn window_reset_ts(now: i64, window_secs: i64) -> i64 {
    now + (window_secs - (now % window_secs))
}

/// unix 秒 → RFC3339（UTC）。CCSwitch query_zenmux 用 `as_str()` 读 resets_at，
/// 只接受 ISO 字符串；前端 countdownStr 用 `new Date(...)` 解析。
fn to_rfc3339(ts: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

/// GET /v1/user/balance — 查询 5h / 7d 窗口配额（不触发 429）。
///
/// 唯一消费者是 CCSwitch（Coding Plan / TokenPlan 模板的 ZenMux 分支），
/// 因此固定输出 ZenMux 兼容格式，卡片以徽章形式单行展示两个窗口：
/// ```json
/// { "success": true, "data": { "quota_5_hour": { "usage_percentage": 0.43, "resets_at": "2026-08-02T12:00:00Z" }, "quota_7_day": { ... } } }
/// ```
/// usage_percentage 为 0~1 小数，resets_at 为重置边界的 RFC3339 时间；
/// 未设限的窗口省略字段（CCSwitch 不渲染徽章）。
pub async fn user_balance(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let api_key = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| headers.get("x-api-key").and_then(|v| v.to_str().ok()))
        .unwrap_or("");

    let info = match verify_service_key(&state, api_key).await {
        Some(info) => info,
        None => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": {"type": "authentication_error", "message": "Invalid API key"}})),
            ))
        }
    };

    let now = chrono::Utc::now().timestamp();
    let (used_5h, used_7d) = state
        .database
        .get_service_key_usage(&info.id, now)
        .unwrap_or((0, 0));

    // 只有设限（limit > 0）的窗口才输出，未设限的窗口 CCSwitch 侧不渲染徽章。
    let mut data = serde_json::Map::new();
    if info.quota_5h > 0 {
        data.insert(
            "quota_5_hour".to_string(),
            json!({
                "usage_percentage": (used_5h as f64) / (info.quota_5h as f64),
                "resets_at": to_rfc3339(window_reset_ts(now, FIVE_HOURS)),
            }),
        );
    }
    if info.quota_7d > 0 {
        data.insert(
            "quota_7_day".to_string(),
            json!({
                "usage_percentage": (used_7d as f64) / (info.quota_7d as f64),
                "resets_at": to_rfc3339(window_reset_ts(now, SEVEN_DAYS)),
            }),
        );
    }
    Ok(Json(json!({ "success": true, "data": data })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_resets_in() {
        assert_eq!(format_resets_in(59), "1m");
        assert_eq!(format_resets_in(60), "1m");
        assert_eq!(format_resets_in(9061), "2h31m");
        assert_eq!(format_resets_in(2 * 86400 + 17 * 3600 + 59), "2d17h");
    }

    #[test]
    fn test_window_reset_ts_is_future_multiple() {
        // now = 1000，5h 窗口对齐到 18000 的倍数 → 下一个重置在 18000
        assert_eq!(window_reset_ts(1000, FIVE_HOURS), 18000);
        // 恰好落在边界上 → 下一周期
        assert_eq!(window_reset_ts(18000, FIVE_HOURS), 36000);
        // 7d 窗口
        assert_eq!(window_reset_ts(1_000_000, SEVEN_DAYS), 1_209_600);
    }
}
