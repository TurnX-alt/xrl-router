//! 密钥选取（round-robin）与基于上游状态码的健康反馈。

use crate::gateway::server::AppState;
use crate::keys::KeyPool;

use super::route::PickedKey;

/// Pick the next available key for a provider from the pool (round-robin,
/// skips Red/Yellow). Returns PickedKey with plaintext key, id, name, masked.
/// Called in the retry loop so 401/402/403/429 rotate keys.
pub(super) fn pick_key_for(state: &AppState, provider_id: &str) -> Option<PickedKey> {
    match state.keys.get_next_key(provider_id) {
        Ok(entry) => Some(PickedKey {
            key_hash: entry.key_hash,
            id: entry.id,
            name: entry.name,
            key_masked: entry.key_masked,
        }),
        Err(_) => None,
    }
}

/// Drive the key pool health based on an upstream HTTP status code.
/// 401/403 -> red (invalid key), 402/429 -> yellow (quota/rate limit),
/// 2xx -> green (success). 5xx and other 4xx (400/404…) are NOT key problems,
/// so they leave the key state untouched.
pub(super) fn update_key_health(pool: &KeyPool, provider_id: &str, key: &str, status: u16) {
    match status {
        401 | 403 => { let _ = pool.mark_key_invalid(provider_id, key); }
        402 | 429 => { let _ = pool.mark_key_low_quota(provider_id, key); }
        200..=299 => { let _ = pool.record_key_success(provider_id, key, 0); }
        _ => {}
    }
}
