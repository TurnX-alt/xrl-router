//! KeyPool 的数据结构、错误类型与冷却常量。

use crate::types::KeyStatus;
use thiserror::Error;

/// 黄灯 key 冷却时间（秒）：429/402 后冷却 5 分钟，到期自动恢复可用。
pub(crate) const YELLOW_COOLDOWN_SECS: i64 = 300;

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
    #[allow(dead_code)]
    DatabaseError(String),
}

pub type Result<T> = std::result::Result<T, KeyPoolError>;

/// Key pool statistics
#[derive(Debug, Clone)]
pub struct KeyPoolStats {
    pub total: usize,
    pub green: usize,
    pub yellow: usize,
    pub red: usize,
}
