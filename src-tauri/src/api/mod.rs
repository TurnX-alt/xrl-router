pub mod proxy;

use crate::db::Database;
use crate::gateway::server::AppState;
use crate::keys::KeyPool;
use crate::middleware::rate_limit::rate_limit_middleware;
use crate::models::ModelRegistry;
use crate::providers::ProviderRegistry;
use crate::types::{ApiKey, Model, Provider, ProviderKind};
use axum::{
    extract::{ws::{WebSocket, WebSocketUpgrade, Message}, Json, Path, Query, State},
    http::{HeaderMap, StatusCode},
    middleware,
    response::IntoResponse,
    routing::{delete, get, post, put},
    Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use argon2::{
    password_hash::{
        rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
    },
    Argon2,
};
use std::sync::Arc;

/// Build the router with all routes
pub fn build_router(state: Arc<AppState>) -> Router {
    // Rate-limited proxy endpoints
    let proxy_routes = Router::new()
        .route("/v1/chat/completions", post(proxy::proxy_openai_chat))
        .route("/v1/messages", post(proxy::proxy_anthropic_messages))
        .route("/v1/models", get(proxy::proxy_list_models))
        .layer(middleware::from_fn_with_state(
            state.rate_limiter.clone(),
            rate_limit_middleware,
        ));

    Router::new()
        // Health check
        .route("/health", get(health_check))
        .route("/", get(health_check))
        // WebSocket endpoint (no rate limiting)
        .route("/ws", get(ws_handler))
        // Provider management
        .route("/api/providers", get(list_providers).post(create_provider))
        .route(
            "/api/providers/:id",
            get(get_provider).put(update_provider).delete(delete_provider),
        )
        // API Key management
        .route("/api/keys", get(list_keys).post(create_key))
        .route(
            "/api/keys/:id",
            get(get_key).put(update_key).delete(delete_key),
        )
        // Model management
        .route("/api/models", get(list_models).post(create_model))
        .route(
            "/api/models/:id",
            get(get_model).put(update_model).delete(delete_model),
        )
        // Fetch upstream models (proxy to avoid CORS and inject API key)
        .route("/api/proxy/models", get(proxy_fetch_models))
        // Statistics
        .route("/api/stats", get(get_stats))
        // Service Key management (SHA-256 hashed)
        .route("/api/service-keys", get(list_service_keys).post(create_service_key))
        .route("/api/service-keys/:id", put(update_service_key).delete(delete_service_key))
        // App settings
        .route("/api/settings", get(get_settings).put(update_settings))
        // Merge rate-limited proxy routes
        .merge(proxy_routes)
        .with_state(state)
}

// ============================================================================
// Health Check
// ============================================================================

async fn health_check(State(state): State<Arc<AppState>>) -> impl IntoResponse {
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

// ============================================================================
// Provider CRUD
// ============================================================================

async fn list_providers(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let providers = state.providers.list_all();
    Json(providers)
}

#[derive(Deserialize)]
struct CreateProviderRequest {
    name: String,
    kind: String,
    base_url: String,
    api_path: Option<String>,
    config: Option<serde_json::Value>,
}

async fn create_provider(
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

async fn get_provider(
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
struct UpdateProviderRequest {
    name: Option<String>,
    kind: Option<String>,
    base_url: Option<String>,
    api_path: Option<String>,
    config: Option<serde_json::Value>,
    enabled: Option<bool>,
}

async fn update_provider(
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

async fn delete_provider(
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

    if let Err(e) = state.database.delete_provider(&id) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ));
    }

    Ok(Json(serde_json::json!({"status": "deleted"})))
}

// ============================================================================
// API Key CRUD
// ============================================================================

#[derive(Deserialize)]
struct ProviderFilter {
    provider_id: Option<String>,
}

async fn list_keys(
    State(state): State<Arc<AppState>>,
    Query(filter): Query<ProviderFilter>,
) -> impl IntoResponse {
    let keys = state.database.list_all_keys().unwrap_or_default();

    let filtered = match &filter.provider_id {
        Some(id) => keys.into_iter().filter(|k| k.provider_id == *id).collect::<Vec<_>>(),
        None => keys,
    };

    let keys_with_plain: Vec<serde_json::Value> = filtered
        .into_iter()
        .map(|k| {
            let plain = crate::crypto::decrypt(&k.key_hash, &state.master_key).ok();
            let mut val = serde_json::to_value(&k).unwrap_or_default();
            if let Some(obj) = val.as_object_mut() {
                // 可用性走内存：用 pool 实时状态覆盖 DB status 残留
                if let Some(live) = state.keys.get_key_status(&k.id) {
                    obj.insert("status".to_string(), serde_json::json!(live.to_string()));
                }
                obj.insert("key_plain".to_string(), serde_json::json!(plain));
            }
            val
        })
        .collect();
    Json(keys_with_plain)
}

#[derive(Deserialize)]
struct CreateKeyRequest {
    provider_id: String,
    name: String,
    key: String,
}

async fn create_key(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateKeyRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if !state.providers.contains(&req.provider_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Provider not found"})),
        ));
    }

    let masked = if req.key.len() > 8 {
        format!("{}...{}", &req.key[..4], &req.key[req.key.len() - 4..])
    } else {
        "***".to_string()
    };

    // Encrypt the raw key at rest (AES-256-GCM); key_hash stores ciphertext.
    let key_cipher = match crate::crypto::encrypt(&req.key, &state.master_key) {
        Ok(c) => c,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            ))
        }
    };

    let api_key = ApiKey {
        id: uuid::Uuid::new_v4().to_string(),
        provider_id: req.provider_id,
        name: req.name,
        key_hash: key_cipher,
        key_masked: masked,
        key_plain: None,
        status: "green".to_string(),
        last_error: None,
        last_error_code: None,
        last_error_time: None,
        last_used_at: None,
        balance: None,
        balance_updated_at: None,
        total_requests: 0,
        total_tokens: 0,
        created_at: Utc::now().timestamp(),
        updated_at: Utc::now().timestamp(),
    };

    if let Err(e) = state.database.save_api_key(&api_key) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ));
    }

    Ok((StatusCode::CREATED, Json(api_key)))
}

