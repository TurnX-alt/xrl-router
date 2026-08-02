# AGENTS.md

本文件为 AI Agent 在 xrl-router 项目上工作时必须遵守的边界与约束。

## 项目范围

xrl-router 是一个**单用户本地 LLM API 网关**，以 Tauri 2 桌面应用形式运行。Rust 后端（`src-tauri/src/`）跑 axum HTTP 服务在 `127.0.0.1:19068`，Vue 3 前端（`src/`）跑在 Tauri WebView 里。所有数据存本地 SQLite。

## 代码组织

```
src-tauri/src/                 后端 Rust
├── main.rs                    入口（thin wrapper，仅调用 lib）
├── lib.rs                     Tauri setup + 数据目录 + master key + DB + 系统托盘 + 网关启动
├── config.rs                  环境变量配置
├── error.rs                   AppError 统一错误类型
├── crypto/mod.rs              AES-256-GCM + Argon2 + master key
├── gateway/server.rs          AppState + start_gateway + CORS
├── api/
│   ├── router.rs              axum 路由表（唯一）
│   ├── handlers/*             管理 API 处理器（按实体分文件）
│   └── proxy/*                LLM 代理核心（handler/auth/route/key_rotation/upstream/websearch/sniff/translate）
├── db/*                       SQLite 封装（mod.rs + schema.rs + 按实体分文件）
├── types/*                    数据结构定义（Provider/Model/ApiKey/Chat/Route/...）
├── providers/                 Provider 适配器（proxy 不经过它）
│    ├─ adapter.rs             Adapter async trait（chat/chat_stream/health_check）
│    ├─ anthropic.rs           AnthropicAdapter 实现
│    └─ openai.rs              OpenAIAdapter 实现
├── plugin/*                   插件系统（mod.rs + registry/keys/health/types）
├── keys/pool/*                KeyPool（mod.rs + types/rotation/health/persistence）
├── models/mod.rs              ModelRegistry
├── middleware/rate_limit.rs   令牌桶限流
├── search/bing.rs             Bing 搜索（WebSearch 劫持用）

src/                           前端 Vue 3
├── main.ts / App.vue / router.ts
├── api.ts                     REST 客户端（BASE_URL 硬编码为 http://localhost:19068）
├── ws.ts                      WebSocket 客户端（自动重连 3s）
├── theme.ts                   明/暗主题（localStorage 持久化）
├── views/*                    5 个页面（Providers/ProviderNew/Keys/Stats/Settings）
├── components/*               AppShell / ConnectionStatus / PluginRegisterDialog
└── stores/*                   4 个 Pinia stores（providers/keys/models/dashboard）

docs/                          文档（本目录）
```

## 关键约定

### 数据目录

生产环境的数据目录由 Tauri 的 `app.path().app_data_dir()` 解析（macOS: `~/Library/Application Support/im.xrl.router/`），**不要**在代码里硬编码相对路径 `data/` ——安装后的 app bundle 工作目录不可写，会导致启动闪退。

### 数据库迁移

- 迁移定义在 `src-tauri/src/db/schema.rs` 的 `MIGRATIONS` 数组
- 每个元素是一条完整 SQL，启动时按序执行
- 当前版本：**V13**（`providers.sort_order`）
- 新增迁移：追加到数组末尾，**不要**修改已有迁移
- 用 `ON CONFLICT DO UPDATE`（UPSERT），**不要用** `INSERT OR REPLACE`（会触发 `ON DELETE CASCADE` 清空子表，`db/mod.rs` 有回归测试）

### 密钥双轨

- **Provider API Key**：AES-256-GCM 加密存储到 `api_keys.key_hash`，主密钥在 `master.key`
- **Service Key**：Argon2 哈希存储到 `service_keys.key_hash`，创建时仅返回一次明文
- 不要混淆这两套；验证 Service Key 必须逐条 `verify_password`（盐随机不可比）

### 代理只支持流式

`api/proxy/handler.rs` 强制 `stream: true`。不要加非流式分支 ——Claude Code 等主流客户端始终流式，加非流式只会增加代码复杂度。

### 协议转换

- 实现在 `api/proxy/translate/`，按方向分 `to_openai.rs` / `to_anthropic.rs`，共享类型在 `common.rs`
- 不兼容特性（thinking、tool_choice 等）要显式转换并记 warn 日志，不要静默丢弃

### 密钥池

- 健康状态**纯内存**（启动全 green），DB 的 `status`/`last_error` 列保留但不再读写
- **轮询指针**持久化到 `settings` 表（键名 `keypool_index_{provider_id}`）
- 锁序生死攸关：`keys/pool/mod.rs` 注释里有详细规则，违反会跟插件的 `keys_update` 形成 ABBA 死锁

### 前端

