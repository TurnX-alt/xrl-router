# Spec: 局域网快速部署 Claude Code（install 页面 + 分发链接）

## 目标

让主人把本机作为局域网 API 网关：局域网设备打开一个 install 页面，复制一行命令到终端，
即可安装 Claude Code CLI 并把 `~/.claude/settings.json` 指向本机网关。

## 技术约束

浏览器沙箱无法直接安装 CLI、无法改写客户端 `~/.claude/settings.json`。最接近"自动"的形态
是 install 页面生成单行安装命令，用户复制粘贴到终端执行一次（装 CLI + 写 settings.json）。

## 架构：双 listener 分离监听

```
┌──────────────────── xrl-router (axum) ────────────────────┐
│  admin listener  127.0.0.1:19068                           │
│    /api/*  /health  /ws  /ws/plugin  /v1/*  /install/local-ip │  ← Tauri UI + 本机既有客户端
│  public listener 0.0.0.0:19069                             │
│    /v1/*（代理，套 rate_limit）  /install（静态页）         │  ← 局域网设备访问
└────────────────────────────────────────────────────────────┘
```

**职责分工**:
- **admin**：管理/密钥 CRUD + `/v1/*` 代理（**必须保留**——拆双端口前 `/v1/*` 就在
  19068 上，本机既有客户端如 CC Switch 直连 19068 取模型列表/余额，拆走会 404 而坏）。
- **public**：`/v1/*` 代理（需 service key 鉴权）+ `/install` 静态页（无 key 也可访问，
  但无 key 时只显示"请获取链接"提示）。CORS 全开（CLI 无 origin 约束、install 同源）。

`/v1/*` 由共享 `proxy_routes(state)` 构建（套 `rate_limit_middleware`），admin 与 public
各自 merge——注意返回 `Router<AppState>`（未 with_state），由调用方 `.with_state` 收敛。

## 配置

`Config`（`src-tauri/src/config.rs`）新增字段，环境变量覆盖：

| 字段 | 默认值 | 环境变量 |
|---|---|---|
| `public_host` | `0.0.0.0` | `PUBLIC_HOST` |
| `public_port` | `19069` | `PUBLIC_PORT` |
| `enable_public` | `true` | `ENABLE_PUBLIC`（1/true） |

`addr()` → 管理 listener 地址；`public_addr()` → 公共 listener 地址。

## 路由

`build_admin_router(state)`（`api/router.rs`）：原 `build_router` 全部 `/api/*` +
`/health` + `/ws` + `/ws/plugin` + `GET /api/install/local-ip`。

`build_public_router(state)`：`/v1/chat/completions`、`/v1/messages`、`/v1/models`、
`/v1/user/balance`（套 `rate_limit_middleware`）+ `GET /install`。

`build_router` 保留为 admin 别名（兼容 `lib.rs` 与冒烟测试）。

## install 页面契约 — `src-tauri/assets/install.html`

编译进二进制（`include_str!`），`serve_install_page` 返回 `Html<&'static str>`。

**输入**：URL query `?t=<明文 service key>`。无 `t` → 显示"请从密钥管理页获取分发链接"。

**语言**：页面内联双语字典（zh/en，纯静态自包含，无构建依赖），标题「客户分发 / Client Deploy」，右上角固定语言切换按钮。语言优先级：URL `?lang=` 参数 > localStorage `install-lang` > `navigator.language`（en 前缀 → English，否则中文）。切换语言时重渲染并重新拉取模型列表。

**拉取模型**：页面打开时用 `t` 调 `GET /v1/models`（`x-api-key: <t>`，同源 19069 无 CORS 问题），
取可用别名列表（`data[].id` = display_name）。默认选中第一项（后端按 tier 排序，通常为主模型）。
拉取失败则提示可忽略，但 Claude Code 会用官方模型名请求而 404。

**输出**：按平台（Windows PowerShell / macOS·Linux Bash）生成单行命令。命令由两段组成：
- A 段：`npm i -g @anthropic-ai/claude-code`（装 CLI）
- B 段：写 `~/.claude/settings.json`，合并 `env` 块与顶层配置（**保留既有字段**）：

