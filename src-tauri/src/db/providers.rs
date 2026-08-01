//! Provider 表 CRUD（UPSERT，避免 REPLACE 触发子表级联删除）。

use crate::types::{Provider, ProviderKind};

impl super::Database {
    pub fn save_provider(&self, provider: &Provider) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        // 注意：不能用 INSERT OR REPLACE —— REPLACE = DELETE + INSERT，
        // DELETE 会触发 models/api_keys 的 ON DELETE CASCADE，把子表数据全清掉。
        // UPSERT 只更新本行，不碰子表。
        conn.execute(
            "INSERT INTO providers (id, name, kind, base_url, api_path, config_json, enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
                name=excluded.name, kind=excluded.kind, base_url=excluded.base_url,
                api_path=excluded.api_path, config_json=excluded.config_json,
                enabled=excluded.enabled, updated_at=excluded.updated_at",
            rusqlite::params![
                provider.id,
                provider.name,
                provider.kind.to_string(),
                provider.base_url,
                provider.api_path,
                serde_json::to_string(&provider.config)?,
                provider.enabled,
                provider.created_at,
                provider.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn delete_provider(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        // usage_log 已自包含（V12），不再预清理；直接删除 provider 即可。
        conn.execute("DELETE FROM providers WHERE id = ?1", rusqlite::params![id])?;
        Ok(())
    }

    pub fn list_all_providers(&self) -> anyhow::Result<Vec<Provider>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, kind, base_url, api_path, config_json, enabled, created_at, updated_at FROM providers"
        )?;

        let providers = stmt.query_map([], |row| {
            let kind_str: String = row.get(2)?;
            let config_str: String = row.get(5)?;
            Ok(Provider {
                id: row.get(0)?,
                name: row.get(1)?,
                kind: ProviderKind::from_str(&kind_str),
                base_url: row.get(3)?,
                api_path: row.get(4)?,
                config: serde_json::from_str(&config_str).unwrap_or_default(),
                enabled: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?;

        let mut result = Vec::new();
        for provider in providers {
            result.push(provider?);
        }
        Ok(result)
    }
}