- UI 用 Material Design 3（`@material/web`），**不要**引入其他组件库
- 颜色用 CSS 变量 `var(--md-sys-color-*)`，**不要**硬编码 hex
- MWC 组件在 `main.ts` 按需导入，**不要**导入 `all.js`
- `api.ts` 的 `BASE_URL` 是写死的 `http://localhost:19068`，前端不走相对路径

## 测试

- 测试写在内联 `#[cfg(test)] mod tests` 块里，**不要**新建 `tests/` 目录
- 用 `Database::open_in_memory()` 跑内存数据库，**不要**写文件
- 关键回归：`db/mod.rs` 有 UPSERT 测试、`gateway/server.rs` 有端到端冒烟测试（真实 TCP）、`keys/pool/mod.rs` 有指针持久化测试
- 前端**没有测试框架**（无 Vitest/Playwright），暂时不要加

## Non-Goals（明确不做的事）

Agent 倾向于扩展。以下功能**不要主动实现**，即使用户描述看似匹配：

### 架构层面

- ❌ **不做云端 SaaS / 多租户 / 多实例部署**。项目是单用户桌面应用，SQLite 单文件，所有"加个 PostgreSQL 支持多用户"的提议都拒绝
- ❌ **不做 Docker 容器化**。Tauri 是桌面框架，容器化没意义
- ❌ **不做 CLI 模式（无 GUI）**。Tauri 的 setup 流程依赖 app handle，拆出来工程量大
- ❌ **不做横向扩展 / 负载均衡**。单实例足够本地场景
- ❌ **不做远程管理界面**。绑定 `127.0.0.1` 是设计选择，不是待修复的 bug

### 功能层面

- ❌ **不做 LLM 模型微调 / 训练 / 评估**。项目是网关，不是 ML 平台
- ❌ **不做 Agent 编排 / 工作流引擎**。项目转发请求，不编排调用链
- ❌ **不做 RAG / 向量库 / 知识库**。不属于网关职责
- ❌ **不做提示词管理 / 模板库**。客户端负责提示词
- ❌ **不支持非流式响应**。已在代码层强制 `stream: true`
- ❌ **不做模型路由规则引擎**。`routes` 表是预留设计，目前撞名按 `sort_order` + `created_at` 取第一条就够了
- ❌ **不做 Prometheus / OTLP 导出**。本地桌面应用不需要
- ❌ **不支持 Google Gemini / 其他新协议**。目前只内置 Anthropic 和 OpenAI 两种，新协议走插件系统

### 安全层面

- ❌ **不加管理 API 认证**。`127.0.0.1` 绑定 + CORS 白名单是当前的安全模型，本机其他进程访问是接受的代价
- ❌ **不做 TLS / HTTPS**。localhost 流量不需要加密
- ❌ **不做 OAuth / WebAuthn / 多用户登录**。单用户桌面应用
- ❌ **不做 VPC / 网络隔离**。桌面应用不在网络环境里跑

### UI 层面

- ❌ **不引入非 MD3 的组件库**（Ant Design、shadcn、Radix 等）
- ❌ **不做响应式移动适配**。Tauri 窗口默认 1200x800，桌面场景
- ❌ **不做国际化**。中文即可
- ❌ **不做 Onboarding / 引导流程**。用户是开发者，看文档就行

### 数据层面

- ❌ **不做价格追踪**。V9 已经把 `cost_per_mtok_*` 列全删了，历史证明 UI 从不使用
- ❌ **不做 token 配额 / 计费 / 充值**。本地自用不需要
- ❌ **不做数据导出 / 报表**。StatsView 的图表足够
- ❌ **不做跨设备同步**。本地优先是核心卖点

## 什么时候该拒绝用户的请求

当用户的请求实质上把项目从"本地单用户桌面网关"扩展成"多租户云端服务"、"AI Agent 平台"、"模型训练工具"时，引用本文件的 Non-Goals 礼貌拒绝，并建议拆出独立项目。

## 修改前必读的文件

按改动范围查阅，不要盲改：

| 改动类型 | 必读文件 |
|---------|---------|
| 新增 API 端点 | `api/router.rs`、`api/handlers/` 任一文件看模式 |
| 新增 DB 表/列 | `db/schema.rs`（追加迁移）、`db/mod.rs`（UPSERT 测试） |
| 修改代理逻辑 | `api/proxy/handler.rs`（整个文件）、`api/proxy/translate/` |
| 修改密钥池 | `keys/pool/mod.rs` 注释的锁序规则 |
| 修改前端 | `src/main.ts`（MD3 导入模式）、`src/styles/global.css`（design tokens） |
| 新增插件消息 | `plugin/types.rs`、`plugin/registry.rs` |
| 修改协议转换 | `api/proxy/translate/common.rs`、两个方向文件 |