async fn get_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    match state.database.get_api_key(&id) {
        Ok(Some(key)) => Ok(Json(key)),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Key not found"})),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )),
    }
}

#[derive(Deserialize)]
struct UpdateKeyRequest {
    name: Option<String>,
    status: Option<String>,
}

async fn update_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateKeyRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let key = match state.database.get_api_key(&id) {
        Ok(Some(k)) => k,
        Ok(None) => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "Key not found"})),
            ))
        }
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            ))
        }
    };

    let mut updated = key.clone();
    if let Some(name) = req.name {
        updated.name = name;
    }
    if let Some(status) = req.status {
        updated.status = status;
    }
    updated.updated_at = Utc::now().timestamp();

    if let Err(e) = state.database.save_api_key(&updated) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ));
    }

    Ok(Json(updated))
}

async fn delete_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if let Err(e) = state.database.delete_api_key(&id) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ));
    }

    Ok(Json(serde_json::json!({"status": "deleted"})))
}

// ============================================================================
// Model CRUD
// ============================================================================

async fn list_models(
    State(state): State<Arc<AppState>>,
    Query(filter): Query<ProviderFilter>,
) -> impl IntoResponse {
    let models = state.database.list_all_models().unwrap_or_default();

    let filtered = match &filter.provider_id {
        Some(id) => models
            .into_iter()
            .filter(|m| m.provider_id == *id)
            .collect::<Vec<_>>(),
        None => models,
    };

    Json(filtered)
}

#[derive(Deserialize)]
struct CreateModelRequest {
    provider_id: String,
    model_id: String,
    display_name: String,
    tier: String,
    context_window: Option<i64>,
    max_output_tokens: Option<i64>,
    cost_per_1k_input: Option<f64>,
    cost_per_1k_output: Option<f64>,
    capabilities: Option<String>,
}

