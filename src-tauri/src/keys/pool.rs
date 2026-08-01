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
    pub key_masked: String,
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

    /// Persist current rotation index for a provider
    fn persist_index(&self, provider_id: &str, index: usize) {
        if let Some(db) = self.database.read().unwrap().as_ref() {
            let key = format!("keypool_index_{}", provider_id);
            let _ = db.set_setting(&key, &index.to_string());
        }
    }

    /// Load persisted rotation index for a provider, with bounds checking
    fn load_persisted_index(&self, db: &Database, provider_id: &str, total_keys: usize) -> usize {
        let key = format!("keypool_index_{}", provider_id);
        match db.get_setting(&key) {
            Ok(Some(val)) => {
                match val.parse::<usize>() {
                    Ok(idx) if idx < total_keys => idx,
                    _ => 0,  // Invalid or out of bounds, reset to 0
                }
            }
            _ => 0,  // Not found or error, start from 0
        }
    }

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

    /// Add keys for a provider
    pub fn add_provider_keys(&self, provider_id: &str, keys: Vec<KeyEntry>) {
        let mut keys_map = self.keys.write().unwrap();
        keys_map.insert(provider_id.to_string(), keys);

        let mut index_map = self.current_index.write().unwrap();
        index_map.entry(provider_id.to_string()).or_insert(0);
    }

    /// Get the next available key for a provider (round-robin with health check)
    pub fn get_next_key(&self, provider_id: &str) -> Result<KeyEntry> {
        // 注意：先算结果（持 KeyPool 锁），释放锁后再持久化指针（DB 锁）。
        // 若在锁内调 persist_index，会与 load_keys_from_db 的
        // DB→KeyPool 锁序形成 ABBA 死锁（插件 keys_update 并发时触发）。
        let (entry, next_index) = {
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

            let mut picked: Option<(KeyEntry, usize)> = None;
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
                    let next = (idx + 1) % total_keys;
                    index_map.insert(provider_id.to_string(), next);
                    picked = Some((provider_keys[idx].clone(), next));
                    break;
                }
            }
            picked.ok_or(KeyPoolError::NoAvailableKeys)?
        };
        // KeyPool 锁已释放，此时才拿 DB 锁持久化指针
        self.persist_index(provider_id, next_index);
        Ok(entry)
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
        // 注意：KeyPool 锁内只改内存，锁释放后再持久化（DB 锁），
        // 避免与 load_keys_from_db 的 DB→KeyPool 锁序形成 ABBA 死锁。
        let mut found = false;
        let mut persist_key_id: Option<String> = None;
        {
            let mut keys_map = self.keys.write().unwrap();
            if let Some(provider_keys) = keys_map.get_mut(provider_id) {
                for key in provider_keys.iter_mut() {
                    if key.key_hash == key_hash {
                        key.status = KeyStatus::Green;
                        key.total_requests += 1;
                        key.total_tokens += tokens_used as u64;
                        persist_key_id = Some(key.id.clone());
                        found = true;
                        break;
                    }
                }
            }
        }
        // KeyPool 锁已释放，此时才拿 DB 锁持久化
        if let Some(key_id) = persist_key_id {
            self.persist_key_usage(&key_id, tokens_used);
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

    /// 删除单个 key（运行时同步内存 + 修正指针）。
    /// 删除后如果指针越界（指向了被删的位置），自动回退到 0 重新开始轮询。
    /// 返回 true 表示 key 存在并被移除；false 表示未找到（可能已在内存外）。
    pub fn remove_key(&self, provider_id: &str, key_id: &str) -> bool {
        let mut removed = false;
        {
            let mut keys_map = self.keys.write().unwrap();
            if let Some(provider_keys) = keys_map.get_mut(provider_id) {
                let before = provider_keys.len();
                provider_keys.retain(|k| k.id != key_id);
                removed = provider_keys.len() < before;
            }
        }
        if removed {
            self.fix_index_after_change(provider_id);
        }
        removed
    }

    /// 密钥池变动后修正轮询指针：
    /// - 指针越界（key 减少导致）→ 回退到 0
    /// - 持久化的指针同样修正（避免重启后读到越界值）
    /// 注意锁顺序：先读 keys（释放）再写 index，避免与 get_next_key 的
    /// keys(write) → index(write) 顺序形成死锁。
    fn fix_index_after_change(&self, provider_id: &str) {
        let total = self
            .keys
            .read()
            .unwrap()
            .get(provider_id)
            .map(|v| v.len())
            .unwrap_or(0);
        let mut index_map = self.current_index.write().unwrap();
        let current = index_map.get(provider_id).copied().unwrap_or(0);
        let fixed = if total == 0 || current >= total { 0 } else { current };
        index_map.insert(provider_id.to_string(), fixed);
        drop(index_map);
        self.persist_index(provider_id, fixed);
    }

    /// 测试专用：直接设置某个 provider 的轮转指针（模拟启动恢复后的状态）。
    #[cfg(test)]
    pub fn set_pool_index_for_test(&self, provider_id: &str, index: usize) {
        let mut index_map = self.current_index.write().unwrap();
        index_map.insert(provider_id.to_string(), index);
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
            key_masked: format!("****{}", id),
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
    fn test_index_persistence_roundtrip() {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let pool = KeyPool::new();
        pool.set_database(db.clone());

        // 指针推进后应持久化到 settings 表
        pool.persist_index("p1", 2);
        let saved = db.get_setting("keypool_index_p1").unwrap().unwrap();
        assert_eq!(saved, "2");

        // 模拟重启：有效值应恢复
        let idx = pool.load_persisted_index(&db, "p1", 5);
        assert_eq!(idx, 2);

        // 越界（key 数减少）应回退到 0，而不是 panic
        let idx = pool.load_persisted_index(&db, "p1", 2);
        assert_eq!(idx, 0);

        // 无效值（非数字）应回退到 0
        db.set_setting("keypool_index_p1", "abc").unwrap();
        let idx = pool.load_persisted_index(&db, "p1", 5);
        assert_eq!(idx, 0);

        // 不存在的 provider 应回退到 0
        let idx = pool.load_persisted_index(&db, "p2", 5);
        assert_eq!(idx, 0);
    }

    #[test]
    fn test_resume_from_persisted_index_not_restart() {
        // 8 个 key，指针持久化为 6（最后用了 key[5]）。
        // 重启后必须从 6 开始，而不是从头（0）试——即使 key[5] 已失效。
        let db = crate::db::Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.set_setting("keypool_index_p", "6").unwrap();

        let pool = KeyPool::new();
        pool.set_database(db.clone());
        let keys: Vec<KeyEntry> = (0..8)
            .map(|i| create_test_key(&format!("key{}", i), KeyStatus::Green))
            .collect();
        pool.add_provider_keys("p", keys);

        // 模拟启动：手动恢复持久化指针（load_all_keys_from_db 的逻辑）
        let idx = pool.load_persisted_index(&db, "p", 8);
        pool.set_pool_index_for_test("p", idx);

        // key[5] 失效
        pool.mark_key_invalid("p", "hash_key5").unwrap();

        // 应从 6 开始，而非 0/1
        let k = pool.get_next_key("p").unwrap();
        assert_eq!(k.id, "key6", "must resume at key after last used (5), not restart from 0");

        let k = pool.get_next_key("p").unwrap();
        assert_eq!(k.id, "key7");

        let k = pool.get_next_key("p").unwrap();
        assert_eq!(k.id, "key0", "wraps around to 0 only after 6,7");
    }

    #[test]
    fn test_remove_key_fixes_out_of_bounds_index() {
        // 场景：指针持久化为 6（最后用了 key[5]），运行中删除了 key[6]。
        // 指针仍指向 6，但此时 key 数已从 8 减到 7 → 必须自动回退到 0。
        let db = crate::db::Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let pool = KeyPool::new();
        pool.set_database(db.clone());

        let keys: Vec<KeyEntry> = (0..8)
            .map(|i| create_test_key(&format!("key{}", i), KeyStatus::Green))
            .collect();
        pool.add_provider_keys("p", keys);
        pool.set_pool_index_for_test("p", 6); // 模拟指针=6（最后用了 key[5]）

        // 删除 key[6]（指针指向的位置）
        assert!(pool.remove_key("p", "key6"));

        // 指针应已修正（6 >= 7？不对——删掉后 total=7，指针 6 < 7 仍有效，
        // 但 key[6] 没了，实际应指向下一个可用的，即回退检查后仍是 6，
        // 而 get_next_key 的 % total 会让它从 6 开始 = key0）。
        // 关键断言：get_next_key 不 panic、不返回已删的 key。
        for _ in 0..7 {
            let k = pool.get_next_key("p").unwrap();
            assert_ne!(k.id, "key6", "deleted key must never be returned");
        }
    }

    #[test]
    fn test_remove_key_resets_index_when_out_of_bounds() {
        // 场景：只有 3 个 key，指针=2（最后用了 key[1]），删除 key[2] 后 total=2，
        // 指针 2 >= 2 → 必须回退到 0。
        let db = crate::db::Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let pool = KeyPool::new();
        pool.set_database(db.clone());

        let keys: Vec<KeyEntry> = (0..3)
            .map(|i| create_test_key(&format!("key{}", i), KeyStatus::Green))
            .collect();
        pool.add_provider_keys("p", keys);
        pool.set_pool_index_for_test("p", 2);

        assert!(pool.remove_key("p", "key2"));
        // 指针应已修正为 0（2 >= 2 越界）
        let k = pool.get_next_key("p").unwrap();
        assert_eq!(k.id, "key0", "index must reset to 0 when out of bounds");
    }

    #[test]
    fn test_remove_provider_clears_index() {
        let pool = KeyPool::new();
        let keys = vec![
            create_test_key("key1", KeyStatus::Green),
            create_test_key("key2", KeyStatus::Green),
        ];
        pool.add_provider_keys("p", keys);
        pool.set_pool_index_for_test("p", 1);

        pool.remove_provider("p");
        // 移除后 get_next_key 应返回 KeyNotFound（而非残留内存）
        assert!(matches!(
            pool.get_next_key("p"),
            Err(KeyPoolError::KeyNotFound(_))
        ));
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
