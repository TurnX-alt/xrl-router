# xrl-router

> **多 Provider AI LLM API 路由网关** — 桌面端 Tauri 2 应用，下游统一暴露 Anthropic Messages API，内置支持 Anthropic 和 OpenAI
>
> **版本**: 26.7.31+2230

📎 [产品需求文档 (PRD)](./PRD.md) · [技术规格说明书 (TS)](./TS.md)

---

## 特性

| 特性 | 说明 |
|------|------|
| **统一入口** | 客户端通过单一 Anthropic API 端点访问所有 LLM Provider |
| **协议转换** | Anthropic Messages API ↔ OpenAI Chat Completions API（流式） |
| **模型别名** | `real_model<-alias` 语法，客户端使用别名、后端自动映射 |
| **密钥池** | 红绿灯三色健康状态 + 轮询调度 + **指针持久化**，自动跳过失效密钥且重启后从上次位置继续 |
| **缓存追踪** | 自动提取并持久化上游 API 的缓存命中信息（cache_read_input_tokens） |
| **超时保护** | 请求头和响应体的独立超时机制，防止死锁和挂起 |
| **AES-256-GCM 加密** | Provider API Key 使用 AES-256-GCM 加密存储，主密钥独立于数据库 |
| **Argon2 哈希** | Service Key 使用 Argon2 哈希存储，防彩虹表 |
| **WebSearch 劫持** | 可选的本地 Bing 搜索劫持，当请求包含 `web_search` tool 时自动拦截 |
| **WebSocket 实时推送** | 密钥状态变更和用量统计变更通过 WebSocket 实时推送到前端 |
| **插件系统** | 外部服务可通过 WebSocket 注册为"委托供应商"，将非标 API 桥接为标准 OpenAI 接口 |
| **系统托盘** | 关闭窗口时最小化到托盘继续运行，网关服务不中断 |
| **Dashboard API** | 概览和用量统计端点，支持时间粒度和时区偏移 |
| **桌面应用** | Tauri 2 封装，MD3 风格管理面板，开箱即用 |
| **本地优先** | 所有数据存储在本地 SQLite，零数据外泄 |

## 技术栈

| 层 | 技术 |
|---|---|
| 后端 | Rust (edition 2021) + Tauri 2 + axum 0.7 + tokio |
| 数据库 | SQLite 3 (rusqlite 0.32 bundled, WAL 模式) |
| HTTP 客户端 | reqwest 0.12 (流式 SSE, 超时保护) |
| 前端 | Vue 3 + Pinia + Vue Router 4 |
| UI | Material Web Components (MD3) + MDI 图标 |
| 图表 | Chart.js + vue-chartjs |
| 构建 | Vite 8 (前端) + Cargo (后端) |

## 快速开始

### 前置要求

- **Rust** >= 1.75.0
- **Tauri CLI** (`cargo install tauri-cli`)
- **Node.js** >= 20 + **pnpm**

### 安装与运行

```bash
# 前端依赖（package.json 在项目根目录）
pnpm install

# 开发模式（前端 :5173 + 后端 :19068）
pnpm dev

# 生产构建
pnpm build
```

### 配置

通过环境变量配置（均有默认值）：

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `PORT` | `19068` | HTTP 监听端口 |
| `HOST` | `127.0.0.1` | 绑定地址 |
| `DB_PATH` | _(系统数据目录下 `xrl-router.db`)_ | SQLite 文件路径（默认用系统应用数据目录；可覆盖） |
| `LOG_LEVEL` | `info` | 日志级别 |
| `API_KEY` | *(无)* | 可选的全局访问密钥 |
| `CORS_ORIGINS` | `localhost:5173/19068` + `tauri://localhost` | 允许的跨域来源（逗号分隔） |

首次启动自动在系统应用数据目录（macOS: `~/Library/Application Support/im.xrl.router/`）创建数据库并执行 10 版迁移。

## 架构

