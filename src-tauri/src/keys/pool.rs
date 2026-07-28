use crate::crypto;
use crate::types::KeyStatus;
use crate::db::Database;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use thiserror::Error;

/// 黄灯 key 冷却时间（秒）：429/402 后冷却 5 分钟，到期自动恢复可用。
const YELLOW_COOLDOWN_SECS: i64 = 300;

/// Key entry for the pool (simplified version of types::Key)
#[derive(Debug, Clone)]
pub struct KeyEntry {
    pub id: String,
    pub provider_id: String,
    pub name: String,
    pub key_hash: String,
    pub status: KeyStatus,
    pub last_error_time: Option<i64>,
    pub total_requests: u64,
    pub total_tokens: u64,
}

#[derive(Error, Debug)]
pub enum KeyPoolError {
    #[error("No available keys")]
    NoAvailableKeys,
    #[error("Key not found: {0}")]
    KeyNotFound(String),
    #[error("Database error: {0}")]
    DatabaseError(String),
}

pub type Result<T> = std::result::Result<T, KeyPoolError>;

/// Key pool manager with traffic light health tracking and DB persistence
#[derive(Clone)]
pub struct KeyPool {
    /// All keys indexed by provider_id
    keys: Arc<RwLock<HashMap<String, Vec<KeyEntry>>>>,
    /// Current rotation index per provider
    current_index: Arc<RwLock<HashMap<String, usize>>>,
    /// Database reference for persistence (optional, set after construction)
    database: Arc<RwLock<Option<Database>>>,
    key_stats_tx: tokio::sync::broadcast::Sender<serde_json::Value>,
}

