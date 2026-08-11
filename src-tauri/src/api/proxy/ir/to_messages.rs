//! IR → Anthropic Messages 方向的格式翻译：请求体与流式事件渲染。
//!
//! IR 以 Anthropic SSE 为骨架，所以渲染器相对简单——
//! 主要工作是把强类型 IR 序列化为 `Value` / SSE 字节。

use bytes::Bytes;
use serde_json::{json, Value};

use super::types::*;

/// 将 IR 请求体序列化为 Anthropic Messages 格式。
pub fn ir_req_to_messages(req: &IrRequest) -> Value {
    let mut out = json!({
        "model": req.model,
        "stream": req.stream,
    });

    // System prompt
    if let Some(ref system) = req.system {
        out["system"] = match system {
            IrSystemContent::Text(t) => json!(t),
            IrSystemContent::Blocks(blocks) => {
                let arr: Vec<Value> = blocks
                    .iter()
                    .map(|b| {
                        let mut obj = json!({"type": "text", "text": b.text});
                        if let Some(ref cc) = b.cache_control {
                            obj["cache_control"] = cc.clone();
                        }
                        obj
                    })
                    .collect();
                json!(arr)
            }
        };
    }

    // Messages
    let messages: Vec<Value> = req
        .messages
        .iter()
        .map(|msg| {
            let role = match msg.role {
                IrRole::User => "user",
                IrRole::Assistant => "assistant",
            };
            let content = render_anthropic_content(&msg.content);
            json!({"role": role, "content": content})
        })
        .collect();
    out["messages"] = json!(messages);

    // Tools
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                let mut obj = json!({
                    "name": t.name,
                    "input_schema": t.input_schema,
                });
                if let Some(ref desc) = t.description {
                    obj["description"] = json!(desc);
                }
                obj
            })
            .collect();
        out["tools"] = json!(tools);
    }

    // Tool choice
    if let Some(ref tc) = req.tool_choice {
        out["tool_choice"] = match tc {
            IrToolChoice::Auto => json!({"type": "auto"}),
            IrToolChoice::Any => json!({"type": "any"}),
            IrToolChoice::None => json!({"type": "none"}),
            IrToolChoice::Tool { name } => json!({"type": "tool", "name": name}),
        };
    }

    // Thinking config
    let thinking_enabled = req.thinking.as_ref().map(|t| t.enabled).unwrap_or(false);
    if thinking_enabled {
        let mut t = json!({"type": "enabled"});
        if let Some(budget) = req.thinking.as_ref().and_then(|t| t.budget_tokens) {
            t["budget_tokens"] = json!(budget);
        }
        out["thinking"] = t;
    }

    // Pass through scalar params
    if let Some(max_tokens) = req.max_tokens {
        out["max_tokens"] = json!(max_tokens);
    }
    // Anthropic spec：thinking 与 temperature/top_p 互斥，同时给出会 400
    if !thinking_enabled {
        if let Some(temperature) = req.temperature {
            out["temperature"] = json!(temperature);
        }
        if let Some(top_p) = req.top_p {
            out["top_p"] = json!(top_p);
        }
    }

    out
}

/// 将 IR content blocks 渲染为 Anthropic content 数组。
fn render_anthropic_content(blocks: &[IrContentBlock]) -> Value {
    let arr: Vec<Value> = blocks
        .iter()
        .filter_map(|block| match block {
            IrContentBlock::Text { text, cache_control } => {
                let mut obj = json!({"type": "text", "text": text});
                if let Some(cc) = cache_control {
                    obj["cache_control"] = cc.clone();
                }
                Some(obj)
            }
            IrContentBlock::Image { source } => {
                let src = match source {
                    IrImageSource::Base64 { media_type, data } => {
                        json!({"type": "base64", "media_type": media_type, "data": data})
                    }
                    IrImageSource::Url { url } => json!({"type": "url", "url": url}),
                };
                Some(json!({"type": "image", "source": src}))
            }
            IrContentBlock::Thinking {
                thinking,
                signature,
            } => {
                let mut obj = json!({"type": "thinking", "thinking": thinking});
                if let Some(sig) = signature {
                    obj["signature"] = json!(sig);
                }
                Some(obj)
            }
            IrContentBlock::ToolUse { id, name, input } => {
                Some(json!({"type": "tool_use", "id": id, "name": name, "input": input}))
            }
            IrContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let c = match content {
                    IrToolResultContent::Text(t) => json!(t),
                    IrToolResultContent::Blocks(blocks) => render_anthropic_content(blocks),
                };
                let mut obj = json!({"type": "tool_result", "tool_use_id": tool_use_id, "content": c});
                if *is_error {
                    obj["is_error"] = json!(true);
                }
                Some(obj)
            }
        })
        .collect();
    json!(arr)
}

