# Spec: WebSearch 劫持

## 代理劫持模式

Router 注入 `web_search` 工具，tool-calling loop 本地 Bing 搜索。模型通过标准 tool-calling 自主决定是否搜索、搜索什么、搜索几次。

## 目标

拦截 LLM 请求中的搜索类工具调用，代理自身完成本地 Bing 搜索，将结果以 `tool_result` 回传给模型。模型通过标准 tool-calling 自主决定是否搜索、搜索什么、搜索几次。

## 触发条件

1. `settings.websearch_hijack` 为 `true`（设置页「路由」Tab 开关）
2. 请求通过 `proxy_stream` 进入（三种客户端格式：Messages / Chat Completions / Responses）

## 流程

```
客户端请求（可能自带搜索工具）
    ↓
handler.rs → IR 解析（from_messages / from_chat_completions / from_responses）
    ↓
stream.rs → 路由解析前：
  ensure_websearch_tool (websearch.rs):
    1. 移除所有客户端自带的搜索类工具（WebSearch / web_search / web_search_*）
    2. 改写 tool_choice 中对被移除工具的引用
    3. 注入代理自己的 web_search 工具定义（query 参数）
    ↓
路由解析 → 上游 2xx → 分支：
  ├─ websearch OFF → forward_stream_ir()（直接流式转发）
  └─ websearch ON  → execute_websearch_tool_loop():
        for round in 0..MAX_TOOL_ROUNDS:
          1. 序列化 IR → 上游格式，发起请求（缓冲完整响应，不推客户端）
          2. accumulate_ir_events() 重建完整 assistant 消息
          3. 检测 web_search tool_use：
             ├─ 无 → 缓冲内容即最终回答，流式推给客户端，结束
             ├─ 有且查询重复 → 无进展检测命中，进入收尾（步骤 7）
             └─ 有 → bing::search(search_http, query)（纯 HTTP 执行）
                      → 追加 assistant msg + tool_result 到 ir.messages
                      → tool_choice = Auto，进入下一轮
        收尾（循环耗尽或检测到无进展）：
          - 收集全部搜索结果文本，移除 messages 中的 tool_use/tool_result 痕迹
          - 移除 web_search 工具 + tool_choice = None
          - 搜索结果合并为纯文本指令追加（"以下是网络搜索获得的相关信息…"）
          - 追加一轮强制无搜索的最终回答
```

## 输入契约

### IR 层 tool 检测（is_search_tool_name）

```rust
fn is_search_tool_name(name: &str) -> bool {
    name.starts_with("web_search") || name.eq_ignore_ascii_case("WebSearch")
}
```

覆盖三种来源：
- 代理注入的 `web_search`
- Anthropic 服务端内置的 `web_search_20250305` 等变体
- Claude Code 客户端的 `WebSearch`（PascalCase）

### 工具替换（ensure_websearch_tool）

开启劫持时，**移除**客户端自带的全部搜索类工具，**注入**代理的 `web_search`：

```json
{
  "name": "web_search",
  "description": "Search the web for current information. ...",
  "input_schema": {
    "type": "object",
    "properties": { "query": { "type": "string", "description": "The search query" } },
    "required": ["query"]
  }
}
```

- 若客户端 `tool_choice` 为 `{type: "tool", name: "<搜索工具>"}`，改写目标为 `web_search`，避免上游引用不存在的工具
- 非搜索类工具（Read/Bash 等）不受影响
- 替换是强制性的：即使客户端已提供 `web_search`，也统一替换为代理定义，保证 schema 一致

### server-side 工具归一化（from_messages.rs / from_responses.rs）

Anthropic Messages 客户端发送的 server-side 内置工具（如 `web_search_20250305`）可能只有 `type` 没有 `name`。IR 解析层将其归一化为 `name="web_search"`，保证检测对 Messages / Responses 客户端生效。

### Bing 搜索（纯 Rust HTTP + cookie 预热复用）

