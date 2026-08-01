//! service_keys 表 CRUD（argon2 哈希存储，哈希函数见 `crypto`）。

impl super::Database {
    pub fn save_service_key(&self, id: &str, name: &str, key_hash: &str, key_masked: &str) -> anyhow::Result<()> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        // UPSERT 而非 INSERT OR REPLACE：REPLACE 会触发 usage_log.service_key_id 的 FK 清理。
        conn.execute(
            "INSERT INTO service_keys (id, name, key_hash, key_masked, total_requests, total_tokens, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 0, 0, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                name=excluded.name, key_hash=excluded.key_hash,
                key_masked=excluded.key_masked, updated_at=excluded.updated_at",
            rusqlite::params![id, name, key_hash, key_masked, now, now],
        )?;
        Ok(())
    }

    pub fn list_service_keys(&self) -> anyhow::Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, key_masked, allowed_models, total_requests, total_tokens, last_used_at, created_at FROM service_keys"
        )?;

        let keys = stmt.query_map([], |row| {
            let allowed_str: String = row.get(3)?;
            let allowed: serde_json::Value =
                serde_json::from_str(&allowed_str).unwrap_or(serde_json::json!([]));
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "key_masked": row.get::<_, String>(2)?,
                "allowed_models": allowed,
                "total_requests": row.get::<_, i64>(4)?,
                "total_tokens": row.get::<_, i64>(5)?,
                "last_used_at": row.get::<_, Option<i64>>(6)?,
                "created_at": row.get::<_, i64>(7)?,
            }))
        })?;

        let mut result = Vec::new();
        for key in keys {
            result.push(key?);
        }
        Ok(result)
    }

    pub fn delete_service_key(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        // usage_log 已自包含（V12），不再预清理；直接删除 service_key 即可。
        conn.execute("DELETE FROM service_keys WHERE id = ?1", rusqlite::params![id])?;
        Ok(())
    }

    /// Update a service key's name and/or allowed_models
    pub fn update_service_key(
        &self,
        id: &str,
        name: Option<&str>,
        allowed_models: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();
        if let Some(n) = name {
            conn.execute(
                "UPDATE service_keys SET name = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![n, now, id],
            )?;
        }
        if let Some(a) = allowed_models {
            conn.execute(
                "UPDATE service_keys SET allowed_models = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![a, now, id],
            )?;
        }
        Ok(())
    }
}
