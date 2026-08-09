//! IR → OpenAI Chat Completions 方向的格式翻译：请求体与流式事件渲染。
//!
//! Chat Completions 的 SSE 格式与 Anthropic 差异较大：
//! - 没有 content_block_start/stop，所有 delta 在 choices[0].delta 中
//! - tool_calls 有独立的 index 和 id
//! - finish_reason 在最后一个 chunk 的 choices[0] 中
//! - usage 在最后一个 chunk 的顶层（需 stream_options.include_usage）

use bytes::Bytes;
use serde_json::{json, Value};

use super::types::*;

/// 将 IR 请求体序列化为 OpenAI Chat Completions 格式。
pub fn ir_req_to_chat_completions(req: &IrRequest) -> Value {
    let mut out = json!({
        "model": req.model,
        "stream": req.stream,
    });

    // 构建 messages 数组
    let mut messages: Vec<Value> = vec![];

    // 收集 messages 中出现的 tool_calls 名称（LiteLLM 要求 tools 定义必须
    // 覆盖 tool_calls 历史，否则返回 400 "Can only get item pairs from a mapping"）
    let mut tool_names_in_messages: std::collections::HashSet<String> = std::collections::HashSet::new();

    // System prompt → messages[0]
    if let Some(ref system) = req.system {
        let system_content = match system {
            IrSystemContent::Text(t) => t.clone(),
            IrSystemContent::Blocks(blocks) => blocks
                .iter()
                .map(|b| b.text.clone())
                .collect::<Vec<_>>()
                .join("\n"),
        };
        messages.push(json!({"role": "system", "content": system_content}));
    }

    // Messages
    for msg in &req.messages {
        let role = match msg.role {
            IrRole::User => "user",
            IrRole::Assistant => "assistant",
        };

        let mut msg_obj = json!({"role": role});
        let mut text_parts: Vec<String> = vec![];
        let mut tool_calls: Vec<Value> = vec![];
        // 是否渲染过任何内容（文本/图像/工具调用）——纯 tool_result 的消息
        // 不应产生空 user 消息（OpenAI 对 content:"" 的 user 消息会 400）
        let mut rendered_any = false;
        // reasoning_content 默认不输出（历史上下文中的 thinking 块在此处被丢弃）

        for block in &msg.content {
            match block {
                IrContentBlock::Text { text, .. } => {
                    text_parts.push(text.clone());
                    rendered_any = true;
                }
                IrContentBlock::Thinking { .. } => {
                    // thinking 块被丢弃，不转换为 reasoning_content
                }
                IrContentBlock::ToolUse { id, name, input } => {
                    tool_names_in_messages.insert(name.clone());
                    rendered_any = true;
                    tool_calls.push(json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string())
                        }
                    }));
                }
                IrContentBlock::ToolResult { tool_use_id, content, is_error } => {
                    // ToolResult 作为独立的 tool role 消息
                    let result_text = match content {
                        IrToolResultContent::Text(t) => t.clone(),
                        IrToolResultContent::Blocks(blocks) => blocks
                            .iter()
                            .filter_map(|b| {
                                if let IrContentBlock::Text { text, .. } = b {
                                    Some(text.clone())
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    };
                    let mut tool_msg = json!({
                        "role": "tool",
                        "tool_call_id": tool_use_id,
                        "content": result_text
                    });
                    // 工具失败标记透传（DeepSeek 等上游支持 is_error）
                    if *is_error {
                        tool_msg["is_error"] = json!(true);
                    }
                    messages.push(tool_msg);
                }
                IrContentBlock::Image { source } => {
                    // Image → content parts
                    let image_url = match source {
                        IrImageSource::Url { url } => url.clone(),
                        IrImageSource::Base64 { media_type, data } => {
                            format!("data:{};base64,{}", media_type, data)
                        }
                    };
                    rendered_any = true;
                    // 如果有文本，先添加文本
                    if !text_parts.is_empty() {
                        let text = text_parts.join("\n");
                        msg_obj["content"] = json!([
                            {"type": "text", "text": text},
                            {"type": "image_url", "image_url": {"url": image_url}}
                        ]);
                        text_parts.clear();
                    } else {
                        msg_obj["content"] = json!([
                            {"type": "image_url", "image_url": {"url": image_url}}
                        ]);
                    }
                }
            }
        }

        // 设置 content
        if !text_parts.is_empty() {
            let text = text_parts.join("\n");
            if msg_obj.get("content").is_none() {
                msg_obj["content"] = json!(text);
            }
        } else if msg_obj.get("content").is_none() && tool_calls.is_empty() {
            msg_obj["content"] = json!("");
        }

        // reasoning_content 默认不输出（历史上下文中的 thinking 块在此处被丢弃）
        // if let Some(reasoning) = reasoning_content {
        //     msg_obj["reasoning_content"] = json!(reasoning);
        // }

        // 设置 tool_calls
        if !tool_calls.is_empty() {
            msg_obj["tool_calls"] = json!(tool_calls);
            if msg_obj.get("content").is_none() {
                // OpenAI SDK 要求 assistant 消息 content 可为空字符串（null 会触发
                // 部分上游 400；SDK 解析 tool_calls-only 消息时 content=None 兼容）
                msg_obj["content"] = json!("");
            }
        }

        if rendered_any || !tool_calls.is_empty() {
            messages.push(msg_obj);
        }
    }

    out["messages"] = json!(messages);

    // Tools
    // LiteLLM 要求 tools 定义覆盖 messages 中出现的所有 tool_calls：
    // 即使请求未提供 tools 定义，也要从 tool_calls 历史提取名称生成最小定义，
    // 否则 LiteLLM 返回 400 "Can only get item pairs from a mapping"。
    let mut tool_definitions: Vec<Value> = Vec::new();
    if !req.tools.is_empty() {
        tool_definitions = req
            .tools
            .iter()
            .map(|t| {
                let mut obj = json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "parameters": t.input_schema,
                    }
                });
                if let Some(ref desc) = t.description {
                    obj["function"]["description"] = json!(desc);
                }
                obj
            })
            .collect();
    }

    // 为 tool_calls 历史中未定义的工具生成最小定义
    if !tool_names_in_messages.is_empty() {
        let defined_tool_names: std::collections::HashSet<String> = tool_definitions
            .iter()
            .filter_map(|t| {
                t.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .map(|s| s.to_string())
            })
            .collect();

        for tool_name in &tool_names_in_messages {
            if !defined_tool_names.contains(tool_name) {
                tool_definitions.push(json!({
                    "type": "function",
                    "function": {
                        "name": tool_name,
                        "description": "",
                        "parameters": {"type": "object", "properties": {}},
                    }
                }));
            }
        }
    }

    // 仅当有工具定义时才输出 tools 字段
    if !tool_definitions.is_empty() {
        out["tools"] = json!(tool_definitions);
    }

    // Tool choice
    if let Some(ref tc) = req.tool_choice {
        out["tool_choice"] = match tc {
            IrToolChoice::Auto => json!("auto"),
            IrToolChoice::Any => json!("required"),
            IrToolChoice::None => json!("none"),
            IrToolChoice::Tool { name } => {
                json!({"type": "function", "function": {"name": name}})
            }
        };
    }

    // Thinking config → reasoning_effort
    if let Some(ref thinking) = req.thinking {
        if thinking.enabled {
            let effort = match thinking.budget_tokens {
                Some(b) if b <= 2048 => "low",
                Some(b) if b <= 8192 => "medium",
                _ => "high",
            };
            out["reasoning_effort"] = json!(effort);
        }
    }

    // Pass through scalar params
    if let Some(max_tokens) = req.max_tokens {
        // o1/o3 等 reasoning 模型只接受 max_completion_tokens，传 max_tokens 会 400
        if is_reasoning_model(&req.model) {
            out["max_completion_tokens"] = json!(max_tokens);
        } else {
            out["max_tokens"] = json!(max_tokens);
        }
    }
    if let Some(temperature) = req.temperature {
        out["temperature"] = json!(temperature);
    }
    if let Some(top_p) = req.top_p {
        out["top_p"] = json!(top_p);
    }

    out
}

