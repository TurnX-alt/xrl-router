# xrl-router — 产品需求文档

> **版本**: 26.7.31+2230 · **文档类型**: Product Requirements Document
>
> 📎 [配套技术文档](./TS.md)

---

## 目录

1. [背景与动机](#1-背景与动机)
2. [竞品分析](#2-竞品分析)
3. [产品定位与目标](#3-产品定位与目标)
4. [用户画像与场景](#4-用户画像与场景)
5. [功能需求](#5-功能需求)
6. [非功能需求](#6-非功能需求)
7. [成功指标](#7-成功指标)
8. [版本规划与路线图](#8-版本规划与路线图)
9. [风险与缓解](#9-风险与缓解)
10. [开放问题](#10-开放问题)

---

## 1. 背景与动机

### 1.1 问题陈述

随着 LLM 生态爆发式增长，开发者面临以下痛点：

| 痛点 | 描述 |
|------|------|
| **协议碎片化** | Anthropic、OpenAI、Google 等主流 Provider 的 API 格式各不相同，客户端集成多家需要维护多套代码 🍥 |
| **密钥管理分散** | 每个 Provider 的 API Key 独立管理，缺乏统一的健康监控和轮换机制 |
| **本地开发不便** | 现有的多 Provider 网关多为 SaaS（OpenRouter）或服务端部署（LiteLLM），本地开发体验差 |
| **Claude Code 集成** | Claude Code 等客户端原生仅支持 Anthropic API，要接入其他 Provider 需要中间层代理 |

### 1.2 为什么不是现有方案

| 现有方案 | 不足 |
|---------|------|
| **OpenRouter** | 云端 SaaS，依赖网络、有额外延迟，不适合对数据敏感的场景 |
| **LiteLLM** | Python 实现，部署较重；服务端思维，无本地桌面体验 |
| **one-api** | Go 实现，功能丰富但无桌面客户端；Web UI 设计陈旧 |
| **Portkey** | 商业化产品，部分功能收费；不支持本地部署 |

### 1.3 核心洞察

> 开发者需要一个 **本地优先、轻量、美观** 的 LLM 网关桌面应用——它能像本地代理一样运行，零配置即可让 Claude Code 等客户端接入 Anthropic 和 OpenAI 喵～

---

## 2. 竞品分析

### 2.1 功能对比

| 功能 | xrl-router | LiteLLM | OpenRouter | one-api | Portkey |
|------|-----------|---------|------------|---------|---------|
| **本地桌面应用** | ✅ Tauri | ❌ Python 服务 | ❌ SaaS | ❌ Docker 服务 | ❌ SaaS/SDK |
| **统一 Anthropic API 格式** | ✅ 内置 Anthropic/OpenAI | ✅ 支持多种格式 | ✅ 多种格式 | ⚠️ OpenAI 为主 | ✅ 多种格式 |
| **Anthropic → OpenAI 转换** | ✅ 完整（含流式） | ✅ 完整 | ✅ 完整 | ⚠️ 部分 | ✅ 完整 |
| **密钥池 + 健康监控** | ✅ 红绿灯三色 | ✅ 密钥轮换 | ❌ 无（平台管理） | ✅ 多渠道 | ✅ 密钥管理 |
| **模型别名** | ✅ `real<-alias` | ✅ Model Alias | ⚠️ 有限 | ✅ 渠道映射 | ✅ Virtual Keys |
| **GUI 管理面板** | ✅ MD3 桌面 UI | ⚠️ Admin UI | ❌ Dashboard | ⚠️ Web UI | ✅ Dashboard |
| **开源** | ✅ MIT | ✅ MIT | ❌ 闭源 | ✅ MIT | ❌ 部分开源 |

### 2.2 差异化价值

1. **桌面原生**：唯一以桌面应用形态运行的 LLM 网关，开箱即用
2. **Anthropic-first**：下游统一 Anthropic Messages API，内置支持 Anthropic 和 OpenAI 喵～
3. **本地优先**：所有数据存储在本地 SQLite，零数据外泄风险
4. **轻量美观**：Rust 后端 + Vue3 MD3 前端，资源占用低、UI 现代

---

## 3. 产品定位与目标

### 3.1 一句话定位

> **xrl-router** — 运行在桌面上的 LLM API 统一网关，让任何客户端通过一套 API 访问所有大模型。

### 3.2 产品目标

| 目标 | 衡量方式 |
|------|---------|
| **统一接入** | 用户只需配置一次，即可通过单一 Anthropic API 端点访问所有 Provider |
| **零摩擦启动** | 从打开应用到发出第一个 LLM 请求，< 3 分钟 |
| **可靠运行** | 密钥自动容错，单个 Key 失效不影响服务 |
| **透明可观测** | 所有请求的 token 用量、延迟、成功率可追踪 |

### 3.3 非目标（明确不做的事）

- ❌ 不做云端 SaaS / 多租户平台
- ❌ 不做 LLM 模型微调 / 训练管理
- ❌ 不做 Agent 编排 / 工作流引擎
- ❌ 不替代 Claude Code / Cursor 等客户端本身

---

## 4. 用户画像与场景

### 4.1 用户画像

#### 👤 主要用户：AI 开发者

| 属性 | 描述 |
|------|------|
| 技术水平 | 熟悉 API 调用，了解 REST/HTTP 基础概念 |
| 使用频率 | 每天使用，作为日常开发基础设施 |
| 核心诉求 | 一个端点接入所有模型，密钥自动管理，本地运行不依赖网络 |
| 痛点 | 切换 Provider 需要改代码、密钥散落各处、不知道哪个 Key 还能用 |

#### 👤 次要用户：Claude Code / AI IDE 用户

| 属性 | 描述 |
|------|------|
| 技术水平 | 会使用终端，不一定深入了解 API 细节 |
| 使用频率 | 日常编码时持续使用 |
| 核心诉求 | 让 Claude Code 能用非 Anthropic 的模型（如 GPT-4o、DeepSeek） |
| 痛点 | Claude Code 只支持 Anthropic API，想用其他 Provider 但没有代理层 |

### 4.2 用户故事

| ID | 角色 | 故事 | 验收标准 |
|----|------|------|---------|
| US-01 | 开发者 | 作为开发者，我想在桌面应用中添加一个 OpenAI Provider，以便我能通过统一网关调用 GPT-4o | 填写名称、URL、API Key 后，Provider 出现在列表中并可立即使用 |
| US-02 | 开发者 | 作为开发者，我想为同一个 Provider 配置多个 API Key，以便自动轮换避免限流 | 同一 Provider 下可添加多个 Key，系统自动轮询调度 |
| US-03 | Claude Code 用户 | 作为 Claude Code 用户，我想让 Claude Code 调用 GPT-4o，以便降低使用成本 | 配置好 Provider 和别名后，Claude Code 用 `my-alias` 即可路由到 GPT-4o |
| US-04 | 开发者 | 作为开发者，我想查看每个 Key 的健康状态，以便知道哪些还能用 | Key 列表中以红绿灯颜色直观展示状态 |
| US-05 | 开发者 | 作为开发者，我想查看按日统计的 token 用量图表，以便了解消耗趋势 | StatsView 页面展示折线图/柱状图 |
| US-06 | 开发者 | 作为开发者，我想在 Key 失效时自动跳过，以便不影响正常使用 | 401 错误自动标红并切换到下一个可用 Key |
| US-07 | 插件开发者 | 作为插件开发者，我想将非标 API（如 DEAP）桥接为标准 API，以便 xrl-router 用户可以使用 | 插件通过 WebSocket 注册，自动创建委托供应商 |
| US-08 | 用户 | 作为用户，我想在插件启动时自动发现并添加供应商，以便零配置使用 | 插件启动后弹窗确认，密钥自动同步 |

### 4.3 核心使用场景

#### 场景 A：首次配置

```
1. 用户打开 xrl-router 桌面应用
2. 在「供应商」页面点击「添加」
3. 选择 Provider 类型（OpenAI），填写 Endpoint URL 和 API Key
4. 在「密钥」页面创建一个 Service Key
5. 在 Claude Code 中配置 base URL 和 Service Key
6. 开始使用 Claude Code 调用 GPT-4o 🍥
```

#### 场景 B：日常使用

```
1. 用户每天开机后启动 xrl-router（或设为开机自启）
2. 通过 Claude Code / 其他客户端正常使用
3. 偶尔打开管理面板查看用量统计
4. 如果某个 Key 失效，看到红灯后在管理面板替换
```

#### 场景 C：密钥故障恢复

```
1. Provider 返回 401 → 对应 Key 自动标红 🔴
2. 系统自动切换到下一个可用 Key
3. 后续请求透明继续，用户无感知
4. 用户稍后在管理面板查看红灯原因，更新 Key
```

#### 场景 D：插件自动发现（新增）

```
1. 用户启动 xrl-router-plugin-wukong（DEAP 桥接插件）
2. 插件通过 WebSocket 连接到 xrl-router，发送注册信息
3. xrl-router 弹出对话框：「发现插件：悟空穿透」
4. 用户点击「添加供应商」，进入 ProviderNewView
5. 连接信息（API 格式、Base URL、API Key）自动填充且只读
6. 用户可修改供应商名称和模型别名
7. 点击保存后，插件供应商自动激活，密钥每 60s 自动同步
8. 用户可直接在 Claude Code 中使用 DEAP 模型
```

#### 场景 E：插件忽略与重新注册（新增）

```
1. 用户收到插件发现对话框，点击「忽略」
2. 插件记录被删除，关联的 provider 和模型也被清理
3. 插件 WebSocket 连接断开，触发重连机制
4. 插件重连后重新发送注册信息
5. xrl-router 再次弹出对话框，用户可选择添加
```

---

## 5. 功能需求

### 5.1 功能清单（按优先级）

#### P0 — 核心功能（必须有）

| ID | 功能 | 描述 |
|----|------|------|
| F-01 | **Provider 管理** | 增删改查内置 Provider（Anthropic、OpenAI）|
| F-02 | **API Key 管理** | 增删改查 Provider API Key，支持密钥池轮询 |
| F-03 | **Service Key 管理** | 创建/撤销客户端访问凭证 |
| F-04 | **LLM 代理** | 下游 Anthropic Messages API → 上游透传或协议转换（仅支持流式） |
| F-05 | **流式代理** | 流式 SSE 逐 chunk 转发 + 实时转换 |
| F-06 | **模型别名** | 支持 `real_model<-alias` 语法，客户端使用别名 |
| F-07 | **密钥健康监控** | 红绿灯三色状态 + 自动跳过失效 Key |
| F-08 | **桌面应用** | Tauri 2 封装，WebView 加载管理面板 |
| F-09 | **请求超时保护** | 请求头和响应体的独立超时机制，防止死锁和挂起（60s 头超时 + 120s 流超时） |
| F-10 | **密钥轮询指针持久化** | 轮询指针持久化到 settings 表，重启后从上次位置继续，避免从头重试 |

#### P1 — 重要功能（应该有）

| ID | 功能 | 描述 |
|----|------|------|
| F-11 | **用量统计** | 按日聚合的 token 用量和请求数图表 |
| F-12 | **模型注册** | 管理可用模型列表，支持层级分类 |
| F-13 | **Provider 启用/禁用** | 快速切换 Provider 是否参与路由 |
| F-14 | **健康检查** | `/health` 端点，供监控系统探活 |
| F-15 | **缓存追踪** | 自动提取并持久化上游 API 的缓存命中信息（cache_read_input_tokens） |

#### P2 — 锦上添花（可以有）

| ID | 功能 | 描述 |
|----|------|------|
| F-16 | **路由规则** | 基于优先级和权重的多 Provider 负载分发 |
| F-17 | **暗色模式** | 跟随系统自动切换明/暗主题 |
| F-18 | **WebSocket 实时推送** | 管理面板实时获取状态更新 |
| F-19 | **WebSearch 劫持** | 可选拦截含 web_search tool 的请求，用本地 Bing 搜索代替上游 Provider 执行搜索 |
| F-20 | **模型同步** | 从上游 Provider 自动拉取可用模型列表 |
| F-21 | **Dashboard API** | 概览和用量统计端点 |
| F-22 | **系统托盘** | 关闭窗口时最小化到托盘继续运行 |
| F-23 | **插件系统** | 支持外部服务通过 WebSocket 注册为委托供应商，将非标 API 桥接为标准 API |
| F-24 | **委托供应商** | 插件提供的供应商类型，连接信息自动填充且只读，密钥自动同步 |
| F-25 | **插件自动发现** | 插件启动后自动注册，弹出对话框引导用户添加 |
| F-26 | **密钥自动同步** | 插件定期检测密钥变化，通过 WebSocket 自动同步到 Router 密钥池 |
| F-27 | **插件忽略与重注册** | 用户可忽略插件（彻底删除），插件重连后重新注册 |
| F-28 | **插件健康监控** | 心跳检测（30s 间隔），超时 90s 未收到则标记离线 |

### 5.2 协议转换规格

下游统一使用 **Anthropic Messages API** 格式，上游按 Provider 类型处理：

| 上游类型 | 处理方式 | 实现方式 |
|---------|---------|---------|
| Anthropic | 直接透传，零转换 | 内置适配器 |
| OpenAI / Compatible | Anthropic ↔ OpenAI 转换 | 内置适配器 |
| 插件（委托供应商） | 插件将非标 API 桥接为标准 API（OpenAI 或 Anthropic） | 插件负责协议转换 + 业务头注入，Router 只管密钥轮换 |

**必须支持的转换特性**：

- ✅ 文本消息 (text content blocks)
- ✅ 系统提示 (system prompt)
- ✅ 工具调用 (tool_use / tool_calls)
- ✅ 工具结果 (tool_result)
- ✅ 思考过程 (thinking / reasoning_content)
- ✅ 工具选择 (tool_choice)
- ✅ 流式响应 (SSE streaming)

### 5.4 插件系统规格（新增）

xrl-router 支持通过 WebSocket 注册外部服务作为**委托供应商 (Delegated Provider)**。插件的职责是**将非标 API 转化为标准 API**（如 OpenAI Chat Completions 或 Anthropic Messages），Router 负责密钥轮换、健康监控和用量统计。

**插件注册协议**：

```json
// 插件 → Router: WebSocket 消息
{
  "type": "register",
  "plugin_id": "xrl-router-plugin-wukong",
  "provider": {
    "kind": "openai",
    "base_url": "http://localhost:19067",
    "api_path": "/v1/chat/completions"
  },
  "models": [
    {"model_id": "dingtalk-auto", "display_name": "DingTalk Auto", "tier": "custom"},
    {"model_id": "claude-opus-4-8", "display_name": "Claude Opus 4.8", "tier": "opus"}
  ],
  "keys": ["sk-deap-xxx", "sk-deap-yyy"]
}
```

**插件生命周期**：

1. **注册**：插件启动 → WS 连接 `/ws/plugin` → 发送 `register` 消息
2. **确认**：Router 弹出对话框 → 用户确认 → 创建委托供应商（`enabled=true`）
3. **密钥同步**：插件定期检测密钥变化 → 通过 `keys_update` 同步到 Router
4. **心跳**：插件每 30s 发送心跳 → 超时 90s 未收到则标记离线（`enabled=false`）
5. **忽略**：用户点击「忽略」→ 彻底删除插件 + 关联 provider + 模型 → WS 断开 → 插件重连后重新注册

**委托供应商 vs 普通供应商**：

| 维度 | 普通供应商 | 委托供应商（插件） |
|------|-----------|------------------|
| API 格式 | 用户选择 (OpenAI/Anthropic) | 插件提供（可转为 OpenAI 或 Anthropic） |
| Base URL | 用户填写 | 插件通过 WS 推送 |
| API Key | 用户手动填入 | 插件自动同步 |
| 密钥轮换 | Router KeyPool | Router KeyPool（完全一致） |
| 连接状态 | N/A | 必须 WS 在线才能消费 |

### 5.3 密钥池规格

| 状态 | 颜色 | 触发条件 | 行为 |
|------|------|---------|------|
| 正常 | 🟢 绿 | 初始状态 / 请求成功 | 正常使用 |
| 低配额 | 🟡 黄 | HTTP 402 / 429 / 5xx | 暂时跳过，等待恢复 |
| 失效 | 🔴 红 | HTTP 401 | 永久跳过，需人工处理 |

**轮询机制**：
- 健康状态为纯内存管理（启动时所有 Key 均为绿色初始状态），DB 中的 `status` 列不再被读写
- **轮询指针持久化**：每次成功使用某个 Key 后，将下一个轮询位置持久化到 `settings` 表（键名 `keypool_index_{provider_id}`）
- 重启后从上次成功使用的 Key 位置继续，而非每次都从 0 开始重试
- 指针越界或无效时自动回退到 0

---

## 6. 非功能需求

### 6.1 性能

| 指标 | 目标 |
|------|------|
| 启动时间 | 应用启动到 HTTP 服务就绪 ≤ 3 秒 |
| 代理延迟（透传） | 额外延迟 ≤ 5ms（仅网络转发开销） |
| 代理延迟（协议转换） | 额外延迟 ≤ 20ms（JSON 解析 + 转换） |
| 内存占用 | 空闲状态 ≤ 100MB |
| 并发支持 | 单实例支持 ≤ 50 并发请求 |
| 请求头超时 | 60 秒（防止上游不返回响应头导致死锁） |
| 流式响应超时 | 120 秒（防止上游断流但不关闭连接导致永久挂起） |

### 6.2 可靠性

| 指标 | 目标 |
|------|------|
| 单 Key 故障恢复 | 自动切换到下一个可用 Key，客户端无感知 |
| 数据持久性 | SQLite 事务提交后立即持久化 |
| 崩溃恢复 | 重启后数据库完整，历史数据不丢失；轮询指针从上次位置继续 |
| 死锁防护 | `std::sync::Mutex` 锁的获取和释放必须在同一作用域内完成 |

### 6.3 安全性

| 要求 | 说明 |
|------|------|
| Service Key 存储 | **Argon2** 哈希（随机 salt + PHC 格式），不可明文或简单哈希 |
| Provider Key 存储 | **AES-256-GCM 加密存储**（主密钥 `data/master.key`，自动生成，权限 0600），运行时解密使用 ✅ 已实现 |
| 管理 API 访问控制 | 绑定 `127.0.0.1`，仅本机可访问 ✅ 已实现 |
| CORS 策略 | origin 白名单机制，默认含 `localhost:5173`、`localhost:19068`、`tauri://localhost`、`https://tauri.localhost` ✅ 已实现 |
| 请求频率限制 | 按 Service Key 限流（令牌桶，默认 60 req/min） ✅ 已实现 |

### 6.4 兼容性

| 维度 | 要求 |
|------|------|
| Anthropic API 版本 | 兼容 `2023-06-01` 及以上 |
| OpenAI API 版本 | 兼容 Chat Completions v1 |
| 操作系统 | macOS (primary)、Windows、Linux |
| Claude Code | 兼容最新版 Claude Code CLI |

### 6.5 可观测性

| 要求 | 说明 |
|------|------|
| 结构化日志 | JSON 格式（tracing + JSON 输出），包含 trace_id、provider、model、latency ✅ 已实现 |
| 请求审计 | 所有 LLM 请求记录到 usage_log 表 |
| 健康检查 | `/health` 端点返回系统状态摘要 |

---

## 7. 成功指标

### 7.1 北极星指标

> **成功发出首个代理请求的时间（Time to First Request）** ≤ 3 分钟

### 7.2 功能指标

| 指标 | 目标 |
|------|------|
| Provider 接入成功率 | ≥ 99%（有效 Key 的情况下） |
| 协议转换正确率 | 100%（无数据丢失或格式错误） |
| Key 故障自动切换成功率 | 100%（有备用 Key 时） |
| 管理面板核心流程完成率 | 100%（添加 Provider → 创建 Key → 调用成功） |

### 7.3 质量指标

| 指标 | 目标 |
|------|------|
| E2E 测试覆盖率 | 核心流程 100% 覆盖 |
| 崩溃率 | ≤ 0.1% |
| 协议转换额外延迟 | P99 ≤ 20ms |

---

## 8. 版本规划与路线图

### v0.1 — MVP 🍥

**目标**：能用的最小可用版本

- [x] Tauri 2 桌面应用
- [x] Provider CRUD + 管理面板
- [x] API Key CRUD + 密钥池
- [x] Anthropic Messages API 代理（透传 + OpenAI 转换）
- [x] 流式 SSE 代理
- [x] 模型别名
- [x] Service Key 认证
- [x] Vue3 + MD3 基础 UI

### v0.2 — 完善 🐾

**目标**：好用、可靠

- [x] Service Key 哈希存储 — 实际采用 **Argon2**（随机 salt + PHC 格式），替代明文存储
- [x] 管理 API 绑定 `127.0.0.1`（认证层待 v0.3）
- [x] CORS 策略收紧 — 已实现 origin 白名单（默认含 localhost:5173/19068 + tauri://localhost + https://tauri.localhost）
- [x] SQLite WAL 模式
- [x] 请求频率限制（令牌桶 60 req/min，按 Service Key）
- [x] 统一错误响应格式（CRUD: `{error:{code,message}}`，proxy: Anthropic 风格 `{error:{type,message}}`）
- [x] 前端离线状态处理 + 错误边界（ConnectionStatus + errorHandler）
- [x] 结构化日志（tracing + JSON 输出，含 trace_id/provider/model/latency）
- [x] 协议转换不兼容性文档化
- [x] 废弃代码清理（webauthn 前后端彻底删除）
- [x] 缓存追踪（cache_read_input_tokens）— 自动提取并持久化上游 API 的缓存命中信息
- [x] 请求超时保护（60s 请求头超时 + 120s 流超时）— 防止死锁和挂起
- [x] 密钥轮询指针持久化 — 轮询指针持久化到 settings 表，重启后从上次位置继续

### v0.3 — 进阶

**目标**：强大、智能

- [x] Provider API Key AES-256-GCM 加密存储 — 已实现（`crypto/mod.rs` + `master.key`，提前至 v0.2 完成）
- [x] WebSocket 实时状态推送 — 已实现（后端 `/ws` 端点 + broadcast channel + 前端 `ws.ts` 自动重连，提前至 v0.2 完成）
- [ ] 管理 API 认证层（Basic Auth / Session Token）
- [ ] CORS 路由级白名单（管理 API 严格、公开 API 宽松）
- [ ] 路由规则引擎（优先级 + 权重负载分发）
- [ ] 指数退避重试机制
- [ ] Prometheus Metrics 导出
- [ ] 更多 Provider 支持（DeepSeek、Gemini）

### v1.0 — 正式版

**目标**：生产就绪

- [ ] 全面安全审计
- [ ] 性能基准测试 + 优化
- [ ] 完整 API 文档
- [ ] 用户引导 / Onboarding 流程
- [ ] 多语言支持（中/英文 UI）
- [ ] 自动更新机制

---

## 9. 风险与缓解

| 风险 | 概率 | 影响 | 缓解策略 |
|------|------|------|---------|
| 上游 API 格式变更导致转换失败 | 中 | 高 | 版本锁定 + 兼容性测试套件 |
| SQLite 在高并发下成为瓶颈 | 低 | 中 | WAL 模式 + 异步批量写入 |
| Tauri WebView 兼容性差异 | 中 | 中 | 三平台 CI 测试 |
| 密钥泄露 | 低 | 极高 | Argon2 哈希 + AES-256-GCM 加密存储 + 最小权限 |
| 协议转换丢失特性（如 thinking） | 中 | 中 | 不兼容特征显式报错，不静默丢弃 |
| 上游挂起导致网关卡死 | 低 | 高 | 独立超时保护（60s 头超时 + 120s 流超时） |
| 社区参与度低 | 高 | 低 | 保持文档质量，降低贡献门槛 |

---

## 10. 开放问题

以下问题需要后续讨论决策：

| # | 问题 | 影响范围 | 当前状态 |
|---|------|---------|---------|
| Q1 | 是否支持多实例部署？当前为单实例 SQLite，无法横向扩展 | 可扩展性 | 暂不支持，单用户桌面场景 |
| Q2 | 管理面板是否需要认证？当前无认证，绑定 localhost 是否足够 | 安全性 | v0.2 已实施 localhost 绑定；认证层推迟到 v0.3 |
| Q3 | 是否需要支持 Anthropic SDK 自动更新？ | 兼容性 | 手动版本锁定 |
| Q4 | `routes` 表是否需要在 v0.2 实现？ | 功能完整性 | 推迟到 v0.3 |
| Q5 | 是否提供 CLI 模式（无 GUI）？ | 部署灵活性 | 待评估 |