```
┌─── Tauri 桌面应用 ────────────────────────────────────────────────────────────┐
│                                                                                │
│  WebView (Vue3 SPA)                         后端 (Rust + axum)                 │
│  ┌─────────────────────┐                    ┌────────────────────────────┐     │
│  │ ProvidersView       │  REST (无认证)     │ /api/providers             │     │
│  │ KeysView            │───────────────────▶│ /api/keys                  │     │
│  │ StatsView           │                    │ /api/models                │     │
│  │ SettingsView        │                    │ /api/stats                 │     │
│  │                     │  WebSocket         │ /api/settings              │     │
│  │                     │═══════════════════▶│ /api/proxy/models          │     │
│  │                     │  (实时推送)        │ /ws                        │     │
│  └─────────────────────┘                    └────────────────────────────┘     │
│                                         (同一进程，:19068)                     │
└────────────────────────────────────────────────────────────────────────────────┘

Claude Code (Anthropic Messages API 客户端)
    │
    │  x-api-key: xrl-xxxx (Service Key — Argon2 哈希验证)
    │  POST /v1/messages (Anthropic 格式，仅流式)
    │  POST /v1/chat/completions (OpenAI 格式，仅流式)
    ▼
┌────────────────────────────────────────────────────────────────────────────────┐
│                              xrl-router  :19068                                 │
│                                                                                 │
│  /v1/messages ──┐                                                               │
│                 ├─▶ 路由解析 → Service Key 验证 (Argon2) + 别名映射              │
│  /v1/chat/... ──┘                                                               │
│                                                                                 │
│    ┌───────────────┐     ┌─────────────────────────┐                            │
│    ▼               ▼     ▼                         │                            │
│  Anthropic      OpenAI   WebSearch 劫持            │                            │
│  (内置透传)     (内置转换) (Bing 搜索拦截)          │                            │
│                  │                                  │                            │
│                  │  SniffStream + 超时保护          │                            │
│                  └──────────▶ token 用量统计 ◀──────┘                            │
└────────────────────────────────────────────────────────────────────────────────┘
```

### 协议转换

下游统一 **Anthropic Messages API** 格式，上游按 Provider 类型处理：

| 上游 | 处理方式 | 说明 |
|------|---------|------|
| **Anthropic** | 直接透传 | 零转换，含流式 SSE |
| **OpenAI** | Anthropic ↔ OpenAI 转换 | 支持 text、tools、thinking、tool_choice 等 |
| **插件 (委托供应商)** | 插件将非标 API 桥接为标准 API | 插件负责协议转换 + 业务头注入，Router 只管密钥轮换 |

> 仅支持流式响应（`stream: true`）。Claude Code 等主流客户端始终使用流式，非流式无实际场景。

### 插件系统 (Plugin System)

xrl-router 支持通过 WebSocket 注册外部服务作为**委托供应商 (Delegated Provider)**。插件的职责是**将非标 API 转化为标准 API**（如 OpenAI Chat Completions 或 Anthropic Messages），Router 负责密钥轮换、健康监控和用量统计。

**架构**：

```
xrl-router (Router)                    xrl-router-plugin-wukong (Plugin)
    │                                        │
    │  WS /ws/plugin                         │
    │◀═══════════════════════════════════════│  注册 + 心跳 + 密钥同步
    │                                        │
    │  POST /v1/chat/completions             │
    │  Authorization: Bearer <deap_key>      │
    │═══════════════════════════════════════▶│  Router 带密钥发请求
    │                                        │
    │                                        │  注入 DEAP 业务头
    │                                        │  透传密钥
    │                                        │  POST https://api-deap.dingtalk.com/...
    │  ◀═════════════════════════════════════│  返回结果
    │                                        │
```

**关键设计**：