/// o1/o3 系列（reasoning 模型）只接受 `max_completion_tokens`。
/// 前缀匹配覆盖 o1、o1-mini、o3、o3-mini、o4-mini 等。
fn is_reasoning_model(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    ["o1", "o3", "o4"].iter().any(|p| {
        m == *p || m.starts_with(&format!("{}-", p)) || m.starts_with(&format!("{}/", p))
    })
}

// ═══════════════════════════════════════════════════════════════════
// 流式事件渲染
// ═══════════════════════════════════════════════════════════════════

/// Chat Completions SSE 渲染状态机。
///
/// Chat 的 SSE 格式特点：
/// - 每个 chunk 都有完整的 id/model/choices 结构
/// - delta 中只有变化的字段
/// - tool_calls 有独立的 index
/// - finish_reason 在最后一个 chunk
pub struct ChatCompletionsRenderState {
    msg_id: String,
    model: String,
    created: i64,
    /// 已发送的 tool_call 数量
    tool_count: usize,
    /// 是否已发送第一个 chunk（需要包含 role）
    first_chunk_sent: bool,
    /// 是否已发送 [DONE]（MessageStop 或 finalize，只发一次）
    done_sent: bool,
    /// IR block index → OpenAI tool_call index 映射（参数续传路由用）
    tool_index_map: Vec<usize>,
}

