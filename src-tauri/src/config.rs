use serde::Deserialize;
use tracing::warn;

/// Application configuration loaded from environment variables.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub port: u16,
    pub host: String,
    pub db_path: String,
    pub log_level: String,
    pub api_key: Option<String>,
    pub cors_origins: Vec<String>,
    /// 公共监听 host：install 页面 + /v1 代理，供局域网设备访问。
    pub public_host: String,
    /// 公共监听端口（与管理端口分离，避免 0.0.0.0 含 127.0.0.1 的绑定冲突）。
    pub public_port: u16,
    /// 是否启用公共 listener（绑定 0.0.0.0，向局域网开放）。
    pub enable_public: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 19068,
            host: "127.0.0.1".to_string(),
            db_path: "data/xrl-router.db".to_string(),
            log_level: "info".to_string(),
            api_key: None,
            cors_origins: vec![
                "http://localhost:5173".to_string(),
                "http://127.0.0.1:5173".to_string(),
                "http://localhost:19068".to_string(),
                "http://127.0.0.1:19068".to_string(),
                "tauri://localhost".to_string(),
                "https://tauri.localhost".to_string(),
                "http://tauri.localhost".to_string(),
            ],
            public_host: "0.0.0.0".to_string(),
            public_port: 19069,
            enable_public: true,
        }
    }
}

impl Config {
    /// Load configuration from environment variables.
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(port) = std::env::var("PORT").and_then(|p| p.parse().map_err(|_| std::env::VarError::NotPresent)) {
            config.port = port;
        } else if std::env::var("PORT").is_ok() {
            warn!("Invalid PORT value, using default 19068");
        }

        if let Ok(host) = std::env::var("HOST") {
            config.host = host;
        }

        if let Ok(db_path) = std::env::var("DB_PATH") {
            config.db_path = db_path;
        }

        if let Ok(log_level) = std::env::var("LOG_LEVEL") {
            config.log_level = log_level;
        }

        if let Ok(api_key) = std::env::var("API_KEY") {
            if !api_key.is_empty() {
                config.api_key = Some(api_key);
            }
        }

        if let Ok(origins) = std::env::var("CORS_ORIGINS") {
            let parsed: Vec<String> = origins
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !parsed.is_empty() {
                config.cors_origins = parsed;
            }
        }

        if let Ok(host) = std::env::var("PUBLIC_HOST") {
            config.public_host = host;
        }

        if let Ok(port) = std::env::var("PUBLIC_PORT").and_then(|p| p.parse().map_err(|_| std::env::VarError::NotPresent)) {
            config.public_port = port;
        } else if std::env::var("PUBLIC_PORT").is_ok() {
            warn!("Invalid PUBLIC_PORT value, using default 19069");
        }

        if let Ok(val) = std::env::var("ENABLE_PUBLIC") {
            config.enable_public = matches!(val.as_str(), "1" | "true" | "TRUE");
        }

        config
    }

    /// Get the socket address string for the admin (management) listener.
    pub fn addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// Get the socket address string for the public (LAN-facing) listener.
    pub fn public_addr(&self) -> String {
        format!("{}:{}", self.public_host, self.public_port)
    }
}
