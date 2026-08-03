# Architecture Decision Records

设计背后的历史原因。防止架构漂移。

---

## ADR-001: 选择 Tauri 2 作为桌面框架

**日期**: 2026-07-28  
**状态**: 已接受

### 背景

需要一个轻量级桌面应用来运行本地 LLM API 网关。考虑过 Electron、原生应用、纯 CLI。

### 决策

采用 Tauri 2：Rust 后端 + WebView 前端。

### 原因

1. **轻量**: 安装包 < 10MB，内存占用 < 100MB（Electron 通常 > 200MB）
2. **性能**: Rust 后端处理高并发代理请求，比 Node.js/Python 更高效
3. **安全**: Rust 内存安全 + 系统级加密库原生支持
4. **跨平台**: 一套代码编译 macOS/Windows/Linux
5. **现代化**: 前端用 Vue 3 + Material Web，用户体验好

### 代价

- 需要 Rust 工具链（学习曲线）
- WebView 渲染在某些系统可能不一致
- 调试比 Electron 复杂

---

## ADR-002: 仅支持流式代理（stream=true）

**日期**: 2026-07-28  
**状态**: 已接受

### 背景

LLM API 支持流式（SSE）和非流式两种模式。完整支持需要两套代码路径。

### 决策

强制 `stream=true`。即使客户端发送 `stream=false`，也会被静默覆写为 `true` 后继续处理（不返回 400）。

### 原因

1. **简化实现**: 只需一套流式处理逻辑，代码量减少 40%
2. **用户体验**: Claude Code、ChatGPT 等主流客户端都默认流式，响应更快
3. **资源效率**: 流式可以边生成边传输，不需要缓存完整响应
4. **协议转换**: 流式转换可以逐 chunk 处理，内存占用低

### 代价

- 无法支持需要完整响应的场景（如某些 batch 处理）
- 客户端必须支持 SSE

### 替代方案

如果未来需要非流式，可以新增 `/v1/messages/sync` 端点，不影响现有流式逻辑。

---

## ADR-003: 密钥健康状态纯内存存储

**日期**: 2026-07-28  
**状态**: 已接受

### 背景

密钥健康状态（green/yellow/red）需要持久化还是仅内存？

### 决策

健康状态仅存内存，启动时全部初始化为 green。只有轮询指针（`current_index`）持久化到 `settings` 表。

### 原因

1. **启动恢复**: 重启后从上次轮询位置继续，跳过已失效的 key
2. **减少 IO**: 每次健康状态变更都写 DB 会产生大量小事务
3. **语义合理**: 健康状态是运行时概念，重启后重新探测更合理
4. **指针持久化**: 避免每次都从 key[0] 开始轮询，提升效率

### 代价

- 重启后无法看到历史健康状态
- 需要用户手动观察哪些 key 失效

### 实现

```rust
// keys/pool/persistence.rs
pub fn persist_index(&self, provider_id: &str, index: usize) {
    let key = format!("keypool_index_{}", provider_id);
    settings::set(&key, &index.to_string())?;
}

pub fn load_persisted_index(&self, provider_id: &str) -> usize {
    let key = format!("keypool_index_{}", provider_id);
    settings::get(&key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}
```

---

## ADR-004: usage_log 自包含快照设计

**日期**: 2026-08-01  
**状态**: 已接受

### 背景

`usage_log` 表需要关联 `providers`、`models`、`api_keys`、`service_keys`。是否用外键？

### 决策

`usage_log` 不使用外键，而是存储快照字段：`provider_name`、`model_display_name`、`key_name`、`service_key_name` 等。

### 原因

1. **历史完整性**: 删除 Provider/Model/Key 后，历史统计仍然可见
2. **查询性能**: 统计查询不需要 JOIN 多张表
3. **数据独立**: 即使上游表结构变化，历史记录不受影响
4. **迁移安全**: V12 迁移可以安全地删除外键约束

### 代价

- 数据冗余（每条日志多存 ~100 字节）
- 无法通过外键约束保证一致性

### 实现

```sql
-- V12: usage_log 自包含
ALTER TABLE usage_log ADD COLUMN provider_name TEXT DEFAULT '';
ALTER TABLE usage_log ADD COLUMN model_display_name TEXT DEFAULT '';
ALTER TABLE usage_log ADD COLUMN key_name TEXT DEFAULT '';
ALTER TABLE usage_log ADD COLUMN service_key_name TEXT DEFAULT '';

-- 回填历史数据
UPDATE usage_log SET provider_name = (
  SELECT name FROM providers WHERE id = usage_log.provider_id
);
-- ... 其他字段类似

-- 删除外键约束（重建表）
```