// ═══════════════════════════════════════════════════════════════════
// 流式事件渲染
// ═══════════════════════════════════════════════════════════════════

/// Messages SSE 渲染状态机。
///
/// IR 事件与 Anthropic SSE 几乎同构，状态主要用于：
/// - 记录 msg_id / model（message_start 时捕获，后续 chunk 复用）
/// - 追踪是否已发过 message_start（避免重复）
pub struct MessagesRenderState {
    msg_id: String,
    model: String,
    started: bool,
    /// 最后一个 MessageDelta 的 stop_reason（finalize 时使用）。
    last_stop_reason: Option<IrStopReason>,
    /// 最后一个 MessageDelta 的 usage（finalize 时使用）。
    last_usage: Option<IrUsage>,
}

impl MessagesRenderState {
    pub fn new() -> Self {
        Self {
            msg_id: String::new(),
            model: String::new(),
            started: false,
            last_stop_reason: None,
            last_usage: None,
        }
    }

    /// 将 IR 流式事件渲染为 Anthropic SSE 字节段。
    ///
    /// 返回 `None` 表示该事件不产生输出（如 MessageDelta 延迟到 finalize）。
    pub fn render_event(&mut self, ev: &IrStreamEvent) -> Option<Bytes> {
        let mk = |event_type: &str, payload: Value| -> Bytes {
            let data = serde_json::to_string(&payload).unwrap_or_default();
            Bytes::from(format!("event: {}\ndata: {}\n\n", event_type, data))
        };

        match ev {
            IrStreamEvent::MessageStart { id, model, usage } => {
                self.msg_id = id.clone();
                self.model = model.clone();
                self.started = true;
                // 上游真实 usage（translation 路径为估算值），渲染给客户端
                // 作上下文条 / prompt-cache 感知；缺失时兜底 0
                // 客户端口径：input_tokens 含 cache_creation（写缓存 = 首次处理输入）
                let u = usage.as_ref();
                let input_tokens = u.map(|u| u.input_tokens).unwrap_or(0);
                let cache_creation = u.map(|u| u.cache_creation_input_tokens).unwrap_or(0);
                let cache_read = u.map(|u| u.cache_read_input_tokens).unwrap_or(0);
                Some(mk(
                    "message_start",
                    json!({
                        "type": "message_start",
                        "message": {
                            "id": id,
                            "type": "message",
                            "role": "assistant",
                            "model": model,
                            "content": [],
                            "stop_reason": null,
                            "stop_sequence": null,
                            "usage": {
                                "input_tokens": input_tokens + cache_creation,
                                "output_tokens": 0,
                                "cache_creation_input_tokens": cache_creation,
                                "cache_read_input_tokens": cache_read,
                            }
                        }
                    }),
                ))
            }
            IrStreamEvent::ContentBlockStart { index, block } => {
                let content_block = match block {
                    IrContentBlockStart::Text => json!({"type": "text", "text": ""}),
                    IrContentBlockStart::Thinking { signature } => {
                        let mut block = json!({"type": "thinking", "thinking": ""});
                        // Anthropic 流式 spec：thinking 块的 content_block_start 携带 signature
                        if let Some(sig) = signature {
                            block["signature"] = json!(sig);
                        }
                        block
                    }
                    IrContentBlockStart::ToolUse { id, name } => {
                        json!({"type": "tool_use", "id": id, "name": name, "input": {}})
                    }
                };
                Some(mk(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": index, "content_block": content_block}),
                ))
            }
            IrStreamEvent::ContentBlockDelta { index, delta } => {
                let d = match delta {
                    IrContentDelta::TextDelta(text) => {
                        json!({"type": "text_delta", "text": text})
                    }
                    IrContentDelta::ThinkingDelta(thinking) => {
                        json!({"type": "thinking_delta", "thinking": thinking})
                    }
                    IrContentDelta::InputJsonDelta(partial) => {
                        json!({"type": "input_json_delta", "partial_json": partial})
                    }
                };
                Some(mk(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": index, "delta": d}),
                ))
            }
            IrStreamEvent::ContentBlockStop { index } => Some(mk(
                "content_block_stop",
                json!({"type": "content_block_stop", "index": index}),
            )),
            IrStreamEvent::MessageDelta { stop_reason, usage } => {
                // 暂存，延迟到 finalize 发出（等 usage 到齐）
                if let Some(sr) = stop_reason {
                    self.last_stop_reason = Some(*sr);
                }
                if let Some(u) = usage {
                    self.last_usage = Some(u.clone());
                }
                None
            }
            IrStreamEvent::MessageStop => {
                // MessageStop 也延迟到 finalize
                None
            }
        }
    }

