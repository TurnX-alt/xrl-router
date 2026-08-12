//! 插件健康检查（心跳、超时断连）、配置热更新与在线状态查询。

use tracing::warn;

use super::types::PluginConnection;

impl super::PluginManager {
    /// Update heartbeat timestamp for a plugin.
    pub fn heartbeat(&self, plugin_id: &str) {
        if let Some(mut conn) = self.connections.get_mut(plugin_id) {
            conn.last_heartbeat = chrono::Utc::now().timestamp();
        }
    }

    /// Handle a config_update message (base_url/api_path changed).
    pub fn handle_config_update(
        &self,
        plugin_id: &str,
        base_url: Option<String>,
        api_path: Option<String>,
    ) {
        if let Some(mut conn) = self.connections.get_mut(plugin_id) {
            if let Some(url) = base_url {
                conn.base_url = url;
            }
            if let Some(path) = api_path {
                conn.api_path = path;
            }
        }
    }

    /// Check for heartbeat timeouts and disconnect stale plugins.
    /// Should be called periodically (e.g., every 30s).
    pub fn check_heartbeats(&self, timeout_secs: i64) {
        let now = chrono::Utc::now().timestamp();
        let stale: Vec<String> = self.connections.iter()
            .filter(|c| now - c.last_heartbeat > timeout_secs)
            .map(|c| c.plugin_id.clone())
            .collect();

        for plugin_id in stale {
            warn!("Plugin {} heartbeat timeout (>{timeout_secs}s), disconnecting", plugin_id);
            self.disconnect(&plugin_id);
        }
    }

    /// Check if a plugin is currently connected.
    pub fn is_connected(&self, plugin_id: &str) -> bool {
        self.connections.contains_key(plugin_id)
    }

    /// Check if a plugin still exists in the database.
    /// Used by the WS loop to detect plugins deleted by the user (e.g. ignored),
    /// so the connection can be closed and the plugin will re-register on reconnect.
    pub fn is_registered(&self, plugin_id: &str) -> bool {
        self.get_plugin_by_id(plugin_id)
            .map(|p| p.is_some())
            .unwrap_or(false)
    }

    /// Get the active connection info for a provider (by provider_id).
    #[allow(dead_code)]
    pub fn get_connection_for_provider(&self, provider_id: &str) -> Option<PluginConnection> {
        let plugin_id = self.get_plugin_id_for_provider(provider_id)?;
        self.connections.get(&plugin_id).map(|c| c.clone())
    }

    /// Get the base_url for a connected plugin (by plugin_id).
    pub fn get_base_url(&self, plugin_id: &str) -> Option<String> {
        self.connections.get(plugin_id).map(|c| c.base_url.clone())
    }

    /// Get the api_path for a connected plugin (by plugin_id).
    pub fn get_api_path(&self, plugin_id: &str) -> Option<String> {
        self.connections.get(plugin_id).map(|c| c.api_path.clone())
    }

    /// List all connected plugins.
    pub fn list_connected(&self) -> Vec<PluginConnection> {
        self.connections.iter().map(|c| c.value().clone()).collect()
    }

    /// Get a specific plugin connection by plugin_id.
    #[allow(dead_code)]
    pub fn get_connection(&self, plugin_id: &str) -> Option<PluginConnection> {
        self.connections.get(plugin_id).map(|c| c.clone())
    }
}