| 职责 | Router | Plugin |
|------|--------|--------|
| 密钥池管理 | ✅ 轮询 + 红绿灯 + 持久化 | ❌ 不管密钥 |
| 协议转换 | ✅ Anthropic ↔ OpenAI | ✅ 非标 → 标准 (OpenAI/Anthropic) |
| 业务头注入 | ❌ | ✅ 注入上游 API 所需的业务头 |
| 健康监控 | ✅ 基于请求响应 | ❌ |
| 用量统计 | ✅ usage_log | ❌ |

**插件注册流程**：

1. 插件启动 → WS 连接 `ws://localhost:19068/ws/plugin`
2. 发送 `register` 消息（plugin_id, provider 配置, 模型列表, 密钥列表）
3. Router 弹出对话框 → 用户确认 → 创建委托供应商（`enabled=true`）
4. 插件定期检测密钥变化 → 通过 `keys_update` 同步到 Router
5. 插件每 30s 发送心跳 → 超时 90s 未收到则标记离线（`enabled=false`）
6. 用户「忽略」插件 → 彻底删除插件 + 关联 provider + 模型 → WS 断开 → 插件重连后重新注册

**委托供应商 vs 普通供应商**：

| 维度 | 普通供应商 | 委托供应商（插件） |
|------|-----------|------------------|
| API 格式 | 用户选择 (OpenAI/Anthropic) | 插件提供（可转为 OpenAI 或 Anthropic） |
| Base URL | 用户填写 | 插件通过 WS 推送 |
| API Key | 用户手动填入 | 插件自动同步 |
| 密钥轮换 | Router KeyPool | Router KeyPool（完全一致） |
| 连接状态 | N/A | 必须 WS 在线才能消费 |

**插件 API 端点**：

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/ws/plugin` | WebSocket 插件注册端点 |
| `GET` | `/api/plugins` | 列出已注册插件 |
| `GET` | `/api/plugins/:id` | 获取插件详情（供 ProviderNewView 预填） |
| `POST` | `/api/plugins/:id/confirm` | 确认激活插件供应商 |
| `DELETE` | `/api/plugins/:id` | 删除插件（彻底清理 provider + 模型） |

## API 端点

### 公开 API（需 Service Key）

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/health` | 健康检查（含 DB、Provider、Key 状态） |
| `GET` | `/v1/models` | 模型列表（含别名） |
| `POST` | `/v1/messages` | Anthropic Messages API 代理（仅流式） |
| `POST` | `/v1/chat/completions` | OpenAI Chat Completions API 代理（仅流式） |