impl ChatCompletionsRenderState {
    pub fn new() -> Self {
        Self {
            msg_id: String::new(),
            model: String::new(),
            created: chrono::Utc::now().timestamp(),
            tool_count: 0,
            first_chunk_sent: false,
            done_sent: false,
            tool_index_map: vec![],
        }
    }

    /// 将 IR 流式事件渲染为 Chat Completions SSE 字节段。
    ///
    /// 返回 `None` 表示该事件不产生输出（如 ContentBlockStart/Stop）。
    pub fn render_event(&mut self, ev: &IrStreamEvent) -> Option<Bytes> {
        let mk = |payload: Value| -> Bytes {
            let data = serde_json::to_string(&payload).unwrap_or_default();
            Bytes::from(format!("data: {}\n\n", data))
        };

        match ev {
            IrStreamEvent::MessageStart { id, model, .. } => {
                self.msg_id = id.clone();
                self.model = model.clone();
                // Chat 不需要单独的 start 事件，第一个 delta 会包含 role
                None
            }
            IrStreamEvent::ContentBlockStart { index, block } => {
                match block {
                    IrContentBlockStart::ToolUse { id, name } => {
                        // tool_use start 需要发送 tool_call header
                        let tool_index = self.tool_count;
                        self.tool_count += 1;
                        // 记录 IR block index → OpenAI tool index 映射
                        if self.tool_index_map.len() <= *index {
                            self.tool_index_map.resize(*index + 1, 0);
                        }
                        self.tool_index_map[*index] = tool_index;
                        let mut delta_obj = json!({});
                        if !self.first_chunk_sent {
                            delta_obj["role"] = json!("assistant");
                            self.first_chunk_sent = true;
                        }
                        delta_obj["tool_calls"] = json!([{
                            "index": tool_index,
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": ""
                            }
                        }]);

                        Some(mk(json!({
                            "id": self.msg_id,
                            "object": "chat.completion.chunk",
                            "created": self.created,
                            "model": self.model,
                            "choices": [{
                                "index": 0,
                                "delta": delta_obj,
                                "finish_reason": null
                            }]
                        })))
                    }
                    _ => {
                        // text/thinking: Chat 没有 content_block_start
                        None
                    }
                }
            }
            IrStreamEvent::ContentBlockDelta { index, delta } => {
                let mut delta_obj = json!({});

                // 第一个 chunk 需要包含 role
                if !self.first_chunk_sent {
                    delta_obj["role"] = json!("assistant");
                    self.first_chunk_sent = true;
                }

                match delta {
                    IrContentDelta::TextDelta(text) => {
                        delta_obj["content"] = json!(text);
                    }
                    IrContentDelta::ThinkingDelta(thinking) => {
                        delta_obj["reasoning_content"] = json!(thinking);
                    }
                    IrContentDelta::InputJsonDelta(partial) => {
                        // tool_calls arguments delta：查 IR block index → OpenAI tool index 映射
                        let tool_index = self
                            .tool_index_map
                            .get(*index)
                            .copied()
                            .unwrap_or(0);
                        delta_obj["tool_calls"] = json!([{
                            "index": tool_index,
                            "function": {
                                "arguments": partial
                            }
                        }]);
                    }
                }

                Some(mk(json!({
                    "id": self.msg_id,
                    "object": "chat.completion.chunk",
                    "created": self.created,
                    "model": self.model,
                    "choices": [{
                        "index": 0,
                        "delta": delta_obj,
                        "finish_reason": null
                    }]
                })))
            }
            IrStreamEvent::ContentBlockStop { .. } => {
                // Chat 没有 content_block_stop
                None
            }
            IrStreamEvent::MessageDelta { stop_reason, usage } => {
                let finish_reason = stop_reason.map(|sr| sr.as_chat_finish_reason());

                let mut chunk = json!({
                    "id": self.msg_id,
                    "object": "chat.completion.chunk",
                    "created": self.created,
                    "model": self.model,
                    "choices": [{
                        "index": 0,
                        "delta": {},
                        "finish_reason": finish_reason
                    }]
                });

                // 添加 usage（如果有）
                if let Some(u) = usage {
                    let output_tokens = if u.output_tokens > 0 {
                        u.output_tokens
                    } else {
                        u.output_chars / 4
                    };
                    chunk["usage"] = json!({
                        "prompt_tokens": u.input_tokens + u.cache_read_input_tokens,
                        "completion_tokens": output_tokens,
                        "total_tokens": u.input_tokens + u.cache_read_input_tokens + output_tokens,
                        "prompt_tokens_details": {
                            "cached_tokens": u.cache_read_input_tokens
                        }
                    });
                }

                Some(mk(chunk))
            }
            IrStreamEvent::MessageStop => {
                // Chat 的流结束标记（只发一次）
                self.done_sent = true;
                Some(Bytes::from("data: [DONE]\n\n"))
            }
        }
    }

    /// 流结束时渲染收尾（[DONE]）。
    ///
    /// Chat 的 finalize 很简单，因为 MessageDelta 已经包含了 finish_reason。
    /// 仅当 MessageStop 事件未发过 [DONE] 时补发（上游异常断流场景）。
    pub fn finalize(&mut self, _usage: &IrUsage) -> Vec<Bytes> {
        if self.done_sent {
            vec![]
        } else {
            vec![Bytes::from("data: [DONE]\n\n")]
        }
    }
}