    /// 流结束时渲染收尾事件（message_delta + message_stop）。
    ///
    /// 使用累积的 usage 确保 token 计数准确。
    pub fn finalize(&mut self, usage: &IrUsage) -> Vec<Bytes> {
        let mut events = Vec::new();
        let mk = |event_type: &str, payload: Value| -> Bytes {
            let data = serde_json::to_string(&payload).unwrap_or_default();
            Bytes::from(format!("event: {}\ndata: {}\n\n", event_type, data))
        };

        let stop_reason = self
            .last_stop_reason
            .map(|sr| sr.as_anthropic_str())
            .unwrap_or("end_turn");

        let output_tokens = if usage.output_tokens > 0 {
            usage.output_tokens
        } else {
            usage.output_chars / 4
        };

        events.push(mk(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": stop_reason, "stop_sequence": null},
                "usage": {
                    "input_tokens": usage.input_tokens,
                    "output_tokens": output_tokens,
                    "cache_read_input_tokens": usage.cache_read_input_tokens,
                }
            }),
        ));
        events.push(mk("message_stop", json!({"type": "message_stop"})));
        events
    }

    /// 渲染 `server_tool_use` 内容块（Anthropic server-side tool 调用）。
    ///
    /// 用于 WebSearch 劫持：模型调用代理的 `web_search` 工具时，以官方
    /// server-side tool 格式呈现给 Claude Code——`content_block_start`
    /// （type=server_tool_use）+ `input_json_delta`（查询词）+ `content_block_stop`。
    /// 返回三个 SSE 事件字节的拼接。
    pub(crate) fn render_server_tool_use(
        &mut self,
        index: usize,
        id: &str,
        query: &str,
    ) -> Bytes {
        let mk = |event_type: &str, payload: Value| -> Bytes {
            let data = serde_json::to_string(&payload).unwrap_or_default();
            Bytes::from(format!("event: {}\ndata: {}\n\n", event_type, data))
        };

        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(&mk(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": index,
                "content_block": {
                    "type": "server_tool_use",
                    "id": id,
                    "name": "web_search",
                    "input": {}
                }
            }),
        ));
        // 查询词以 input_json_delta 增量呈现（Claude Code 显示卡片上的查询内容）
        let input_json = serde_json::json!({"query": query}).to_string();
        out.extend_from_slice(&mk(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": {"type": "input_json_delta", "partial_json": input_json}
            }),
        ));
        out.extend_from_slice(&mk(
            "content_block_stop",
            json!({"type": "content_block_stop", "index": index}),
        ));
        Bytes::from(out)
    }

    /// 渲染 `web_search_tool_result` 内容块（server-side tool 的搜索结果）。
    ///
    /// 对应官方 `WebSearchToolResultBlock`：`content_block_start`
    /// （type=web_search_tool_result，content 为 `web_search_result` 数组）+
    /// `content_block_stop`。`encrypted_content` 为官方必填字段，此处
    /// 用摘要文本的 base64 填充（Claude Code 卡片展示 title/url）。
    pub(crate) fn render_web_search_tool_result(
        &self,
        index: usize,
        tool_use_id: &str,
        results: &[crate::search::SearchResult],
    ) -> Bytes {
        let mk = |event_type: &str, payload: Value| -> Bytes {
            let data = serde_json::to_string(&payload).unwrap_or_default();
            Bytes::from(format!("event: {}\ndata: {}\n\n", event_type, data))
        };

        let content: Vec<Value> = results
            .iter()
            .map(|r| {
                json!({
                    "type": "web_search_result",
                    "title": r.title,
                    "url": r.url,
                    // 官方字段：内容以 base64 呈现，这里用摘要文本
                    "encrypted_content": base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        r.snippet.as_bytes(),
                    )
                })
            })
            .collect();

        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(&mk(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": index,
                "content_block": {
                    "type": "web_search_tool_result",
                    "tool_use_id": tool_use_id,
                    "content": content
                }
            }),
        ));
        out.extend_from_slice(&mk(
            "content_block_stop",
            json!({"type": "content_block_stop", "index": index}),
        ));
        Bytes::from(out)
    }
}

