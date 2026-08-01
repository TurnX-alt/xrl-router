//! 轮询取 key：round-robin + 健康过滤 + 指针推进。
//!
//! 持久化指针（DB 锁）下沉到 `mod.rs` 的 `persist_index`；
//! 本方法严格「先在 KeyPool 锁内算结果 → 释放 → 再持久化」。

use chrono::Utc;

use crate::types::KeyStatus;

use super::types::{KeyEntry, KeyPoolError, Result, YELLOW_COOLDOWN_SECS};

impl super::KeyPool {
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
}
