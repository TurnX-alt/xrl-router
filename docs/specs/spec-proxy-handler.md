# Spec: LLM 代理处理器

## 目标

实现 `/v1/messages` 和 `/v1/chat/completions` 的代理转发，支持密钥轮换、协议转换、流式响应。

## 输入契约

### POST /v1/messages

```json
{
  "model": "claude-opus-4-8",
  "messages": [{"role": "user", "content": "Hello"}],
  "max_tokens": 4096,
  "stream": true
}
```

**必需头**:
- `x-api-key: sk-xxx` 或 `Authorization: Bearer sk-xxx`
- `Content-Type: application/json`

### POST /v1/chat/completions

```json
{
  "model": "gpt-4o",
  "messages": [{"role": "user", "content": "Hello"}],
  "stream": true
}
```

## 输出契约

### 成功响应（流式）

**Content-Type**: `text/event-stream`

**Anthropic 格式**:
```
data: {"type":"message_start","message":{"id":"msg_xxx",...}}

data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}

data: {"type":"message_stop"}
```

**OpenAI 格式**:
```
data: {"id":"chatcmpl-xxx","choices":[{"delta":{"content":"Hello"}}]}

data: [DONE]
```

### 错误响应

```json
{
  "error": {
    "type": "authentication_error",
    "message": "Invalid API key"
  }
}
```

**状态码**:
- `400` 请求格式错误（含模型不存在）
- `401` API key 无效
- `403` 模型不在白名单
- `429` 速率限制
- `500` 内部错误
- `502` 上游 API 错误
- `503` 无可用密钥

## 关键约束

1. **强制 stream=true**: 即使客户端发送 `stream=false`，也会被静默覆写为 `true` 后继续处理（不返回 400）
2. **模型替换**: 将 `display_name` 替换为上游的 `model_id`
3. **密钥轮换**: 401/403 标红，402/429 标黄，自动切换下一个 key
4. **超时控制**: 连接 10s，响应 60s，流间隔 120s
5. **重试上限**: 最多重试 `key_count` 次，防止死循环

## 错误处理

| 场景 | 行为 |
|------|------|
| API key 无效 | 返回 401，不重试 |
| 模型不存在 | 返回 400，不重试 |
| 上游 401/403 | 标红当前 key，切换下一个，重试 |
| 上游 402/429 | 标黄当前 key，切换下一个，重试 |
| 上游 5xx | 不切换 key，直接返回错误 |
| 连接超时 | 返回 503，不重试 |
| 响应超时 | 返回 504，不重试 |
| 所有 key 都不可用 | 返回 503 |

## 实现位置

- `src-tauri/src/api/proxy/handler.rs` - 主处理逻辑
- `src-tauri/src/api/proxy/auth.rs` - 认证
- `src-tauri/src/api/proxy/route.rs` - 路由解析
- `src-tauri/src/api/proxy/key_rotation.rs` - 密钥轮换
- `src-tauri/src/api/proxy/translate/` - 协议转换

## 测试要求

1. **单元测试**: 认证、路由解析、密钥轮换逻辑
2. **集成测试**: 模拟上游 API，测试完整流程
3. **边界测试**: 所有 key 都 Red、上游超时、协议转换错误

## 完成标准

- [x] 支持 `/v1/messages` 和 `/v1/chat/completions`
- [x] 强制流式响应
- [x] 密钥轮换（Red/Yellow/Green）
- [x] 协议转换（Anthropic ↔ OpenAI）
- [x] 超时控制
- [x] 错误处理
- [x] 记录 `usage_log`
- [x] 通过网关冒烟测试