---

## ADR-005: 管理 API 无认证，绑定 127.0.0.1

**日期**: 2026-07-28  
**状态**: 已接受

### 背景

管理 API（`/api/providers`、`/api/keys` 等）是否需要认证？

### 决策

管理 API 无认证，仅通过绑定 `127.0.0.1` + CORS 白名单保护。

### 原因

1. **本地场景**: 桌面应用运行在用户本机，威胁模型是"本机其他进程"
2. **简化使用**: 无需登录/Token，打开应用即用
3. **Tauri 隔离**: WebView 与后端同进程，不需要跨域认证
4. **CORS 保护**: 浏览器端恶意网页无法跨域调用

### 代价

- 本机恶意进程可以访问管理 API
- 不适合多用户共享场景

### 威胁模型

- **已防护**: 远程攻击（绑定 127.0.0.1）、浏览器跨域攻击（CORS）
- **未防护**: 本机恶意进程读取密钥、修改配置
- **接受风险**: 桌面应用场景，用户应保证本机安全

### 未来改进

如需多用户或远程管理，可新增 `/api/auth/login` + JWT Token，管理 API 加 `Authorization` 头校验。

---

## ADR-006: 删除价格字段（V9 迁移）

**日期**: 2026-07-29  
**状态**: 已接受

### 背景

V7 添加了 `cost_per_mtok_input`、`cost_per_mtok_output` 等价格字段，但前端从未展示。

### 决策

V9 迁移删除所有价格相关字段：
- `models.cost_per_mtok_input`
- `models.cost_per_mtok_output`
- `models.cost_per_mtok_cache_read`
- `models.cost_per_mtok_cache_write`
- `usage_log.cost_estimate`

### 原因

1. **未使用**: 前端从未读取或展示价格数据
2. **复杂度**: 价格计算需要考虑缓存、不同供应商定价策略
3. **维护成本**: 需要定期更新价格表
4. **简化 schema**: 减少不必要的字段

### 代价

- 未来如需成本统计，需要重新添加字段 + 迁移
- 用户无法在本地查看 API 调用成本

### 替代方案

如需成本统计，可以：
1. 导出 `usage_log` 到 CSV，用 Excel 计算
2. 新增独立的价格表（不嵌入 models 表）

---

## ADR-007: 缓存概念纠正（V10 迁移）

**日期**: 2026-07-30  
**状态**: 已接受

### 背景

V7 引入了 `cache_creation_input_tokens` 和 `cache_read_input_tokens` 两个字段。但"写缓存"本质上是首次处理的输入，不应单独计数。

### 决策

V10 迁移：
- 删除 `cache_creation_input_tokens`
- 将历史数据合并到 `prompt_tokens`
- 只保留 `cache_read_input_tokens`（真正的缓存命中）

### 原因

1. **概念清晰**: "写缓存"只是首次处理输入，本质是输入 token
2. **简化统计**: 总输入 = `prompt_tokens`（含写缓存）+ `cache_read_input_tokens`（缓存命中）
3. **对齐上游**: OpenAI 的 `prompt_tokens` 已包含所有输入（含写缓存）

### 代价

- 历史数据需要迁移（`prompt_tokens = prompt_tokens + cache_creation`）
- 无法区分"首次处理的输入"和"缓存命中的输入"

### 实现

```sql
-- V10: 缓存概念纠正
UPDATE usage_log
SET prompt_tokens = prompt_tokens + cache_creation_input_tokens
WHERE cache_creation_input_tokens > 0;

ALTER TABLE usage_log DROP COLUMN cache_creation_input_tokens;
```

---

## ADR-008: Provider API Key 使用 AES-256-GCM 加密

**日期**: 2026-07-28  
**状态**: 已接受

### 背景

Provider API Key 需要持久化存储。如何保护？

### 决策

使用 AES-256-GCM 对称加密，主密钥存储在 `master.key` 文件（权限 0600）。

### 原因

1. **可逆**: 需要解密后发送给上游 API（不能用哈希）
2. **强加密**: AES-256-GCM 是 NIST 标准，抗已知攻击
3. **认证加密**: GCM 模式提供完整性校验，防篡改
4. **简单部署**: 单个主密钥文件，易于备份

### 实现

```rust
// crypto/mod.rs
pub fn encrypt(plaintext: &str, master_key: &[u8]) -> Result<String> {
    let cipher = Aes256Gcm::new_from_slice(master_key)?;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher.encrypt(&nonce, plaintext.as_ref())?;
    
    // nonce || ciphertext
    let mut result = nonce.to_vec();
    result.extend_from_slice(&ciphertext);
    Ok(BASE64.encode(&result))
}
```

