use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::Emitter;
use tracing::{info, warn, error};

use crate::db::Database;
use crate::crypto::MasterKey;
use crate::keys::KeyPool;
use crate::types::{ApiKey, Provider, ProviderKind};

/// In-memory state for a connected plugin.
#[derive(Debug, Clone)]
pub struct PluginConnection {
    pub plugin_id: String,
    pub provider_id: Option<String>,
    pub base_url: String,
    pub api_path: String,
    pub kind: String,
    pub models: Vec<PluginModel>,
    pub last_heartbeat: i64,
}

/// Model info sent by a plugin during registration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginModel {
    pub model_id: String,
    pub display_name: String,
    pub tier: String,
}

/// Register message sent by a plugin on WS connect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRegisterMsg {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub plugin_id: String,
    pub provider: PluginProviderInfo,
    #[serde(default)]
    pub models: Vec<PluginModel>,
    #[serde(default)]
    pub keys: Vec<String>,
}

/// Provider info within a register message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginProviderInfo {
    pub kind: String,
    pub base_url: String,
    pub api_path: String,
}

/// keys_update message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginKeysUpdateMsg {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub keys: Vec<String>,
}

/// heartbeat message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginHeartbeatMsg {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(default)]
    pub timestamp: Option<i64>,
}

/// config_update message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfigUpdateMsg {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_path: Option<String>,
}

/// Generic WS message from plugin (loosely typed for flexible parsing).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginWsMsg {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// Manages plugin connections and lifecycle.
#[derive(Clone)]
pub struct PluginManager {
    connections: Arc<DashMap<String, PluginConnection>>,
    database: Database,
    app_handle: Arc<std::sync::Mutex<Option<tauri::AppHandle>>>,
    /// Provider registry map for in-memory sync (register/confirm/disconnect update both DB + memory).
    /// Shared DashMap reference — the same map held by ProviderRegistry.
    providers: Arc<DashMap<String, crate::types::Provider>>,
}

impl PluginManager {
    pub fn new(
        database: Database,
        providers: Arc<DashMap<String, crate::types::Provider>>,
    ) -> Self {
        Self {
            connections: Arc::new(DashMap::new()),
            database,
            app_handle: Arc::new(std::sync::Mutex::new(None)),
            providers,
        }
    }

    /// Set the Tauri AppHandle for emitting events to the frontend.
    /// Called from lib.rs setup() after Tauri is fully initialized.
    pub fn set_app_handle(&self, handle: tauri::AppHandle) {
        let mut h = self.app_handle.lock().unwrap();
        *h = Some(handle);
    }

    /// Emit a Tauri event to the frontend (no-op if AppHandle not set).
    fn emit_event(&self, event: &str, payload: serde_json::Value) {
        if let Some(handle) = self.app_handle.lock().unwrap().as_ref() {
            let _ = handle.emit(event, payload);
        }
    }

