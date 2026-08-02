# Spec: 用量统计

## 目标

记录每次 LLM 请求的用量信息，提供聚合统计查询，支持前端图表展示。

## 数据结构

### usage_log 表

```sql
CREATE TABLE usage_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp INTEGER NOT NULL,
    provider_id TEXT NOT NULL,
    provider_name TEXT NOT NULL,
    model_id TEXT NOT NULL,
    model_display_name TEXT NOT NULL,
    key_id TEXT,
    key_name TEXT,
    key_masked TEXT,
    service_key_id TEXT,
    service_key_name TEXT,
    service_key_masked TEXT,
    request_type TEXT NOT NULL,
    prompt_tokens INTEGER NOT NULL,
    completion_tokens INTEGER NOT NULL,
    cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
    latency_ms INTEGER NOT NULL,
    success INTEGER NOT NULL,
    error_message TEXT
);
```

**设计要点**:
- **自包含快照**: 写入时保存名称，不依赖外键
- **删除安全**: 删除 Provider/Model/Key 不影响历史统计
- **缓存追踪**: `cache_read_input_tokens` 记录缓存命中

## 输入契约

### 记录用量

```rust
pub fn insert_usage_log(
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
    cache_read_input_tokens: i64,
    latency_ms: i64,
    success: bool,
    error_message: Option<&str>,
) -> anyhow::Result<()>
```

### 查询统计

```rust
pub fn get_usage_by_day_and_key(
    from_ts: i64,
    to_ts: i64,
    bucket_seconds: i64,  // 3600 (hour) | 86400 (day)
    tz_offset: i32,
) -> anyhow::Result<Vec<serde_json::Value>>
```

## 输出契约

统计查询返回 `Vec<serde_json::Value>`（使用 `json!({})` 宏构建，无强类型结构体）。

### 统计维度

**按 Service Key + 时间桶分组**:
```sql
SELECT
    service_key_id,
    service_key_name,
    SUM(prompt_tokens) as prompt_tokens,
    SUM(completion_tokens) as completion_tokens,
    SUM(cache_read_input_tokens) as cache_read_input_tokens,
    SUM(prompt_tokens + completion_tokens) as total_tokens,
    COUNT(*) as requests,
    CAST((timestamp + ?) / ? AS INTEGER) as bucket
FROM usage_log
WHERE timestamp >= ? AND timestamp < ?
GROUP BY service_key_id, bucket
ORDER BY bucket ASC
```

**时间桶格式**: `"h{bucket}"` 或 `"d{bucket}"`，其中 bucket = `floor((timestamp + tz_offset) / bucket_seconds)`

**Top Model**:
```sql
SELECT
    model_id,
    model_display_name as model_name,
    COUNT(*) as requests
FROM usage_log
WHERE timestamp >= ? AND timestamp < ?
GROUP BY model_id
ORDER BY requests DESC
LIMIT 1
```

## 关键约束

1. **写入性能**: 每次请求都写一条记录，需要高效插入
2. **查询性能**: 大量历史数据时，聚合查询需要索引
3. **时区处理**: 使用 `tz_offset` 参数调整时区
4. **粒度支持**: `hour` 和 `day` 两种粒度
5. **无外键**: 删除 Provider/Model/Key 不影响统计

## 索引

```sql
CREATE INDEX idx_usage_timestamp ON usage_log(timestamp);
CREATE INDEX idx_usage_provider ON usage_log(provider_id);
CREATE INDEX idx_usage_service_key ON usage_log(service_key_id);
```

## 错误处理

| 场景 | 行为 |
|------|------|
| 插入失败 | 记录 warn 日志，不影响请求响应 |
| 查询失败 | 返回空结果 |
| 无数据 | 返回空数组，`top_model` 为 null |

## 实现位置

- `src-tauri/src/db/usage.rs` - 插入和查询逻辑
- `src-tauri/src/api/handlers/stats.rs` - HTTP API 处理
- `src-tauri/src/api/proxy/handler.rs` - 异步记录用量

## 测试要求

1. **单元测试**: 插入逻辑、查询逻辑
2. **集成测试**: 完整统计流程（写入 + 查询）
3. **性能测试**: 大量数据插入和查询的性能
4. **边界测试**: 空数据、时区边界、粒度切换

## 完成标准

- [x] 每次请求记录 `usage_log`
- [x] 按 Service Key 分组统计
- [x] 按小时/天粒度聚合
- [x] Top Model 统计
- [x] 时区偏移支持
- [x] 索引优化查询性能
- [x] 异步写入（不影响请求延迟）
- [x] 通过所有单元测试
