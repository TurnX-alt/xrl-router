//! 从 DB 加载 key 入池（含解密 + 轮询指针恢复）。
//!
//! 严格遵循「conn 锁必须在块内释放」——否则下方对 KeyPool 加锁 / 对 DB
//! 二次加锁会与代理请求路径形成 ABBA 死锁，或因 Mutex 不可重入而自锁。

use std::collections::HashMap;

use crate::crypto;
use crate::db::Database;
use crate::types::KeyStatus;

use super::types::KeyEntry;

impl super::KeyPool {
    /// Load keys from database for a provider, decrypting key_hash with master key.
    /// Called when plugin keys are synced, so the pool holds plaintext keys for upstream requests.
    pub fn load_keys_from_db(
        &self,
        provider_id: &str,
        db: &Database,
        master_key: &crypto::MasterKey,
    ) -> std::result::Result<(), String> {
        // 注意：conn 锁必须在块内释放（Mutex 不可重入 + 锁序一致性），
        // 否则下方 add_provider_keys 会在持有 DB 锁时拿 KeyPool 锁，
        // 与代理请求路径形成 ABBA 死锁（插件 keys_update 并发时触发）。
        let keys: Vec<KeyEntry> = {
            let conn = db.conn();
            let mut stmt = conn.prepare(
                "SELECT id, provider_id, name, key_hash, key_masked, status, last_error_time, total_requests, total_tokens
                 FROM api_keys WHERE provider_id = ?1"
            ).map_err(|e| e.to_string())?;

            let rows: Vec<KeyEntry> = stmt
                .query_map(rusqlite::params![provider_id], |row| {
                    let cipher: String = row.get(3)?;
                    // Decrypt; on failure (e.g. legacy plaintext) fall back to raw value
                    let plain = crypto::decrypt(&cipher, master_key)
                        .unwrap_or_else(|_| cipher.clone());
                    Ok(KeyEntry {
                        id: row.get(0)?,
                        provider_id: row.get(1)?,
                        name: row.get(2)?,
                        key_hash: plain,
                        key_masked: row.get(4)?,
                        status: KeyStatus::Green,
                        last_error_time: None,
                        total_requests: row.get::<_, i64>(7)? as u64,
                        total_tokens: row.get::<_, i64>(8)? as u64,
                    })
                })
                .map_err(|e| e.to_string())?
                .filter_map(|r| r.ok())
                .collect();
            rows
        }; // conn 锁在此释放

        if !keys.is_empty() {
            self.add_provider_keys(provider_id, keys);
        }

        Ok(())
    }

    /// Load all keys from database into memory, decrypting key_hash with master key.
    /// Called once at startup so the pool holds plaintext keys for upstream requests.
    pub fn load_all_keys_from_db(&self, db: &Database, master_key: &crypto::MasterKey) {
        // 注意：conn 锁必须在块内释放（Mutex 不可重入），否则下方
        // load_persisted_index → db.get_setting() 会对同一把锁二次加锁而死锁。
        let rows: Vec<KeyEntry> = {
            let conn = db.conn();
            let mut stmt = match conn.prepare(
                "SELECT id, provider_id, name, key_hash, key_masked, status, last_error_time, total_requests, total_tokens
                 FROM api_keys",
            ) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("Failed to load keys from db: {}", e);
                    return;
                }
            };

            stmt.query_map([], |row| {
                let cipher: String = row.get(3)?;
                // Decrypt; on failure (e.g. legacy plaintext) fall back to raw value
                // so old keys still work until rotated.
                let plain = crypto::decrypt(&cipher, master_key).unwrap_or_else(|_| cipher.clone());
                Ok(KeyEntry {
                    id: row.get(0)?,
                    provider_id: row.get(1)?,
                    name: row.get(2)?,
                    key_hash: plain,
                    key_masked: row.get(4)?,
                    // 可用性纯内存：启动一律视为可用，运行时按请求结果探测。
                    status: KeyStatus::Green,
                    last_error_time: None,
                    total_requests: row.get::<_, i64>(7)? as u64,
                    total_tokens: row.get::<_, i64>(8)? as u64,
                })
            })
            .ok()
            .map(|iter| iter.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
        }; // conn 锁在此释放

        let mut grouped: HashMap<String, Vec<KeyEntry>> = HashMap::new();
        for k in rows {
            grouped.entry(k.provider_id.clone()).or_default().push(k);
        }

        let mut keys_map = self.keys.write().unwrap();
        for (pid, ks) in grouped {
            if !ks.is_empty() {
                keys_map.insert(pid, ks);
            }
        }
        // Restore rotation indices from persisted settings (falls back to 0).
        let mut index_map = self.current_index.write().unwrap();
        for pid in keys_map.keys() {
            let total = keys_map.get(pid).map(|v| v.len()).unwrap_or(0);
            let idx = self.load_persisted_index(db, pid, total);
            index_map.insert(pid.clone(), idx);
        }
    }
}