    /// Handle a plugin register message.
    /// Returns (provider_id, needs_confirmation).
    pub fn register(
        &self,
        msg: PluginRegisterMsg,
        keys: Vec<String>,
        master_key: &MasterKey,
        key_pool: &KeyPool,
    ) -> anyhow::Result<(String, bool)> {
        let now = chrono::Utc::now().timestamp();

        // Check if plugin already exists in DB
        let existing = self.get_plugin_by_id(&msg.plugin_id)?;
        if let Some(plugin) = existing {
            // Reconnection: update connection info and re-enable provider
            if let Some(ref provider_id) = plugin.provider_id {
                let conn = PluginConnection {
                    plugin_id: msg.plugin_id.clone(),
                    provider_id: Some(provider_id.clone()),
                    base_url: msg.provider.base_url.clone(),
                    api_path: msg.provider.api_path.clone(),
                    kind: msg.provider.kind.clone(),
                    models: msg.models.clone(),
                    last_heartbeat: now,
                };
                self.connections.insert(msg.plugin_id.clone(), conn);

                // Re-enable provider
                self.database.execute(
                    "UPDATE providers SET enabled = 1, updated_at = ?1 WHERE id = ?2",
                    rusqlite::params![now, provider_id],
                )?;
                self.update_plugin_status(&msg.plugin_id, "active", Some(provider_id))?;

                // Sync keys if provided
                if !keys.is_empty() {
                    self.sync_keys(provider_id, &keys, master_key, key_pool)?;
                }

                self.emit_event("plugin-online", serde_json::json!({
                    "plugin_id": msg.plugin_id,
                    "provider_id": provider_id,
                }));

                info!("Plugin reconnected: {} → provider {}", msg.plugin_id, provider_id);
                return Ok((provider_id.clone(), false));
            }
        }

        // New plugin: create provider + save plugin record
        let provider_id = uuid::Uuid::new_v4().to_string();
        let provider_name = msg.plugin_id.clone(); // default name = plugin_id

        let config = serde_json::json!({
            "plugin_id": msg.plugin_id,
            "delegated": true
        });

        let provider = Provider {
            id: provider_id.clone(),
            name: provider_name,
            kind: ProviderKind::from_str(&msg.provider.kind),
            base_url: msg.provider.base_url.clone(),
            api_path: msg.provider.api_path.clone(),
            config,
            enabled: false, // disabled until user confirms in dialog
            created_at: now,
            updated_at: now,
        };
        self.database.save_provider(&provider)?;
        // 同步内存 registry（register 时 ProviderRegistry 尚未加载，需手动插入）
        self.providers.insert(provider.id.clone(), provider.clone());

        // Save models
        for m in &msg.models {
            let model = crate::types::Model {
                id: uuid::Uuid::new_v4().to_string(),
                provider_id: provider_id.clone(),
                model_id: m.model_id.clone(),
                display_name: m.display_name.clone(),
                tier: m.tier.clone(),
                context_window: 128000,
                max_output_tokens: 4096,
                capabilities: "[\"text\",\"tools\"]".to_string(),
                enabled: true,
                created_at: now,
                updated_at: now,
            };
            self.database.save_model(&model)?;
        }

        // Save plugin record
        self.save_plugin(&msg.plugin_id, Some(&provider_id), "pending", now)?;

        // Store connection in memory
        let conn = PluginConnection {
            plugin_id: msg.plugin_id.clone(),
            provider_id: Some(provider_id.clone()),
            base_url: msg.provider.base_url.clone(),
            api_path: msg.provider.api_path.clone(),
            kind: msg.provider.kind.clone(),
            models: msg.models.clone(),
            last_heartbeat: now,
        };
        self.connections.insert(msg.plugin_id.clone(), conn);

        // Sync initial keys
        if !keys.is_empty() {
            self.sync_keys(&provider_id, &keys, master_key, key_pool)?;
        }

        // Emit event to frontend to show registration dialog
        self.emit_event("plugin-register", serde_json::json!({
            "plugin_id": msg.plugin_id,
            "provider_id": provider_id,
            "provider_name": provider.name,
            "kind": provider.kind.to_string(),
            "base_url": provider.base_url,
            "api_path": provider.api_path,
            "models": msg.models.iter().map(|m| serde_json::json!({
                "model_id": m.model_id,
                "display_name": m.display_name,
                "tier": m.tier,
            })).collect::<Vec<_>>(),
            "key_count": keys.len(),
        }));

        info!("Plugin registered: {} → provider {}", msg.plugin_id, provider_id);
        Ok((provider_id, true))
    }

    /// Confirm a plugin (user clicked "add" in dialog). Enables the provider.
    pub fn confirm(&self, plugin_id: &str) -> anyhow::Result<()> {
        let now = chrono::Utc::now().timestamp();
        self.update_plugin_status(plugin_id, "active", None)?;

        // Enable the associated provider
        if let Some(conn) = self.connections.get(plugin_id) {
            if let Some(ref provider_id) = conn.provider_id {
                self.database.execute(
                    "UPDATE providers SET enabled = 1, updated_at = ?1 WHERE id = ?2",
                    rusqlite::params![now, provider_id],
                )?;
                // 同步内存 registry
                if let Some(mut p) = self.providers.get_mut(provider_id) {
                    p.enabled = true;
                    p.updated_at = now;
                }
                self.emit_event("plugin-activated", serde_json::json!({
                    "plugin_id": plugin_id,
                    "provider_id": provider_id,
                }));
            }
        }
        Ok(())
    }

    /// Sync keys for a provider: add new, remove old, keep existing.
    pub fn sync_keys(
        &self,
        provider_id: &str,
        new_keys: &[String],
        master_key: &MasterKey,
        key_pool: &KeyPool,
    ) -> anyhow::Result<usize> {
        // Load existing keys and decrypt
        let existing = self.database.list_all_keys()?
            .into_iter()
            .filter(|k| k.provider_id == provider_id)
            .collect::<Vec<_>>();

        let existing_raw: Vec<(String, ApiKey)> = existing.iter().map(|k| {
            let raw = crate::crypto::decrypt(&k.key_hash, master_key)
                .unwrap_or_else(|_| k.key_hash.clone());
            (raw, k.clone())
        }).collect();

        let existing_raw_set: std::collections::HashSet<&str> =
            existing_raw.iter().map(|(r, _)| r.as_str()).collect();
        let new_keys_set: std::collections::HashSet<&str> =
            new_keys.iter().map(|s| s.as_str()).collect();

        // Remove keys no longer present
        for (_, key) in &existing_raw {
            let raw = crate::crypto::decrypt(&key.key_hash, master_key)
                .unwrap_or_else(|_| key.key_hash.clone());
            if !new_keys_set.contains(raw.as_str()) {
                self.database.delete_api_key(&key.id)?;
                key_pool.remove_key(provider_id, &key.id);
            }
        }

        // Add new keys
        let now = chrono::Utc::now().timestamp();
        let mut added = 0usize;
        for key_str in new_keys {
            if !existing_raw_set.contains(key_str.as_str()) {
                let key_id = uuid::Uuid::new_v4().to_string();
                let masked = if key_str.len() > 8 {
                    format!("{}...{}", &key_str[..4], &key_str[key_str.len()-4..])
                } else {
                    "***".to_string()
                };
                let encrypted = crate::crypto::encrypt(key_str, master_key)?;
                let api_key = ApiKey {
                    id: key_id,
                    provider_id: provider_id.to_string(),
                    name: format!("plugin-key-{}", added + 1),
                    key_hash: encrypted,
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
                    created_at: now,
                    updated_at: now,
                };
                self.database.save_api_key(&api_key)?;
                added += 1;
            }
        }

        // Reload key pool for this provider
        if added > 0 || !existing.is_empty() {
            key_pool.remove_provider(provider_id);
            key_pool.load_keys_from_db(provider_id, &self.database, master_key)
                .map_err(|e| anyhow::anyhow!("{}", e))?;
        }

        Ok(added)
    }

