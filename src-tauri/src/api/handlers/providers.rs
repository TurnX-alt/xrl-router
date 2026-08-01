//! Provider 管理 handler（CRUD + KeyPool/registry 内存同步）。

use std::sync::Arc;

use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::Utc;
use serde::Deserialize;

use crate::gateway::server::AppState;
use crate::types::{Provider, ProviderKind};

pub(crate) async fn list_providers(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let providers = state.providers.list_all();
    Json(providers)
}

#[derive(Deserialize)]
pub(crate) struct CreateProviderRequest {
    name: String,
    kind: String,
    base_url: String,
    api_path: Option<String>,
    config: Option<serde_json::Value>,
}

pub(crate) async fn create_provider(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateProviderRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let provider = Provider {
        id: uuid::Uuid::new_v4().to_string(),
        name: req.name,
        kind: ProviderKind::from_str(&req.kind),
        base_url: req.base_url,
        api_path: req.api_path.unwrap_or_else(|| "/v1/chat/completions".to_string()),
        config: req.config.unwrap_or_else(|| serde_json::json!({})),
        enabled: true,
        created_at: Utc::now().timestamp(),
        updated_at: Utc::now().timestamp(),
    };

    state.providers.insert(provider.clone());

    if let Err(e) = state.database.save_provider(&provider) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ));
    }

    Ok((StatusCode::CREATED, Json(provider)))
}

pub(crate) async fn get_provider(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    match state.providers.get(&id) {
        Some(provider) => Ok(Json(provider)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Provider not found"})),
        )),
    }
}

#[derive(Deserialize)]
pub(crate) struct UpdateProviderRequest {
    name: Option<String>,
    kind: Option<String>,
    base_url: Option<String>,
    api_path: Option<String>,
    config: Option<serde_json::Value>,
    enabled: Option<bool>,
}

pub(crate) async fn update_provider(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateProviderRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let provider = match state.providers.get(&id) {
        Some(p) => p,
        None => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "Provider not found"})),
            ))
        }
    };

    let mut updated = provider.clone();
    if let Some(name) = req.name {
        updated.name = name;
    }
    if let Some(kind) = req.kind {
        updated.kind = ProviderKind::from_str(&kind);
    }
    if let Some(base_url) = req.base_url {
        updated.base_url = base_url;
    }
    if let Some(api_path) = req.api_path {
        updated.api_path = api_path;
    }
    if let Some(config) = req.config {
        updated.config = config;
    }
    if let Some(enabled) = req.enabled {
        updated.enabled = enabled;
    }
    updated.updated_at = Utc::now().timestamp();

    state.providers.insert(updated.clone());

    if let Err(e) = state.database.save_provider(&updated) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ));
    }

    Ok(Json(updated))
}

pub(crate) async fn delete_provider(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if !state.providers.contains(&id) {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Provider not found"})),
        ));
    }

    state.providers.remove(&id);
    // 同步 KeyPool 内存：移除该 provider 的密钥 + 轮询指针。
    state.keys.remove_provider(&id);

    if let Err(e) = state.database.delete_provider(&id) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ));
    }

    Ok(Json(serde_json::json!({"status": "deleted"})))
}
