# Spec: 协议转换

## 目标

实现 Anthropic Messages API ↔ OpenAI Chat Completions API 的双向协议转换。

## 转换方向

### Anthropic → OpenAI

客户端使用 Anthropic 格式，上游是 OpenAI API。

**请求转换**:

| Anthropic 字段 | OpenAI 字段 | 说明 |
|---|---|---|
| `messages[].content` | `messages[].content` | 内容块数组转字符串 |
| `system` (顶层) | `messages[0].role=system` | 系统提示 |
| `tools[].input_schema` | `tools[].function.parameters` | 工具定义 |
| `tool_choice: "auto"` | `tool_choice: "auto"` | 直接映射 |
| `tool_choice: "any"` | `tool_choice: "required"` | 强制调用 |
| `max_tokens` | `max_tokens` | 直接映射 |

**响应转换**:

| OpenAI 字段 | Anthropic 字段 | 说明 |
|---|---|---|
| `choices[].message.content` | `content[].type=text` | 文本内容 |
| `choices[].message.tool_calls` | `content[].type=tool_use` | 工具调用 |
| `choices[].finish_reason` | `stop_reason` | 结束原因 |
| `usage.prompt_tokens` | `usage.input_tokens` | 输入 token |
| `usage.completion_tokens` | `usage.output_tokens` | 输出 token |

### OpenAI → Anthropic

客户端使用 OpenAI 格式，上游是 Anthropic API。

**请求转换**:

| OpenAI 字段 | Anthropic 字段 | 说明 |
|---|---|---|
| `messages[].content` | `messages[].content` | 字符串转内容块 |
| `messages[0].role=system` | `system` (顶层) | 系统提示 |
| `tools[].function.parameters` | `tools[].input_schema` | 工具定义 |
| `tool_choice: "auto"` | `tool_choice: "auto"` | 直接映射 |
| `tool_choice: "required"` | `tool_choice: "any"` | 强制调用 |

**响应转换**:

| Anthropic 字段 | OpenAI 字段 | 说明 |
|---|---|---|
| `content[].type=text` | `choices[].message.content` | 文本内容 |
| `content[].type=tool_use` | `choices[].message.tool_calls` | 工具调用 |
| `stop_reason` | `choices[].finish_reason` | 结束原因 |
| `usage.input_tokens` | `usage.prompt_tokens` | 输入 token |
| `usage.output_tokens` | `usage.completion_tokens` | 输出 token |

## 输入契约

### Anthropic 请求

```json
{
  "model": "claude-opus-4-8",
  "messages": [
    {"role": "user", "content": "Hello"}
  ],
  "max_tokens": 4096,
  "system": "You are a helpful assistant",
  "tools": [
    {
      "name": "get_weather",
      "description": "Get weather info",
      "input_schema": {
        "type": "object",
        "properties": {
          "location": {"type": "string"}
        },
        "required": ["location"]
      }
    }
  ],
  "tool_choice": "auto"
}
```

### OpenAI 请求

```json
{
  "model": "gpt-4o",
  "messages": [
    {"role": "system", "content": "You are a helpful assistant"},
    {"role": "user", "content": "Hello"}
  ],
  "max_tokens": 4096,
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "get_weather",
        "description": "Get weather info",
        "parameters": {
          "type": "object",
          "properties": {
            "location": {"type": "string"}
          },
          "required": ["location"]
        }
      }
    }
  ],
  "tool_choice": "auto"
}
```

## 输出契约

### Anthropic 响应（流式）

```
event: message_start
data: {"type":"message_start","message":{"id":"msg_xxx","type":"message","role":"assistant","content":[],"model":"claude-opus-4-8","stop_reason":null,"usage":{"input_tokens":25,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":15}}

event: message_stop
data: {"type":"message_stop"}
```

### OpenAI 响应（流式）

```
data: {"id":"chatcmpl-xxx","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}

data: {"id":"chatcmpl-xxx","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4o","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}

data: {"id":"chatcmpl-xxx","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4o","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}

data: [DONE]
```

## 关键约束

1. **流式转换**: 逐 chunk 实时转换，不缓冲完整响应
2. **保留未知字段**: 不删除无法识别的字段，原样传递
3. **错误容忍**: 单个 chunk 转换失败不影响整个流
4. **token 统计**: 转换过程中累计 token 使用量
5. **thinking 字段**: thinking/reasoning_content 双向转换，内容原样传递（无截断）

## 错误处理

| 场景 | 行为 |
|------|------|
| 请求格式错误 | 返回 400，不转发 |
| 响应格式错误 | 跳过该 chunk，继续处理 |
| 未知字段 | 原样传递 |
| 转换失败 | 记录 warn 日志，继续处理 |

## 实现位置

- `src-tauri/src/api/proxy/translate/mod.rs` - 转换入口
- `src-tauri/src/api/proxy/translate/common.rs` - 公共类型
- `src-tauri/src/api/proxy/translate/to_openai.rs` - Anthropic → OpenAI
- `src-tauri/src/api/proxy/translate/to_anthropic.rs` - OpenAI → Anthropic

## 测试要求

1. **单元测试**: 每个转换函数的输入输出
2. **集成测试**: 完整请求-响应流程
3. **边界测试**: 空消息、多工具调用、thinking 双向转换
4. **流式测试**: 逐 chunk 转换的正确性

## 完成标准

- [x] Anthropic → OpenAI 请求转换
- [x] Anthropic → OpenAI 响应转换（流式）
- [x] OpenAI → Anthropic 请求转换
- [x] OpenAI → Anthropic 响应转换（流式）
- [x] 工具调用转换（tools + tool_choice）
- [x] thinking 字段处理（双向转换，原样传递）
- [x] token 统计累计
- [x] 通过所有单元测试
