//! 数据管理 handler：导出、导入、重置。

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;

use crate::gateway::server::AppState;

/// GET /api/data/export — 返回 SQL 文本（JSON 包装）。
pub(crate) async fn export_data(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    match state.database.export_sql() {
        Ok(sql) => Ok(Json(serde_json::json!({ "sql": sql }))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )),
    }
}

#[derive(Deserialize)]
pub(crate) struct ImportDataRequest {
    sql: String,
}

/// POST /api/data/import — 接收 SQL 文本并执行导入。
pub(crate) async fn import_data(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ImportDataRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    match state.database.import_sql(&req.sql) {
        Ok(()) => Ok(Json(serde_json::json!({"status": "ok"}))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )),
    }
}

/// POST /api/data/reset — 清除所有用户数据（保留 schema_version）。
pub(crate) async fn reset_data(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    match state.database.reset_all_data() {
        Ok(()) => Ok(Json(serde_json::json!({"status": "ok"}))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )),
    }
}