impl Default for MessagesRenderState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ir_req_to_messages_basic() {
        let req = IrRequest {
            model: "claude-opus-4-8".to_string(),
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
        let v = ir_req_to_messages(&req);
        assert_eq!(v["model"], "claude-opus-4-8");
        assert_eq!(v["system"], "Be helpful.");
        assert_eq!(v["max_tokens"], 4096);
        assert_eq!(v["stream"], true);
        assert_eq!(v["messages"][0]["role"], "user");
    }

    #[test]
    fn test_ir_req_to_messages_with_tools() {
        let req = IrRequest {
            model: "claude".to_string(),
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
                budget_tokens: Some(5000),
            }),
            stream: true,
        };
        let v = ir_req_to_messages(&req);
        assert_eq!(v["tools"][0]["name"], "search");
        assert_eq!(v["tools"][0]["description"], "Search the web");
        assert_eq!(v["tool_choice"]["type"], "any");
        assert_eq!(v["thinking"]["type"], "enabled");
        assert_eq!(v["thinking"]["budget_tokens"], 5000);
        // thinking 与 temperature 互斥（Anthropic spec），temperature 被剔除
        assert!(v.get("temperature").is_none());
    }

    #[test]
    fn test_ir_req_to_messages_temperature_without_thinking() {
        let req = IrRequest {
            model: "claude".to_string(),
            system: None,
            messages: vec![],
            tools: vec![],
            tool_choice: None,
            max_tokens: Some(1024),
            temperature: Some(0.7),
            top_p: None,
            thinking: None,
            stream: true,
        };
        let v = ir_req_to_messages(&req);
        assert_eq!(v["temperature"], 0.7);
    }

    #[test]
    fn test_ir_req_to_messages_thinking_drops_temperature() {
        let req = IrRequest {
            model: "claude".to_string(),
            system: None,
            messages: vec![],
            tools: vec![],
            tool_choice: None,
            max_tokens: Some(1024),
            temperature: Some(0.7),
            top_p: Some(0.9),
            thinking: Some(IrThinkingConfig {
                enabled: true,
                budget_tokens: Some(5000),
            }),
            stream: true,
        };
        let v = ir_req_to_messages(&req);
        // Anthropic spec：thinking 与 temperature/top_p 互斥
        assert_eq!(v["thinking"]["type"], "enabled");
        assert!(v.get("temperature").is_none());
        assert!(v.get("top_p").is_none());
    }

    #[test]
    fn test_ir_req_to_messages_image() {
        let req = IrRequest {
            model: "claude".to_string(),
            system: None,
            messages: vec![IrMessage {
                role: IrRole::User,
                content: vec![
                    IrContentBlock::Text {
                        text: "What is this?".to_string(),
                        cache_control: None,
                    },
                    IrContentBlock::Image {
                        source: IrImageSource::Base64 {
                            media_type: "image/png".to_string(),
                            data: "abc123".to_string(),
                        },
                    },
                ],
            }],
            tools: vec![],
            tool_choice: None,
            max_tokens: Some(1024),
            temperature: None,
            top_p: None,
            thinking: None,
            stream: false,
        };
        let v = ir_req_to_messages(&req);
        let content = &v["messages"][0]["content"];
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["type"], "base64");
        assert_eq!(content[1]["source"]["data"], "abc123");
    }

    #[test]
    fn test_render_event_message_start() {
        let mut state = MessagesRenderState::new();
        let ev = IrStreamEvent::MessageStart {
            id: "msg_123".to_string(),
            model: "claude-opus-4-8".to_string(),
            usage: Some(IrUsage { input_tokens: 150, cache_read_input_tokens: 8000, ..Default::default() }),
        };
        let bytes = state.render_event(&ev).unwrap();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("event: message_start"));
        assert!(s.contains("\"id\":\"msg_123\""));
        assert!(s.contains("\"model\":\"claude-opus-4-8\""));
        // usage 渲染（input 侧真实值 + cache_read）
        assert!(s.contains("\"input_tokens\":150"));
        assert!(s.contains("\"cache_read_input_tokens\":8000"));
    }

    #[test]
    fn test_render_event_content_block_text() {
        let mut state = MessagesRenderState::new();
        let ev = IrStreamEvent::ContentBlockStart {
            index: 0,
            block: IrContentBlockStart::Text,
        };
        let bytes = state.render_event(&ev).unwrap();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("event: content_block_start"));
        assert!(s.contains("\"type\":\"text\""));
    }

    #[test]
    fn test_render_event_content_block_thinking_signature() {
        let mut state = MessagesRenderState::new();
        let ev = IrStreamEvent::ContentBlockStart {
            index: 0,
            block: IrContentBlockStart::Thinking {
                signature: Some("sig_abc123".to_string()),
            },
        };
        let bytes = state.render_event(&ev).unwrap();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("event: content_block_start"));
        assert!(s.contains("\"type\":\"thinking\""));
        assert!(s.contains("\"signature\":\"sig_abc123\""));

        // 无签名时不输出 signature 字段
        let ev2 = IrStreamEvent::ContentBlockStart {
            index: 0,
            block: IrContentBlockStart::Thinking { signature: None },
        };
        let bytes2 = state.render_event(&ev2).unwrap();
        let s2 = String::from_utf8_lossy(&bytes2);
        assert!(!s2.contains("signature"));
    }

    #[test]
    fn test_render_event_text_delta() {
        let mut state = MessagesRenderState::new();
        let ev = IrStreamEvent::ContentBlockDelta {
            index: 0,
            delta: IrContentDelta::TextDelta("Hello".to_string()),
        };
        let bytes = state.render_event(&ev).unwrap();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("event: content_block_delta"));
        assert!(s.contains("\"type\":\"text_delta\""));
        assert!(s.contains("\"text\":\"Hello\""));
    }

    #[test]
    fn test_render_event_message_delta_deferred() {
        let mut state = MessagesRenderState::new();
        let ev = IrStreamEvent::MessageDelta {
            stop_reason: Some(IrStopReason::EndTurn),
            usage: Some(IrUsage {
                output_tokens: 100,
                ..Default::default()
            }),
        };
        // MessageDelta 延迟到 finalize，render_event 返回 None
        assert!(state.render_event(&ev).is_none());
        assert_eq!(state.last_stop_reason, Some(IrStopReason::EndTurn));
    }

    #[test]
    fn test_finalize_emits_message_delta_and_stop() {
        let mut state = MessagesRenderState::new();
        state.last_stop_reason = Some(IrStopReason::ToolUse);

        let usage = IrUsage {
            output_tokens: 300,
            cache_read_input_tokens: 8000,
            ..Default::default()
        };
        let events = state.finalize(&usage);
        assert_eq!(events.len(), 2);

        let s0 = String::from_utf8_lossy(&events[0]);
        assert!(s0.contains("event: message_delta"));
        assert!(s0.contains("\"stop_reason\":\"tool_use\""));
        assert!(s0.contains("\"output_tokens\":300"));

        let s1 = String::from_utf8_lossy(&events[1]);
        assert!(s1.contains("event: message_stop"));
    }

    #[test]
    fn test_finalize_fallback_chars_to_tokens() {
        let mut state = MessagesRenderState::new();
        let usage = IrUsage {
            output_tokens: 0,
            output_chars: 120, // 120 / 4 = 30
            ..Default::default()
        };
        let events = state.finalize(&usage);
        let s = String::from_utf8_lossy(&events[0]);
        assert!(s.contains("\"output_tokens\":30"));
    }
}

    // ── WebSearch server-side tool 渲染测试 ──

    /// server_tool_use 块应输出合法的 SSE 事件序列：
    /// content_block_start(type=server_tool_use) → input_json_delta → content_block_stop。
    #[test]
    fn test_render_server_tool_use() {
        let mut state = MessagesRenderState::new();
        let bytes = state.render_server_tool_use(0, "toolu_1", "张雪峰 高考志愿");
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("event: content_block_start"));
        assert!(s.contains("\"type\":\"server_tool_use\""));
        assert!(s.contains("\"name\":\"web_search\""));
        assert!(s.contains("\"id\":\"toolu_1\""));
        assert!(s.contains("event: content_block_delta"));
        assert!(s.contains("input_json_delta"));
        // 查询词应在 partial_json 中（URL 编码无关，直接 JSON 字符串）
        assert!(s.contains("张雪峰"));
        assert!(s.contains("event: content_block_stop"));
        // index 一致
        assert!(s.contains("\"index\":0"));
    }

    /// web_search_tool_result 块应输出 content_block_start(type=web_search_tool_result)
    /// + content_block_stop，content 为 web_search_result 数组（title/url/encrypted_content）。
    #[test]
    fn test_render_web_search_tool_result() {
        let state = MessagesRenderState::new();
        let results = vec![
            crate::search::SearchResult {
                title: "张雪峰报志愿逻辑".into(),
                url: "https://zhuanlan.zhihu.com/p/1".into(),
                snippet: "核心观点".into(),
            },
            crate::search::SearchResult {
                title: "2026志愿指南".into(),
                url: "https://example.com/2".into(),
                snippet: "指南内容".into(),
            },
        ];
        let bytes = state.render_web_search_tool_result(1, "toolu_1", &results);
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("event: content_block_start"));
        assert!(s.contains("\"type\":\"web_search_tool_result\""));
        assert!(s.contains("\"tool_use_id\":\"toolu_1\""));
        assert!(s.contains("\"type\":\"web_search_result\""));
        assert!(s.contains("\"title\":\"张雪峰报志愿逻辑\""));
        assert!(s.contains("\"url\":\"https://zhuanlan.zhihu.com/p/1\""));
        // encrypted_content 为摘要的 base64（官方必填字段）
        assert!(s.contains("\"encrypted_content\":"));
        assert!(s.contains("event: content_block_stop"));
        // index 一致
        assert!(s.contains("\"index\":1"));
    }

    /// 空结果时 content 为空数组（不 panic，Claude Code 显示「无结果」）。
    #[test]
    fn test_render_web_search_tool_result_empty() {
        let state = MessagesRenderState::new();
        let bytes = state.render_web_search_tool_result(0, "toolu_2", &[]);
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("\"type\":\"web_search_tool_result\""));
        assert!(s.contains("\"content\":[]"));
        assert!(s.contains("event: content_block_stop"));
    }
