//! 健康检查端点：聚合 providers / models / keys / database 状态。

use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;
use chrono::Utc;

use crate::gateway::server::AppState;

pub(crate) async fn health_check(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let enabled_providers = state.providers.get_enabled();
    let total_providers = state.providers.len();
    let total_models = state.models.len();

    // Check database connectivity
    let db_status = match state.database.test_connection() {
        Ok(_) => "ok",
        Err(_) => "error",
    };

    // Get key pool stats
    let mut key_stats = std::collections::HashMap::new();
    for provider in &enabled_providers {
        if let Some(stats) = state.keys.get_stats(&provider.id) {
            key_stats.insert(
                provider.name.clone(),
                serde_json::json!({
                    "total": stats.total,
                    "green": stats.green,
                    "yellow": stats.yellow,
                    "red": stats.red,
                }),
            );
        }
    }

    Json(serde_json::json!({
        "status": "ok",
        "service": "xrl-router",
        "version": env!("CARGO_PKG_VERSION"),
        "timestamp": Utc::now().timestamp(),
        "database": db_status,
        "providers": {
            "total": total_providers,
            "enabled": enabled_providers.len(),
        },
        "models": {
            "total": total_models,
        },
        "keys": key_stats,
    }))
}
