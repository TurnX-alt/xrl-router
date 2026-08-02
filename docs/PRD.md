# xrl-router — 产品需求文档

> 版本: CalVer (tauri 26.8.1) · 更新日期: 2026-08-01
>
> 📎 [架构文档](./ARCHITECTURE.md) · [决策记录](./DECISIONS.md) · [规格契约](./specs/)

---

## 1. 背景与动机

### 1.1 问题陈述

LLM 生态的协议碎片化：Anthropic、OpenAI 等 Provider 的 API 格式互不兼容。开发者要接入多家 Provider 需维护多套客户端代码，密钥散落各处，缺乏统一的健康监控和轮换机制喵～

具体痛点：

- **Claude Code 等客户端只认 Anthropic API**，想用 OpenAI 模型（GPT-4o、DeepSeek）没有代理层
- **密钥管理分散**，每个 Provider 独立管理，哪个 Key 还能用全靠人肉记忆
- **现有方案偏服务端**——OpenRouter 是云端 SaaS、LiteLLM 是 Python 服务、one-api 需要 Docker 部署，本地开发体验差

### 1.2 为什么不用现有方案

| 方案 | 不足 |
|------|------|
| **OpenRouter** | 云端 SaaS，依赖网络，数据经第三方 |
| **LiteLLM** | Python 实现，部署重；服务端思维，无桌面体验 |
| **one-api** | Go 实现，功能丰富但无桌面客户端 |
| **Portkey** | 商业化，部分功能收费，不支持纯本地 |

### 1.3 核心洞察

> 开发者需要一个**本地优先、轻量、美观**的 LLM 网关桌面应用——像本地代理一样运行，让 Claude Code 等客户端零配置接入 Anthropic 和 OpenAI 喵～

---

## 2. 产品定位与目标

### 2.1 一句话定位

**xrl-router** — 运行在桌面上的 LLM API 统一网关，让任何客户端通过一套 API 访问所有大模型。

### 2.2 产品目标

| 目标 | 衡量方式 |
|------|---------|
| 统一接入 | 通过单一端点访问所有 Provider |
| 零摩擦启动 | 从打开应用到发出第一个请求 < 3 分钟 |
| 可靠运行 | 单个 Key 失效不影响服务 |
| 透明可观测 | 所有请求的 token 用量、延迟、成功率可追踪 |

---

## 3. 用户画像与场景

### 3.1 主要用户：AI 开发者

| 属性 | 描述 |
|------|------|
| 技术水平 | 熟悉 API 调用，了解 REST/HTTP |
| 使用频率 | 每天使用，作为日常开发基础设施 |
| 核心诉求 | 一个端点接入所有模型，密钥自动管理，本地运行 |
| 痛点 | 切换 Provider 要改代码、密钥散落各处 |

### 3.2 次要用户：Claude Code / AI IDE 用户

| 属性 | 描述 |
|------|------|
| 技术水平 | 会用终端，不一定了解 API 细节 |
| 使用频率 | 日常编码时持续使用 |
| 核心诉求 | 让 Claude Code 能用非 Anthropic 的模型 |
| 痛点 | Claude Code 只支持 Anthropic API |

### 3.3 核心使用场景

**场景 A：首次配置**
1. 打开 xrl-router 桌面应用
2. 在「供应商」页面添加 Provider（选类型、填 URL、填 Key）
3. 创建 Service Key
4. 在 Claude Code 配置 base URL 和 Service Key
5. 开始使用

**场景 B：密钥故障恢复**
1. Provider 返回 401 → Key 自动标红
2. 系统自动切换到下一个可用 Key
3. 后续请求透明继续
4. 用户稍后在面板查看红灯原因

**场景 C：插件自动发现**
1. 启动 xrl-router-plugin-wukong
2. 插件 WS 连接到 Router → 发送注册信息
3. Router 弹出确认对话框
4. 用户确认 → 委托供应商自动激活
5. 密钥自动同步，可直接在 Claude Code 中使用 DEAP 模型

---

## 4. 功能需求

### 4.1 P0 — 核心功能（已实现 ✅）

| ID | 功能 | 实现位置 |
|----|------|---------|
| F-01 | **Provider CRUD** | `api/handlers/providers.rs` |
| F-02 | **API Key CRUD + 密钥池轮询** | `api/handlers/keys.rs` + `keys/pool/` |
| F-03 | **Service Key 认证（Argon2）** | `api/proxy/auth.rs` |
| F-04 | **LLM 流式代理** | `api/proxy/handler.rs` |
| F-05 | **Anthropic ↔ OpenAI 协议转换** | `api/proxy/translate/` |
| F-06 | **模型别名** | `api/proxy/route.rs` |
| F-07 | **密钥健康监控（红绿灯）** | `keys/pool/health.rs` |
| F-08 | **桌面应用（Tauri 2）** | `src-tauri/` |
| F-09 | **请求超时保护（60s 头 + 120s 流）** | `api/proxy/handler.rs` |
| F-10 | **密钥轮询指针持久化** | `keys/pool/persistence.rs` |
| F-11 | **AES-256-GCM 加密 Provider Key** | `crypto/mod.rs` |
| F-12 | **令牌桶限流（60 req/min）** | `middleware/rate_limit.rs` |