### 管理 API（无认证，Tauri WebView 直连）

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET/POST/PUT/DELETE` | `/api/providers[/:id]` | Provider CRUD |
| `GET/POST/PUT/DELETE` | `/api/keys[/:id]` | API Key CRUD（含 AES-256-GCM 加解密） |
| `GET/POST/PUT/DELETE` | `/api/models[/:id]` | Model CRUD |
| `GET/POST/PUT/DELETE` | `/api/service-keys[/:id]` | Service Key CRUD（Argon2 哈希） |
| `GET` | `/api/stats` | 用量统计（支持 from/to/granularity/tz_offset） |
| `GET` | `/api/proxy/models` | 代理获取上游 Provider 模型列表 |
| `GET/PUT` | `/api/settings` | 应用设置（websearch_hijack 开关） |
| `GET` | `/ws` | WebSocket 实时推送（密钥状态 + 用量变更） |

## 项目结构

```
xrl-router/
├── src-tauri/                  # ═══ 后端 (Rust + Tauri) ═══
│   ├── Cargo.toml
│   ├── tauri.conf.json         # identifier: im.xrl.router
│   ├── build.rs
│   ├── src/
│   │   ├── main.rs             # 入口
│   │   ├── lib.rs              # 库入口（Master Key → DB → 迁移 → AppState → HTTP）
│   │   ├── config.rs           # 环境变量配置
│   │   ├── error.rs            # 错误类型
│   │   ├── crypto/             # AES-256-GCM 加解密 + 主密钥管理
│   │   │   └── mod.rs
│   │   ├── gateway/            # HTTP 网关服务
│   │   │   ├── mod.rs
│   │   │   └── server.rs       # AppState、axum 服务器、CORS
│   │   ├── api/                # HTTP API 处理器
│   │   │   ├── mod.rs          # 路由构建 + CRUD + WebSocket + Settings
│   │   │   ├── proxy.rs        # LLM 代理核心（双协议入口 + WebSearch 劫持 + 超时保护）
│   │   │   └── proxy/
│   │   │       ├── translate.rs # Anthropic ↔ OpenAI 协议转换
│   │   │       └── sniff.rs    # 透传流嗅探（SniffStream 提取 token 用量 + 缓存追踪）
│   │   ├── db/                 # SQLite 封装 + 10 版迁移
│   │   │   ├── mod.rs
│   │   │   ├── schema.rs
│   │   │   └── queries.rs
│   │   ├── types/              # 数据结构定义
│   │   ├── providers/          # Provider 适配器（Anthropic / OpenAI）
│   │   ├── keys/               # 密钥池管理（红绿灯轮询 + 指针持久化，状态纯内存）
│   │   ├── models/             # 模型注册
│   │   ├── middleware/         # 令牌桶限流
│   │   ├── search/             # Bing 搜索（WebSearch 劫持用）
│   │   │   ├── mod.rs
│   │   │   └── bing.rs
│   │   └── protocol/           # 占位模块
│   └── data/                   # 运行时数据（仅开发模式使用；生产环境存于系统应用数据目录）
│       ├── xrl-router.db       # SQLite (WAL)
│       └── master.key          # AES-256-GCM 主密钥
├── src/                        # ═══ 前端 (Vue3 + MD3) ═══
│   ├── main.ts                 # Vue 入口 + MWC 按需导入 + 主题初始化
│   ├── App.vue                 # 根组件
│   ├── router.ts               # 6 条路由
│   ├── api.ts                  # REST 客户端
│   ├── ws.ts                   # WebSocket 客户端（自动重连）
│   ├── theme.ts                # 明/暗主题切换
│   ├── views/                  # 页面
│   ├── components/             # AppShell + ConnectionStatus
│   └── stores/                 # Pinia stores
├── docs/                       # 文档
├── package.json                # 前端依赖（项目根）
├── vite.config.ts
└── index.html
```

## 密钥管理

### 密钥池

每个 Provider 可配置多个 API Key，系统自动轮询调度：

| 状态 | 颜色 | 触发条件 | 行为 |
|------|------|---------|------|
| 正常 | 绿 | 请求成功 | 正常使用 |
| 低配额 | 黄 | 402 / 429 / 5xx | 暂时跳过 |
| 失效 | 红 | 401 | 永久跳过，需人工处理 |

密钥可用性为纯内存状态（启动时全部为绿色），但**轮询指针持久化到 settings 表**——重启后从上次成功使用的 key 位置继续，而非每次都从 0 开始重试。DB 不持久化运行时健康状态。状态变更通过 WebSocket 实时推送到前端。

### Service Key

客户端通过 `x-api-key: xrl-xxxx` 或 `Authorization: Bearer xrl-xxxx` 访问 `/v1/*` 端点。Service Key 使用 **Argon2** 哈希存储（随机盐值），创建时仅返回一次明文。

### Provider API Key

Provider API Key 使用 **AES-256-GCM** 加密后存储到数据库。主密钥（256 位）在首次启动时随机生成并持久化到系统应用数据目录下的 `master.key`（权限 0600）。数据库单独泄露不暴露密钥；丢失 master.key 则已加密的 Provider Key 无法解密。

## 模型层级

| Tier | 说明 | 示例 |
|------|------|------|
| Fable | 顶级智能 | Claude Opus 5, GPT-5 |
| Opus | 高性能 | Claude Opus 4.x, GPT-4o |
| Sonnet | 均衡 | Claude Sonnet, GPT-4o-mini |
| Haiku | 轻量快速 | Claude Haiku, GPT-4o-nano |

