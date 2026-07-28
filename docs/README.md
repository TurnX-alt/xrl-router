# xrl-router

> **多 Provider AI LLM API 路由网关** — 桌面端 Tauri 2 应用，下游统一暴露 Anthropic Messages API，内置支持 Anthropic 和 OpenAI

📎 [产品需求文档 (PRD)](./PRD.md) · [技术规格说明书 (TS)](./TS.md)

---

## 特性

| 特性 | 说明 |
|------|------|
| **统一入口** | 客户端通过单一 Anthropic API 端点访问所有 LLM Provider |
| **协议转换** | Anthropic Messages API ↔ OpenAI Chat Completions API（含流式） |
| **模型别名** | `real_model<-alias` 语法，客户端使用别名、后端自动映射 |
| **密钥池** | 红绿灯三色健康状态 + 轮询调度，自动跳过失效密钥 |
| **AES-256-GCM 加密** | Provider API Key 使用 AES-256-GCM 加密存储，主密钥独立于数据库 |
| **Argon2 哈希** | Service Key 使用 Argon2 哈希存储，防彩虹表 |
| **WebSearch 劫持** | 可选的本地 Bing 搜索劫持，当请求包含 `web_search` tool 时自动拦截 |
| **WebSocket 实时推送** | 密钥状态变更和用量统计变更通过 WebSocket 实时推送到前端 |
| **系统托盘** | 关闭窗口时最小化到托盘继续运行，网关服务不中断 |
| **Dashboard API** | 概览和用量统计端点，支持时间粒度和时区偏移 |
| **模型同步** | 从上游 Provider 自动同步可用模型列表，避免手动维护 |
| **桌面应用** | Tauri 2 封装，MD3 风格管理面板，开箱即用 |
| **本地优先** | 所有数据存储在本地 SQLite，零数据外泄 |

## 技术栈

| 层 | 技术 |
|---|---|
| 后端 | Rust (edition 2021) + Tauri 2 + axum 0.7 + tokio |
| 数据库 | SQLite 3 (rusqlite 0.32 bundled) |
| HTTP 客户端 | reqwest 0.12 (流式 SSE) |
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

首次启动自动在系统应用数据目录（macOS: `~/Library/Application Support/im.xrl.router/`）创建数据库并执行 6 版迁移。

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
│                  │  SniffStream                     │                            │
│                  └──────────▶ token 用量统计 ◀──────┘                            │
└────────────────────────────────────────────────────────────────────────────────┘
```

### 协议转换

下游统一 **Anthropic Messages API** 格式，上游按 Provider 类型处理：

| 上游 | 处理方式 | 说明 |
|------|---------|------|
| **Anthropic** | 直接透传 | 零转换，含流式 SSE |
| **OpenAI** | Anthropic ↔ OpenAI 转换 | 支持 text、tools、thinking、tool_choice 等 |

> 仅支持流式响应（`stream: true`）。Claude Code 等主流客户端始终使用流式，非流式无实际场景。

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
│   │   │   ├── proxy.rs        # LLM 代理核心（双协议入口 + WebSearch 劫持）
│   │   │   └── proxy/
│   │   │       ├── translate.rs # Anthropic ↔ OpenAI 协议转换
│   │   │       └── sniff.rs    # 透传流嗅探（SniffStream 提取 token 用量）
│   │   ├── db/                 # SQLite 封装 + 6 版迁移
│   │   │   ├── mod.rs
│   │   │   ├── schema.rs
│   │   │   └── queries.rs
│   │   ├── types/              # 数据结构定义
│   │   ├── providers/          # Provider 适配器（Anthropic / OpenAI）
│   │   ├── keys/               # 密钥池管理（红绿灯轮询，状态纯内存）
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

密钥可用性为纯内存状态（启动时全部为绿色），DB 不持久化运行时状态。状态变更通过 WebSocket 实时推送到前端。

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