### 代价

- 主密钥文件丢失则所有 Provider Key 不可恢复
- 需要保护 `master.key` 文件权限

### 替代方案

- **硬件密钥**: YubiKey/TPM，但增加部署复杂度
- **密钥管理服务**: AWS KMS/Vault，但需要网络连接

---

## ADR-009: Service Key 使用 Argon2 哈希

**日期**: 2026-07-28  
**状态**: 已接受

### 背景

Service Key（客户端访问令牌）如何存储？

### 决策

使用 Argon2id 哈希算法，随机 salt，存储在 `service_keys.key_hash`。

### 原因

1. **不可逆**: Service Key 不需要解密，只需验证
2. **抗暴力破解**: Argon2 是内存硬算法，GPU/ASIC 攻击成本高
3. **OWASP 推荐**: Password Storage Cheat Sheet 首选 Argon2id
4. **随机 salt**: 防止彩虹表攻击

### 实现

```rust
// crypto/mod.rs
pub fn hash_service_key(raw_key: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2.hash_password(raw_key.as_bytes(), &salt)?;
    Ok(hash.to_string())
}

pub fn verify_service_key(raw_key: &str, hash: &str) -> Result<bool> {
    let parsed = PasswordHash::new(hash)?;
    Ok(Argon2::default()
        .verify_password(raw_key.as_bytes(), &parsed)
        .is_ok())
}
```

### 代价

- 验证需要逐条遍历所有 Service Key（无法索引查找）
- 哈希计算比 SHA-256 慢（故意设计，抗暴力破解）

---

## ADR-010: 数据库 UPSERT 使用 ON CONFLICT DO UPDATE

**日期**: 2026-07-28  
**状态**: 已接受

### 背景

`save_provider`、`save_model` 等方法需要"存在则更新，不存在则插入"。

### 决策

使用 `INSERT ... ON CONFLICT DO UPDATE`，不使用 `INSERT OR REPLACE`。

### 原因

1. **避免级联删除**: `INSERT OR REPLACE` 会触发 `ON DELETE CASCADE`，误删子表数据
2. **语义明确**: `ON CONFLICT DO UPDATE` 明确表示"冲突时更新"
3. **可控更新**: 可以指定哪些字段更新，哪些保留

### 代价

- SQL 语法更复杂
- 需要为每个表定义冲突处理逻辑

### 实现

```rust
// db/providers.rs
pub fn save_provider(provider: &Provider) -> Result<()> {
    db().execute(
        "INSERT INTO providers (id, name, ...) VALUES (?1, ?2, ...)
         ON CONFLICT(id) DO UPDATE SET
           name = excluded.name,
           ...",
        params![provider.id, provider.name, ...],
    )?;
    Ok(())
}
```

### 回归测试

```rust
#[test]
fn test_upsert_no_cascade_delete() {
    // 1. 插入 provider
    // 2. 插入子表数据（api_keys, models）
    // 3. 更新 provider
    // 4. 验证子表数据未被删除
}
```

---

## ADR-011: 协议转换显式处理不兼容特性

**日期**: 2026-07-28  
**状态**: 已接受

### 背景

Anthropic 和 OpenAI API 有语义差异（如 `thinking`、`tool_choice`）。如何处理？

### 决策

显式转换不兼容特性，记录 warn 日志，不静默丢弃。

### 原因

1. **可调试**: warn 日志让用户知道哪些特性被转换/丢弃
2. **可预测**: 明确的行为比隐式丢弃更容易理解
3. **可改进**: 日志帮助识别需要支持的常见特性

### 实现

```rust
// api/proxy/translate/to_openai.rs
pub fn anthropic_req_to_anthropic(req: &AnthropicRequest) -> OpenAIRequest {
    if let Some(thinking) = &req.thinking {
        warn!("thinking 特性转换为 reasoning_content（非官方字段）");
        // 转换为 OpenAI 的 reasoning_content
    }
    
    match req.tool_choice {
        ToolChoice::Any => {
            // Anthropic "any" → OpenAI "required"
            "required"
        }
        // ...
    }
}
```

### 已知不兼容

- `thinking` → `reasoning_content`（非官方）
- `tool_choice.any` → `tool_choice.required`
- `stop_reason.end_turn` → `finish_reason.stop`

---

## ADR-012: WebSearch 劫持使用本地 Bing 搜索

**日期**: 2026-07-28  
**状态**: 已接受

### 背景

上游 LLM API 的 `web_search` 工具需要付费，且结果质量不可控。

### 决策

提供可选的 WebSearch 劫持：拦截包含 `web_search` 工具的请求，用本地 Bing 搜索替代。