impl KeyPool {
    pub fn new() -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(64);
        Self {
            keys: Arc::new(RwLock::new(HashMap::new())),
            current_index: Arc::new(RwLock::new(HashMap::new())),
            database: Arc::new(RwLock::new(None)),
            key_stats_tx: tx,
        }
    }

    /// Set the database reference for persistence
    pub fn set_database(&self, db: Database) {
        let mut db_ref = self.database.write().unwrap();
        *db_ref = Some(db);
    }

    /// Set the broadcast sender for key stats
    pub fn set_key_stats_tx(&mut self, tx: tokio::sync::broadcast::Sender<serde_json::Value>) {
        self.key_stats_tx = tx;
    }

    /// Broadcast key stats for a provider over the channel
    fn broadcast_key_stats(&self, provider_id: &str) {
        if let Some(stats) = self.get_stats(provider_id) {
            let _ = self.key_stats_tx.send(serde_json::json!({
                "type": "key_stats",
                "provider_id": provider_id,
                "green": stats.green,
                "total": stats.total,
            }));
        }
    }

    /// Persist key usage stats to database
    fn persist_key_usage(&self, key_id: &str, tokens: i64) {
        if let Some(db) = self.database.read().unwrap().as_ref() {
            let now = Utc::now().timestamp();
            let _ = db.execute(
                "UPDATE api_keys SET total_requests = total_requests + 1, total_tokens = total_tokens + ?1, last_used_at = ?2, updated_at = ?3 WHERE id = ?4",
                rusqlite::params![tokens, now, now, key_id],
            );
        }
    }

    /// Load keys from database for a provider
    pub fn load_keys_from_db(&self, provider_id: &str, db: &Database) -> std::result::Result<(), String> {
        let conn = db.conn();
        let mut stmt = conn.prepare(
            "SELECT id, provider_id, name, key_hash, status, last_error_time, total_requests, total_tokens
             FROM api_keys WHERE provider_id = ?1"
        ).map_err(|e| e.to_string())?;

        let keys: Vec<KeyEntry> = stmt
            .query_map(rusqlite::params![provider_id], |row| {
                Ok(KeyEntry {
                    id: row.get(0)?,
                    provider_id: row.get(1)?,
                    name: row.get(2)?,
                    key_hash: row.get(3)?,
                    status: KeyStatus::Green,
                    last_error_time: None,
                    total_requests: row.get::<_, i64>(6)? as u64,
                    total_tokens: row.get::<_, i64>(7)? as u64,
                })
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();

        if !keys.is_empty() {
            self.add_provider_keys(provider_id, keys);
        }

        Ok(())
    }

    /// Load all keys from database into memory, decrypting key_hash with master key.
    /// Called once at startup so the pool holds plaintext keys for upstream requests.
    pub fn load_all_keys_from_db(&self, db: &Database, master_key: &crypto::MasterKey) {
        let conn = db.conn();
        let mut stmt = match conn.prepare(
            "SELECT id, provider_id, name, key_hash, status, last_error_time, total_requests, total_tokens
             FROM api_keys",
        ) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Failed to load keys from db: {}", e);
                return;
            }
        };

        let rows: Vec<KeyEntry> = stmt
            .query_map([], |row| {
                let cipher: String = row.get(3)?;
                // Decrypt; on failure (e.g. legacy plaintext) fall back to raw value
                // so old keys still work until rotated.
                let plain = crypto::decrypt(&cipher, master_key).unwrap_or_else(|_| cipher.clone());
                Ok(KeyEntry {
                    id: row.get(0)?,
                    provider_id: row.get(1)?,
                    name: row.get(2)?,
                    key_hash: plain,
                    // 可用性纯内存：启动一律视为可用，运行时按请求结果探测。
                    status: KeyStatus::Green,
                    last_error_time: None,
                    total_requests: row.get::<_, i64>(6)? as u64,
                    total_tokens: row.get::<_, i64>(7)? as u64,
                })
            })
            .ok()
            .map(|iter| iter.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

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
        let mut index_map = self.current_index.write().unwrap();
        for pid in keys_map.keys() {
            index_map.entry(pid.clone()).or_insert(0);
        }
    }

    /// Add keys for a provider
    pub fn add_provider_keys(&self, provider_id: &str, keys: Vec<KeyEntry>) {
        let mut keys_map = self.keys.write().unwrap();
        keys_map.insert(provider_id.to_string(), keys);

        let mut index_map = self.current_index.write().unwrap();
        index_map.entry(provider_id.to_string()).or_insert(0);
    }

    /// Get the next available key for a provider (round-robin with health check)
    pub fn get_next_key(&self, provider_id: &str) -> Result<KeyEntry> {
        let mut keys_map = self.keys.write().unwrap();
        let mut index_map = self.current_index.write().unwrap();

        let provider_keys = keys_map
            .get_mut(provider_id)
            .ok_or_else(|| KeyPoolError::KeyNotFound(provider_id.to_string()))?;

        if provider_keys.is_empty() {
            return Err(KeyPoolError::NoAvailableKeys);
        }

        let current_idx = index_map.get(provider_id).copied().unwrap_or(0);
        let total_keys = provider_keys.len();
        let now = Utc::now().timestamp();

        for offset in 0..total_keys {
            let idx = (current_idx + offset) % total_keys;
            let key = &provider_keys[idx];
            // Red 永不使用；Yellow 冷却到期后恢复可用；Green/Unknown 总是可用。
            let usable = match key.status {
                KeyStatus::Green | KeyStatus::Unknown => true,
                KeyStatus::Yellow => key
                    .last_error_time
                    .map(|t| now - t >= YELLOW_COOLDOWN_SECS)
                    .unwrap_or(true),
                KeyStatus::Red => false,
            };
            if usable {
                index_map.insert(provider_id.to_string(), (idx + 1) % total_keys);
                return Ok(provider_keys[idx].clone());
            }
        }

        Err(KeyPoolError::NoAvailableKeys)
    }

    /// Mark a key as unhealthy (red - invalid) and persist to DB
    pub fn mark_key_invalid(&self, provider_id: &str, key_hash: &str) -> Result<()> {
        let mut found = false;
        {
            let mut keys_map = self.keys.write().unwrap();
            if let Some(provider_keys) = keys_map.get_mut(provider_id) {
                for key in provider_keys.iter_mut() {
                    if key.key_hash == key_hash {
                        key.status = KeyStatus::Red;
                        key.last_error_time = Some(Utc::now().timestamp());
                        found = true;
                        break;
                    }
                }
            }
        }
        // write lock 释放后再广播（broadcast 内部要读 lock，否则死锁）
        if found {
            self.broadcast_key_stats(provider_id);
            Ok(())
        } else {
            Err(KeyPoolError::KeyNotFound(key_hash.to_string()))
        }
    }

    /// Mark a key as low quota (yellow - rate limited) and persist to DB
    pub fn mark_key_low_quota(&self, provider_id: &str, key_hash: &str) -> Result<()> {
        let mut found = false;
        {
            let mut keys_map = self.keys.write().unwrap();
            if let Some(provider_keys) = keys_map.get_mut(provider_id) {
                for key in provider_keys.iter_mut() {
                    if key.key_hash == key_hash {
                        key.status = KeyStatus::Yellow;
                        key.last_error_time = Some(Utc::now().timestamp());
                        found = true;
                        break;
                    }
                }
            }
        }
        if found {
            self.broadcast_key_stats(provider_id);
            Ok(())
        } else {
            Err(KeyPoolError::KeyNotFound(key_hash.to_string()))
        }
    }

    /// Record successful usage of a key and persist to DB
    pub fn record_key_success(
        &self,
        provider_id: &str,
        key_hash: &str,
        tokens_used: i64,
    ) -> Result<()> {
        let mut found = false;
        {
            let mut keys_map = self.keys.write().unwrap();
            if let Some(provider_keys) = keys_map.get_mut(provider_id) {
                for key in provider_keys.iter_mut() {
                    if key.key_hash == key_hash {
                        key.status = KeyStatus::Green;
                        key.total_requests += 1;
                        key.total_tokens += tokens_used as u64;
                        self.persist_key_usage(&key.id, tokens_used);
                        found = true;
                        break;
                    }
                }
            }
        }
        if found {
            self.broadcast_key_stats(provider_id);
            Ok(())
        } else {
            Err(KeyPoolError::KeyNotFound(key_hash.to_string()))
        }
    }

    /// Get key pool statistics for a provider
    pub fn get_stats(&self, provider_id: &str) -> Option<KeyPoolStats> {
        let keys_map = self.keys.read().unwrap();
        let provider_keys = keys_map.get(provider_id)?;

        let total = provider_keys.len();
        let now = Utc::now().timestamp();
        let mut green = 0;
        let mut yellow = 0;
        let mut red = 0;
        for k in provider_keys.iter() {
            match k.status {
                KeyStatus::Green | KeyStatus::Unknown => green += 1,
                KeyStatus::Red => red += 1,
                KeyStatus::Yellow => {
                    // 冷却到期 → 视为可用（green）；冷却中 → yellow
                    if k.last_error_time
                        .map(|t| now - t >= YELLOW_COOLDOWN_SECS)
                        .unwrap_or(true)
                    {
                        green += 1;
                    } else {
                        yellow += 1;
                    }
                }
            }
        }

        Some(KeyPoolStats {
            total,
            green,
            yellow,
            red,
        })
    }

    /// 获取单个 key 的实时可用状态（含黄灯冷却判断）。
    /// 供 list_keys 覆盖 DB 里的 status 残留 —— 可用性纯内存。
    pub fn get_key_status(&self, key_id: &str) -> Option<KeyStatus> {
        let keys_map = self.keys.read().unwrap();
        let now = Utc::now().timestamp();
        for ks in keys_map.values() {
            for k in ks {
                if k.id == key_id {
                    return Some(match k.status {
                        KeyStatus::Yellow if k
                            .last_error_time
                            .map(|t| now - t < YELLOW_COOLDOWN_SECS)
                            .unwrap_or(false) =>
                        {
                            KeyStatus::Yellow // 冷却中
                        }
                        KeyStatus::Yellow => KeyStatus::Green, // 冷却到期
                        s => s,
                    });
                }
            }
        }
        None
    }

    /// Remove all keys for a provider
    pub fn remove_provider(&self, provider_id: &str) {
        let mut keys_map = self.keys.write().unwrap();
        keys_map.remove(provider_id);

        let mut index_map = self.current_index.write().unwrap();
        index_map.remove(provider_id);
    }
}

