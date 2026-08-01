//! 密码学原语：Provider API key 加密（AES-256-GCM）与 Service Key 哈希（argon2）。
//!
//! 主密钥首次启动随机生成，存于 `data/master.key`（权限 0600）。
//! 数据库单独泄露不暴露密钥；丢失 master.key 则已加密的 Provider Key 无法解密。

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::{anyhow, Context, Result};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use base64::Engine;
use rand::rngs::OsRng;
use rand::RngCore;
use std::path::Path;

/// 256 位主密钥。
pub type MasterKey = [u8; 32];

/// 从文件读取主密钥；不存在则随机生成并持久化（unix 下权限 0600）。
pub fn load_or_create_master_key(path: &Path) -> Result<MasterKey> {
    if path.exists() {
        let raw = std::fs::read_to_string(path).context("read master key")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(raw.trim())
            .context("decode master key base64")?;
        if bytes.len() != 32 {
            return Err(anyhow!(
                "master key must be 32 bytes, got {}",
                bytes.len()
            ));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        Ok(key)
    } else {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(key);
        std::fs::write(path, encoded).context("write master key")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0600));
        }
        tracing::info!("Generated new master key at {}", path.display());
        Ok(key)
    }
}

fn cipher(key: &MasterKey) -> Result<Aes256Gcm> {
    Aes256Gcm::new_from_slice(key).map_err(|e| anyhow!("invalid aes key: {e:?}"))
}

/// 加密明文，返回 `base64(nonce || ciphertext)`。
pub fn encrypt(plain: &str, key: &MasterKey) -> Result<String> {
    let cipher = cipher(key)?;
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plain.as_bytes())
        .map_err(|e| anyhow!("encrypt: {e:?}"))?;
    let mut blob = Vec::with_capacity(12 + ciphertext.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);
    Ok(base64::engine::general_purpose::STANDARD.encode(&blob))
}

/// 解密 `base64(nonce || ciphertext)`。
pub fn decrypt(blob: &str, key: &MasterKey) -> Result<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(blob)
        .context("decode ciphertext base64")?;
    if bytes.len() < 13 {
        return Err(anyhow!("ciphertext too short"));
    }
    let (nonce_bytes, ciphertext) = bytes.split_at(12);
    let cipher = cipher(key)?;
    let nonce = Nonce::from_slice(nonce_bytes);
    let plain = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow!("decrypt: {e:?}"))?;
    String::from_utf8(plain).context("plaintext utf8")
}

/// Service Key 的 argon2 哈希（随机盐，盐嵌入返回串）。
pub fn hash_service_key(raw_key: &str) -> Result<String> {
    // 限定在函数内的 OsRng，避免与上方 rand::rngs::OsRng 重名。
    use argon2::password_hash::rand_core::OsRng;
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(raw_key.as_bytes(), &salt)
        .map_err(|e| anyhow!("argon2 hash: {e:?}"))?;
    Ok(hash.to_string())
}

/// 校验 Service Key 明文与已存的 argon2 哈希串。
pub fn verify_service_key(raw_key: &str, stored_hash: &str) -> bool {
    let parsed = match PasswordHash::new(stored_hash) {
        Ok(p) => p,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(raw_key.as_bytes(), &parsed)
        .is_ok()
}