async fn create_model(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateModelRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if !state.providers.contains(&req.provider_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Provider not found"})),
        ));
    }

    let model = Model {
        id: uuid::Uuid::new_v4().to_string(),
        provider_id: req.provider_id,
        model_id: req.model_id,
        display_name: req.display_name,
        tier: req.tier,
        context_window: req.context_window.unwrap_or(128000),
        max_output_tokens: req.max_output_tokens.unwrap_or(4096),
        cost_per_1k_input: req.cost_per_1k_input.unwrap_or(0.0),
        cost_per_1k_output: req.cost_per_1k_output.unwrap_or(0.0),
        capabilities: req.capabilities.unwrap_or_else(|| "[\"text\"]".to_string()),
        enabled: true,
        created_at: Utc::now().timestamp(),
        updated_at: Utc::now().timestamp(),
    };

    if let Err(e) = state.database.save_model(&model) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ));
    }

    Ok((StatusCode::CREATED, Json(model)))
}

async fn get_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    match state.database.get_model(&id) {
        Ok(Some(model)) => Ok(Json(model)),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Model not found"})),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )),
    }
}

#[derive(Deserialize)]
struct UpdateModelRequest {
    display_name: Option<String>,
    tier: Option<String>,
    context_window: Option<i64>,
    max_output_tokens: Option<i64>,
    cost_per_1k_input: Option<f64>,
    cost_per_1k_output: Option<f64>,
    enabled: Option<bool>,
}

async fn update_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateModelRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let model = match state.database.get_model(&id) {
        Ok(Some(m)) => m,
        Ok(None) => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "Model not found"})),
            ))
        }
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            ))
        }
    };

    let mut updated = model.clone();
    if let Some(display_name) = req.display_name {
        updated.display_name = display_name;
    }
    if let Some(tier) = req.tier {
        updated.tier = tier;
    }
    if let Some(context_window) = req.context_window {
        updated.context_window = context_window;
    }
    if let Some(max_output_tokens) = req.max_output_tokens {
        updated.max_output_tokens = max_output_tokens;
    }
    if let Some(cost_per_1k_input) = req.cost_per_1k_input {
        updated.cost_per_1k_input = cost_per_1k_input;
    }
    if let Some(cost_per_1k_output) = req.cost_per_1k_output {
        updated.cost_per_1k_output = cost_per_1k_output;
    }
    if let Some(enabled) = req.enabled {
        updated.enabled = enabled;
    }
    updated.updated_at = Utc::now().timestamp();

    if let Err(e) = state.database.save_model(&updated) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ));
    }

    Ok(Json(updated))
}

async fn delete_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if let Err(e) = state.database.delete_model(&id) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ));
    }

    Ok(Json(serde_json::json!({"status": "deleted"})))
}

// ============================================================================
// Upstream model fetching
// ============================================================================

#[derive(Deserialize)]
struct FetchModelsParams {
    url: String,
    #[serde(rename = "type")]
    kind: String,
    key: Option<String>,
}

