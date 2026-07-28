# xrl-router — 技术规格说明书

> **版本**: 26.7.31+0505 · **文档类型**: Technical Specification

---

## 目录

1. [项目概述](#1-项目概述)
2. [技术架构](#2-技术架构)
3. [目录结构](#3-目录结构)
4. [后端模块详解](#4-后端模块详解)
5. [前端模块详解](#5-前端模块详解)
6. [数据模型](#6-数据模型)
7. [API 规范](#7-api-规范)
8. [核心业务流程](#8-核心业务流程)
9. [认证与安全](#9-认证与安全)
10. [配置与环境](#10-配置与环境)
11. [构建与部署](#11-构建与部署)
12. [已知限制与 TODO](#12-已知限制与-todo)
13. [设计审查：不合理之处与风险识别](#13-设计审查不合理之处与风险识别)
14. [设计审查总结](#14-设计审查总结)
15. [协议转换已知不兼容特性](#15-协议转换已知不兼容特性)

---

## 1. 项目概述

### 1.1 定位

xrl-router 是一个**多 Provider AI LLM API 路由网关**，以 Tauri 2 桌面应用的形式运行，统一管理多个 LLM Provider。下游统一暴露 **Anthropic Messages API** 接口（服务 Claude Code 等客户端），上游根据 Provider 类型进行透传或协议转换喵～

**内置 Provider 类型**（仅 2 种，编译期静态链接）：
- **Anthropic**：直接透传，零转换
- **OpenAI**：Anthropic → OpenAI 协议转换

### 1.2 核心价值

| 价值 | 说明 |
|------|------|
| **统一入口** | 客户端通过单一端点访问所有 LLM Provider |
| **协议转换** | Anthropic Messages API → OpenAI Chat Completions API（当上游为 OpenAI 时） |
| **模型别名** | 支持 `real_model<-alias` 语法，客户端使用别名、后端自动映射到真实模型 |
| **密钥池管理** | 红绿灯三色健康状态 + 轮询调度，自动跳过失效/低配额密钥 |
| **桌面应用** | Tauri 2 封装，前端通过 WebView 直接访问后端 API，无需额外登录 |

### 1.3 用户角色

| 角色 | 说明 |
|------|------|
| **管理员** | 通过 Tauri 桌面应用直接使用管理面板，管理 Provider、密钥、模型配置 |
| **API 消费者** | 使用 Service Key 调用 `/v1/messages`（Anthropic 格式）发送 LLM 请求 |

---

## 2. 技术架构

### 2.1 技术栈总览

| 层 | 技术 | 版本/说明 |
|---|---|---|
| **后端语言** | Rust | edition 2021 |
| **桌面框架** | Tauri 2 | WebView 加载前端，后端在同一进程内启动 HTTP 服务 |
| **HTTP 框架** | axum | 0.7 + tokio 异步运行时 |
| **HTTP 客户端** | reqwest | 0.12，异步，支持流式 SSE |
| **数据库** | SQLite 3 | 通过 rusqlite 0.32 (bundled) |
| **序列化** | serde + serde_json | |
| **日志** | tracing + tracing-subscriber | |
| **前端框架** | Vue 3 | Composition API + `<script setup>` |
| **状态管理** | Pinia | 4 个 Store（providers/keys/models/dashboard） |
| **路由** | Vue Router 4 | History 模式，6 条路由 |
| **UI 组件库** | Material Web Components (MWC) | @material/web v2.2.0，MD3 设计语言 |
| **图标** | MDI | @mdi/font v7 |
| **图表** | Chart.js + vue-chartjs | 统计页面使用 |
| **构建** | Vite 8 (前端) + Cargo (后端) | |
| **加密** | aes-gcm 0.10 + argon2 0.5 | Provider Key 加密 + Service Key 哈希 |

### 2.2 系统架构图

```
┌─── Tauri 桌面应用 ──────────────────────────────────────────────────┐
│                                                                      │
│  WebView (前端 Vue3 SPA)                 后端 (Rust + axum)          │
│  ┌─────────────────────┐                 ┌────────────────────────┐  │
│  │ ProvidersView       │   无认证        │ /api/providers         │  │
│  │ KeysView            │───────────────▶ │ /api/keys              │  │
│  │ StatsView           │                 │ /api/models            │  │
│  │ SettingsView        │                 │ /api/stats             │  │
│  └─────────────────────┘                 └────────────────────────┘  │
│                                         (同一进程，:19068)          │
└──────────────────────────────────────────────────────────────────────┘

Claude Code (Anthropic Messages API 客户端)
    │
    │  x-api-key: xrl-xxxx (Service Key)
    │  POST /v1/messages (Anthropic 格式)
    ▼
┌──────────────────────────────────────────────────────────────────────┐
│                          xrl-router  :19068                          │
│                                                                      │
│  ┌──────────────┐  ┌─────────────┐                                   │
│  │ /v1/messages │  │ /v1/models  │                                   │
│  │ (Anthropic)  │  │ (模型列表)   │                                   │
│  └──────┬───────┘  └─────────────┘                                   │
│         │                                                            │
│  ┌──────┴──────┐                                                     │
│  │  路由解析     │  ← Service Key 验证 + 别名映射                     │
│  └──────┬──────┘                                                     │
│         │                                                            │
│    ┌────┴────┐                                                       │
│    ▼         ▼                                                       │
│  Anthropic  OpenAI                                                   │
│  (内置透传)  (内置转换)                                               │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘
```

### 2.3 协议转换流程

下游统一使用 Anthropic Messages API 格式。上游根据 Provider 类型决定处理方式：

```
Claude Code (Anthropic 格式)
┌──────────────────────┐
│ model: "my-alias"    │
│ system: "你是..."     │
│ messages: [...]      │
│ tools: [anthropic]   │
│ thinking: enabled    │
│ stream: true         │
└──────────┬───────────┘
           │
     ┌─────┼─────────┐
     ▼     ▼
  上游 Anthropic  上游 OpenAI
  (内置)         (内置)
  ┌──────────┐   ┌──────────────────────┐
  │ 直接透传  │   │ Anthropic → OpenAI   │
  │ (无需转换) │   │ 协议转换             │
  └──────────┘   └──────────────────────┘
                       │
                       ▼
                 ┌──────────────────────┐
                 │ model: "real-model"  │
                 │ messages[0]: system  │
                 │ messages[1..n]: ...  │
                 │ tools: [function]    │
                 │ extra_body.thinking  │
                 └──────────────────────┘
                       │
                       ▼ (响应)
                 ┌──────────────────────┐
                 │ choices[0].message   │
                 │ finish_reason: stop  │
                 │ usage: prompt/compl  │
                 │ reasoning_content    │
                 └──────────┬───────────┘
                            │
                            ▼ OpenAI → Anthropic 转换
                 ┌──────────────────────┐
                 │ content: [{text}]    │
                 │ stop_reason: end_turn│
                 │ usage: input/output  │
                 └──────────────────────┘

流式请求: 逐 chunk 解析 SSE → 转换 → 重新封装为 Anthropic SSE 转发
```

---

## 3. 目录结构

```
xrl-router/
├── src-tauri/                   # ═══ 后端 (Rust + Tauri) ═══
│   ├── Cargo.toml               # version 0.2.0
│   ├── tauri.conf.json          # identifier: im.xrl.router
│   ├── build.rs                 # Tauri 构建脚本
│   ├── src/
│   │   ├── main.rs              # 入口：调用 lib.rs 的 run()
│   │   ├── lib.rs               # 库入口：日志 → Config → Master Key → DB → 迁移 → AppState → spawn gateway
│   │   ├── config.rs            # 环境变量配置（PORT/HOST/DB_PATH/LOG_LEVEL/API_KEY/CORS_ORIGINS）
│   │   ├── error.rs             # 错误类型定义
│   │   ├── crypto/              # 加密模块
│   │   │   └── mod.rs           # AES-256-GCM 加解密 + Master Key 管理
│   │   ├── gateway/             # HTTP 网关服务
│   │   │   ├── mod.rs
│   │   │   └── server.rs        # AppState（含 MasterKey/broadcast/websearch_hijack）、CORS
│   │   ├── api/                 # HTTP API 处理器
│   │   │   ├── mod.rs           # 路由构建 + CRUD + WebSocket + Settings + Argon2 Service Key
│   │   │   ├── proxy.rs         # LLM 代理核心（双协议入口 + WebSearch 劫持）
│   │   │   └── proxy/
│   │   │       ├── translate.rs # Anthropic ↔ OpenAI 协议转换
│   │   │       └── sniff.rs     # 透传流嗅探（SniffStream 提取 token 用量）
│   │   ├── db/                  # 数据库层
│   │   │   ├── mod.rs           # SQLite (rusqlite) 封装 + CRUD（WAL 模式）
│   │   │   ├── schema.rs        # 6 版迁移 SQL
│   │   │   └── queries.rs       # 预定义 SQL 查询
│   │   ├── types/               # 数据结构定义
│   │   │   ├── mod.rs           # 统一导出
│   │   │   ├── provider.rs      # Provider, ProviderKind
│   │   │   ├── model.rs         # Model, ModelTier
│   │   │   ├── key.rs           # ApiKey, KeyStatus (green/yellow/red)
│   │   │   ├── chat.rs          # Chat 请求/响应类型
│   │   │   ├── balance.rs       # BalanceInfo
│   │   │   └── route.rs         # Route（预留）
│   │   ├── providers/           # Provider 注册与适配器
│   │   │   ├── mod.rs           # ProviderRegistry
│   │   │   ├── adapter.rs       # Adapter trait
│   │   │   ├── openai.rs        # OpenAI 适配器（内置）
│   │   │   └── anthropic.rs     # Anthropic 适配器（内置）
│   │   ├── keys/                # 密钥管理
│   │   │   ├── mod.rs
│   │   │   └── pool.rs          # KeyPool — 红绿灯轮询（状态纯内存，V5 后不持久化）
│   │   ├── models/              # 模型注册
│   │   │   └── mod.rs           # ModelRegistry
│   │   ├── middleware/          # 中间件
│   │   │   ├── mod.rs
│   │   │   └── rate_limit.rs    # 令牌桶限流（60 req/min，按 Service Key）
│   │   ├── search/              # 搜索引擎（WebSearch 劫持用）
│   │   │   ├── mod.rs
│   │   │   └── bing.rs          # cn.bing.com 搜索（cookie 复用 + 浏览器指纹）
│   │   └── protocol/            # 协议转换（占位，实际在 api/proxy/translate.rs）
│   │       └── mod.rs
│
├── src/                         # ═══ 前端 (Vue3 + MD3) ═══
│   ├── main.ts                  # Vue 入口 + MWC 按需导入 + 主题初始化
│   ├── App.vue                  # 根组件（AppShell + router-view）
│   ├── router.ts                # 6 条路由
│   ├── api.ts                   # REST API 客户端（含 settingsApi/dashboardApi）
│   ├── ws.ts                    # WebSocket 客户端（自动重连 + 事件分发）
│   ├── theme.ts                 # 明/暗主题切换（localStorage 持久化）
│   ├── tauri.d.ts               # Tauri 类型声明
│   ├── views/
│   │   ├── ProvidersView.vue    # 供应商列表 + 密钥管理（内联展开）
│   │   ├── ProviderNewView.vue  # 供应商创建/编辑表单
│   │   ├── KeysView.vue         # Service Key 管理
│   │   ├── StatsView.vue        # 用量统计图表
│   │   └── SettingsView.vue     # 应用设置（含 websearch_hijack 开关）
│   ├── components/
│   │   ├── AppShell.vue         # MD3 导航抽屉 + 主内容区
│   │   └── ConnectionStatus.vue # 离线状态横幅 + 重试按钮
│   └── stores/
│       ├── providers.ts         # Provider 列表状态
│       ├── keys.ts              # API Key 列表状态（按 provider_id 分组）
│       ├── models.ts            # 模型列表状态（按 provider_id 分组）
│       └── dashboard.ts         # 仪表盘数据状态
│
├── docs/
│   ├── README.md                # 项目总览
│   ├── PRD.md                   # 产品需求文档
│   └── TS.md                    # 技术规格说明书（本文档）
│
├── package.json                 # 前端依赖（项目根目录）
├── vite.config.ts               # Vite 8 配置 + 代理
├── index.html                   # MD3 Token + 暗色模式
├── tsconfig.json
└── pnpm-workspace.yaml
```

---

## 4. 后端模块详解

### 4.1 入口 — `lib.rs`

**启动流程**:

```
run()
  ├── tracing_subscriber::fmt().json().init()  → 初始化 JSON 格式日志
  ├── Config::from_env()                       → 从环境变量加载配置（不含路径）
  ├── tauri::Builder::default()
  │     .plugin(tauri_plugin_shell::init())
  │     .setup(|app| {                          → 所有初始化移入此处（可拿 app handle）
  │       app.path().app_data_dir()             → 解析系统应用数据目录
  │       create_dir_all(data_dir)              → 确保数据目录存在
  │       crypto::load_or_create_master_key()   → 加载/生成 AES-256-GCM 主密钥
  │       Database::new(db_path)                → 打开 SQLite 数据库（WAL 模式）
  │       database.migrate()                    → 执行所有待应用的迁移（V1-V6）
  │       AppState::new(config, database, master_key) → 创建共享状态
  │         ├── ProviderRegistry::load_from_db()
  │         ├── ModelRegistry::load_from_db()
  │         ├── KeyPool::load_all_keys_from_db() → AES-256-GCM 解密加载密钥
  │         ├── RateLimiter::new()
  │         └── websearch_hijack ← settings 表
  │       系统托盘（显示窗口 / 退出）
  │       tauri::async_runtime::spawn(async {
  │         gateway::server::start_gateway(state).await  → 启动 axum HTTP 服务器
  │       })
  │     })
  │     .on_window_event(CloseRequested → hide to tray)
  │     .run(tauri::generate_context!())        → 启动 Tauri 窗口
  └── 完成
```

> **设计变更**：所有依赖文件路径的初始化（master key、database、migration）从 `run()` 顶层移入 `setup()` 回调内。原因是需要 `app.path().app_data_dir()` 解析系统数据目录的绝对路径——开发时用 `src-tauri/data/` 没问题，但安装后的 app bundle 工作目录不可写，相对路径 `data/` 会创建失败导致闪退。失败时返回 `Err` 让 Tauri 显示错误，替代了原来的 `process::exit(1)` 静默退出喵～

### 4.2 配置 — `config.rs`

从环境变量加载：

| 环境变量 | 默认值 | 说明 |
|---------|--------|------|
| `PORT` | `19068` | 监听端口 |
| `HOST` | `127.0.0.1` | 绑定地址（默认仅本机可访问） |
| `DB_PATH` | _(系统数据目录下 `xrl-router.db`)_ | SQLite 文件路径（默认使用系统应用数据目录；仅在显式设置时覆盖） |
| `LOG_LEVEL` | `info` | 日志级别 (debug/info/warn/err) |
| `API_KEY` | _(空)_ | 公开 API 密钥（可选，代码中已定义但未使用） |
| `CORS_ORIGINS` | `localhost:5173,127.0.0.1:5173,localhost:19068,127.0.0.1:19068,tauri://localhost,https://tauri.localhost` | CORS Origin 白名单（逗号分隔；空=允许所有） |

> **已移除**: `JWT_SECRET` 和 `ENCRYPTION_KEY` 环境变量已不存在。主密钥自动从系统应用数据目录下的 `master.key` 文件加载/生成喵～

### 4.3 HTTP 服务器 — `gateway/server.rs` + `api/mod.rs`

**架构**: axum + tokio 异步运行时

```
start_gateway(state)
  ├── build_router(state)              → 构建路由表
  ├── 添加 CORS 中间件 (管理路由 origin 白名单，公开代理宽松)
  ├── 添加 tracing 中间件 + 令牌桶限流 (60 req/min)
  ├── TcpListener::bind(addr)
  └── axum::serve(listener, app).await → 开始监听
```

### 4.4 路由表

| 路径 | 方法 | 处理器 | 认证 | 说明 |
|------|------|--------|------|------|
| `/health`, `/` | GET | `health_check` | 否 | 详细健康检查（含 DB/providers/models/keys 统计） |
| `/ws` | GET (WS upgrade) | `ws_handler` | 否 | WebSocket 实时推送（key_stats/usage_stats_changed） |
| `/v1/models` | GET | `proxy::proxy_list_models` | Service Key + Rate Limit | 模型列表（受 `allowed_models` 过滤） |
| `/v1/messages` | POST | `proxy::proxy_anthropic_messages` | Service Key + Rate Limit | Anthropic 格式 LLM 代理（仅流式） |
| `/v1/chat/completions` | POST | `proxy::proxy_openai_chat` | Service Key + Rate Limit | OpenAI 格式 LLM 代理（仅流式） |
| `/api/providers` | GET/POST | `list_providers` / `create_provider` | 否 | Provider 列表/创建 |
| `/api/providers/{id}` | GET/PUT/DELETE | `get_provider` / `update_provider` / `delete_provider` | 否 | Provider 详情/更新/删除 |
| `/api/keys` | GET/POST | `list_keys` / `create_key` | 否 | API Key 列表/创建（返回解密明文 + 内存实时状态） |
| `/api/keys/{id}` | GET/PUT/DELETE | `get_key` / `update_key` / `delete_key` | 否 | API Key 详情/更新/删除 |
| `/api/models` | GET/POST | `list_models` / `create_model` | 否 | 模型列表/创建（支持 `?provider_id=` 过滤） |
| `/api/models/{id}` | GET/PUT/DELETE | `get_model` / `update_model` / `delete_model` | 否 | 模型详情/更新/删除 |
| `/api/proxy/models` | GET | `proxy_fetch_models` | 否 | 代理获取上游模型列表（避免 CORS，注入 API key） |
| `/api/service-keys` | GET/POST | `list_service_keys` / `create_service_key` | 否 | Service Key 列表/创建 |
| `/api/service-keys/{id}` | PUT/DELETE | `update_service_key` / `delete_service_key` | 否 | Service Key 更新/删除 |
| `/api/stats` | GET | `get_stats` | 否 | 用量统计（支持 `from/to/granularity/tz_offset`） |
| `/api/settings` | GET/PUT | `get_settings` / `update_settings` | 否 | 应用设置（websearch_hijack 开关） |

### 4.5 LLM 代理核心 — `api/proxy.rs`

这是项目的核心模块，处理所有 LLM 请求转发。

**请求处理流程**:

```
proxy_anthropic_messages / proxy_openai_chat
  │
  ├── 1. 提取 API Key
  │      ├── x-api-key header，或
  │      └── Authorization: Bearer header
  │
  ├── 2. 提取 model 字段
  │
  ├── 3. verify_service_key()
  │      └── 查 service_keys 表，Argon2 逐条校验
  │      └── 返回 (service_key_id, allowed_models)
  │      └── 失败 → 401 Unauthorized
  │
  ├── 4. allowed_models 白名单检查
  │      └── 非空时客户端只能用白名单内的别名
  │      └── 不匹配 → 403 Forbidden
  │
  ├── 5. resolve_route()
  │      └── 查 models 表（display_name 别名匹配）JOIN providers (WHERE enabled=1)
  │      └── 获取 upstream_url, real_model_id, provider_kind, provider_id
  │      └── 失败 → 400 Bad Request
  │
  ├── 6. WebSearch 劫持判断
  │      └── 如果 websearch_hijack 开关开 + 请求含 web_search tool
  │      └── → run_websearch_loop()：用本地 Bing 搜索替代上游（最多 5 轮）
  │
  ├── 7. 判断上游类型 + 协议转换
  │      ├── 同协议（Anthropic→Anthropic / OpenAI→OpenAI）: 直接透传 + SniffStream
  │      └── 异协议: translate 双向转换
  │
  ├── 8. Key 轮询重试循环
  │      └── pick_key_for() 从 KeyPool 取 key
  │      └── 401/403 → mark_key_invalid(Red) → rotate → replay
  │      └── 402/429 → mark_key_low_quota(Yellow) → rotate → replay
  │
  └── 9. 流式转发
        ├── 同协议: SniffStream 包装字节流（无修改转发 + 后台嗅探 token）
        ├── 异协议: 逐 SSE frame 解析 + translate_chunk + 重新 emit
        └── Usage 记录到 usage_log（token 不足时用 chars/4 兜底估算）
```

> **设计决策**：仅支持流式响应（`stream: true`），非流式分支已移除。理由：Claude Code 等主流客户端始终使用流式，非流式无实际场景，可大幅简化代码逻辑喵。

**SniffStream 透传嗅探** (`api/proxy/sniff.rs`):

当客户端协议与上游协议相同时（如 Anthropic 客户端 → Anthropic 上游），无需翻译但仍需记录 token 用量。`SniffStream` 包装 `reqwest` 字节流：
- **不修改字节**：每个 chunk 原样转发给客户端
- **后台解析 SSE**：按 `\n\n` 分割帧，提取 `usage` 字段
- **Anthropic 模式**：`message_start` → input_tokens，`message_delta` → output_tokens，`content_block_delta` → output_chars
- **OpenAI 模式**：最终 chunk 的 `usage.prompt_tokens/completion_tokens`
- **兜底估算**：当上游未报 token 数时，用 `output_chars / 4` 估算

**模型别名系统**:

在 `models` 表的 `model_id` 和 `display_name` 字段中配置别名映射：
```
model_id = "gpt-4o"           -- 真实模型名（发送给上游）
display_name = "my-alias"     -- 别名（暴露给客户端）
```

- 客户端请求 `alias1` → 后端自动替换为 `real-model-name` 发送给上游
- `/v1/models` 列表向客户端暴露别名而非真实模型名
- 响应中的模型名也被替换回客户端请求的别名

**协议转换映射** (`api/proxy/translate.rs`，Anthropic → OpenAI 方向):

| Anthropic 字段 | OpenAI 字段 | 说明 |
|---------------|-------------|------|
| `system` (text/blocks) | `messages[0].role="system"` | System prompt 转换 |
| `messages[].content.blocks` | `messages[].content` | 消息内容块 |
| `tool_use` block | `tool_calls[].function` | 工具调用 |
| `tool_result` block | `role: "tool"` 消息 | 工具结果 |
| `thinking` block | `reasoning_content` | 思考过程 |
| `tool_choice: auto/any/none/tool` | `tool_choice: auto/required/none/function` | 工具选择 |
| `stop_reason: end_turn/tool_use/max_tokens` | `finish_reason: stop/tool_calls/length` | 停止原因 |
| `usage: input_tokens/output_tokens` | `usage: prompt_tokens/completion_tokens` | Token 统计 |

**响应转换**（OpenAI → Anthropic 方向）:

| OpenAI 字段 | Anthropic 字段 | 说明 |
|-------------|---------------|------|
| `choices[0].message.content` | `content: [{type: "text", text: ...}]` | 文本内容 |
| `choices[0].message.tool_calls` | `content: [{type: "tool_use", ...}]` | 工具调用 |
| `choices[0].message.reasoning_content` | `content: [{type: "thinking", ...}]` | 思考过程 |
| `finish_reason: stop/tool_calls/length` | `stop_reason: end_turn/tool_use/max_tokens` | 停止原因 |
| `usage: prompt_tokens/completion_tokens` | `usage: input_tokens/output_tokens` | Token 统计 |

### 4.6 数据库 — `db/`

**SQLite via rusqlite** (`mod.rs`):

- `Database` 结构体封装 `rusqlite::Connection`
- 支持 `TEXT`/`INTEGER`/`REAL`/`NULL` 类型绑定
- 所有操作通过 `conn()` 获取连接

**迁移系统** (`schema.rs`):

6 版迁移，通过 `schema_version` 表跟踪：

| 版本 | 内容 |
|------|------|
| V1 | 初始 schema: providers, models, api_keys, routes, usage_log + 索引 |
| V2 | service_keys 表 + idx_service_keys_hash 索引 |
| V3 | models 新增 `capabilities` 列；清空 api_keys + service_keys（加密格式变更）；删除 custom/deap providers |
| V4 | usage_log 新增 `service_key_id` 列 + 索引（按服务密钥分组统计） |
| V5 | 密钥可用性状态不再持久化（清掉 DB 中的 status/last_error 残留，运行时全内存 green） |
| V6 | 新增 `settings` key-value 表（存储 websearch_hijack 等运行时开关） |

### 4.7 密钥池 — `keys/pool.rs`

```
KeyPool
  ├── keys: HashMap<provider_id, Vec<KeyEntry>>  ← 按 provider 分组（Arc<RwLock>）
  ├── current_index: HashMap<provider_id, usize>  ← 当前轮询位置
  ├── database: Option<Database>                   ← DB 引用（仅用于 token 统计持久化）
  └── key_stats_tx: broadcast::Sender<Value>       ← WebSocket 广播

get_next_key(provider_id)
  → 从 current_index 开始轮询
  → 跳过 🔴 Red 密钥
  → 跳过 🟡 Yellow 密钥（冷却 300 秒后自动恢复）
  → 返回第一个 🟢 Green 密钥
  → 全部失效时返回 None

mark_key_invalid()    → 401/403 → 🔴 Red → 永久跳过 → 广播 key_stats
mark_key_low_quota()  → 402/429 → 🟡 Yellow → 冷却 5 分钟后恢复 → 广播 key_stats
record_key_success()  → 更新 total_requests/total_tokens/last_used_at → 持久化到 DB → 恢复 🟢

load_all_keys_from_db()
  → AES-256-GCM 解密 DB 中的 key_hash → 内存 KeyEntry
  → 解密失败 fallback 明文（兼容旧格式）
  → 所有 key 启动时一律 Green（V5 后 DB status 列不再被读写）
```

> **V5 设计决策**：密钥健康状态改为纯内存管理，启动时所有 Key 均为绿色。DB 的 `status` 列保留但不再被读写，避免每次状态变更都写 DB 的开销喵～

### 4.8 Provider 适配器 — `providers/`

2 种内置 Provider 适配器实现：

| 适配器 | 文件 | 认证头 | API 路径 | 说明 |
|--------|------|--------|---------|------|
| `OpenAIAdapter` | `openai.rs` | `Authorization: Bearer {key}` | `/chat/completions` | OpenAI 官方 |
| `AnthropicAdapter` | `anthropic.rs` | `x-api-key: {key}` + `anthropic-version` | `/v1/messages` | Anthropic 官方 |

所有 HTTP 请求通过 `reqwest` 异步发送，支持流式 SSE 响应喵～

---

## 5. 前端模块详解

### 5.1 应用入口 — `main.ts`

```typescript
initTheme()                          // 在 mount 前设置主题，避免闪烁

createApp(App)
  .use(createPinia())                // 状态管理
  .use(createRouter({                // 路由
    history: createWebHistory(),
    routes,
  }))
  .mount('#app')
```

**MWC 组件按需导入**: 20+ Material Web Components（button, icon-button, icon, list, list-item, menu, menu-item, divider, dialog, textfield, select, select-option, switch, checkbox, chip-set, input-chip, circular-progress）

### 5.2 路由 — `router.ts`

| 路径 | 组件 | 说明 |
|------|------|------|
| `/providers` | ProvidersView | 供应商列表 |
| `/providers/new` | ProviderNewView | 新建供应商 |
| `/providers/:id/edit` | ProviderNewView | 编辑供应商 |
| `/keys` | KeysView | 服务密钥管理 |
| `/stats` | StatsView | 用量统计 |
| `/settings` | SettingsView | 用户设置 |
| `/` | → `/providers` | 默认重定向 |

### 5.3 状态管理 — `stores/`

#### `providers.ts` — Provider 列表

```typescript
interface ProvidersState {
  providers: Provider[]
  loading: boolean
  error: string | null
}
// 计算属性: enabledProviders, providerCount
// 方法: fetchProviders(), createProvider(), updateProvider(), deleteProvider()
```

#### `keys.ts` — API Key 列表（按 provider_id 分组）

```typescript
interface KeysState {
  keysByProvider: Record<string, ApiKey[]>
  loading: boolean
}
// 方法: fetchKeys(), addKey(), removeKey(), getKeys(providerId)
```

#### `models.ts` — 模型列表（按 provider_id 分组）

```typescript
interface ModelsState {
  modelsByProvider: Record<string, Model[]>
  loading: boolean
}
// 方法: fetchModels(), syncModels(), addModel(), updateModel(), removeModel(), getModels()
```

#### `dashboard.ts` — 仪表盘数据

```typescript
interface DashboardState {
  overview: object | null
  usage: object | null
  loading: boolean
  wsConnected: boolean
}
// 方法: fetchOverview(), fetchUsage(), connectWebSocket(), disconnectWebSocket()
```

### 5.4 API 客户端 — `api.ts`

统一的 REST API 封装：

```typescript
async function request<T>(path, opts): Promise<T>
  → fetch(path, { method, headers: {Content-Type}, body })
  → 自动 JSON 序列化/反序列化
```

API 模块: `providersApi`, `serviceKeysApi`, `keysApi`, `modelsApi`, `statsApi`, `dashboardApi`, `publicApi`, `settingsApi`

### 5.5 页面组件

#### `ProvidersView.vue`
- 显示所有 Provider 卡片列表（网格布局）
- 每张卡片展示: 名称、SVG logo、密钥统计（绿/总数）、Base URL
- 内联展开查看 API Keys + Models
- 右键打开删除对话框
- WebSocket 实时更新密钥统计（监听 `key_stats` 事件）
- 空态: 「空空如也」+ 添加按钮

#### `ProviderNewView.vue`
- 供应商创建/编辑表单
- 字段: 名称、API 格式 (Anthropic/OpenAI)、Base URL、API Keys（多行文本）、可用模型（多行文本）
- API Path 自动派生（Anthropic → `/v1/messages`，OpenAI → `/v1/chat/completions`）
- 编辑模式下回填数据（包括明文密钥和模型别名）

#### `KeysView.vue`
- 显示所有 Service Key 列表（表格形式）
- 展示: 名称、脱敏密钥（****xxxx）、请求/token 统计、上次使用时间
- 操作: 创建新 Service Key（创建后显示明文，仅一次）、删除
- 权限管理对话框（按供应商分组选择可用模型 `allowed_models`）

#### `StatsView.vue`
- Chart.js 折线图（使用 vue-chartjs）
- 时间范围选择器（一天内/一周内/一月内）
- 按密钥分组显示 token 用量
- 支持 hour/day 粒度 + 时区偏移
- WebSocket 监听 `usage_stats_changed` 自动刷新

#### `SettingsView.vue`
- 关于 section（项目描述 + GitHub 链接）
- 主题切换（浅色/深色按钮，持久化到 localStorage）
- 劫持 WebSearch 开关（调用 settingsApi）
- 清除所有本地数据（危险操作，需确认对话框）

### 5.6 UI 设计规范

遵循 **Material Design 3 (MD3)** 设计语言：

- **色彩**: 全部使用 CSS 变量 `var(--md-sys-color-*)`，无硬编码 hex；品牌色（openai/anthropic）已抽为 CSS token
- **形状**: 使用 `var(--md-sys-shape-corner-*)` token
- **字体**: Roboto Flex + MD3 typescale 辅助类
- **暗色模式**: `<html data-theme="dark">` 由 `theme.ts` 控制，支持手动切换 + 跟随系统
- **图标**: MDI (`<span class="mdi mdi-xxx">`)
- **MWC 组件**: 使用 `:value` + `@input` 而非 `v-model`
- **导航**: 药丸形 (`border-radius: 9999px`)

### 5.7 Vite 开发代理

```typescript
proxy: {
  '/api':    'http://localhost:19068',
  '/v1':     'http://localhost:19068',
  '/health': 'http://localhost:19068',
  '/ws': {
    target: 'ws://localhost:19068',
    ws: true,
  },
}
```

### 5.8 WebSocket 客户端 — `ws.ts`

完整实现的 `WebSocketClient` 类：
- 连接 `ws://localhost:19068/ws`
- 自动重连（3 秒延迟）
- 事件发布/订阅系统（`on()`/`off()`/`dispatch()`）
- 支持 7 种事件类型：`key_stats`、`key_health`、`request_metrics`、`balance_update`、`provider_status`、`usage_stats_changed`、`error`
- 通配符 `*` 监听所有事件
- 实际被 ProvidersView（监听 `key_stats`）和 StatsView（监听 `usage_stats_changed`）使用

---

## 6. 数据模型

### 6.1 数据库 Schema (V6)

> **Schema 说明**：当前为 V6，providers 表已使用规范化设计（`base_url + api_path` 拼接 endpoint，密钥统一由 `api_keys` 表管理，模型统一由 `models` 表管理。V3 新增 capabilities，V4 新增 service_key_id，V5 密钥状态纯内存化，V6 新增 settings 表）。

```sql
-- 供应商 (Provider)
CREATE TABLE providers (
    id TEXT PRIMARY KEY,              -- UUID
    name TEXT NOT NULL,               -- 显示名称
    kind TEXT NOT NULL,               -- "openai" / "anthropic"
    base_url TEXT NOT NULL,           -- API 基础 URL（如 https://api.openai.com）
    api_path TEXT NOT NULL DEFAULT '/v1/chat/completions',  -- API 路径
    enabled INTEGER NOT NULL DEFAULT 1,
    config_json TEXT NOT NULL DEFAULT '{}',  -- 扩展配置（Provider 特有的非通用选项）
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- 模型 (Model)
CREATE TABLE models (
    id TEXT PRIMARY KEY,
    provider_id TEXT NOT NULL REFERENCES providers(id),
    model_id TEXT NOT NULL,           -- "gpt-4o", "claude-opus-4-8"
    display_name TEXT NOT NULL,
    tier TEXT NOT NULL DEFAULT 'custom',  -- "fable" / "opus" / "sonnet" / "haiku" / "custom"
    capabilities TEXT NOT NULL DEFAULT '["text"]',
    context_window INTEGER NOT NULL DEFAULT 128000,
    max_output_tokens INTEGER NOT NULL DEFAULT 4096,
    cost_per_1k_input REAL NOT NULL DEFAULT 0.0,
    cost_per_1k_output REAL NOT NULL DEFAULT 0.0,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(provider_id, model_id)
);

-- Provider API 密钥
CREATE TABLE api_keys (
    id TEXT PRIMARY KEY,
    provider_id TEXT NOT NULL REFERENCES providers(id),
    name TEXT NOT NULL DEFAULT '',
    key_hash TEXT NOT NULL,           -- AES-256-GCM 密文（base64(nonce || ciphertext)）
    key_masked TEXT NOT NULL,         -- 脱敏显示（sk-xxxx...xxxx），创建时截取
    status TEXT NOT NULL DEFAULT 'green',  -- V5 后不再被读写，保留兼容
    last_error TEXT,
    last_error_code INTEGER,
    last_error_time INTEGER,
    last_used_at INTEGER,
    balance REAL,
    balance_updated_at INTEGER,
    total_requests INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- 路由规则（预留，用于未来多 Provider 负载分发）
CREATE TABLE routes (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    model_id TEXT NOT NULL REFERENCES models(id),
    provider_id TEXT NOT NULL REFERENCES providers(id),
    priority INTEGER NOT NULL DEFAULT 100,   -- 越低优先级越高
    weight REAL NOT NULL DEFAULT 1.0,        -- 加权分布 0.0-1.0
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- 服务密钥（客户端访问凭证）
CREATE TABLE service_keys (
    id TEXT PRIMARY KEY,              -- "skey_{timestamp}_{random}"
    name TEXT NOT NULL DEFAULT '',
    key_hash TEXT NOT NULL,           -- 密钥哈希值
    key_masked TEXT NOT NULL,         -- 脱敏显示（xrl-xxxx****xxxx）
    allowed_models TEXT NOT NULL DEFAULT '[]',  -- JSON 数组，限制可使用的模型
    total_requests INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    last_used_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- 用量日志
CREATE TABLE usage_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp INTEGER NOT NULL,
    provider_id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    key_id TEXT,                      -- Provider API Key
    service_key_id TEXT,              -- V4: 客户端 Service Key（用于按 Service Key 分组统计）
    request_type TEXT NOT NULL,
    prompt_tokens INTEGER NOT NULL,
    completion_tokens INTEGER NOT NULL,
    latency_ms INTEGER NOT NULL,
    success INTEGER NOT NULL,
    error_message TEXT,
    cost_estimate REAL
);

-- 迁移版本跟踪
CREATE TABLE schema_version (
    version INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL
);

-- V6: 通用应用设置表（key-value）
CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
```

> **设计说明**：
> - `key_hash`：V3 后存储 AES-256-GCM 密文（`base64(nonce || ciphertext)`），而非哈希。GET /api/keys 时解密返回 `key_plain` 字段
> - `key_masked` 保留：虽然理论上是派生字段，但原始密钥加密后不可直接截取，脱敏值必须在创建时保存
> - `total_requests`/`total_tokens` 保留：作为 `usage_log` 的聚合缓存，避免每次查询都全表扫描，属于合理的反规范化
> - `routes` 表保留：为未来多 Provider 负载分发预留，当前未使用
> - `status` 列保留但 V5 后不再被读写：密钥健康状态改为纯内存管理
> - `service_key_id`（V4）：记录请求来自哪个 Service Key，统计可按客户端维度分组
> - `settings`（V6）：存储 `websearch_hijack` 等运行时开关

### 6.2 模型层级系统

| Tier | 说明 | 示例 |
|------|------|------|
| **Fable** | 顶级智能 | Claude Opus 5, GPT-5, Gemini Ultra |
| **Opus** | 高性能 | Claude Opus 4.x, GPT-4o |
| **Sonnet** | 均衡 | Claude Sonnet, GPT-4o-mini, Qwen3.7-Plus |
| **Haiku** | 轻量快速 | Claude Haiku, GPT-4o-nano |
| **Custom** | 用户自定义 | 其他模型 |

---

## 7. API 规范

### 7.1 公开 API（LLM 代理）

所有 `/v1/*` 端点需要 Service Key 认证 + Rate Limit。支持 Anthropic 和 OpenAI 双入口格式。

#### `GET /health`
```json
{
  "status": "ok",
  "service": "xrl-router",
  "version": "0.2.0",
  "timestamp": 1699000000,
  "database": "ok",
  "providers": {"total": 3, "enabled": 2},
  "models": {"total": 10},
  "keys": {"ProviderName": {"total": 5, "green": 4, "yellow": 1, "red": 0}}
}
```

#### `GET /v1/models`
**Header**: `x-api-key: xrl-{service_key}` 或 `Authorization: Bearer xrl-{service_key}`

**响应**:
```json
{
  "object": "list",
  "data": [
    {
      "id": "my-alias",
      "object": "model",
      "created": 1699000000,
      "owned_by": "provider-name",  -- 从 providers 表 JOIN 获取
      "display_name": "My Model",
      "tier": "opus",
      "context_window": 128000
    }
  ]
}
```

#### `POST /v1/messages` — Anthropic Messages API（下游入口之一）
**Header**: `x-api-key: xrl-{service_key}` 或 `Authorization: Bearer xrl-{service_key}`

**请求**: 标准 Anthropic Messages API 格式
```json
{
  "model": "my-alias",
  "max_tokens": 4096,
  "messages": [{"role": "user", "content": "Hello"}],
  "stream": true
}
```

**响应**: 标准 Anthropic Messages API 格式（支持流式 SSE）

**内部处理**:
- 上游为 Anthropic: 直接透传 + SniffStream 嗅探 token
- 上游为 OpenAI: Anthropic → OpenAI 转换请求，OpenAI → Anthropic 转换响应

#### `POST /v1/chat/completions` — OpenAI Chat Completions API（下游入口之二）
**Header**: `x-api-key: xrl-{service_key}` 或 `Authorization: Bearer xrl-{service_key}`

**请求**: 标准 OpenAI Chat Completions API 格式

**内部处理**:
- 上游为 OpenAI: 直接透传 + SniffStream 嗅探 token
- 上游为 Anthropic: OpenAI → Anthropic 转换请求，Anthropic → OpenAI 转换响应

### 7.2 管理 API（无认证）

所有 `/api/*` 端点无需认证，由 Tauri WebView 直接访问。

#### Provider CRUD

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/api/providers` | 列出所有 Provider |
| `POST` | `/api/providers` | 创建新 Provider |
| `GET` | `/api/providers/:id` | 获取单个 Provider |
| `PUT` | `/api/providers/:id` | 更新 Provider |
| `DELETE` | `/api/providers/:id` | 删除 Provider |

**Provider 请求体**:
```json
{
  "name": "My Provider",
  "kind": "openai",
  "base_url": "https://api.example.com",
  "api_path": "/v1/chat/completions",
  "config": {}
}
```

#### API Key CRUD

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/api/keys` | 列出所有 API Key（含解密明文 `key_plain` + 内存实时状态） |
| `POST` | `/api/keys` | 创建新密钥 |
| `GET` | `/api/keys/:id` | 获取单个密钥 |
| `PUT` | `/api/keys/:id` | 更新密钥 |
| `DELETE` | `/api/keys/:id` | 删除密钥 |

#### Model CRUD

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/api/models` | 列出所有模型（支持 `?provider_id=` 过滤） |
| `POST` | `/api/models` | 创建新模型 |
| `GET` | `/api/models/:id` | 获取单个模型 |
| `PUT` | `/api/models/:id` | 更新模型 |
| `DELETE` | `/api/models/:id` | 删除模型 |

#### Service Key CRUD

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/api/service-keys` | 列出所有 Service Key（脱敏） |
| `POST` | `/api/service-keys` | 创建新 Service Key（返回原始密钥，仅此一次可见） |
| `PUT` | `/api/service-keys/:id` | 更新 Service Key（name + allowed_models） |
| `DELETE` | `/api/service-keys/:id` | 删除 Service Key |

**Service Key 创建请求体**:
```json
{
  "name": "My Key"
}
```

**Service Key 创建响应**:
```json
{
  "id": "skey_xxx",
  "name": "My Key",
  "key": "xrl-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
  "key_masked": "****xxxx"
}
```

> ⚠️ `key` 字段仅在创建时返回一次，之后不可再见。存储时使用 **Argon2** 哈希（随机 salt + PHC 格式）。

#### 模型同步

| 方法 | 路径 | 说明 |
|------|------|------|
| `POST` | `/api/models/sync` | 从上游 Provider 同步可用模型列表（`{provider_id}`） |

#### 统计

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/api/stats` | 用量统计（支持 `from/to/granularity(hour|day)/tz_offset`，按 service_key_id 分组） |

#### 应用设置

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/api/settings` | 获取应用设置（`{websearch_hijack: bool}`） |
| `PUT` | `/api/settings` | 更新设置（持久化到 settings 表 + 运行时 AtomicBool） |

#### 上游模型代理

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/api/proxy/models` | 代理获取上游模型列表（`?url=&type=&key=`，避免浏览器 CORS） |

---

## 8. 核心业务流程

### 8.1 完整请求生命周期

```
1. Claude Code 发起请求
   POST /v1/messages
   x-api-key: xrl-abc123
   body: {"model": "smart-alias", "messages": [...], "stream": true}

2. 服务器验证 Service Key
   → verify_service_key(): 查 service_keys 表，Argon2 逐条校验
   → 返回 (service_key_id, allowed_models)

3. Allowed Models 白名单检查
   → 非空时客户端只能用白名单内的别名，不匹配 → 403

4. 路由解析
   → resolve_route(): 查 models 表（display_name 别名匹配）JOIN providers (WHERE enabled=1)
   → 确定: upstream_url, real_model_id, provider_kind, provider_id

5. WebSearch 劫持判断
   → 如果 websearch_hijack 开关开 + tools 含 web_search*
   → → run_websearch_loop()：用本地 Bing 搜索代替上游（最多 5 轮 tool-calling）

6. 协议转换 + 强制流式
   → 同协议: 直接透传 + SniffStream
   → 异协议: translate 双向转换
   → 插入 stream: true, model: real_model_id

7. Key 轮询重试循环
   → pick_key_for() 从 KeyPool 取 key
   → 401/403 → mark_key_invalid(Red) → rotate → replay
   → 402/429 → mark_key_low_quota(Yellow, 5min 冷却) → rotate → replay

8. 流式转发
   → response.bytes_stream() 逐 chunk 读取
   → 同协议: SniffStream 无修改转发 + 后台嗅探 token
   → 异协议: 逐 SSE frame 解析 + translate_chunk + 重新 emit
   → Usage 记录到 usage_log（token 不足时用 chars/4 兜底估算）
```

### 8.2 密钥选择流程

```
收到 LLM 请求
  → KeyPool.get_next_key(provider_id)
     ├── 从 current_index 开始轮询
     ├── 跳过 🔴 Red 密钥
     ├── 跳过 🟡 Yellow 密钥（冷却 300 秒后自动恢复）
     └── 返回 🟢 Green 密钥

  上游返回:
  ├── 200 OK → record_key_success(tokens) → 保持 🟢 → 持久化 total_requests/total_tokens 到 DB
  ├── 401/403 → mark_key_invalid() → 🔴 Red → 永久跳过 → 广播 key_stats
  ├── 402/429 → mark_key_low_quota() → 🟡 Yellow → 冷却 5 分钟 → 广播 key_stats
  └── 其他错误 → 不改变状态，forward_upstream_error 原样转给客户端
```

---

## 9. 认证与安全

### 9.1 单层认证

| 层 | 机制 | 用途 |
|---|---|---|
| **LLM API** | Service Key (`x-api-key` 或 `Authorization: Bearer`) | 保护 `/v1/*` 代理端点 |
| **管理面板** | 无认证 | `/api/*` 端点由 Tauri WebView 直接访问 |

### 9.2 Service Key 验证

```
客户端请求
  → 提取 x-api-key 或 Authorization: Bearer header
  → 查 service_keys 表，Argon2 逐条校验（verify_password）
  → 匹配成功 → 返回 (service_key_id, allowed_models)
  → 匹配失败 → 401 Unauthorized
```

### 9.3 安全注意事项

- Service Key 存储: 使用 **Argon2** 哈希（随机 salt + PHC 格式），创建时返回原始密钥（仅一次可见），不可逆
- Provider API Key 存储: 使用 **AES-256-GCM** 加密存储（`api_keys.key_hash` 实际存储 `base64(nonce || ciphertext)`），主密钥存放在系统应用数据目录下的 `master.key`（自动生成，权限 0600），运行时解密使用
- Master Key 管理: 首次启动时 `OsRng` 随机生成 256 位主密钥，持久化到系统应用数据目录下的 `master.key`（base64 编码，Unix 权限 0600）。数据库单独泄露不暴露密钥；丢失 master.key 则已加密的 Provider Key 无法解密
- CORS: 已实现 origin 白名单机制（默认含 localhost:5173/19068 + tauri://localhost + https://tauri.localhost），不再是 `allow_origin(Any)`
- 限流: 按 Service Key 令牌桶限流（默认 60 req/min），超限返回 429 + `retry-after` header

---

## 10. 配置与环境

### 10.1 环境变量

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `PORT` | `19068` | HTTP 监听端口 |
| `HOST` | `127.0.0.1` | 绑定地址（默认仅本机可访问） |
| `DB_PATH` | _(系统数据目录下 `xrl-router.db`)_ | SQLite 文件路径（默认使用系统应用数据目录；仅在显式设置时覆盖） |
| `LOG_LEVEL` | `info` | 日志级别 |
| `API_KEY` | _(空)_ | 公开 API 密钥（可选，代码中已定义但未使用） |
| `CORS_ORIGINS` | `localhost:5173,127.0.0.1:5173,localhost:19068,127.0.0.1:19068,tauri://localhost,https://tauri.localhost` | CORS Origin 白名单（逗号分隔；空=允许所有） |

> **已移除**: `JWT_SECRET` 和 `ENCRYPTION_KEY` 环境变量已不存在喵。主密钥自动从系统应用数据目录下的 `master.key` 文件加载/生成。

### 10.2 依赖

**后端 (Cargo)**:
- `tauri` 2 — 桌面框架（含 tray-icon feature）
- `tauri-plugin-shell` 2 — Shell 插件
- `tokio` 1 — 异步运行时
- `futures` 0.3 — 异步工具
- `axum` 0.7 — HTTP 框架（含 ws feature）
- `tower` 0.4 + `tower-http` 0.5 — 中间件 (CORS, tracing)
- `reqwest` 0.12 — HTTP 客户端 (json, stream, cookies)
- `rusqlite` 0.32 — SQLite (bundled)
- `serde` 1 + `serde_json` 1 — 序列化
- `chrono` 0.4 — 时间处理
- `uuid` 1 — UUID 生成
- `thiserror` 1 + `anyhow` 1 — 错误处理
- `tracing` 0.1 + `tracing-subscriber` 0.3 — 日志（JSON 格式）
- `dashmap` 6 — 并发 HashMap
- `once_cell` 1 — 延迟初始化
- `async-trait` 0.1 — 异步 trait
- `aes-gcm` 0.10 — AES-256-GCM 加解密
- `argon2` 0.5 — Argon2 哈希
- `sha2` 0.10 — SHA-256（兼容旧格式）
- `scraper` 0.20 — HTML 解析（Bing 搜索用）

**前端 (npm)**:
- `vue` ^3.5.13
- `pinia` ^2.3.0
- `vue-router` ^4.5.0
- `@material/web` ^2.2.0
- `@mdi/font` ^7.4.47
- `chart.js` ^4.4.7
- `vue-chartjs` ^5.3.2

---

## 11. 构建与部署

### 11.1 开发模式

```bash
# 前端开发服务器 (Vite)
pnpm dev            # :5173，代理到 :19068

# 后端 (Rust + Tauri)
cargo tauri dev     # 启动 Tauri 应用 + HTTP 服务器 :19068
```

### 11.2 生产构建

```bash
# Tauri 生产构建（包含前端打包 + Rust 编译 + 桌面应用封装）
cargo tauri build

# 仅后端
cd src-tauri && cargo build --release

# 仅前端
pnpm build           # 产出 dist/
```

### 11.3 运行

```bash
# 设置环境变量（可选）
export PORT=19068

# 启动 Tauri 应用
./src-tauri/target/release/xrl-router
```

首次启动自动在系统应用数据目录创建 `xrl-router.db` 并执行所有迁移，同时生成 `master.key` 主密钥文件喵。

> **数据目录位置**（由 Tauri `app.path().app_data_dir()` 解析）：
> - macOS: `~/Library/Application Support/im.xrl.router/`
> - Linux: `~/.config/im.xrl.router/`
> - Windows: `C:\Users\<user>\AppData\Roaming\im.xrl.router\`
>
> 这样安装后的 app bundle 不会再因相对路径 `data/` 创建失败而闪退。

---

## 12. 已知限制与 TODO

### 12.1 已完成功能

- [x] Rust + Tauri 2 桌面应用
- [x] axum 异步 HTTP 服务器
- [x] SQLite 数据库 + 6 版迁移 (rusqlite bundled, WAL 模式)
- [x] Provider CRUD + 管理面板
- [x] API Key CRUD + 管理面板（含 AES-256-GCM 加解密）
- [x] Model CRUD + 管理面板
- [x] Anthropic Messages API 代理 (`/v1/messages`)
- [x] OpenAI Chat Completions API 代理 (`/v1/chat/completions`)
- [x] Anthropic ↔ OpenAI 协议转换（双向）
- [x] 流式 SSE 代理 + 转换（仅流式，非流式已移除）
- [x] SniffStream 透传流嗅探（token 用量提取）
- [x] 模型别名系统
- [x] 密钥池 + 红绿灯健康状态（纯内存，V5 后不持久化）
- [x] Service Key 认证（Argon2 哈希存储）
- [x] Service Key `allowed_models` 白名单
- [x] Service Key 管理 API (`/api/service-keys`)
- [x] Provider API Key AES-256-GCM 加密存储（crypto/mod.rs + master.key）
- [x] 令牌桶限流（60 req/min，按 Service Key）
- [x] WebSocket 实时推送（后端 /ws + broadcast channel + 前端 ws.ts 自动重连）
- [x] CORS origin 白名单（默认含 localhost:5173/19068 + tauri://localhost）
- [x] WebSearch 劫持（search/bing.rs + api/proxy.rs，最多 5 轮 tool-calling）
- [x] Settings API（websearch_hijack 开关，持久化到 settings 表）
- [x] Dashboard API（overview + usage）
- [x] 模型同步 API（proxy_fetch_models，避免 CORS）
- [x] 系统托盘（关闭窗口最小化到托盘继续运行）
- [x] 主题切换（theme.ts，light/dark，localStorage 持久化）
- [x] Vue3 + MWC MD3 前端
- [x] 前端离线状态处理（ConnectionStatus 组件）
- [x] 前端全局错误边界（errorHandler + unhandledrejection）
- [x] 结构化日志（tracing + JSON 输出，含 trace_id/provider/model/latency）

### 12.2 未完成 / 待改进

- [ ] **管理面板认证**: `/api/*` 端点当前仅靠 `127.0.0.1` 绑定 + CORS 白名单保护，无认证层 — v0.3
- [ ] **CORS 路由级白名单**: 当前全局白名单，未按路由区分管理 API 和公开 API — v0.3
- [ ] **路由规则引擎**: `routes` 表已定义但未使用，需实现优先级 + 权重负载分发
- [ ] **重试机制**: 上游请求失败时的指数退避重试 — v0.3
- [ ] **Prometheus Metrics**: 无 Metrics 导出 — v0.3
- [ ] **`protocol/` 模块**: 当前为占位，实际转换逻辑在 `api/proxy/translate.rs`，可考虑移除或重构
- [ ] **`providers/` 适配器体系**: 当前 proxy.rs 不通过 Adapter trait，直接用 reqwest 发请求，Adapter 体系是遗留设计

---

## 附录 A: 复刻指南

要完整复刻本项目，需要按以下顺序实现：

### Phase 1: 基础设施

1. 初始化 Cargo + Tauri 2 项目
2. 实现 `config.rs` 环境变量加载
3. 实现 `db/mod.rs` SQLite 封装 (rusqlite)
4. 实现 `db/schema.rs` 迁移系统
5. 实现所有 `types/` 数据结构

### Phase 2: 核心服务

6. 实现 `providers/mod.rs` ProviderRegistry
7. 实现 `keys/pool.rs` 密钥池
8. 实现 `models/mod.rs` ModelRegistry
9. 实现 `gateway/server.rs` AppState + axum 服务器

### Phase 3: LLM 代理

10. 实现 `api/proxy/translate.rs` Anthropic → OpenAI 协议转换器
11. 实现 `api/proxy.rs` LLM 代理核心
12. 实现 `api/mod.rs` 路由构建 + CRUD 处理器

### Phase 4: 前端

21. 初始化 Vue3 + Vite + Pinia + Vue Router
22. 配置 MD3 设计 Token (index.html)
23. 实现 `api.ts` REST 客户端
24. 实现 4 个 Store (providers/keys/models/dashboard)
25. 实现 AppShell 导航抽屉
26. 实现 ProvidersView + KeysView + SettingsView + StatsView
27. 配置 Vite 代理

### Phase 5: 集成

28. 在 `lib.rs` 中集成 Tauri 启动 + gateway spawn
29. 端到端测试

---

## 附录 B: 关键文件交叉引用

| 功能 | 核心文件 | 依赖文件 |
|------|---------|---------|
| LLM 代理 | `api/proxy.rs` | `api/proxy/translate.rs`, `gateway/server.rs` |
| 协议转换 | `api/proxy/translate.rs` | `types/chat.rs` |
| 管理 API | `api/mod.rs` | `types/`, `db/mod.rs`, `gateway/server.rs` |
| 密钥管理 | `keys/pool.rs` | `types/key.rs`, `db/mod.rs` |
| Provider 管理 | `providers/mod.rs` | `types/provider.rs`, `db/mod.rs` |
| 模型注册 | `models/mod.rs` | `types/model.rs`, `db/mod.rs` |
| 数据库 | `db/mod.rs` | `db/schema.rs`, `db/queries.rs` |
| 配置 | `config.rs` | — |
| 入口 | `lib.rs` | `config.rs`, `db/`, `gateway/` |
| 前端入口 | `main.ts` | `router.ts`, `App.vue` |

---

## 13. 设计审查：不合理之处与风险识别

基于对整份技术规格的审查，以下列出设计决策中存在的不合理性、潜在风险和待商榷之处喵～

### 13.1 架构层面

#### ⚠️ 13.1.1 SQLite 单文件数据库与网关场景不匹配

**问题**：选择 SQLite 作为网关的核心存储，但未讨论并发写入场景下的限制。

**风险点**：
- SQLite 默认单写者锁，`usage_log` 表在每次请求时都会写入
- 当多个 LLM 请求并发到达时，`usage_log` 写入会产生锁竞争
- 高吞吐场景下（>100 QPS）可能成为瓶颈

**建议**：
- 启用 WAL 模式（Write-Ahead Logging）缓解读写冲突
- 考虑将 `usage_log` 改为异步批量写入（内存缓冲 + 定时 flush）
- 明确性能上限，如"目标支持 ≤50 QPS 并发"

#### ⚠️ 13.1.2 密钥池状态设计变更

~~**问题**：`KeyPool` 的红绿灯状态、当前轮询位置都在内存中，进程重启后丢失。~~

**V2 曾解决**（持久化到 DB）→ **V5 重新改为纯内存**：
- V5 迁移后，密钥健康状态不再持久化到 DB
- 启动时所有 Key 均为绿色初始状态
- DB 的 `status`/`last_error` 列保留但不再被读写
- **理由**：减少每次状态变更的 DB 写入开销，通过 WebSocket 实时推送状态变更到前端

#### ⚠️ 13.1.3 管理 API 无认证，但端口已绑定本机

~~**问题**：`/api/*` 端点声明"无需认证"，但 HTTP 服务器绑定到 `0.0.0.0:19068`。~~

**部分解决**（v0.2）：
- HTTP 服务器已绑定 `127.0.0.1:19068`，仅本机可访问
- CORS 已实现 origin 白名单（默认含 localhost:5173/19068 + tauri://localhost）
- 但仍无认证层（如 Basic Auth / Session Token），属于 v0.3 计划

**剩余风险点**：
- 本机其他进程可直接访问管理 API

### 13.2 安全层面

#### ✅ 13.2.1 Service Key 哈希存储

~~**问题**：文档明确承认 `key_hash` 字段"实际存储明文"，并标记为 TODO。~~

**已解决**（v0.2）：
- Service Key 使用 **Argon2** 哈希存储（随机 salt + PHC 格式），比 SHA-256 更安全
- 创建时返回原始密钥（仅一次可见）

#### ✅ 13.2.2 CORS 策略收紧

~~**问题**：当前仍为 `Access-Control-Allow-Origin: *`，需按路由区分白名单。~~

**已解决**（v0.2）：
- 已实现 origin 白名单机制
- 默认含 `localhost:5173`、`127.0.0.1:5173`、`localhost:19068`、`127.0.0.1:19068`、`tauri://localhost`、`https://tauri.localhost`
- HOST 绑定 `127.0.0.1`，外部网络无法直接访问

**剩余计划**（v0.3）：
- 按路由区分白名单（管理 API 严格、公开 API 宽松）

#### ⚠️ 13.2.3 缺少请求频率限制

~~**问题**：文档未提及任何 Rate Limiting 机制。~~

**已解决**（v0.2）：
- `middleware/rate_limit.rs` 实现令牌桶限流
- 默认 60 req/min，按 Service Key 独立计数
- 超限返回 `429 Too Many Requests` + `retry-after` header

### 13.3 数据模型层面

#### ⚠️ 13.3.1 `routes` 表未被使用

**问题**：`routes` 表定义了路由规则（优先级、权重），但文档中没有任何地方说明它如何被使用。

**证据**：
- §4.5 的"请求处理流程"中，`resolve_route()` 直接查询 `providers` 表
- §8.1 的"完整请求生命周期"同样未提及 `routes` 表

**建议**：
- **方案 A**：如果暂不实现路由规则，在 §12 中明确标注为"预留设计"
- **方案 B**：实现基于 `routes` 表的路由选择（多 Provider 负载分发）

#### ✅ 13.3.2 `provider.config_json` 与扁平化字段重复（已修复）

**原问题**：`providers` 表同时存在 `config_json` 和 `endpoint`, `endpoint_type`, `key_static` 等扁平化字段，职责不清。

**已解决**：删除所有冗余扁平字段，`config_json` 仅用于 Provider 特有的扩展配置，职责清晰。

#### ⚠️ 13.3.3 时间戳使用 INTEGER 而非 TEXT

**问题**：所有 `created_at`、`updated_at` 字段都是 `INTEGER NOT NULL`（Unix 时间戳）。

**风险点**：
- 人类不可读，调试时需要额外转换
- 时区信息隐含在应用层

**建议**：
- 如果坚持用 INTEGER，在文档中明确单位为"秒"还是"毫秒"
- 考虑使用 `TEXT` 存储 ISO 8601 格式，如 `2026-07-30T12:00:00Z`

### 13.4 API 设计层面

#### ⚠️ 13.4.1 `/v1/models` 端点职责不清

**问题**：`/v1/models` 需要 Service Key 认证，但它应该返回什么？

**歧义**：
- **方案 A**：返回当前 Service Key 可用的所有模型（基于 `allowed_models`）
- **方案 B**：返回所有已配置的模型（忽略权限限制）

**建议**：
- 明确语义：应该返回"当前请求方可使用的模型列表"
- 基于 `service_keys.allowed_models` 过滤，如果为空则返回全部

#### ⚠️ 13.4.2 Provider CRUD 的请求体与数据库 Schema 不匹配

**问题**：§7.2 中的 Provider 请求体示例：
```json
{
  "name": "My Provider",
  "kind": "openai",
  "base_url": "https://api.example.com",
  "api_path": "/v1/chat/completions",
  "config": {}
}
```

请求体与 Schema 已对齐。建议补充：
- 明确哪些字段是必填，哪些是可选
- 增加 `enabled` 字段的默认值说明

#### ⚠️ 13.4.3 缺少错误响应格式规范

**已部分实现**（v0.2）：
- CRUD 端点: `{"error": "message"}`（简单字符串格式）
- Proxy 端点: `{"error": {"type": "...", "message": "..."}}`（Anthropic 风格）

**待改进**：
- CRUD 端点建议改为结构化格式 `{"error": {"code": "...", "message": "...", "details": {}}}`

### 13.5 前端设计层面

#### ⚠️ 13.5.1 缺少离线状态处理

~~**问题**：前端通过 HTTP 访问后端 API，但未说明如何处理后端未就绪或断连的情况。~~

**已解决**（v0.2）：
- `components/ConnectionStatus.vue` 实现离线横幅 + 重试按钮
- `api.ts` 中 `connectionState` 追踪连接状态，网络错误时自动标记离线
- 每 5 秒轮询连接状态

#### ⚠️ 13.5.2 缺少前端错误边界

~~**问题**：Vue3 应用未提及全局错误处理。~~

**已解决**（v0.2）：
- `main.ts` 中 `app.config.errorHandler` 捕获未处理异常
- `window.addEventListener('unhandledrejection', ...)` 捕获未处理的 Promise 拒绝

### 13.6 协议转换层面

#### ⚠️ 13.6.1 协议转换的完整性未保证

**已文档化**（v0.2）：
- 已新增「协议转换已知不兼容特性」清单（见 §15），明确列出 thinking、tool_choice 等不兼容项的处理策略

#### ⚠️ 13.6.2 流式转换的性能开销未评估

**问题**：流式请求需要"逐 chunk 解析 SSE → 转换 → 重新封装"，但未评估性能影响。

**风险点**：
- 每个 chunk 都需要 JSON 解析和重新序列化
- 大响应（如 100KB+）可能产生明显的 CPU 开销

**建议**：
- 性能测试：对比透传 vs 转换的延迟差异
- 如果开销明显，考虑流式 JSON 解析库（如 `simd-json`）

### 13.7 运维与可观测性

#### ⚠️ 13.7.1 缺少结构化日志规范

**已部分实现**（v0.2）：
- 使用 `tracing` crate，所有 proxy 请求日志包含 `trace_id`、`model`、`latency_ms`、`status` 等字段
- 日志级别区分: `info`（请求摘要）、`warn`（认证失败/路由失败）、`error`（上游错误）

**待改进**（v0.3）：
- 输出格式改为 JSON（当前为文本格式），便于日志采集系统解析
- 增加 `provider_id` 字段

#### ⚠️ 13.7.2 缺少健康检查的详细指标

**问题**：`/health` 端点只返回 `{"status":"ok"}`，信息量不足。

**建议**：
- 增加详细健康指标：
```json
{
  "status": "ok",
  "uptime_seconds": 3600,
  "active_connections": 5,
  "database": "ok",
  "providers": {"total": 3, "enabled": 2}
}
```

#### ⚠️ 13.7.3 缺少 Metrics 导出

**问题**：文档未提及任何 Metrics 导出机制（如 Prometheus）。

**建议**：
- 至少导出关键指标：请求总数、延迟分布、错误率、按 Provider 分组
- 可选：支持 `/metrics` 端点（Prometheus 格式）

### 13.8 其他

#### ⚠️ 13.8.1 模型 Tier 分类硬编码

**问题**：`model.tier` 字段硬编码为 `fable/opus/sonnet/haiku/custom`，与 Anthropic 模型命名强耦合。

**风险点**：
- 其他 Provider 的模型无法归类（如 GPT-5、Gemini Ultra）
- 新模型不断涌现，Tier 列表需要频繁更新

**建议**：
- 改为开放字符串，由用户自定义 Tier
- 或提供映射规则：`gpt-4o → opus`, `claude-opus-4-8 → opus`

#### ⚠️ 13.8.2 缺少版本兼容性说明

**问题**：文档未说明与 Anthropic/OpenAI API 的版本兼容性。

**建议**：
- 明确支持的 API 版本：
  - Anthropic Messages API: `2023-06-01`（示例）
  - OpenAI Chat Completions: `v1`
- 如果上游 API 升级，如何保证兼容性

#### ⚠️ 13.8.3 `protocol/` 和 `middleware/` 模块状态

**当前状态**（v0.2）：
- `middleware/` — 已实现 `rate_limit.rs`（令牌桶限流），不再为空占位
- `protocol/` — 仍为占位，实际协议转换逻辑在 `api/proxy/translate.rs`

**建议**：
- `protocol/` 如无独立用途可移除，避免混淆
- `middleware/` 后续可扩展（如认证、日志中间件）

---

## 14. 设计审查总结

### 14.1 风险等级矩阵

| 风险等级 | 数量 | 关键项 |
|---------|------|--------|
| 🟢 **已解决** | 3 | Service Key Argon2 哈希、Provider Key AES-256-GCM 加密、CORS 白名单 |
| ⚠️ **中风险** | 4 | 管理 API 无认证、协议转换完整性、SQLite 并发、Adapter 遗留设计 |
| ⚠️ **低风险** | 4 | 时间戳格式、Tier 硬编码、占位模块、routes 表未使用 |

### 14.2 建议优先级

**P0 — 已完成**（v0.2）：
1. ~~Service Key 哈希存储~~ ✅（Argon2）
2. ~~管理 API 绑定 `127.0.0.1`~~ ✅
3. ~~CORS 策略收紧~~ ✅（origin 白名单已实现）
4. ~~Provider API Key 加密~~ ✅（AES-256-GCM）

**P1 — 已完成**（v0.2）：
5. ~~SQLite WAL 模式启用~~ ✅
6. ~~密钥池状态管理~~ ✅（V5 纯内存化）
7. ~~请求频率限制（Rate Limiting）~~ ✅
8. ~~统一错误响应格式~~ ⚠️ Proxy 端点已完成，CRUD 端点待改进
9. ~~WebSocket 实时推送~~ ✅

**P2 — 已完成/进行中**（v0.2）：
10. ~~结构化日志规范~~ ✅（tracing + JSON 输出已启用）
11. ~~健康检查详细指标~~ ✅（`/health` 已返回详细状态）
12. Metrics 导出（Prometheus）— v0.3
13. ~~WebSearch 劫持~~ ✅
14. ~~Settings API~~ ✅

**P3 — 可选改进**（扩展性）：
15. ~~协议转换不兼容性文档化~~ ✅（见 §15）
16. 模型 Tier 开放化
17. `routes` 表功能实现或移除
18. `providers/` Adapter trait 清理（当前 proxy.rs 未使用）

---

## 15. 协议转换已知不兼容特性

> Anthropic Messages API 与 OpenAI Chat Completions API 之间存在语义差异，部分特性无法完美对等转换。以下列出已知不兼容项及其处理策略。

### 15.1 thinking / reasoning_content

| 方向 | 特性 | 不兼容说明 | 处理策略 |
|------|------|-----------|---------|
| Anthropic → OpenAI | `thinking` content block | OpenAI 无官方 `thinking` 字段 | 转换为非官方 `reasoning_content` 字段（部分 OpenAI 兼容模型支持，如 DeepSeek） |
| OpenAI → Anthropic | `reasoning_content` | OpenAI 非官方字段，非所有模型支持 | 转换为 `thinking` content block，`type: "thinking"` |

**风险**: 依赖非官方字段，不同 Provider 的 OpenAI 兼容 API 可能不支持，导致思考过程丢失。

### 15.2 tool_choice 语义差异

| Anthropic | OpenAI | 语义差异 |
|----------|--------|---------|
| `auto` | `auto` | ✅ 对等 |
| `any` | `required` | Anthropic `any` = 至少一个工具调用；OpenAI `required` 语义相同但名称不同 |
| `none` | `none` | ✅ 对等 |
| `tool: {"name": "xxx"}` | `function: {"name": "xxx"}` | 类型名称不同（`tool` vs `function`） |

**处理**: 直接映射，但需注意 `any` ↔ `required` 的名称差异。

### 15.3 流式 SSE 信封格式

| 差异点 | Anthropic SSE | OpenAI SSE |
|--------|--------------|------------|
| 事件类型 | `message_start`, `content_block_start`, `content_block_delta`, `message_delta`, `message_stop` | `chat.completion.chunk` |
| 完成标记 | `event: message_stop` | `data: [DONE]` |
| 增量格式 | `delta.text` | `choices[0].delta.content` |

**处理**: 转换时需补全 SSE 信封结构，确保客户端接收到完整的事件流。Anthropic 侧客户端期望的多事件类型需在 OpenAI 响应中合成。

### 15.4 其他已知差异

| 特性 | 说明 |
|------|------|
| **system prompt** | Anthropic 独立 `system` 字段 → OpenAI 需转为 `messages[0].role="system"` |
| **tool_result** | Anthropic 使用 `tool_result` content block → OpenAI 使用 `role: "tool"` 消息 |
| **stop_reason** | `end_turn` ↔ `stop`、`tool_use` ↔ `tool_calls`、`max_tokens` ↔ `length` |
| **usage 字段** | `input_tokens/output_tokens` ↔ `prompt_tokens/completion_tokens` |
| **多轮 tool_use** | Anthropic 支持单条消息中多个 `tool_use` block → OpenAI 使用 `tool_calls` 数组 |

> **设计原则**: 对不兼容特性显式处理并转换，而非静默丢弃。无法转换的特性应在日志中记录警告。