### 4.2 P1 — 重要功能（已实现 ✅）

| ID | 功能 | 实现位置 |
|----|------|---------|
| F-13 | **用量统计（数据磁贴 + 折线图）** | `api/handlers/stats.rs` + `StatsView.vue` |
| F-14 | **模型注册 + 层级分类** | `api/handlers/models.rs` + `models/mod.rs` |
| F-15 | **Provider 启用/禁用** | `api/handlers/providers.rs` |
| F-16 | **健康检查端点** | `api/handlers/health.rs` |
| F-17 | **缓存追踪（cache_read_input_tokens）** | `api/proxy/sniff.rs` |
| F-18 | **WebSocket 实时推送** | `api/handlers/websocket.rs` + `ws.ts` |
| F-19 | **Service Key 白名单（allowed_models）** | `api/handlers/service_keys.rs` |
| F-20 | **usage_log 自包含快照** | `db/schema.rs` V12 |

### 4.3 P2 — 锦上添花（已实现 ✅）

| ID | 功能 | 实现位置 |
|----|------|---------|
| F-21 | **WebSearch 劫持（Bing 搜索）** | `api/proxy/websearch.rs` + `search/bing.rs` |
| F-22 | **模型同步（从上游拉取）** | `api/handlers/models.rs` |
| F-23 | **系统托盘** | `lib.rs` |
| F-24 | **插件系统（委托供应商）** | `plugin/` |
| F-25 | **插件密钥自动同步** | `plugin/keys.rs` |
| F-26 | **插件心跳监控（30s/90s）** | `plugin/health.rs` |
| F-27 | **供应商拖拽排序** | `ProvidersView.vue` + `api/handlers/providers.rs` (V13) |
| F-28 | **暗色模式** | `theme.ts` + `global.css` |
| F-29 | **上游模型代理获取（避 CORS）** | `api/handlers/models.rs` |
| F-30 | **应用设置（websearch 开关）** | `api/handlers/` + `SettingsView.vue` |
| F-31 | **Token 配额（5h/7d 滚动窗口）** | `api/proxy/quota.rs` + `KeysView.vue` (V14) |
| F-32 | **余额端点（/v1/user/balance）** | `api/proxy/quota.rs` |

### 4.4 未实现（计划中）

| ID | 功能 | 计划版本 |
|----|------|---------|
| F-33 | 管理 API 认证层（Basic Auth / Session Token） | v0.3 |
| F-34 | 路由规则引擎（`routes` 表，优先级 + 权重） | v0.3 |
| F-35 | 指数退避重试 | v0.3 |
| F-36 | 更多 Provider 内置（DeepSeek、Gemini） | v0.3 |
| F-37 | 自动更新机制 | v1.0 |

### 4.5 已知断裂（待修复）

| 问题 | 说明 |
|------|------|
| Dashboard API 前后端断裂 | 前端 `api.ts` 定义了 `dashboardApi`（调用 `/api/dashboard/overview` 和 `/api/dashboard/usage`），`stores/dashboard.ts` 也在使用，但后端 `router.rs` 未注册这两条路由 |

### 4.6 基础设施模块（未在功能需求中单列）

| 模块 | 说明 | 位置 |
|------|------|------|
| Provider Adapter 抽象层 | `Adapter` async trait（chat/chat_stream/health_check），Anthropic 和 OpenAI 各自实现 | `providers/adapter.rs`、`anthropic.rs`、`openai.rs` |
| 统一错误类型 | `thiserror`-based `AppError` 枚举，跨模块统一数据库/JSON/HTTP/认证/加密/限流错误 | `error.rs` |

---

## 5. 协议转换规格

下游统一暴露 Anthropic Messages API 和 OpenAI Chat Completions API 双入口。上游按 Provider 类型处理：

| 上游类型 | 处理方式 |
|---------|---------|
| Anthropic | 直接透传 + SniffStream 嗅探 token |
| OpenAI / Compatible | 双向协议转换（请求 + 流式响应） |
| 插件（委托供应商） | 插件负责非标→标准，Router 只管密钥轮换 |

### 5.1 必须支持的转换特性

- ✅ 文本消息 (text content blocks)
- ✅ 系统提示 (system prompt → messages[0].role="system")
- ✅ 工具调用 (tool_use ↔ tool_calls)
- ✅ 工具结果 (tool_result ↔ role: "tool")
- ✅ 思考过程 (thinking ↔ reasoning_content，非官方字段)
- ✅ 工具选择 (tool_choice: auto/any/none ↔ auto/required/none)
- ✅ 流式响应 (SSE streaming，逐 chunk 转换)
- ✅ 缓存 token (cache_read_input_tokens)

