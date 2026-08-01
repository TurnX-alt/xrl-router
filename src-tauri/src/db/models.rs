//! models 表 CRUD。

use crate::types::Model;

impl super::Database {
    pub fn save_model(&self, model: &Model) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        // UPSERT 而非 INSERT OR REPLACE：REPLACE 会触发 usage_log 的 FK 删除/报错。
        conn.execute(
            "INSERT INTO models (id, provider_id, model_id, display_name, tier, context_window, max_output_tokens, capabilities, enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET
                provider_id=excluded.provider_id, model_id=excluded.model_id,
                display_name=excluded.display_name, tier=excluded.tier,
                context_window=excluded.context_window, max_output_tokens=excluded.max_output_tokens,
                capabilities=excluded.capabilities, enabled=excluded.enabled,
                updated_at=excluded.updated_at",
            rusqlite::params![
                model.id,
                model.provider_id,
                model.model_id,
                model.display_name,
                model.tier,
                model.context_window,
                model.max_output_tokens,
                model.capabilities,
                model.enabled,
                model.created_at,
                model.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_model(&self, id: &str) -> anyhow::Result<Option<Model>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, provider_id, model_id, display_name, tier, context_window, max_output_tokens, capabilities, enabled, created_at, updated_at FROM models WHERE id = ?1"
        )?;

        let model = stmt.query_row(rusqlite::params![id], |row| {
            Ok(Model {
                id: row.get(0)?,
                provider_id: row.get(1)?,
                model_id: row.get(2)?,
                display_name: row.get(3)?,
                tier: row.get(4)?,
                context_window: row.get(5)?,
                max_output_tokens: row.get(6)?,
                capabilities: row.get(7)?,
                enabled: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        });

        match model {
            Ok(m) => Ok(Some(m)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn delete_model(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        // usage_log 已自包含（V12），不再预清理；直接删除 model 即可。
        conn.execute("DELETE FROM models WHERE id = ?1", rusqlite::params![id])?;
        Ok(())
    }

    pub fn list_all_models(&self) -> anyhow::Result<Vec<Model>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, provider_id, model_id, display_name, tier, context_window, max_output_tokens, capabilities, enabled, created_at, updated_at FROM models"
        )?;

        let models = stmt.query_map([], |row| {
            Ok(Model {
                id: row.get(0)?,
                provider_id: row.get(1)?,
                model_id: row.get(2)?,
                display_name: row.get(3)?,
                tier: row.get(4)?,
                context_window: row.get(5)?,
                max_output_tokens: row.get(6)?,
                capabilities: row.get(7)?,
                enabled: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        })?;

        let mut result = Vec::new();
        for model in models {
            result.push(model?);
        }
        Ok(result)
    }
}
