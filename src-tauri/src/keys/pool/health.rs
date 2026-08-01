//! 健康状态管理：traffic-light 标记、用量记录、统计与实时可用性查询。
//!
//! 所有写操作在 KeyPool write lock 释放后才广播 / 持久化，避免 broadcast
//! 内部读锁与已持有的写锁死锁。

use chrono::Utc;

use crate::types::KeyStatus;

use super::types::{KeyPoolStats, Result, YELLOW_COOLDOWN_SECS};

impl super::KeyPool {
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
            Err(super::types::KeyPoolError::KeyNotFound(key_hash.to_string()))
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
            Err(super::types::KeyPoolError::KeyNotFound(key_hash.to_string()))
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
            Err(super::types::KeyPoolError::KeyNotFound(key_hash.to_string()))
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
}
