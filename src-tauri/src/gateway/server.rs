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
use tracing::{error, info};

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
///
/// 双 listener 分离监听：管理 Router 绑 127.0.0.1:port（仅本机访问 /api/* 管理
/// 与密钥端点），公共 Router 绑 0.0.0.0:public_port（install 页面 + /v1/* 代理，
/// 供局域网设备访问）。两端口分离避免 0.0.0.0 含 127.0.0.1 的绑定冲突。
pub async fn start_gateway(state: Arc<AppState>) -> Result<()> {
    let cors = build_cors_layer(&state.config);

    // 既有后台 task：每 5s 广播 usage_stats_changed（Key counts 已在 pool.rs 按变更广播）
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

    // 既有后台 task：每 30s 检查插件心跳，断开 >90s 无心跳的插件
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

    // 管理 listener：127.0.0.1，承载全部 /api/* + /health + /ws（CORS 用白名单）
    {
        let admin_router = crate::api::build_admin_router(state.clone()).layer(cors);
        let admin_addr = state.config.addr();
        let listener = tokio::net::TcpListener::bind(&admin_addr).await?;
        info!("Admin listener on http://{}", admin_addr);
        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, admin_router).await {
                error!("Admin listener error: {}", e);
            }
        });
    }

    // 公共 listener：0.0.0.0，承载 /install + /v1/*（CORS 全开——CLI 无 origin 约束、install 同源）
    if state.config.enable_public {
        let public_router = crate::api::build_public_router(state.clone())
            .layer(CorsLayer::new().allow_methods(Any).allow_headers(Any).allow_origin(Any));
        let public_addr = state.config.public_addr();
        let listener = tokio::net::TcpListener::bind(&public_addr).await?;
        info!("Public listener on http://{}", public_addr);
        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, public_router).await {
                error!("Public listener error: {}", e);
            }
        });
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    /// 端到端冒烟测试：真实 TCP 起网关，实测四条路径。
    /// 覆盖 build_admin_router → handlers/* → proxy 认证 → AppState（DB 迁移、
    /// providers/models 注册表、密钥池、插件管理器）的完整拆分后链路。
    /// admin router 已含 /v1/*（本机兼容入口），故单用 admin 即可测全链路；
    /// 另起 public router 验证 /install 静态页。
    #[tokio::test]
    async fn test_gateway_smoke_end_to_end() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let config = Config {
            port: 0,
            host: "127.0.0.1".to_string(),
            ..Default::default()
        };
        let state = Arc::new(AppState::new(config, db, [7u8; 32]));
        let router = crate::api::build_admin_router(state.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });

        let client = reqwest::Client::new();
        // 等服务器就绪
        for _ in 0..50 {
            if client.get(format!("http://{}/health", addr)).send().await.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // /health：完整链路（DB 连接、providers/models 注册表、key pool）
        let resp = client.get(format!("http://{}/health", addr)).send().await.unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], "ok");
        assert_eq!(body["service"], "xrl-router");
        assert_eq!(body["database"], "ok");

        // /api/providers：CRUD handler 路径（空库返回空数组）
        let resp = client.get(format!("http://{}/api/providers", addr)).send().await.unwrap();
        assert_eq!(resp.status(), 200);
        let providers: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(providers.as_array().map(|a| a.len()), Some(0));

        // /v1/models：proxy 认证路径（无 service key 应 401）
        let resp = client.get(format!("http://{}/v1/models", addr)).send().await.unwrap();
        assert_eq!(resp.status(), 401);

        // /v1/chat/completions：proxy 认证 + 路由解析路径（无 service key 应 401）
        let resp = client
            .post(format!("http://{}/v1/chat/completions", addr))
            .header("Content-Type", "application/json")
            .body(r#"{"model":"test","messages":[{"role":"user","content":"hi"}]}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);

        // /v1/user/balance：无 service key 应 401
        let resp = client
            .get(format!("http://{}/v1/user/balance", addr))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);

        // 配额 429：创建 service key（quota_5h=10）→ 写入 15 tokens 用量 → 请求应 429。
        let raw_key = "xrl-test-quota-key";
        let key_hash = crate::crypto::hash_service_key(raw_key).unwrap();
        state.database.save_service_key("sk-quota", "限额测试", &key_hash, "****uota").unwrap();
        let now = chrono::Utc::now().timestamp();
        state.database.insert_usage_log(
            now,
            "p1", "P1", "m1", "M1",
            Some("pk1"), "PK", "pk-masked",
            Some("sk-quota"), "限额测试", "****uota",
            "/v1/messages",
            10, 5, 10, true, None, 0,
        ).unwrap();
        state.database.update_service_key("sk-quota", None, None, Some(10), None).unwrap();
        let resp = client
            .post(format!("http://{}/v1/chat/completions", addr))
            .header("Content-Type", "application/json")
            .header("x-api-key", raw_key)
            .body(r#"{"model":"test","messages":[{"role":"user","content":"hi"}]}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 429);
        // retry-after 应存在且为正值（在消费 body 之前读取）
        let retry_after = resp.headers().get("retry-after").and_then(|v| v.to_str().ok());
        assert!(retry_after.is_some(), "429 应携带 retry-after 头");
        assert!(retry_after.unwrap().parse::<i64>().unwrap() > 0);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"]["type"], "quota_error");

        // /v1/user/balance：CCSwitch TokenPlan（ZenMux 分支）兼容格式
        // （5h 设限 → quota_5_hour；7d 未设限 → 字段省略）
        let resp = client
            .get(format!("http://{}/v1/user/balance", addr))
            .header("x-api-key", raw_key)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let zm: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(zm["success"], true);
        assert_eq!(zm["data"]["quota_5_hour"]["usage_percentage"], 1.5);
        assert!(
            zm["data"]["quota_5_hour"]["resets_at"].as_str().unwrap().contains("T"),
            "resets_at 应为 ISO 字符串（CCSwitch 用 as_str 解析）"
        );
        assert!(zm["data"].get("quota_7_day").is_none(), "未设限窗口应省略");

        // /install：public router 静态页（独立 listener 验证，防与 admin merge 冲突）
        let pub_state = state.clone();
        let pub_router = crate::api::build_public_router(pub_state);
        let pub_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let pub_addr = pub_listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(pub_listener, pub_router).await.unwrap();
        });
        for _ in 0..50 {
            if client.get(format!("http://{}/install", pub_addr)).send().await.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let resp = client.get(format!("http://{}/install", pub_addr)).send().await.unwrap();
        assert_eq!(resp.status(), 200);
        let html = resp.text().await.unwrap();
        assert!(html.contains("快速部署"), "/install 应返回 install 页面 HTML");
    }
}