    /// Handle a keys_update message from a connected plugin.
    pub fn handle_keys_update(
        &self,
        plugin_id: &str,
        keys: Vec<String>,
        master_key: &MasterKey,
        key_pool: &KeyPool,
    ) -> anyhow::Result<usize> {
        let provider_id = self.connections
            .get(plugin_id)
            .and_then(|c| c.provider_id.clone())
            .ok_or_else(|| anyhow::anyhow!("Plugin not connected: {}", plugin_id))?;
        self.sync_keys(&provider_id, &keys, master_key, key_pool)
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

    /// Update heartbeat timestamp for a plugin.
    pub fn heartbeat(&self, plugin_id: &str) {
        if let Some(mut conn) = self.connections.get_mut(plugin_id) {
            conn.last_heartbeat = chrono::Utc::now().timestamp();
        }
    }

    /// Handle plugin disconnect (WS closed).
    pub fn disconnect(&self, plugin_id: &str) {
        if let Some((_, conn)) = self.connections.remove(plugin_id) {
            if let Some(ref provider_id) = conn.provider_id {
                let now = chrono::Utc::now().timestamp();
                // Disable provider so resolve_route skips it
                let _ = self.database.execute(
                    "UPDATE providers SET enabled = 0, updated_at = ?1 WHERE id = ?2",
                    rusqlite::params![now, provider_id],
                );
                // 同步内存 registry
                if let Some(mut p) = self.providers.get_mut(provider_id) {
                    p.enabled = false;
                    p.updated_at = now;
                }
                let _ = self.update_plugin_status(plugin_id, "offline", Some(provider_id));
                self.emit_event("plugin-offline", serde_json::json!({
                    "plugin_id": plugin_id,
                    "provider_id": provider_id,
                }));
                warn!("Plugin disconnected: {} → provider {} disabled", plugin_id, provider_id);
            }
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
    pub fn get_connection(&self, plugin_id: &str) -> Option<PluginConnection> {
        self.connections.get(plugin_id).map(|c| c.clone())
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

    // ---- DB helpers ----

    fn get_plugin_by_id(&self, id: &str) -> anyhow::Result<Option<PluginRecord>> {
        let conn = self.database.conn();
        let mut stmt = conn.prepare(
            "SELECT id, provider_id, status, last_heartbeat_at, created_at, updated_at FROM plugins WHERE id = ?1"
        )?;
        let result = stmt.query_row(rusqlite::params![id], |row| {
            Ok(PluginRecord {
                id: row.get(0)?,
                provider_id: row.get(1)?,
                status: row.get(2)?,
                last_heartbeat_at: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        });
        match result {
            Ok(r) => Ok(Some(r)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn get_plugin_id_for_provider(&self, provider_id: &str) -> Option<String> {
        let conn = self.database.conn();
        let mut stmt = conn.prepare(
            "SELECT id FROM plugins WHERE provider_id = ?1"
        ).ok()?;
        stmt.query_row(rusqlite::params![provider_id], |row| row.get::<_, String>(0)).ok()
    }

    fn save_plugin(&self, id: &str, provider_id: Option<&str>, status: &str, now: i64) -> anyhow::Result<()> {
        self.database.execute(
            "INSERT INTO plugins (id, provider_id, status, last_heartbeat_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                provider_id=excluded.provider_id, status=excluded.status,
                last_heartbeat_at=excluded.last_heartbeat_at, updated_at=excluded.updated_at",
            rusqlite::params![id, provider_id, status, now, now, now],
        )?;
        Ok(())
    }

    fn update_plugin_status(&self, id: &str, status: &str, provider_id: Option<&str>) -> anyhow::Result<()> {
        let now = chrono::Utc::now().timestamp();
        if let Some(pid) = provider_id {
            self.database.execute(
                "UPDATE plugins SET status = ?1, provider_id = ?2, updated_at = ?3 WHERE id = ?4",
                rusqlite::params![status, pid, now, id],
            )?;
        } else {
            self.database.execute(
                "UPDATE plugins SET status = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![status, now, id],
            )?;
        }
        Ok(())
    }
}

/// Database record for a plugin.
#[derive(Debug, Clone)]
pub struct PluginRecord {
    pub id: String,
    pub provider_id: Option<String>,
    pub status: String,
    pub last_heartbeat_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}
