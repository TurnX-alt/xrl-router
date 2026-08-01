use crate::config::Config;
use crate::db::Database;
use crate::keys::KeyPool;
use crate::middleware::RateLimiter;
use crate::models::ModelRegistry;
use crate::plugin::PluginManager;
use crate::providers::ProviderRegistry;
use anyhow::Result;
use axum::http::HeaderValue;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

/// Shared application state accessible by all handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub database: Database,
    pub providers: ProviderRegistry,
    pub keys: KeyPool,
    pub models: ModelRegistry,
    pub rate_limiter: RateLimiter,
    pub master_key: crate::crypto::MasterKey,
    pub key_stats_tx: tokio::sync::broadcast::Sender<serde_json::Value>,
    /// WebSearch 劫持开关（运行时可改、无锁读）。
    pub websearch_hijack: Arc<std::sync::atomic::AtomicBool>,
    /// Plugin manager: tracks connected plugins and their delegated providers.
    pub plugins: PluginManager,
}

impl AppState {
    pub fn new(config: Config, database: Database, master_key: crate::crypto::MasterKey) -> Self {
        let providers = ProviderRegistry::new(database.clone());
        let _ = providers.load_from_db();
        let models = ModelRegistry::new(database.clone());
        let _ = models.load_from_db();

        let (key_stats_tx, _) = tokio::sync::broadcast::channel(64);

        let mut keys = KeyPool::new();
        keys.set_database(database.clone());
        keys.load_all_keys_from_db(&database, &master_key);
        keys.set_key_stats_tx(key_stats_tx.clone());

        let rate_limiter = RateLimiter::new();
        let websearch_hijack = Arc::new(std::sync::atomic::AtomicBool::new(
            database
                .get_setting("websearch_hijack")
                .ok()
                .flatten()
                .map(|v| v == "true")
                .unwrap_or(false),
        ));

        let plugins = PluginManager::new(database.clone(), providers.providers_map());

        Self {
            config,
            database,
            providers,
            keys,
            models,
            rate_limiter,
            master_key,
            key_stats_tx,
            websearch_hijack,
            plugins,
        }
    }
}

/// Start the gateway HTTP server as a background service.
pub async fn start_gateway(state: Arc<AppState>) -> Result<()> {
    let cors = build_cors_layer(&state.config);

    let addr = state.config.addr();

    // Build the full router with all endpoints
    let router = crate::api::build_router(state.clone());

    let app = router.layer(cors);

    // Spawn a periodic task that signals the stats page to refetch every 5s.
    // (Key counts already broadcast on change via pool.rs — no need to repeat here.)
    {
        let tx = state.key_stats_tx.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            loop {
                interval.tick().await;
                let _ = tx.send(serde_json::json!({
                    "type": "usage_stats_changed",
                    "timestamp": chrono::Utc::now().timestamp(),
                }));
            }
        });
    }

    // Spawn plugin heartbeat checker: every 30s, disconnect stale plugins (>90s no heartbeat).
    {
        let plugins = state.plugins.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                plugins.check_heartbeats(90);
            }
        });
    }

    info!("Gateway server listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Build a CORS layer constrained to configured local origins (tightens the
/// previous `allow_origin: *` policy). Falls back to permissive only if the
/// origin list is explicitly empty.
fn build_cors_layer(config: &Config) -> CorsLayer {
    let mut layer = CorsLayer::new()
        .allow_methods(Any)
        .allow_headers(Any);
    if config.cors_origins.is_empty() {
        layer = layer.allow_origin(Any);
    } else {
        let origins: Vec<HeaderValue> = config
            .cors_origins
            .iter()
            .filter_map(|o| o.parse::<HeaderValue>().ok())
            .collect();
        layer = layer.allow_origin(origins);
    }
    layer
}