/// GET /api/proxy/models?url=&type=&key= — proxy an upstream /models request,
/// avoiding browser CORS and injecting the API key server-side.
async fn proxy_fetch_models(Query(params): Query<FetchModelsParams>) -> axum::response::Response {
    let client = reqwest::Client::new();
    let mut req = client.get(&params.url);
    if let Some(key) = params.key {
        if !key.is_empty() {
            if params.kind == "anthropic" {
                req = req
                    .header("x-api-key", &key)
                    .header("anthropic-version", "2023-06-01");
            } else {
                req = req.header("Authorization", format!("Bearer {}", key));
            }
        }
    }
    match req.send().await {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                match resp.json::<serde_json::Value>().await {
                    Ok(body) => {
                        let models: Vec<String> = body
                            .get("data")
                            .and_then(|d| d.as_array())
                            .map(|a| {
                                a.iter()
                                    .filter_map(|m| m["id"].as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default();
                        Json(serde_json::json!({"models": models})).into_response()
                    }
                    Err(e) => (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({"error": e.to_string()})),
                    )
                        .into_response(),
                }
            } else {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({"error": format!("upstream returned {}", status)})),
                )
                    .into_response()
            }
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ============================================================================
// Statistics
// ============================================================================

#[derive(Deserialize)]
struct StatsQuery {
    from: Option<i64>,
    to: Option<i64>,
    /// "hour" -> hourly buckets, anything else -> daily buckets.
    granularity: Option<String>,
    /// Local timezone offset in seconds (e.g. UTC+8 = 28800), so buckets align
    /// to local day/hour boundaries instead of UTC.
    tz_offset: Option<i64>,
}

async fn get_stats(
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

    Json(serde_json::json!({ "data": data }))
}

// ============================================================================
// App Settings
// ============================================================================

async fn get_settings(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(serde_json::json!({
        "websearch_hijack": state.websearch_hijack.load(std::sync::atomic::Ordering::Relaxed),
    }))
}

#[derive(Deserialize)]
struct UpdateSettingsRequest {
    websearch_hijack: Option<bool>,
}

async fn update_settings(
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
    Ok(Json(serde_json::json!({"status": "ok"})))
}

// ============================================================================
// Service Key CRUD (SHA-256 hashed)
// ============================================================================

#[derive(Deserialize)]
struct CreateServiceKeyRequest {
    name: String,
}

#[derive(Serialize)]
struct CreateServiceKeyResponse {
    id: String,
    name: String,
    key: String,  // Only returned once at creation time
    key_masked: String,
}

/// Create a new service key with SHA-256 hashing
async fn create_service_key(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateServiceKeyRequest>,
) -> Result<(StatusCode, Json<CreateServiceKeyResponse>), (StatusCode, Json<serde_json::Value>)> {
    // Generate random key
    let raw_key = format!("xrl-{}", uuid::Uuid::new_v4().to_string().replace("-", ""));

    // Compute argon2 hash
    let key_hash = match hash_service_key(&raw_key) {
        Ok(h) => h,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            ))
        }
    };

    // Create masked version: **** + last 4 chars
    let key_masked = if raw_key.len() >= 4 {
        format!("****{}", &raw_key[raw_key.len() - 4..])
    } else {
        "****".to_string()
    };

    let id = uuid::Uuid::new_v4().to_string();

    // Save to database
    if let Err(e) = state.database.save_service_key(&id, &req.name, &key_hash, &key_masked) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ));
    }

    // Return the raw key (only time it's visible)
    Ok((
        StatusCode::CREATED,
        Json(CreateServiceKeyResponse {
            id,
            name: req.name,
            key: raw_key,
            key_masked,
        }),
    ))
}

/// List all service keys (masked)
async fn list_service_keys(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, Json<serde_json::Value>)> {
    match state.database.list_service_keys() {
        Ok(keys) => Ok(Json(keys)),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )),
    }
}

/// Delete a service key
async fn delete_service_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if let Err(e) = state.database.delete_service_key(&id) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ));
    }

    Ok(Json(serde_json::json!({"status": "deleted"})))
}

#[derive(Deserialize)]
struct UpdateServiceKeyRequest {
    name: Option<String>,
    allowed_models: Option<Vec<String>>,
}

/// Update a service key (name and/or allowed_models)
async fn update_service_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateServiceKeyRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let allowed_json = req
        .allowed_models
        .as_ref()
        .map(|m| serde_json::to_string(m).unwrap_or_else(|_| "[]".to_string()));
    if let Err(e) = state
        .database
        .update_service_key(&id, req.name.as_deref(), allowed_json.as_deref())
    {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ));
    }
    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// Hash a service key using argon2 (random salt; the stored string embeds the salt).
pub fn hash_service_key(raw_key: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(raw_key.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("argon2 hash: {e:?}"))?;
    Ok(hash.to_string())
}

/// Verify a raw service key against a stored argon2 hash string.
pub fn verify_service_key(raw_key: &str, stored_hash: &str) -> bool {
    let parsed = match PasswordHash::new(stored_hash) {
        Ok(p) => p,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(raw_key.as_bytes(), &parsed)
        .is_ok()
}

// ============================================================================
// WebSocket
// ============================================================================

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: Arc<AppState>) {
    let mut rx = state.key_stats_tx.subscribe();
    while let Ok(msg) = rx.recv().await {
        let text = msg.to_string();
        if socket.send(Message::Text(text.into())).await.is_err() {
            break;
        }
    }
}