/// Key pool statistics
#[derive(Debug, Clone)]
pub struct KeyPoolStats {
    pub total: usize,
    pub green: usize,
    pub yellow: usize,
    pub red: usize,
}

impl Default for KeyPool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_key(id: &str, status: KeyStatus) -> KeyEntry {
        KeyEntry {
            id: id.to_string(),
            provider_id: "test_provider".to_string(),
            name: format!("Test Key {}", id),
            key_hash: format!("hash_{}", id),
            status,
            last_error_time: None,
            total_requests: 0,
            total_tokens: 0,
        }
    }

    #[test]
    fn test_round_robin() {
        let pool = KeyPool::new();
        let keys = vec![
            create_test_key("key1", KeyStatus::Green),
            create_test_key("key2", KeyStatus::Green),
            create_test_key("key3", KeyStatus::Green),
        ];

        pool.add_provider_keys("test_provider", keys);

        let k1 = pool.get_next_key("test_provider").unwrap();
        assert_eq!(k1.id, "key1");

        let k2 = pool.get_next_key("test_provider").unwrap();
        assert_eq!(k2.id, "key2");

        let k3 = pool.get_next_key("test_provider").unwrap();
        assert_eq!(k3.id, "key3");

        // Should wrap around
        let k4 = pool.get_next_key("test_provider").unwrap();
        assert_eq!(k4.id, "key1");
    }

    #[test]
    fn test_skip_red_keys() {
        let pool = KeyPool::new();
        let keys = vec![
            create_test_key("key1", KeyStatus::Red),
            create_test_key("key2", KeyStatus::Green),
            create_test_key("key3", KeyStatus::Green),
        ];

        pool.add_provider_keys("test_provider", keys);

        let k = pool.get_next_key("test_provider").unwrap();
        assert_ne!(k.id, "key1"); // Should skip red key
    }

    #[test]
    fn test_no_available_keys() {
        let pool = KeyPool::new();
        let keys = vec![
            create_test_key("key1", KeyStatus::Red),
            create_test_key("key2", KeyStatus::Red),
        ];

        pool.add_provider_keys("test_provider", keys);

        let result = pool.get_next_key("test_provider");
        assert!(matches!(result, Err(KeyPoolError::NoAvailableKeys)));
    }

    #[test]
    fn test_mark_invalid() {
        let pool = KeyPool::new();
        let keys = vec![create_test_key("key1", KeyStatus::Green)];

        pool.add_provider_keys("test_provider", keys);
        pool.mark_key_invalid("test_provider", "hash_key1").unwrap();

        let result = pool.get_next_key("test_provider");
        assert!(matches!(result, Err(KeyPoolError::NoAvailableKeys)));
    }

    #[test]
    fn test_record_success() {
        let pool = KeyPool::new();
        let keys = vec![create_test_key("key1", KeyStatus::Yellow)];

        pool.add_provider_keys("test_provider", keys);
        pool.record_key_success("test_provider", "hash_key1", 100).unwrap();

        let k = pool.get_next_key("test_provider").unwrap();
        assert_eq!(k.total_requests, 1);
        assert_eq!(k.total_tokens, 100);
    }
}