```rust
pub async fn search(search: &SearchHttp, query: &str) -> anyhow::Result<Vec<SearchResult>>
```

**SearchHttp**（`search/bing.rs`，AppState 全局复用）：

```rust
pub struct SearchHttp {
    client: reqwest::Client,      // 完整浏览器头 + cookie_store(true)
    prewarmed: AtomicBool,        // 懒预热标记
    prewarm_lock: Mutex<()>,      // 并发首搜只预热一次
}
```

**SearchResult**:

```rust
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}
```

**为什么纯 HTTP 可行（替代早期 WebView）**：

早期 HTTP 爬虫（裸 reqwest，仅 UA）被 Bing 识别为非浏览器请求，对中文查询返回降级「热门站点推荐」结果（实测查询「张雪峰 2026」只返回「张」字的字典释义），曾改用隐藏 WebView。实测发现**关键在完整浏览器头**（尤其 `sec-ch-ua` 系列 + UA + Accept-Language），而非 TLS 指纹或 JS 执行——reqwest 带完整浏览器头 + `cookie_store(true)` + 预热即可拿到与 WebView 完全相同的正常结果，无需 WebView/Tauri 依赖。

**关键策略**:

- **完整浏览器头**：Chrome UA + Accept + Accept-Language(zh-CN) + `sec-ch-ua` 系列 + `Upgrade-Insecure-Requests`——Bing 据此识别浏览器会话
- **cookie 复用**：`cookie_store(true)` 让 cookie 会话持续，后续搜索不降级
- **懒预热**：首次搜索前 GET `cn.bing.com/` 主页建 cookie（幂等，Mutex 串行化），预热失败不阻断（降级检测兜底）
- **直连**：不继承系统代理（`http::build_http_client()` 会带代理，Bing 对代理出口 IP 降级）；reqwest 默认直连
- **降级检测 + 简化重试**：结果与查询不相关（字典释义页）时用首词重搜（`is_degraded_results` / `simplify_query`）
- **解析**：`parse_results(html)` 用 scraper 解析 `ol#b_results > li.b_algo`，复用 `decode_ck_href`（base64url 解码）+ `is_external_url`（过滤 bing.com/microsoft.com/msn.com），最多 8 条
- **双域名回退**：`www.bing.com` 优先（国际站，质量高）；空壳/失败/降级时回退 `cn.bing.com`（国内站）
- **总超时**：20s（HTTP 请求 + 读 body）

## 输出契约

### tool_result 格式

搜索结果以 `tool_result` 消息回传给模型，格式：

```
[1] 标题1
URL1
摘要: 摘要1

[2] 标题2
URL2
摘要: 摘要2
```

### 中间轮次缓冲

所有搜索轮次的响应均缓冲（`forward_stream_ir_to_buffer`），仅最终回答流式转发给客户端。客户端看到的是一次正常的流式响应。

### Messages 客户端：server-side tool 渲染（Claude Code 卡片）

对 `ClientFormat::Messages` 客户端（Claude Code），最终流以 **Anthropic 官方 server-side web_search 格式**合成（`render_websearch_messages_final`），让 Claude Code 显示「搜索中 + 搜索结果」卡片：

> **前置条件**：Claude Code 通过**第三方 base_url**（非 `api.anthropic.com`）连接时默认以 thirdParty 模式运行，会**禁用 WebSearch 工具**（客户端不发 `web_search`、不渲染搜索卡片）。必须在 Claude Code 环境设置 `_CLAUDE_CODE_ASSUME_FIRST_PARTY_BASE_URL=true` 强制 firstParty 模式，Router 的劫持与卡片渲染才会生效。