### 原因

1. **成本节约**: 避免上游 web_search 费用
2. **可控性**: 本地搜索可以定制（如使用 cn.bing.com）
3. **隐私**: 搜索请求不经过上游 API

### 实现

```rust
// api/proxy/websearch.rs
pub fn run_websearch_loop(/* ... */) -> Response {
    let mut messages = initial_messages.clone();
    
    for _ in 0..5 {
        // 1. 发送给上游（stream=false）
        let resp = send_to_upstream(&messages)?;
        
        // 2. 检查是否需要搜索
        if let Some(tool_calls) = extract_tool_calls(&resp) {
            for call in tool_calls {
                if call.name == "web_search" {
                    // 3. 本地 Bing 搜索
                    let results = bing::search(&call.query)?;
                    
                    // 4. 构造 tool_result
                    messages.push(Message::ToolResult {
                        tool_call_id: call.id,
                        content: format_results(&results),
                    });
                }
            }
        } else {
            // 5. 无工具调用，返回最终响应
            return resp;
        }
    }
}
```

### 代价

- 需要维护 Bing 搜索 scraper（反爬策略变化）
- 最多 5 轮 tool-calling loop，延迟增加
- 搜索结果质量可能不如上游 API

### 开关

`settings.websearch_hijack` 控制是否启用，默认关闭。

---

## ADR-013: 插件系统采用 WebSocket 注册

**日期**: 2026-08-01  
**状态**: 已接受

### 背景

需要支持"委托供应商"（如钉钉 DEAP），插件负责协议转换 + 业务头注入。

### 决策

插件通过 WebSocket 连接 `/ws/plugin`，发送注册/心跳/密钥同步消息。

### 原因

1. **实时通信**: WebSocket 支持双向消息，适合心跳 + 密钥同步
2. **生命周期管理**: 连接断开自动检测（90s 无心跳标记离线）
3. **解耦**: 插件是独立进程，崩溃不影响 Router

### 协议

```typescript
// 插件 → Router
{ type: "register", plugin_id: "wukong", provider: { kind: "deap", base_url: "http://...", api_path: "/v1/..." }, models: [...], keys: [...] }
{ type: "heartbeat" }
{ type: "keys_update", provider_id: "...", keys: ["sk-xxx", "sk-yyy"] }

// Router → 插件
{ type: "registered", plugin_id: "wukong" }
```

插件状态使用纯字符串（非枚举），取值为 `"pending"`（等待确认）、`"active"`（已确认）、`"offline"`（心跳超时）。

### 职责分工

| 职责 | Router | Plugin |
|------|--------|--------|
| 密钥轮换 | ✅ | ❌ |
| 健康监控 | ✅ | ❌ |
| 用量统计 | ✅ | ❌ |
| 协议转换 | ❌ | ✅ |
| 业务头注入 | ❌ | ✅ |

### 代价

- 需要维护 WebSocket 连接状态
- 插件离线时委托供应商不可用

---

## ADR-014: 模型撞名按 sort_order + created_at 排序

**日期**: 2026-08-01  
**状态**: 已接受

### 背景

多个 Provider 提供相同 `display_name` 的模型（如 `claude-opus-4-8`）。如何选择？

### 决策

路由解析时按 `sort_order ASC, created_at ASC` 排序，取第一条。

### 原因

1. **可预测**: 用户可以通过拖拽排序控制优先级
2. **公平**: 相同优先级时，先创建的优先
3. **简单**: 不需要复杂的负载均衡算法

### 实现

```rust
// api/proxy/route.rs
pub fn resolve_route(state: &AppState, display_name: &str) -> Option<ResolvedRoute> {
    let model = db().query_row(
        "SELECT m.*, p.* FROM models m
         JOIN providers p ON m.provider_id = p.id
         WHERE m.display_name = ?1 AND m.enabled = 1 AND p.enabled = 1
         ORDER BY p.sort_order ASC, p.created_at ASC
         LIMIT 1",
        params![display_name],
        |row| Ok(ModelProvider { /* ... */ }),
    ).ok()?;
    
    // ...
}
```

### 代价

- 无法实现加权负载均衡（如 70% 流量到 Provider A，30% 到 B）
- 主 Provider 故障时，需要等所有 key 都 Red 才会切换到备用

### 未来改进

如需负载均衡，可以新增 `routes` 表（已预留），支持 `weight` 字段。

---

## ADR-015: Token 配额用滚动窗口 + 按需聚合（V14）

**日期**: 2026-08-02  
**状态**: 已接受

### 背景

