//! Provider 表 CRUD（UPSERT，避免 REPLACE 触发子表级联删除）。

use crate::types::Provider;

impl super::Database {
    pub fn save_provider(&self, provider: &Provider) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        // 注意：不能用 INSERT OR REPLACE —— REPLACE = DELETE + INSERT，
        // DELETE 会触发 models/api_keys 的 ON DELETE CASCADE，把子表数据全清掉。
        // UPSERT 只更新本行，不碰子表。
        conn.execute(
            "INSERT INTO providers (id, name, kind, base_url, api_path, config_json, enabled, created_at, updated_at, sort_order)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(id) DO UPDATE SET
                name=excluded.name, kind=excluded.kind, base_url=excluded.base_url,
                api_path=excluded.api_path, config_json=excluded.config_json,
                enabled=excluded.enabled, updated_at=excluded.updated_at,
                sort_order=excluded.sort_order",
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
                provider.sort_order,
            ],
        )?;
        Ok(())
    }

    /// 批量重排供应商：按传入 id 顺序写入 0..n 的 sort_order（事务内执行）。
    pub fn reorder_providers(&self, ids: &[String]) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        for (i, id) in ids.iter().enumerate() {
            tx.execute(
                "UPDATE providers SET sort_order = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![i as i64, chrono::Utc::now().timestamp(), id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn next_sort_order(&self) -> anyhow::Result<i64> {
        let conn = self.conn.lock().unwrap();
        let max: i64 = conn.query_row(
            "SELECT COALESCE(MAX(sort_order), -1) FROM providers",
            [],
            |row| row.get(0),
        )?;
        Ok(max + 1)
    }

    pub fn delete_provider(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        // usage_log 已自包含（V12），不再预清理；直接删除 provider 即可。
        conn.execute("DELETE FROM providers WHERE id = ?1", rusqlite::params![id])?;
        Ok(())
    }
}