impl Default for ChatCompletionsRenderState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ir_req_to_chat_completions_basic() {
        let req = IrRequest {
            model: "gpt-4o".to_string(),
            system: Some(IrSystemContent::Text("Be helpful.".to_string())),
            messages: vec![IrMessage {
                role: IrRole::User,
                content: vec![IrContentBlock::Text {
                    text: "Hello".to_string(),
                    cache_control: None,
                }],
            }],
            tools: vec![],
            tool_choice: None,
            max_tokens: Some(4096),
            temperature: None,
            top_p: None,
            thinking: None,
            stream: true,
        };
        let v = ir_req_to_chat_completions(&req);
        assert_eq!(v["model"], "gpt-4o");
        assert_eq!(v["messages"][0]["role"], "system");
        assert_eq!(v["messages"][0]["content"], "Be helpful.");
        assert_eq!(v["messages"][1]["role"], "user");
        assert_eq!(v["messages"][1]["content"], "Hello");
        assert_eq!(v["max_tokens"], 4096);
        assert_eq!(v["stream"], true);
    }

    #[test]
    fn test_ir_req_to_chat_completions_with_tools() {
        let req = IrRequest {
            model: "gpt-4o".to_string(),
            system: None,
            messages: vec![],
            tools: vec![IrTool {
                name: "search".to_string(),
                description: Some("Search the web".to_string()),
                input_schema: json!({"type": "object", "properties": {"q": {"type": "string"}}}),
            }],
            tool_choice: Some(IrToolChoice::Any),
            max_tokens: Some(1024),
            temperature: Some(0.7),
            top_p: None,
            thinking: Some(IrThinkingConfig {
                enabled: true,
                budget_tokens: Some(16384),
            }),
            stream: true,
        };
        let v = ir_req_to_chat_completions(&req);
        assert_eq!(v["tools"][0]["type"], "function");
        assert_eq!(v["tools"][0]["function"]["name"], "search");
        assert_eq!(v["tools"][0]["function"]["description"], "Search the web");
        assert_eq!(v["tool_choice"], "required");
        assert_eq!(v["reasoning_effort"], "high");
        assert_eq!(v["temperature"], 0.7);
    }

    #[test]
    fn test_ir_req_to_chat_completions_o1_uses_max_completion_tokens() {
        let req = IrRequest {
            model: "o1".to_string(),
            system: None,
            messages: vec![IrMessage {
                role: IrRole::User,
                content: vec![IrContentBlock::Text {
                    text: "Hello".to_string(),
                    cache_control: None,
                }],
            }],
            tools: vec![],
            tool_choice: None,
            max_tokens: Some(8192),
            temperature: None,
            top_p: None,
            thinking: None,
            stream: false,
        };
        let v = ir_req_to_chat_completions(&req);
        // o1 系列只接受 max_completion_tokens
        assert!(v.get("max_tokens").is_none());
        assert_eq!(v["max_completion_tokens"], 8192);
    }

    #[test]
    fn test_ir_req_to_chat_completions_gpt_keeps_max_tokens() {
        let req = IrRequest {
            model: "gpt-4o".to_string(),
            system: None,
            messages: vec![],
            tools: vec![],
            tool_choice: None,
            max_tokens: Some(4096),
            temperature: None,
            top_p: None,
            thinking: None,
            stream: false,
        };
        let v = ir_req_to_chat_completions(&req);
        assert_eq!(v["max_tokens"], 4096);
        assert!(v.get("max_completion_tokens").is_none());
    }

    #[test]
    fn test_ir_req_to_chat_completions_tool_use_and_result() {
        let req = IrRequest {
            model: "gpt-4o".to_string(),
            system: None,
            messages: vec![
                IrMessage {
                    role: IrRole::Assistant,
                    content: vec![IrContentBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: "search".to_string(),
                        input: json!({"q": "test"}),
                    }],
                },
                IrMessage {
                    role: IrRole::User,
                    content: vec![IrContentBlock::ToolResult {
                        tool_use_id: "call_1".to_string(),
                        content: IrToolResultContent::Text("result text".to_string()),
                        is_error: false,
                    }],
                },
            ],
            tools: vec![],
            tool_choice: None,
            max_tokens: None,
            temperature: None,
            top_p: None,
            thinking: None,
            stream: false,
        };
        let v = ir_req_to_chat_completions(&req);
        // Assistant message with tool_calls
        assert_eq!(v["messages"][0]["role"], "assistant");
        assert_eq!(v["messages"][0]["tool_calls"][0]["id"], "call_1");
        assert_eq!(v["messages"][0]["tool_calls"][0]["function"]["name"], "search");
        // Tool result message
        assert_eq!(v["messages"][1]["role"], "tool");
        assert_eq!(v["messages"][1]["tool_call_id"], "call_1");
        assert_eq!(v["messages"][1]["content"], "result text");
    }

    #[test]
    fn test_render_event_text_delta() {
        let mut state = ChatCompletionsRenderState::new();
        state.msg_id = "chatcmpl-1".to_string();
        state.model = "gpt-4o".to_string();

        let ev = IrStreamEvent::ContentBlockDelta {
            index: 0,
            delta: IrContentDelta::TextDelta("Hello".to_string()),
        };
        let bytes = state.render_event(&ev).unwrap();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("data: "));
        assert!(s.contains("\"content\":\"Hello\""));
        assert!(s.contains("\"role\":\"assistant\"")); // 第一个 chunk 包含 role
    }

    #[test]
    fn test_render_event_reasoning_delta() {
        let mut state = ChatCompletionsRenderState::new();
        state.msg_id = "chatcmpl-1".to_string();
        state.model = "qwen-max".to_string();

        let ev = IrStreamEvent::ContentBlockDelta {
            index: 0,
            delta: IrContentDelta::ThinkingDelta("thinking...".to_string()),
        };
        let bytes = state.render_event(&ev).unwrap();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("\"reasoning_content\":\"thinking...\""));
    }

    #[test]
    fn test_render_event_message_delta_with_finish() {
        let mut state = ChatCompletionsRenderState::new();
        state.msg_id = "chatcmpl-1".to_string();
        state.model = "gpt-4o".to_string();

        let ev = IrStreamEvent::MessageDelta {
            stop_reason: Some(IrStopReason::EndTurn),
            usage: Some(IrUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_read_input_tokens: 80,
                ..Default::default()
            }),
        };
        let bytes = state.render_event(&ev).unwrap();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("\"finish_reason\":\"stop\""));
        assert!(s.contains("\"prompt_tokens\":180")); // 100 + 80
        assert!(s.contains("\"completion_tokens\":50"));
        assert!(s.contains("\"cached_tokens\":80"));
    }

    #[test]
    fn test_render_event_message_stop() {
        let mut state = ChatCompletionsRenderState::new();
        let ev = IrStreamEvent::MessageStop;
        let bytes = state.render_event(&ev).unwrap();
        let s = String::from_utf8_lossy(&bytes);
        assert_eq!(s.trim(), "data: [DONE]");
    }

    #[test]
    fn test_render_event_tool_use_start() {
        let mut state = ChatCompletionsRenderState::new();
        state.msg_id = "chatcmpl-1".to_string();
        state.model = "gpt-4o".to_string();
        state.tool_count = 1;

        let ev = IrStreamEvent::ContentBlockStart {
            index: 0,
            block: IrContentBlockStart::ToolUse {
                id: "call_1".to_string(),
                name: "search".to_string(),
            },
        };
        let bytes = state.render_event(&ev).unwrap();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("\"tool_calls\""));
        assert!(s.contains("\"id\":\"call_1\""));
        assert!(s.contains("\"name\":\"search\""));
    }

    #[test]
    fn test_render_event_input_json_delta_uses_mapped_index() {
        let mut state = ChatCompletionsRenderState::new();
        state.msg_id = "chatcmpl-1".to_string();
        state.model = "gpt-4o".to_string();

        // 模拟 thinking(block 0) + text(block 1) 后接 tool(block 2)：
        // Chat 侧 thinking 被丢弃，tool_call index 应独立计数从 0 开始
        let start = IrStreamEvent::ContentBlockStart {
            index: 2,
            block: IrContentBlockStart::ToolUse {
                id: "call_1".to_string(),
                name: "search".to_string(),
            },
        };
        let bytes = state.render_event(&start).unwrap();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("\"index\":0"));
        assert!(s.contains("\"id\":\"call_1\""));

        // 参数续传：IR block 2 → OpenAI tool index 0
        let delta = IrStreamEvent::ContentBlockDelta {
            index: 2,
            delta: IrContentDelta::InputJsonDelta("{\"q\":".to_string()),
        };
        let bytes = state.render_event(&delta).unwrap();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("\"tool_calls\""));
        assert!(s.contains("\"index\":0"));
        assert!(s.contains("\"arguments\":\"{\\\"q\\\":\""));
    }

    #[test]
    fn test_finalize_no_double_done() {
        let mut state = ChatCompletionsRenderState::new();
        // MessageStop 已发过 [DONE]
        let ev = IrStreamEvent::MessageStop;
        assert!(state.render_event(&ev).is_some());
        // finalize 不应再发
        assert!(state.finalize(&IrUsage::default()).is_empty());
    }

    #[test]
    fn test_finalize_sends_done_when_missing() {
        let mut state = ChatCompletionsRenderState::new();
        // 没有 MessageStop（异常断流）：finalize 补发 [DONE]
        let events = state.finalize(&IrUsage::default());
        assert_eq!(events.len(), 1);
        let s = String::from_utf8_lossy(&events[0]);
        assert_eq!(s.trim(), "data: [DONE]");
            // 没有 MessageStop（异常断流）：finalize 补发 [DONE]
        let events = state.finalize(&IrUsage::default());
        assert_eq!(events.len(), 1);
        let s = String::from_utf8_lossy(&events[0]);
        assert_eq!(s.trim(), "data: [DONE]");
    }

    /// 回归：纯 tool_result 的 user 消息不应产生空 user 消息（OpenAI 400）
    #[test]
    fn test_subagent_tool_result_only_user_message() {
        let req = IrRequest {
            model: "gpt-4o".to_string(),
            system: None,
            messages: vec![
                IrMessage {
                    role: IrRole::Assistant,
                    content: vec![IrContentBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: "Bash".to_string(),
                        input: json!({"command": "ls"}),
                    }],
                },
                IrMessage {
                    role: IrRole::User,
                    content: vec![IrContentBlock::ToolResult {
                        tool_use_id: "call_1".to_string(),
                        content: IrToolResultContent::Text("file.txt".to_string()),
                        is_error: false,
                    }],
                },
            ],
            tools: vec![],
            tool_choice: None,
            max_tokens: None,
            temperature: None,
            top_p: None,
            thinking: None,
            stream: false,
        };
        let v = ir_req_to_chat_completions(&req);
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2, "assistant(tool_calls) + tool 各一条，不得有空的 user 消息: {:?}", msgs);
        assert_eq!(msgs[0]["role"], "assistant");
        assert_eq!(msgs[1]["role"], "tool");
        assert_eq!(msgs[1]["tool_call_id"], "call_1");
    }

    /// round-trip：Messages 上游工具流（thinking → text → tool_use）→ IR →
    /// Chat 渲染 → Chat 解析器读回，工具参数与文本不混、index 不冲突
    #[test]
    fn test_subagent_messages_tool_flow_roundtrip_via_chat() {
        use crate::api::proxy::ir::from_chat_completions::ChatCompletionsParseState;
        use crate::api::proxy::ir::from_messages::MessagesParseState;

        // 1. Anthropic 上游 SSE → IR 事件
        let mut mps = MessagesParseState::new();
        let chunks = [
            json!({"type": "message_start", "message": {"id": "msg_rt", "type": "message", "role": "assistant", "model": "claude-sonnet-4-5", "content": [], "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 10, "output_tokens": 0, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": "sig_rt"}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "planning"}}),
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Let me run ls"}}),
            json!({"type": "content_block_stop", "index": 1}),
            json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {}}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "{\"command\":"}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "\"ls\"}"}}),
            json!({"type": "content_block_stop", "index": 2}),
            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use", "stop_sequence": null}, "usage": {"output_tokens": 12, "cache_read_input_tokens": 0}}),
            json!({"type": "message_stop"}),
        ];
        let mut ir_events = vec![];
        for c in &chunks {
            ir_events.extend(crate::api::proxy::ir::from_messages::messages_chunk_to_ir(c, &mut mps));
        }

        // 2. IR → Chat SSE
        let mut render = ChatCompletionsRenderState::new();
        let mut chat_sses: Vec<String> = vec![];
        for ev in &ir_events {
            if let Some(b) = render.render_event(ev) {
                chat_sses.push(String::from_utf8_lossy(&b).to_string());
            }
        }
        for b in render.finalize(&mps.usage) {
            chat_sses.push(String::from_utf8_lossy(&b).to_string());
        }

        // 3. Chat SSE → Chat 解析器读回
        let mut cps = ChatCompletionsParseState::new();
        cps.usage.input_tokens = 10;
        let mut back: Vec<IrStreamEvent> = vec![];
        for frame in &chat_sses {
            let data = frame.trim().strip_prefix("data: ").unwrap_or(frame.trim());
            if data == "[DONE]" { continue; }
            let v: serde_json::Value = serde_json::from_str(data).unwrap();
            back.extend(crate::api::proxy::ir::from_chat_completions::chat_completions_chunk_to_ir(&v, &mut cps));
        }

        // 4. 断言：工具参数完整、文本独立、各块只关一次
        let tool_args: String = back.iter().filter_map(|e| match e {
            IrStreamEvent::ContentBlockDelta { delta: IrContentDelta::InputJsonDelta(p), .. } => Some(p.clone()),
            _ => None,
        }).collect();
        assert!(tool_args.contains("\"ls\""), "工具参数应完整读回: {}", tool_args);
        let text: String = back.iter().filter_map(|e| match e {
            IrStreamEvent::ContentBlockDelta { delta: IrContentDelta::TextDelta(t), .. } => Some(t.clone()),
            _ => None,
        }).collect();
        assert_eq!(text, "Let me run ls", "文本应独立: {}", text);
        let stops: Vec<usize> = back.iter().filter_map(|e| match e {
            IrStreamEvent::ContentBlockStop { index } => Some(*index),
            _ => None,
        }).collect();
        // thinking 经 reasoning_content 对称往返，读回 3 块各关一次
        assert_eq!(stops, vec![0, 1, 2], "thinking/text/tool 各关一次: {:?}", stops);
        // 文本 delta 与工具 args delta 不得混在同一块
        let text_indices: Vec<usize> = back.iter().filter_map(|e| match e {
            IrStreamEvent::ContentBlockDelta { index, delta: IrContentDelta::TextDelta(_) } => Some(*index),
            _ => None,
        }).collect();
        assert!(text_indices.iter().all(|i| *i == 1), "文本应全在独立块: {:?}", text_indices);
        let args_indices: Vec<usize> = back.iter().filter_map(|e| match e {
            IrStreamEvent::ContentBlockDelta { index, delta: IrContentDelta::InputJsonDelta(_) } => Some(*index),
            _ => None,
        }).collect();
        assert!(args_indices.iter().all(|i| *i == 2), "工具参数应全在 tool 块: {:?}", args_indices);
    }

}
