use rusqlite::{Connection, Result};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::{info, error};
use serde::{Deserialize, Serialize};

use crate::types::{Provider, ProviderKind, ApiKey, Model};

pub mod schema;
pub mod queries;

/// Database wrapper for SQLite operations with thread-safe access.
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    /// Open a new database connection with WAL mode enabled for better concurrency.
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA busy_timeout = 5000;
             PRAGMA synchronous = NORMAL;"
        )?;
        info!("SQLite WAL mode enabled");
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Open an in-memory database (for tests).
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Run all pending migrations.
    pub fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // Create schema_version table if not exists
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY,
                applied_at INTEGER NOT NULL
            );"
        )?;

        // Get current schema version
        let current_version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if current_version >= schema::MIGRATIONS.len() as i64 {
            info!("Database schema is up to date (v{})", current_version);
            return Ok(());
        }

        info!(
            "Running database migrations from v{} to v{}...",
            current_version,
            schema::MIGRATIONS.len()
        );

        // Run pending migrations
        for (i, migration) in schema::MIGRATIONS.iter().enumerate().skip(current_version as usize) {
            let version = (i + 1) as i64;

            conn.execute_batch(migration)?;

            conn.execute(
                "INSERT OR REPLACE INTO schema_version (version, applied_at) VALUES (?, ?)",
                rusqlite::params![version, chrono::Utc::now().timestamp()],
            )?;

            info!("  Migration v{} applied", version);
        }

        info!("Database migrations complete");
        Ok(())
    }

    /// Get a lock on the database connection.
    pub fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap()
    }

    /// Test database connectivity.
    pub fn test_connection(&self) -> anyhow::Result<()> {
        let conn = self.conn();
        conn.execute_batch("SELECT 1")?;
        Ok(())
    }

    /// Execute a query and return affected rows count.
    pub fn execute(&self, sql: &str, params: impl rusqlite::Params) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute(sql, params)
    }

    /// Execute a batch of SQL statements.
    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(sql)
    }

    // Provider CRUD methods
    pub fn save_provider(&self, provider: &Provider) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        // 注意：不能用 INSERT OR REPLACE —— REPLACE = DELETE + INSERT，
        // DELETE 会触发 models/api_keys 的 ON DELETE CASCADE，把子表数据全清掉。
        // UPSERT 只更新本行，不碰子表。
        conn.execute(
            "INSERT INTO providers (id, name, kind, base_url, api_path, config_json, enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
                name=excluded.name, kind=excluded.kind, base_url=excluded.base_url,
                api_path=excluded.api_path, config_json=excluded.config_json,
                enabled=excluded.enabled, updated_at=excluded.updated_at",
            rusqlite::params![
                provider.id,
                provider.name,
                provider.kind.to_string(),
                provider.base_url,
                provider.api_path,
                serde_json::to_string(&provider.config)?,
                provider.enabled,
                provider.created_at,
                provider.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn delete_provider(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        // 删除该 provider 的 usage_log（含其 key/model 引用），避免 FK RESTRICT；
        // 随后 CASCADE 自动清理 routes/api_keys。
        conn.execute(
            "DELETE FROM usage_log WHERE provider_id = ?1",
            rusqlite::params![id],
        )?;
        conn.execute("DELETE FROM providers WHERE id = ?1", rusqlite::params![id])?;
        Ok(())
    }

    pub fn list_all_providers(&self) -> anyhow::Result<Vec<Provider>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, kind, base_url, api_path, config_json, enabled, created_at, updated_at FROM providers"
        )?;

        let providers = stmt.query_map([], |row| {
            let kind_str: String = row.get(2)?;
            let config_str: String = row.get(5)?;
            Ok(Provider {
                id: row.get(0)?,
                name: row.get(1)?,
                kind: ProviderKind::from_str(&kind_str),
                base_url: row.get(3)?,
                api_path: row.get(4)?,
                config: serde_json::from_str(&config_str).unwrap_or_default(),
                enabled: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?;

        let mut result = Vec::new();
        for provider in providers {
            result.push(provider?);
        }
        Ok(result)
    }

    // API Key CRUD methods
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
        // 先解除 usage_log 引用（key_id 可空，SET NULL 保留统计），避免 FK RESTRICT
        conn.execute(
            "UPDATE usage_log SET key_id = NULL WHERE key_id = ?1",
            rusqlite::params![id],
        )?;
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

    // Service Key CRUD methods
    pub fn save_service_key(&self, id: &str, name: &str, key_hash: &str, key_masked: &str) -> anyhow::Result<()> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        // UPSERT 而非 INSERT OR REPLACE：REPLACE 会触发 usage_log.service_key_id 的 FK 清理。
        conn.execute(
            "INSERT INTO service_keys (id, name, key_hash, key_masked, total_requests, total_tokens, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 0, 0, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                name=excluded.name, key_hash=excluded.key_hash,
                key_masked=excluded.key_masked, updated_at=excluded.updated_at",
            rusqlite::params![id, name, key_hash, key_masked, now, now],
        )?;
        Ok(())
    }

    pub fn list_service_keys(&self) -> anyhow::Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, key_masked, allowed_models, total_requests, total_tokens, last_used_at, created_at FROM service_keys"
        )?;

        let keys = stmt.query_map([], |row| {
            let allowed_str: String = row.get(3)?;
            let allowed: serde_json::Value =
                serde_json::from_str(&allowed_str).unwrap_or(serde_json::json!([]));
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "key_masked": row.get::<_, String>(2)?,
                "allowed_models": allowed,
                "total_requests": row.get::<_, i64>(4)?,
                "total_tokens": row.get::<_, i64>(5)?,
                "last_used_at": row.get::<_, Option<i64>>(6)?,
                "created_at": row.get::<_, i64>(7)?,
            }))
        })?;

        let mut result = Vec::new();
        for key in keys {
            result.push(key?);
        }
        Ok(result)
    }

    pub fn delete_service_key(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        // 先解除 usage_log 引用（service_key_id 可空，SET NULL 保留统计），避免 FK RESTRICT
        conn.execute(
            "UPDATE usage_log SET service_key_id = NULL WHERE service_key_id = ?1",
            rusqlite::params![id],
        )?;
        conn.execute("DELETE FROM service_keys WHERE id = ?1", rusqlite::params![id])?;
        Ok(())
    }

    /// Update a service key's name and/or allowed_models
    pub fn update_service_key(
        &self,
        id: &str,
        name: Option<&str>,
        allowed_models: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();
        if let Some(n) = name {
            conn.execute(
                "UPDATE service_keys SET name = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![n, now, id],
            )?;
        }
        if let Some(a) = allowed_models {
            conn.execute(
                "UPDATE service_keys SET allowed_models = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![a, now, id],
            )?;
        }
        Ok(())
    }

    // Model CRUD methods
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
        // model_id NOT NULL：删除其历史 usage_log，避免 FK RESTRICT
        conn.execute(
            "DELETE FROM usage_log WHERE model_id = ?1",
            rusqlite::params![id],
        )?;
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

    // Statistics methods
    pub fn get_stats(&self) -> anyhow::Result<(i64, i64)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT
                COALESCE(SUM(prompt_tokens + completion_tokens + cache_read_input_tokens), 0) as total_tokens,
                COUNT(*) as total_requests
             FROM usage_log"
        )?;

        let stats = stmt.query_row([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
            ))
        })?;

        Ok(stats)
    }

    pub fn get_stats_by_provider(&self) -> anyhow::Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT
                p.name as provider_name,
                COUNT(*) as requests,
                COALESCE(SUM(u.prompt_tokens + u.completion_tokens + u.cache_read_input_tokens), 0) as tokens
             FROM usage_log u
             JOIN providers p ON u.provider_id = p.id
             GROUP BY p.id, p.name"
        )?;

        let stats = stmt.query_map([], |row| {
            Ok(serde_json::json!({
                "provider_name": row.get::<_, String>(0)?,
                "requests": row.get::<_, i64>(1)?,
                "tokens": row.get::<_, i64>(2)?,
            }))
        })?;

        let mut result = Vec::new();
        for stat in stats {
            result.push(stat?);
        }
        Ok(result)
    }

    /// Get a setting value by key (generic key-value store).
    pub fn get_setting(&self, key: &str) -> anyhow::Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = ?1")?;
        let row = stmt.query_row(rusqlite::params![key], |row| row.get::<_, String>(0));
        match row {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Set (upsert) a setting value.
    pub fn set_setting(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    // Usage logging + stats aggregation ---------------------------------

    /// Append one row to usage_log. Called once per proxied request (success or failure).
    pub fn insert_usage_log(
        &self,
        timestamp: i64,
        provider_id: &str,
        model_id: &str,
        key_id: Option<&str>,
        service_key_id: Option<&str>,
        request_type: &str,
        prompt_tokens: i64,
        completion_tokens: i64,
        latency_ms: i64,
        success: bool,
        error_message: Option<&str>,
        cache_read_input_tokens: i64,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO usage_log (timestamp, provider_id, model_id, key_id, service_key_id, request_type, prompt_tokens, completion_tokens, latency_ms, success, error_message, cache_read_input_tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            rusqlite::params![
                timestamp,
                provider_id,
                model_id,
                key_id,
                service_key_id,
                request_type,
                prompt_tokens,
                completion_tokens,
                latency_ms,
                success as i32,
                error_message,
                cache_read_input_tokens,
            ],
        )?;
        Ok(())
    }

    /// Per-bucket, per-key token aggregation in [from_ts, to_ts].
    /// `bucket_seconds` controls the time bucket (3600 = hour, 86400 = day).
    /// The bucket label is encoded `h{bucket}` for hourly and `d{bucket}` for daily,
    /// where `bucket = floor(unix_seconds / bucket_seconds)`; the frontend chart axis
    /// matches on the prefix.
    pub fn get_usage_by_day_and_key(
        &self,
        from_ts: i64,
        to_ts: i64,
        bucket_seconds: i64,
        tz_offset: i64,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let prefix = if bucket_seconds == 3600 { "h" } else { "d" };
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT
                COALESCE(u.service_key_id, '') AS skid,
                COALESCE(s.name, '') AS key_name,
                COALESCE(s.key_masked, '') AS key_masked,
                CAST((u.timestamp + ?4) / ?3 AS INTEGER) AS bucket,
                SUM(u.prompt_tokens) AS prompt_tokens,
                SUM(u.completion_tokens) AS completion_tokens,
                SUM(u.cache_read_input_tokens) AS cache_read_tokens,
                COUNT(*) AS requests
             FROM usage_log u
             LEFT JOIN service_keys s ON u.service_key_id = s.id
             WHERE u.timestamp >= ?1 AND u.timestamp <= ?2
             GROUP BY COALESCE(u.service_key_id, ''), bucket
             ORDER BY bucket, skid",
        )?;

        let rows = stmt.query_map(rusqlite::params![from_ts, to_ts, bucket_seconds, tz_offset], |row| {
            let prompt: i64 = row.get(4)?;
            let completion: i64 = row.get(5)?;
            let cache_read: i64 = row.get(6)?;
            let bucket: i64 = row.get(3)?;
            let key_id: String = row.get(0)?;
            let key_name: String = row.get(1)?;
            let key_masked: String = row.get(2)?;
            // 按「服务密钥」分组的可读标签（客户端调本代理用的密钥）。
            let key_label = if key_id.is_empty() {
                "(未认证)".to_string()
            } else if key_name.is_empty() {
                if key_masked.is_empty() { key_id.clone() } else { key_masked.clone() }
            } else if key_masked.is_empty() {
                key_name.clone()
            } else {
                format!("{} ({})", key_name, key_masked)
            };
            Ok(serde_json::json!({
                "key_id": key_id,
                "key_name": key_name,
                "key_masked": key_masked,
                "key_label": key_label,
                "day": format!("{}{}", prefix, bucket),
                "prompt_tokens": prompt,
                "completion_tokens": completion,
                "cache_read_input_tokens": cache_read,
                "total_tokens": prompt + completion + cache_read,
                "requests": row.get::<_, i64>(7)?,
            }))
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// 在 [from_ts, to_ts] 内按模型聚合用量，用于前端「最爱用的模型」磁贴。
    /// 返回 (model_id, display_name, total_tokens, requests)，按请求次数降序，仅取 Top 1。
    pub fn get_usage_by_model(
        &self,
        from_ts: i64,
        to_ts: i64,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT
                u.model_id,
                COALESCE(m.display_name, u.model_id) AS model_name,
                SUM(u.prompt_tokens) AS prompt_tokens,
                SUM(u.completion_tokens) AS completion_tokens,
                SUM(u.cache_read_input_tokens) AS cache_read_tokens,
                COUNT(*) AS requests
             FROM usage_log u
             LEFT JOIN models m ON u.model_id = m.id
             WHERE u.timestamp >= ?1 AND u.timestamp <= ?2
             GROUP BY u.model_id
             ORDER BY requests DESC
             LIMIT 1",
        )?;

        let rows = stmt.query_map(rusqlite::params![from_ts, to_ts], |row| {
            let model_id: String = row.get(0)?;
            let model_name: String = row.get(1)?;
            let prompt: i64 = row.get(2)?;
            let completion: i64 = row.get(3)?;
            let cache_read: i64 = row.get(4)?;
            let requests: i64 = row.get(5)?;
            Ok(serde_json::json!({
                "model_id": model_id,
                "model_name": model_name,
                "prompt_tokens": prompt,
                "completion_tokens": completion,
                "cache_read_input_tokens": cache_read,
                "total_tokens": prompt + completion + cache_read,
                "requests": requests,
            }))
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全新数据库应能从 V1 一路迁移到最新版本；价格相关列在 V9 被移除，
    /// usage_log 保留 cache token 列用于统计。
    #[test]
    fn test_full_migration_drops_cost_columns() {
        let db = Database::open_in_memory().expect("open in-memory db");
        db.migrate().expect("migrate from scratch");

        let conn = db.conn();
        // 价格相关列应全部被移除。
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(models)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            !cols.iter().any(|c| c.starts_with("cost_")),
            "cost columns must be dropped: {:?}",
            cols
        );

        // usage_log 应有 cache 列（V7），不应再有 cost_estimate（V9 移除）。
        let ucols: Vec<String> = conn
            .prepare("PRAGMA table_info(usage_log)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(ucols.contains(&"cache_read_input_tokens".to_string()));
        assert!(!ucols.contains(&"cache_creation_input_tokens".to_string()));
        assert!(!ucols.contains(&"cost_estimate".to_string()));
    }

    /// 回归测试：save_provider/save_api_key/save_model 必须用 UPSERT。
    /// 若用 INSERT OR REPLACE，REPLACE 会触发子表的 ON DELETE CASCADE，
    /// 更新 provider 时会把 models/api_keys 全部清空。
    #[test]
    fn test_save_does_not_cascade_delete_children() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();

        let provider = Provider {
            id: "p1".to_string(),
            name: "P".to_string(),
            kind: ProviderKind::Openai,
            base_url: "https://example.com".to_string(),
            api_path: "/v1/chat/completions".to_string(),
            config: serde_json::json!({}),
            enabled: true,
            created_at: 1,
            updated_at: 1,
        };
        db.save_provider(&provider).unwrap();

        // 插入一个 model + 一个 key
        db.save_model(&Model {
            id: "m1".to_string(),
            provider_id: "p1".to_string(),
            model_id: "gpt-x".to_string(),
            display_name: "gpt-x".to_string(),
            tier: "custom".to_string(),
            context_window: 128000,
            max_output_tokens: 4096,
            capabilities: "[\"text\"]".to_string(),
            enabled: true,
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();
        db.save_api_key(&ApiKey {
            id: "k1".to_string(),
            provider_id: "p1".to_string(),
            name: "K".to_string(),
            key_hash: "h".to_string(),
            key_masked: "m".to_string(),
            key_plain: None,
            status: "green".to_string(),
            last_error: None,
            last_error_code: None,
            last_error_time: None,
            last_used_at: None,
            balance: None,
            balance_updated_at: None,
            total_requests: 0,
            total_tokens: 0,
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();

        // 更新 provider（模拟维护供应商保存）
        let mut updated = provider.clone();
        updated.name = "P2".to_string();
        updated.updated_at = 2;
        db.save_provider(&updated).unwrap();

        // 子表必须完好
        // 子表必须完好（conn 锁必须在块内释放，Mutex 不可重入）
        let (models, keys): (i64, i64) = {
            let conn = db.conn();
            let models: i64 = conn
                .query_row("SELECT COUNT(*) FROM models WHERE provider_id='p1'", [], |r| r.get(0))
                .unwrap();
            let keys: i64 = conn
                .query_row("SELECT COUNT(*) FROM api_keys WHERE provider_id='p1'", [], |r| r.get(0))
                .unwrap();
            (models, keys)
        };
        assert_eq!(models, 1, "update must not cascade-delete models");
        assert_eq!(keys, 1, "update must not cascade-delete api_keys");

        // 更新 model 也不得触发 usage_log 问题（这里至少保证不丢行）
        let mut mu = db.get_model("m1").unwrap().unwrap();
        mu.display_name = "gpt-y".to_string();
        db.save_model(&mu).unwrap();
        let m2 = db.get_model("m1").unwrap().unwrap();
        assert_eq!(m2.display_name, "gpt-y");
    }
}
