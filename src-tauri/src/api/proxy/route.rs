//! 模型名 → 上游 URL 的路由解析（含插件委托 provider 的实时覆盖）。

use tracing::warn;

use crate::gateway::server::AppState;

/// 一条已解析的路由：上游 URL、provider/model 标识、（可选）插件 ID。
pub(super) struct ResolvedRoute {
    pub(super) upstream_url: String,
    pub(super) provider_kind: String,
    pub(super) provider_id: String,
    pub(super) provider_name: String,
    pub(super) real_model_id: String,
    /// models.id (UUID primary key) — needed for usage_log.model_id FK.
    pub(super) model_row_id: String,
    /// Plugin ID if this is a delegated provider (None for regular providers).
    pub(super) plugin_id: Option<String>,
}

/// 从 KeyPool 取出的下一个可用 key（明文 hash + 标识）。
pub(super) struct PickedKey {
    pub(super) key_hash: String,
    pub(super) id: String,
    pub(super) name: String,
    pub(super) key_masked: String,
}

/// Resolve a route for the given model name using V5 normalized schema.
/// 1. Look up model by display_name (alias) in models table
/// 2. JOIN providers to get base_url, api_path, kind, config_json
/// 3. If delegated (plugin_id in config_json), override base_url/api_path from PluginManager
/// 4. Return None if delegated provider's plugin is offline
pub(super) async fn resolve_route(state: &AppState, model_name: &str) -> Option<ResolvedRoute> {
    let conn = state.database.conn();

    // Find model by display_name (alias) ONLY — calling with the real model_id
    // is rejected; clients must use the alias.
    let mut stmt = conn
        .prepare(
            "SELECT m.id, m.model_id, m.provider_id, p.name, p.base_url, p.api_path, p.kind, p.config_json
             FROM models m
             JOIN providers p ON m.provider_id = p.id
             WHERE m.display_name = ?1
               AND m.enabled = 1
               AND p.enabled = 1
             ORDER BY p.sort_order ASC, p.created_at ASC
             LIMIT 1",
        )
        .ok()?;

    let (model_row_id, real_model_id, provider_id, provider_name, base_url, api_path, kind, config_json_str): (
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
    ) = stmt
        .query_row([&model_name.to_string()], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
            ))
        })
        .ok()?;

    // Check if this provider is delegated (has plugin_id in config_json)
    let config: serde_json::Value = serde_json::from_str(&config_json_str).unwrap_or_default();
    let plugin_id = config.get("plugin_id").and_then(|v| v.as_str()).map(String::from);

    // For delegated providers, override base_url/api_path from PluginManager (live WS data)
    let (final_base_url, final_api_path) = if let Some(ref pid) = plugin_id {
        // Plugin must be connected — otherwise this provider shouldn't be enabled
        // (disconnect() sets enabled=false), but double-check for safety.
        let pm_base = state.plugins.get_base_url(pid);
        let pm_path = state.plugins.get_api_path(pid);
        match (pm_base, pm_path) {
            (Some(b), Some(p)) => (b, p),
            _ => {
                warn!("Delegated provider {} has plugin_id {} but plugin is offline", provider_id, pid);
                return None;
            }
        }
    } else {
        (base_url, api_path)
    };

    let upstream_url = format!("{}{}", final_base_url, final_api_path);

    Some(ResolvedRoute {
        upstream_url,
        provider_kind: kind,
        provider_id,
        provider_name,
        real_model_id,
        model_row_id,
        plugin_id,
    })
}