需求：每个 Service Key 可配置 5 小时 / 7 天内的 token 上限，触顶返回 429。需要决定窗口口径与用量来源。

### 决策

1. **滚动窗口而非固定时段**：窗口按 Unix 时间对齐（`now % window_secs`），不是自然日/自然小时。与上游计费（Anthropic 5h、OpenAI 类似滚动周期）语义一致，实现只依赖 `usage_log.timestamp` 单列。
2. **上限持久化、用量按需聚合**：`service_keys` 只存 `quota_5h/quota_7d`（0 = 不设限）；已用量每次从 `usage_log` 条件聚合（`SUM(prompt + completion + cache_read)`）。不维护额外计数器，避免写路径多一次同步、且重启后天然一致。
3. **429 采用 quota_error 类型**：模拟 Anthropic 错误体风格，携带 `retry-after` 头（剩余秒数）；`message` 内含可读的重置时间（`Resets in 2h31m.`）。

### 原因

1. **正确性**：固定时段在窗口边界会瞬时放行大量请求（月初/日初全额重置），滚动窗口平滑且与上游配额对齐
2. **简单**：单条 SQL 即得两窗口用量，无新增状态
3. **一致**：`/v1/user/balance` 与表格「限额」列共用同一聚合函数，展示与判定永不分叉

### 代价

- 每个代理请求多一次 SQLite 条件聚合查询（有 `idx_usage_service_key` + `idx_usage_timestamp` 索引，单用户本地规模无感）
- 聚合统计的是「已写库」的用量，正在流式传输的请求有 ≤ 5 分钟延迟才计入（可接受：流式请求的 token 是渐进消耗的）

### 未来改进

如未来需要更细粒度（按模型、按分钟），可在同一聚合函数上加条件扩展。

---

## ADR-016: 统一 HTTP 客户端工厂 + 系统代理自动继承

**日期**: 2026-08-03  
**状态**: 已接受

### 背景

项目有 6 处出站 HTTP 请求（代理转发、WebSearch Bing 搜索、Provider 适配器、上游模型拉取），各自用 `reqwest::Client::new()` 或 `Client::builder()` 独立构建。国内网络下钉钉 DEAP 等上游需走 Clash 等代理才能连通，但散落构建无法统一注入代理。

### 决策

新增 `http.rs` 模块作为唯一 HTTP 客户端工厂：

1. `system_proxy()`: 解析系统代理，OnceLock 缓存（代理在运行期间几乎不变）
   - 优先读环境变量（`HTTPS_PROXY` > `HTTP_PROXY` > `ALL_PROXY`，大小写兼容）
   - Windows 回退到注册表 `HKCU\...\Internet Settings`（ProxyEnable + ProxyServer）
   - 跳过 PAC（AutoConfigURL）
2. `build_http_client() -> ClientBuilder`: 返回带系统代理的 builder，调用方可继续链式覆盖 timeout / cookie_store
3. `http_client() -> Client`: 便捷方法，默认构建
4. NO_PROXY 默认豁免 `localhost`、`127.0.0.1`、`[::1]`（插件系统上游在本机），并附加环境变量 `NO_PROXY` 的额外项

所有出站 HTTP 请求必须使用工厂方法，不允许直接 `reqwest::Client::new()`。

### 原因

1. **统一代理**：6 处调用点只需改一行就全部接入代理，未来新增出站请求也不会遗漏
2. **零配置**：Windows 用户配 Clash 系统代理后，xrl-router 自动继承，无需在应用内手动设置
3. **性能**：OnceLock 缓存代理解析结果，只读一次注册表（`reg query` 调用 ~50ms）
4. **可测试**：工厂方法返回 builder 而非 final client，调用方可覆盖 timeout 等参数

### 代价

- 代理在应用运行期间不可变（Clash 端口固定，实际无影响）
- Windows 注册表解析依赖 `reg query` 子进程（仅首次调用，失败时静默回退到无代理）
- 非 Windows 系统只支持环境变量（无注册表回退，但跨平台标准做法）

### 迁移

6 处调用点已全部替换：
- `api/proxy/handler.rs` (2 处): `reqwest::Client::builder()` → `crate::http::build_http_client()`
- `api/proxy/websearch.rs` (1 处): `reqwest::Client::builder()` → `crate::http::build_http_client()`
- `api/handlers/models.rs` (1 处): `reqwest::Client::new()` → `crate::http::http_client()`
- `providers/anthropic.rs` (1 处): `Client::new()` → `crate::http::http_client()`
- `providers/openai.rs` (1 处): `Client::new()` → `crate::http::http_client()`
- `search/bing.rs` (1 处): `reqwest::Client::builder()` → `crate::http::build_http_client()`

