//! api_keys 表 CRUD。

use crate::types::ApiKey;

impl super::Database {
    pub fn save_api_key(&self, key: &ApiKey) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        // UPSERT 而非 INSERT OR REPLACE：REPLACE 会触发 usage_log 的 FK 删除/报错，
        // 且会丢失 total_requests/total_tokens 等统计字段。
        conn.execute(
            "INSERT INTO api_keys (id, provider_id, name, key_hash, key_masked, status, last_error, last_error_code, last_error_time, last_used_at, balance, balance_updated_at, total_requests, total_tokens, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
             ON CONFLICT(id) DO UPDATE SET
                provider_id=excluded.provider_id, name=excluded.name, key_hash=excluded.key_hash,
                key_masked=excluded.key_masked, status=excluded.status, last_error=excluded.last_error,
                last_error_code=excluded.last_error_code, last_error_time=excluded.last_error_time,
                last_used_at=excluded.last_used_at, balance=excluded.balance,
                balance_updated_at=excluded.balance_updated_at, updated_at=excluded.updated_at",
            rusqlite::params![
                key.id,
                key.provider_id,
                key.name,
                key.key_hash,
                key.key_masked,
                key.status,
                key.last_error,
                key.last_error_code,
                key.last_error_time,
                key.last_used_at,
                key.balance,
                key.balance_updated_at,
                key.total_requests,
                key.total_tokens,
                key.created_at,
                key.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_api_key(&self, id: &str) -> anyhow::Result<Option<ApiKey>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, provider_id, name, key_hash, key_masked, status, last_error, last_error_code, last_error_time, last_used_at, balance, balance_updated_at, total_requests, total_tokens, created_at, updated_at FROM api_keys WHERE id = ?1"
        )?;

        let key = stmt.query_row(rusqlite::params![id], |row| {
            Ok(ApiKey {
                id: row.get(0)?,
                provider_id: row.get(1)?,
                name: row.get(2)?,
                key_hash: row.get(3)?,
                key_masked: row.get(4)?,
                key_plain: None,
                status: row.get(5)?,
                last_error: row.get(6)?,
                last_error_code: row.get(7)?,
                last_error_time: row.get(8)?,
                last_used_at: row.get(9)?,
                balance: row.get(10)?,
                balance_updated_at: row.get(11)?,
                total_requests: row.get(12)?,
                total_tokens: row.get(13)?,
                created_at: row.get(14)?,
                updated_at: row.get(15)?,
            })
        });

        match key {
            Ok(k) => Ok(Some(k)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn delete_api_key(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        // usage_log 已自包含（V12），不再预清理；直接删除 key 即可。
        conn.execute("DELETE FROM api_keys WHERE id = ?1", rusqlite::params![id])?;
        Ok(())
    }

    pub fn list_all_keys(&self) -> anyhow::Result<Vec<ApiKey>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, provider_id, name, key_hash, key_masked, status, last_error, last_error_code, last_error_time, last_used_at, balance, balance_updated_at, total_requests, total_tokens, created_at, updated_at FROM api_keys"
        )?;

        let keys = stmt.query_map([], |row| {
            Ok(ApiKey {
                id: row.get(0)?,
                provider_id: row.get(1)?,
                name: row.get(2)?,
                key_hash: row.get(3)?,
                key_masked: row.get(4)?,
                key_plain: None,
                status: row.get(5)?,
                last_error: row.get(6)?,
                last_error_code: row.get(7)?,
                last_error_time: row.get(8)?,
                last_used_at: row.get(9)?,
                balance: row.get(10)?,
                balance_updated_at: row.get(11)?,
                total_requests: row.get(12)?,
                total_tokens: row.get(13)?,
                created_at: row.get(14)?,
                updated_at: row.get(15)?,
            })
        })?;

        let mut result = Vec::new();
        for key in keys {
            result.push(key?);
        }
        Ok(result)
    }
}
