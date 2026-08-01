//! usage_log 写入与统计聚合（V12 起统计自包含，不再 JOIN 父表）。

impl super::Database {
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
                u.provider_name,
                COUNT(*) as requests,
                COALESCE(SUM(u.prompt_tokens + u.completion_tokens + u.cache_read_input_tokens), 0) as tokens
             FROM usage_log u
             GROUP BY u.provider_id, u.provider_name"
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

    /// Append one row to usage_log. Called once per proxied request (success or failure).
    /// 统计信息自包含：写入时快照 provider_name / model_display_name / key_name / key_masked /
    /// service_key_name / service_key_masked，确保删除父表行后统计不受影响。
    pub fn insert_usage_log(
        &self,
        timestamp: i64,
        provider_id: &str,
        provider_name: &str,
        model_id: &str,
        model_display_name: &str,
        key_id: Option<&str>,
        key_name: &str,
        key_masked: &str,
        service_key_id: Option<&str>,
        service_key_name: &str,
        service_key_masked: &str,
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
            "INSERT INTO usage_log (timestamp, provider_id, provider_name, model_id, model_display_name, key_id, key_name, key_masked, service_key_id, service_key_name, service_key_masked, request_type, prompt_tokens, completion_tokens, latency_ms, success, error_message, cache_read_input_tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            rusqlite::params![
                timestamp,
                provider_id,
                provider_name,
                model_id,
                model_display_name,
                key_id,
                key_name,
                key_masked,
                service_key_id,
                service_key_name,
                service_key_masked,
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
                u.service_key_name AS key_name,
                u.service_key_masked AS key_masked,
                CAST((u.timestamp + ?4) / ?3 AS INTEGER) AS bucket,
                SUM(u.prompt_tokens) AS prompt_tokens,
                SUM(u.completion_tokens) AS completion_tokens,
                SUM(u.cache_read_input_tokens) AS cache_read_tokens,
                COUNT(*) AS requests
             FROM usage_log u
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
                u.model_display_name,
                SUM(u.prompt_tokens) AS prompt_tokens,
                SUM(u.completion_tokens) AS completion_tokens,
                SUM(u.cache_read_input_tokens) AS cache_read_tokens,
                COUNT(*) AS requests
             FROM usage_log u
             WHERE u.timestamp >= ?1 AND u.timestamp <= ?2
             GROUP BY u.model_id
             ORDER BY requests DESC
             LIMIT 1",
        )?;

        let rows = stmt.query_map(rusqlite::params![from_ts, to_ts], |row| {
            let model_id: String = row.get(0)?;
            let model_name: String = row.get(1)?;  // model_display_name
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