```jsonc
{
  "env": {
    "ANTHROPIC_AUTH_TOKEN": "<key>",
    "ANTHROPIC_BASE_URL": "<页面 origin>",
    // 以下 4 槽位：_MODEL 与 _MODEL_NAME 统一设为网关别名（= display_name，用户在下拉选定）。
    // 不写官方 ID——Claude Code 无论发 _MODEL 还是 _MODEL_NAME 的值，都能命中网关别名，
    // 避免官方 ID 在网关模型表里查不到而 404。
    "ANTHROPIC_DEFAULT_FABLE_MODEL": "<别名>",
    "ANTHROPIC_DEFAULT_FABLE_MODEL_NAME": "<别名>",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL": "<别名>",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME": "<别名>",
    "ANTHROPIC_DEFAULT_OPUS_MODEL": "<别名>",
    "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME": "<别名>",
    "ANTHROPIC_DEFAULT_SONNET_MODEL": "<别名>",
    "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME": "<别名>",
    "CLAUDE_CODE_SUBAGENT_MODEL": "<别名>"   // 后台子代理任务
  },
  "permissions": { "defaultMode": "bypassPermissions" },
  "skipDangerousModePermissionPrompt": true,
  "skipWebFetchPreflight": true
}
```

未选中别名（拉取失败/网关无模型）→ 省略全部 `_MODEL_NAME`/`SUPAGENT` 行，只写 base+token。

**勾选框**（实时重算命令长度）:
- ☐ 我已安装 Claude Code CLI → 勾选省略 A 段（仅 B 段）

> 注：不设「我已安装 Node.js」勾选——装 CLI 仍需 node，该勾选不影响命令，无意义已移除。

平台可手动切换（防 UA 误判）。复制按钮用 `navigator.clipboard`。

`ANTHROPIC_BASE_URL` 只填到端口（`http://<IP>:19069`），不带 `/v1/messages`（Claude Code 自拼）。

**为何 `_MODEL` 也用网关别名**：Claude Code 把 `_MODEL` 的值作为 API 请求 model 字段，
网关 `resolve_route` 按 `models.display_name` 查找。若 `_MODEL` 写官方 ID（如
`claude-sonnet-4-6`）而网关无此别名 → 404。故 `_MODEL`/`_MODEL_NAME`/`CLAUDE_CODE_SUBAGENT_MODEL`
全部用网关别名（install 页面从 `GET /v1/models` 拉取 `data[].id` 让用户选），
无论 Claude Code 发哪个变量的值都命中网关。

**引号约定**：PS 用单引号字符串（key/base/别名不含单引号）；bash 里整个脚本包在
`node -e "..."` 双引号中，值一律用**单引号**包裹（双引号值会截断外层引号）。

## 分发链接 — 密钥管理页

`KeysView.vue`「密钥已创建」dialog（创建 key 后弹出，明文仅此一次）:
- 内容区：明文密钥框 + 分发链接框（`http://<本机IP>:19069/install?t=<明文key>`）。
- actions：「复制」+「复制分发链接」+「完成」。
- 本机 IP 由 `GET /api/install/local-ip` 返回（UDP socket 连 8.8.8.8:80 取出口 IP）。
  `deployLink` computed 依赖 `newKeyPlain` + `localIp`，`watch(newKeyPlain)` 异步取 IP。

分发 key 即普通长期 service key（argon2 哈希，明文仅创建时返回），撤销就在密钥列表删除。
**系统不自动删除/提示删除**——删不删由主人手动决定。

## 安全模型

- 公共端口 `0.0.0.0:19069`：局域网任何人可调 `/v1/*`（需有效 key）、可看 `/install`
  （无 key 仅显示提示）。
- 管理 `/api/*` 仍绑 127.0.0.1，局域网无法访问。
- 明文 key 在 install URL query：局域网嗅探可见，主人复制给特定用户，撤销即删。

## 鉴权

install 页面不调任何后端（key 在 URL 里）。`/v1/*` 复用现有 `verify_service_key`
（`api/proxy/auth.rs`）：`/v1/messages` 优先读 `x-api-key` 回退 `Authorization: Bearer`；
Claude Code 用 `ANTHROPIC_AUTH_TOKEN` 走 `Authorization: Bearer`，网关已兼容。

## 完成条件

1. `cargo build` 通过；`vue-tsc --noEmit` 通过。
2. 本机 `curl 127.0.0.1:19068/health` → 200；局域网 `curl <本机IP>:19068/health` → 拒绝。
3. 局域网浏览器开 `http://<本机IP>:19069/install`（无 t）→ 显示提示页。
4. 密钥管理页创建密钥 → dialog 显示明文 key + 分发链接 → 复制分发链接。
5. 局域网设备打开分发链接 → 看到平台命令 → 终端运行 → `settings.json` 含
   `env.ANTHROPIC_BASE_URL`/`ANTHROPIC_AUTH_TOKEN` 且其他字段保留。
6. 该设备 `claude` 发消息 → 网关统计页见流量（`/v1/messages` 命中、key 鉴权通过）。
7. 密钥列表删除该 key → 该设备再用同 key → 401。
