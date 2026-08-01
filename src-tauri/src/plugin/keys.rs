//! 插件密钥增量同步：对比已有密钥做 add/remove，加密入库并同步 KeyPool 内存。

use crate::crypto::{decrypt, encrypt, MasterKey};
use crate::keys::KeyPool;
use crate::types::ApiKey;

impl super::PluginManager {
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
            let raw = decrypt(&k.key_hash, master_key)
                .unwrap_or_else(|_| k.key_hash.clone());
            (raw, k.clone())
        }).collect();

        let existing_raw_set: std::collections::HashSet<&str> =
            existing_raw.iter().map(|(r, _)| r.as_str()).collect();
        let new_keys_set: std::collections::HashSet<&str> =
            new_keys.iter().map(|s| s.as_str()).collect();

        // Remove keys no longer present
        for (_, key) in &existing_raw {
            let raw = decrypt(&key.key_hash, master_key)
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
                let encrypted = encrypt(key_str, master_key)?;
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
}