```text
message_start
  content_block_start: server_tool_use {name:"web_search", input:{}}     ← 每轮搜索
  content_block_delta: input_json_delta（查询词）
  content_block_stop
  content_block_start: web_search_tool_result {tool_use_id, content:[
    {type:"web_search_result", title, url, encrypted_content: base64(snippet)}
  ]}                                                                      ← 每轮搜索结果
  content_block_stop
  content_block_start: text（最终回答）
  content_block_delta: text_delta
  content_block_stop
message_delta + message_stop
```

- 对应官方 `server_tool_use` + `web_search_tool_result` 内容块类型（非标准 `tool_use`/`tool_result` 配对）
- `encrypted_content` 为官方必填字段，以摘要文本的 base64 填充（Claude Code 卡片展示 title/url）
- 无搜索轮次时保持正常缓冲流，零改动
- Chat/Responses 客户端无 Anthropic server-tool 概念，保持现状（缓冲 + 最终回答）

## 错误处理

| 场景 | 行为 |
|------|------|
| Bing 搜索返回空结果 | 回传 "No web search results found for: {query}" |
| Bing 搜索失败（网络/反爬） | 回传 "Web search unavailable: {error}. Do NOT make up information." |
| HTTP 搜索超时（20s） | 回传 "Web search unavailable: {error}. Do NOT make up information." |
| 模型未调用 web_search | 缓冲内容即最终回答，直接流式转发 |
| 查询重复（无进展） | 连续 2 轮查询词相似（编辑距离阈值 0.6）→ 停止循环，基于已有结果收尾 |
| 循环耗尽（10 轮仍搜索） | 清理工具痕迹 + 搜索结果合并为文本指令 + tool_choice=None，强制一轮无搜索回答 |
| 上游请求失败 | 向客户端发送 api_error 事件 |

## 实现位置

- `src-tauri/src/api/proxy/websearch.rs` — `ensure_websearch_tool()`（工具替换）+ `execute_websearch_tool_loop()`（tool-calling 循环）
- `src-tauri/src/search/bing.rs` — Bing 搜索（纯 HTTP + 完整浏览器头 + cookie 预热复用 + 双域名回退 + ck/a 解码 + b_algo 解析）
- （已移除）`search/bridge.rs` / `search/webview_search.rs` — WebView 方案已废弃，改用纯 HTTP

- `src-tauri/src/api/proxy/stream.rs` — 劫持入口（路由解析前确保工具 + 2xx 后分支到循环）
- `src-tauri/src/api/proxy/ir/from_messages.rs` / `from_responses.rs` — server-side 工具归一化
- `src-tauri/src/gateway/server.rs` — `websearch_hijack` 运行时开关 + `search_http` 字段（SearchHttp）
- `src-tauri/src/lib.rs` — Tauri 初始化（`on_window_event` main-only 守卫）

## 测试要求

1. **单元测试**: 工具替换（移除 WebSearch / web_search_20250305、保留普通工具、tool_choice 改写）、Bing HTML 解析（b_algo 容器、ck/a 解码、内部链接过滤、空 HTML）、降级检测（is_degraded_results / simplify_query）、server-tool 渲染（server_tool_use / web_search_tool_result SSE 格式）
2. **集成测试**: 完整 execute_websearch_tool_loop 流程

## 完成标准

- [x] 检测搜索类工具（IR 层，覆盖三种客户端格式 + Claude Code WebSearch）
- [x] 工具替换：移除客户端搜索工具，注入代理 web_search（含 tool_choice 改写）
- [x] 纯 HTTP 搜索（完整浏览器头 + cookie 预热复用，解决降级问题，无 WebView 依赖）
- [x] 本地 Bing 搜索（www.bing.com / cn.bing.com 双域名回退）
- [x] cookie 预热复用（首次搜索起不降级）
- [x] tool-calling 循环（最多 10 轮安全网 + 无进展检测，中间轮次缓冲，仅最终回答流式）
- [x] 循环耗尽兜底（移除工具强制最终回答）
- [x] ck/a 重定向解码 + b_algo 容器解析
- [x] 通过所有单元测试（179 passed）