### 5.2 仅支持流式

非流式分支已移除。所有代理请求强制 `stream: true`。

---

## 6. 密钥池规格

| 状态 | 颜色 | 触发条件 | 行为 |
|------|------|---------|------|
| 正常 | 🟢 绿 | 初始 / 请求成功 | 正常使用 |
| 低配额 | 🟡 黄 | 402 / 429 | 跳过，冷却 300 秒后自动恢复 |
| 失效 | 🔴 红 | 401 / 403 | 永久跳过 |

- 健康状态纯内存（启动全 green），DB `status` 列保留但不读写
- 轮询指针持久化到 `settings` 表（`keypool_index_{provider_id}`）
- 重启后从上次位置继续，而非从 key[0] 开始

---

## 6.1 Token 配额规格（5h / 7d 滚动窗口）

每个 Service Key 可配置两个滚动窗口的 token 上限：**5 小时** 和 **7 天**，默认都是 0（不设限）。

| 项 | 规则 |
|----|------|
| 窗口定义 | 滚动窗口，按 Unix 时间对齐（`now % window_secs`），非自然日 |
| 用量口径 | `prompt + completion + cache_read_input_tokens`，从 usage_log 按需聚合 |
| 超限判定 | `used >= limit`（limit > 0）即 429；任一窗口触顶即拒绝 |
| 恢复方式 | 窗口滚动重置后自动恢复，无需人工干预 |
| 错误响应 | `429` + `retry-after` 头 + `quota_error` 错误体（message 含重置时间） |
| 查询端点 | `GET /v1/user/balance`（认证同代理端点）返回设限窗口的用量，格式为 CCSwitch ZenMux 兼容：`{"success": true, "data": {"quota_5_hour": {"usage_percentage": 0.43, "resets_at": "..."}, "quota_7_day": {...}}}`；未设限窗口省略字段 |

用途：把单个密钥的消费上限锁住，防止一个密钥把上游额度耗尽；配额在应用内管理页面配置。

---

## 7. 插件系统规格

外部服务通过 WebSocket 注册为「委托供应商」。职责分工：

| 职责 | Router | Plugin |
|------|--------|--------|
| 密钥池管理 | ✅ 轮询 + 红绿灯 + 持久化 | ❌ |
| 协议转换 | ✅ Anthropic ↔ OpenAI | ✅ 非标 → 标准 |
| 业务头注入 | ❌ | ✅ |
| 健康监控 | ✅ 基于请求响应 | ❌ |
| 用量统计 | ✅ usage_log | ❌ |

**生命周期**：注册 → 用户确认 → 激活 → 密钥同步（`keys_update`）→ 心跳（30s/90s）→ 忽略（彻底删除 + WS 断开 → 插件重连后重新注册）

---

## 8. 非功能需求

### 8.1 性能

| 指标 | 目标 |
|------|------|
| 启动到就绪 | ≤ 3 秒 |
| 代理额外延迟（透传） | ≤ 5ms |
| 代理额外延迟（转换） | ≤ 20ms |
| 内存占用（空闲） | ≤ 100MB |
| 并发 | ≤ 50 请求 |
| 请求头超时 | 60 秒 |
| 流 chunk 间隔超时 | 120 秒 |

### 8.2 安全

| 要求 | 实现 |
|------|------|
| Service Key 存储 | Argon2 哈希（随机盐 + PHC 格式） |
| Provider Key 存储 | AES-256-GCM 加密（主密钥 `master.key`，权限 0600） |
| 管理 API | 绑定 `127.0.0.1`，仅本机可访问 |
| CORS | origin 白名单（localhost + 127.0.0.1 的 5173/19068 双端口 + tauri://localhost + https://tauri.localhost，共 6 个） |
| 频率限制 | 令牌桶 60 req/min，按 Service Key |

### 8.3 兼容性

| 维度 | 要求 |
|------|------|
| Anthropic API | 兼容 `2023-06-01` |
| OpenAI API | 兼容 Chat Completions v1 |
| 操作系统 | macOS (primary)、Windows、Linux |

---

## 9. 成功指标

### 9.1 北极星指标

> **成功发出首个代理请求的时间（Time to First Request）** ≤ 3 分钟

### 9.2 功能指标

| 指标 | 目标 |
|------|------|
| Provider 接入成功率 | ≥ 99%（有效 Key） |
| 协议转换正确率 | 100%（无数据丢失） |
| Key 故障自动切换 | 100%（有备用 Key 时） |

---

## 10. 风险

| 风险 | 概率 | 缓解 |
|------|------|------|
| 上游 API 格式变更 | 中 | 版本锁定 + 兼容性测试 |
| SQLite 高并发瓶颈 | 低 | WAL 模式 + 异步批量写入 |
| 协议转换丢失特性 | 中 | 不兼容特性显式报错 |
| 上游挂起网关卡死 | 低 | 独立超时保护（60s + 120s） |
| 密钥泄露 | 低 | AES-256-GCM + Argon2 |
