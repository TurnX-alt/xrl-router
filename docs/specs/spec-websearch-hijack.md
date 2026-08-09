# Spec: WebSearch 劫持

## 目标

拦截包含 `web_search` 工具的 LLM 请求，代理自身完成本地 Bing 搜索，将结果注入 IR 后交回正常流式转发路径——跳过 LLM tool-calling loop。

## 触发条件

1. `settings.websearch_hijack` 为 `true`（设置页「路由」Tab 开关）
2. IR 请求的 `tools` 数组包含名称以 `web_search` 开头的工具（覆盖 Anthropic server-side `web_search_20250305` 等变体）

## 流程

```
客户端请求（含 web_search 工具）
    ↓
handler.rs → IR 解析（from_messages / from_chat_completions / from_responses）
    ↓
stream.rs → 路由解析 → WebSearch 劫持判断
    ↓
命中 → enrich_ir_with_search (websearch.rs):
  1. 提取搜索关键词（最后一条 user 消息文本）
  2. 本地 Bing 搜索（绕过代理直连 cn.bing.com，独立 cookie 会话）
  3. 搜索结果注入 IR system block
  4. 清除 tools / tool_choice
    ↓
交回 proxy_stream 正常流式转发
（key failover / 双层重试 / SSE 即时响应 由 proxy_stream 天然支持）
```

## 输入契约

### IR 层 tool 检测

```rust
pub(super) fn has_websearch_tool_ir(req: &IrRequest) -> bool {
    req.tools.iter().any(|t| t.name.starts_with("web_search"))
}
```

### server-side 工具归一化（from_messages.rs）

Anthropic Messages 客户端发送的 server-side 内置工具（如 `web_search_20250305`）可能只有 `type` 没有 `name`。`from_messages.rs` 将其归一化为 `name="web_search"`，保证 websearch 劫持对 Messages 客户端生效。

### Bing 搜索

```rust
pub async fn search(query: &str) -> anyhow::Result<Vec<SearchResult>>
```

**SearchResult**:

```rust
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}
```

**关键策略**:
- **绕过代理直连**：cn.bing.com 是国内站点，走代理会导致出口 IP 在海外，Bing 降级为"热门站点推荐"模式（返回今日头条/百度热搜等非相关结果）
- **独立 cookie 会话**：每次搜索创建新 client + 新 cookie jar，避免全局 cookie 累积污染搜索结果
- **浏览器指纹**：Chrome User-Agent + Sec-CH-UA + 完整 Accept 头
- **结果提取**：`li.b_algo` 容器内提取 `h2 a`（链接）和 `.b_caption p`（摘要），排除 bing.com/microsoft.com/msn.com 内部链接，最多 8 条

## 输出契约

### system block 注入格式

搜索结果以 `IrSystemBlock` 形式追加到 IR 的 system prompt：

```
[Web Search Results for: <query>]
[1] 标题1
URL1
摘要1

[2] 标题2
URL2
摘要2

Use the above search results to answer the user's question. Cite sources using [N] notation.
```

### 清除 tools / tool_choice

注入完成后：
- `ir_request.tools = Vec::new()`
- `ir_request.tool_choice = None`

修改后的 IR 交回 `proxy_stream`，上游 LLM 收到的是带搜索结果 system block 的普通请求，不再触发 tool calling。

## 错误处理

| 场景 | 行为 |
|------|------|
| Bing 搜索返回空结果 | 注入 "No web search results found for: {query}" |
| Bing 搜索失败（网络/反爬） | 注入 "Web search unavailable: {error}. Do NOT make up information." |
| 反爬检测（captcha/Challenge） | 记录 warn 日志，结果照常解析 |
| 未命中 web_search 工具 | 正常转发，不劫持 |

## 实现位置

- `src-tauri/src/api/proxy/websearch.rs` — `enrich_ir_with_search()` + `extract_search_query()` + `format_search_text()`
- `src-tauri/src/search/bing.rs` — Bing 搜索（绕过代理直连 + 独立 cookie + b_algo 解析）
- `src-tauri/src/api/proxy/stream.rs` — 劫持入口（路由解析后、上下文预警前）
- `src-tauri/src/api/proxy/ir/from_messages.rs` — server-side 工具归一化

## 测试要求

1. **单元测试**: tool 检测、Bing HTML 解析（b_algo 容器、内部链接过滤、空 HTML）
2. **集成测试**: 完整 enrich_ir_with_search 流程
3. **网络测试**（`#[ignore]`）: 真实搜索、连续搜索验证 cookie 不累积

## 完成标准

- [x] 检测 `web_search` 工具（IR 层，覆盖三种客户端格式）
- [x] 本地 Bing 搜索（cn.bing.com，绕过代理直连）
- [x] 浏览器指纹（User-Agent、Sec-CH-UA、完整 Accept 头）
- [x] 独立 cookie 会话（每次搜索新建 client）
- [x] 搜索结果注入 IR system block
- [x] 清除 tools / tool_choice
- [x] 交回 proxy_stream 正常流式转发（key failover 天然支持）
- [x] b_algo 容器解析（链接 + 摘要同一容器内提取）
- [x] 通过所有单元测试
