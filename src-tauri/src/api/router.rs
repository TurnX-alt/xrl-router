//! 全部 Axum 路由的定义。

use std::sync::Arc;

use axum::middleware;
use axum::routing::{delete, get, post, put};
use axum::Router;

use crate::gateway::server::AppState;
use crate::middleware::rate_limit::rate_limit_middleware;

use super::handlers;
use super::proxy;

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
        .route("/health", get(handlers::health_check))
        .route("/", get(handlers::health_check))
        // WebSocket endpoint (no rate limiting)
        .route("/ws", get(handlers::ws_handler))
        // Provider management
        .route("/api/providers", get(handlers::list_providers).post(handlers::create_provider))
        .route(
            "/api/providers/:id",
            get(handlers::get_provider).put(handlers::update_provider).delete(handlers::delete_provider),
        )
        // API Key management
        .route("/api/keys", get(handlers::list_keys).post(handlers::create_key))
        .route(
            "/api/keys/:id",
            get(handlers::get_key).put(handlers::update_key).delete(handlers::delete_key),
        )
        // Model management
        .route("/api/models", get(handlers::list_models).post(handlers::create_model))
        .route(
            "/api/models/:id",
            get(handlers::get_model).put(handlers::update_model).delete(handlers::delete_model),
        )
        // Fetch upstream models (proxy to avoid CORS and inject API key)
        .route("/api/proxy/models", get(handlers::proxy_fetch_models))
        // Statistics
        .route("/api/stats", get(handlers::get_stats))
        // Service Key management (argon2 hashed)
        .route("/api/service-keys", get(handlers::list_service_keys).post(handlers::create_service_key))
        .route("/api/service-keys/:id", put(handlers::update_service_key).delete(handlers::delete_service_key))
        // App settings
        .route("/api/settings", get(handlers::get_settings).put(handlers::update_settings))
        // Plugin management
        .route("/ws/plugin", get(handlers::plugin_ws_handler))
        .route("/api/plugins", get(handlers::list_plugins))
        .route("/api/plugins/:id", get(handlers::get_plugin).delete(handlers::delete_plugin))
        .route("/api/plugins/:id/confirm", post(handlers::confirm_plugin))
        // Merge rate-limited proxy routes
        .merge(proxy_routes)
        .with_state(state)
}
