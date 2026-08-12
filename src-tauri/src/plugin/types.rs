//! 插件协议的数据结构：连接态、注册/心跳/配置消息、DB 记录。

use serde::{Deserialize, Serialize};

/// In-memory state for a connected plugin.
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginKeysUpdateMsg {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub keys: Vec<String>,
}

/// heartbeat message.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginHeartbeatMsg {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(default)]
    pub timestamp: Option<i64>,
}

/// config_update message.
#[allow(dead_code)]
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
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginWsMsg {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// Database record for a plugin.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct PluginRecord {
    pub id: String,
    pub provider_id: Option<String>,
    pub status: String,
    pub last_heartbeat_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}
