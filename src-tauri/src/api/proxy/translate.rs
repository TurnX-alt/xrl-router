use serde_json::{json, Value};
use std::collections::HashMap;

/// Streaming conversion state for OpenAI -> Anthropic chunk translation.
///
/// Anthropic content blocks are strictly sequential and non-overlapping: a
/// block must be `content_block_start`-ed before any delta, every delta must
/// target the currently-open block, and the block must be `content_block_stop`-ed
/// before the next begins. OpenAI streams routinely violate this:
///   - qwen3.7-max interleaves `reasoning_content` and `content` across chunks;
///   - tool_call `index` values are independent of any thinking/text and reuse 0.
///
/// This state remaps the OpenAI stream onto a valid Anthropic block sequence —
/// thinking (if any reasoning arrives before text/tools), then text (if any
/// content), then one tool_use block per tool call — each at a unique
/// monotonically-increasing index. Reasoning that arrives after text/tools has
/// started is dropped, since Anthropic cannot represent interleaved thinking.
pub struct StreamState {
    started: bool,
    /// Next content_block index to hand out (monotonic).
    next_index: i64,
    /// A thinking block is currently open (at `thinking_index`).
    thinking_open: bool,
    thinking_index: i64,
    /// A thinking block was opened at least once.
    thinking_used: bool,
    /// A text block is currently open (at `text_index`).
    text_open: bool,
    text_index: i64,
    /// OpenAI tool_call index -> Anthropic block index (routes argument deltas).
    tool_index_map: HashMap<i64, i64>,
    /// Anthropic indices of tool blocks still open (closed at finish/synthesize).
    open_tool_blocks: Vec<i64>,
    /// Token usage accumulated from the upstream stream. input_tokens is set
    /// from the upstream usage object (when available); output_tokens likewise.
    /// output_chars accumulates emitted text/thinking char counts as a fallback
    /// (chars / 4) when the upstream reports no token counts.
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub output_chars: u64,
}

impl StreamState {
    pub fn new() -> Self {
        Self {
            started: false,
            next_index: 0,
            thinking_open: false,
            thinking_index: 0,
            thinking_used: false,
            text_open: false,
            text_index: 0,
            tool_index_map: HashMap::new(),
            open_tool_blocks: Vec::new(),
            input_tokens: 0,
            output_tokens: 0,
            output_chars: 0,
        }
    }

    /// Allocate the next sequential content_block index.
    fn alloc_index(&mut self) -> i64 {
        let i = self.next_index;
        self.next_index += 1;
        i
    }

    /// Whether any non-thinking block (text or tool) has started — once true,
    /// the thinking block (if opened) is closed and further reasoning is dropped.
    fn body_started(&self) -> bool {
        self.text_open || !self.open_tool_blocks.is_empty()
    }

    /// Emit `content_block_stop` for every currently-open content block, without
    /// touching the message envelope. Used both at finish_reason and when the
    /// upstream stream ends without one — an unclosed block makes Claude Code
    /// reject the stream as malformed.
    pub fn close_open_blocks(&mut self) -> Vec<Value> {
        let mut events = Vec::new();
        if self.thinking_open {
            events.push(json!({"type": "content_block_stop", "index": self.thinking_index}));
            self.thinking_open = false;
        }
        if self.text_open {
            events.push(json!({"type": "content_block_stop", "index": self.text_index}));
            self.text_open = false;
        }
        for idx in self.open_tool_blocks.drain(..) {
            events.push(json!({"type": "content_block_stop", "index": idx}));
        }
        events
    }
}

impl Default for StreamState {
    fn default() -> Self {
        Self::new()
    }
}

/// Translate Anthropic request to OpenAI format.
pub fn anthropic_req_to_openai(req: &Value) -> Value {
    let mut openai_req = json!({
        "model": req["model"],
        "messages": [],
        "stream": req.get("stream").and_then(|v| v.as_bool()).unwrap_or(false),
    });

    // Convert system prompt to messages[0]
    if let Some(system) = req.get("system") {
        let system_content = match system {
            Value::String(s) => s.clone(),
            Value::Array(blocks) => blocks
                .iter()
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        if !system_content.is_empty() {
            openai_req["messages"]
                .as_array_mut()
                .unwrap()
                .push(json!({"role": "system", "content": system_content}));
        }
    }

    // Convert messages
    if let Some(messages) = req.get("messages").and_then(|m| m.as_array()) {
        for msg in messages {
            let role = msg["role"].as_str().unwrap_or("user");
            let content = match &msg["content"] {
                Value::String(s) => json!(s),
                Value::Array(blocks) => {
                    // Convert content blocks to plain text
                    let text: String = blocks
                        .iter()
                        .filter_map(|b| {
                            if b["type"].as_str() == Some("text") {
                                b["text"].as_str()
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    json!(text)
                }
                _ => json!(""),
            };

            let mut openai_msg = json!({
                "role": role,
                "content": content,
            });

            // thinking blocks -> reasoning_content; tool_use -> tool_calls; tool_result -> tool messages
            if let Some(blocks) = msg["content"].as_array() {
                // reasoning_content from thinking blocks (assistant messages)
                let reasoning: String = blocks
                    .iter()
                    .filter_map(|b| {
                        if b["type"].as_str() == Some("thinking") {
                            b["thinking"].as_str()
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !reasoning.is_empty() {
                    openai_msg["reasoning_content"] = json!(reasoning);
                }

                // tool_use blocks -> tool_calls
                let tool_calls: Vec<Value> = blocks
                    .iter()
                    .filter(|b| b["type"].as_str() == Some("tool_use"))
                    .map(|b| {
                        json!({
                            "id": b["id"],
                            "type": "function",
                            "function": {
                                "name": b["name"],
                                "arguments": b["input"].to_string(),
                            }
                        })
                    })
                    .collect();
                if !tool_calls.is_empty() {
                    openai_msg["tool_calls"] = json!(tool_calls);
                }

                // tool_result -> tool role messages
                let tool_results: Vec<Value> = blocks
                    .iter()
                    .filter(|b| b["type"].as_str() == Some("tool_result"))
                    .map(|b| {
                        json!({
                            "role": "tool",
                            "tool_call_id": b["tool_use_id"],
                            "content": b["content"].as_str().unwrap_or(""),
                        })
                    })
                    .collect();
                if !tool_results.is_empty() {
                    openai_req["messages"]
                        .as_array_mut()
                        .unwrap()
                        .extend(tool_results);
                    continue;
                }
            }

            openai_req["messages"]
                .as_array_mut()
                .unwrap()
                .push(openai_msg);
        }
    }

    // Convert tools
    if let Some(tools) = req.get("tools").and_then(|t| t.as_array()) {
        let openai_tools: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t["name"],
                        "description": t.get("description").unwrap_or(&json!("")),
                        "parameters": t.get("input_schema").unwrap_or(&json!({})),
                    }
                })
            })
            .collect();
        openai_req["tools"] = json!(openai_tools);
    }

    // Convert tool_choice
    if let Some(tool_choice) = req.get("tool_choice") {
        let openai_tool_choice = match tool_choice {
            Value::String(s) => match s.as_str() {
                "auto" => json!("auto"),
                "any" => json!("required"),
                "none" => json!("none"),
                _ => json!("auto"),
            },
            Value::Object(obj) => {
                if obj.get("type").and_then(|t| t.as_str()) == Some("tool") {
                    if let Some(name) = obj.get("name").and_then(|n| n.as_str()) {
                        json!({"type": "function", "function": {"name": name}})
                    } else {
                        json!("auto")
                    }
                } else {
                    json!("auto")
                }
            }
            _ => json!("auto"),
        };
        openai_req["tool_choice"] = openai_tool_choice;
    }

    // thinking config -> reasoning_effort
    if let Some(thinking) = req.get("thinking") {
        if thinking.get("type").and_then(|t| t.as_str()) == Some("enabled") {
            openai_req["reasoning_effort"] = json!("high");
        }
    }

    // Pass through max_tokens, temperature, top_p
    if let Some(max_tokens) = req.get("max_tokens") {
        openai_req["max_tokens"] = max_tokens.clone();
    }
    if let Some(temperature) = req.get("temperature") {
        openai_req["temperature"] = temperature.clone();
    }
    if let Some(top_p) = req.get("top_p") {
        openai_req["top_p"] = top_p.clone();
    }

    openai_req
}

/// Translate OpenAI request to Anthropic format.
pub fn openai_req_to_anthropic(req: &Value) -> Value {
    let mut anthropic_req = json!({
        "model": req["model"],
        "max_tokens": req.get("max_tokens").or_else(|| req.get("max_completion_tokens")).unwrap_or(&json!(4096)),
        "stream": req.get("stream").and_then(|v| v.as_bool()).unwrap_or(false),
        "messages": [],
    });

    // Convert messages
    if let Some(messages) = req.get("messages").and_then(|m| m.as_array()) {
        let mut system_prompts = Vec::new();

        for msg in messages {
            let role = msg["role"].as_str().unwrap_or("user");

            if role == "system" {
                if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                    system_prompts.push(content.to_string());
                }
                continue;
            }

            let content = match msg.get("content") {
                Some(Value::String(s)) => json!(s),
                Some(Value::Array(parts)) => {
                    let blocks: Vec<Value> = parts
                        .iter()
                        .filter_map(|p| match p["type"].as_str() {
                            Some("text") => Some(json!({"type": "text", "text": p["text"]})),
                            Some("image_url") => {
                                Some(json!({"type": "image", "source": p["image_url"]}))
                            }
                            _ => None,
                        })
                        .collect();
                    json!(blocks)
                }
                _ => json!(""),
            };

            let mut anthropic_msg = json!({
                "role": role,
                "content": content,
            });

            // reasoning_content -> thinking block (assistant messages)
            if let Some(rc) = msg.get("reasoning_content").and_then(|r| r.as_str()) {
                if !rc.is_empty() {
                    let thinking_block = json!({"type": "thinking", "thinking": rc});
                    match &anthropic_msg["content"] {
                        Value::Array(_) => {
                            anthropic_msg["content"]
                                .as_array_mut()
                                .unwrap()
                                .insert(0, thinking_block);
                        }
                        other => {
                            let text = other.clone();
                            anthropic_msg["content"] =
                                json!([thinking_block, {"type": "text", "text": text}]);
                        }
                    }
                }
            }

            // tool_calls -> tool_use blocks
            if let Some(tool_calls) = msg.get("tool_calls").and_then(|t| t.as_array()) {
                let blocks: Vec<Value> = tool_calls
                    .iter()
                    .map(|tc| {
                        json!({
                            "type": "tool_use",
                            "id": tc["id"],
                            "name": tc["function"]["name"],
                            "input": tc["function"]
                                .get("arguments")
                                .and_then(|a| a.as_str())
                                .and_then(|s| serde_json::from_str(s).ok())
                                .unwrap_or(json!({})),
                        })
                    })
                    .collect();
                if !blocks.is_empty() {
                    anthropic_msg["content"] = json!(blocks);
                }
            }

            // tool role messages -> tool_result
            if role == "tool" {
                anthropic_msg = json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": msg.get("tool_call_id").unwrap_or(&json!("")),
                        "content": msg.get("content").unwrap_or(&json!("")),
                    }],
                });
            }

            anthropic_req["messages"]
                .as_array_mut()
                .unwrap()
                .push(anthropic_msg);
        }

        if !system_prompts.is_empty() {
            anthropic_req["system"] = json!(system_prompts.join("\n"));
        }
    }

    // Convert tools
    if let Some(tools) = req.get("tools").and_then(|t| t.as_array()) {
        let anthropic_tools: Vec<Value> = tools
            .iter()
            .filter_map(|t| {
                if t["type"].as_str() == Some("function") {
                    Some(json!({
                        "name": t["function"]["name"],
                        "description": t["function"].get("description").unwrap_or(&json!("")),
                        "input_schema": t["function"]
                            .get("parameters")
                            .unwrap_or(&json!({"type": "object", "properties": {}})),
                    }))
                } else {
                    None
                }
            })
            .collect();
        if !anthropic_tools.is_empty() {
            anthropic_req["tools"] = json!(anthropic_tools);
        }
    }

    // Convert tool_choice
    if let Some(tool_choice) = req.get("tool_choice") {
        let anthropic_tool_choice = match tool_choice {
            Value::String(s) => match s.as_str() {
                "auto" => json!({"type": "auto"}),
                "required" => json!({"type": "any"}),
                "none" => json!({"type": "none"}),
                _ => json!({"type": "auto"}),
            },
            Value::Object(obj) => {
                if obj.get("type").and_then(|t| t.as_str()) == Some("function") {
                    if let Some(name) = obj
                        .get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(|n| n.as_str())
                    {
                        json!({"type": "tool", "name": name})
                    } else {
                        json!({"type": "auto"})
                    }
                } else {
                    json!({"type": "auto"})
                }
            }
            _ => json!({"type": "auto"}),
        };
        anthropic_req["tool_choice"] = anthropic_tool_choice;
    }

    // reasoning_effort -> thinking config
    if let Some(effort) = req.get("reasoning_effort").and_then(|e| e.as_str()) {
        if effort == "high" || effort == "medium" {
            anthropic_req["thinking"] = json!({"type": "enabled", "budget_tokens": 10000});
        }
    }

    // Pass through temperature, top_p
    if let Some(temperature) = req.get("temperature") {
        anthropic_req["temperature"] = temperature.clone();
    }
    if let Some(top_p) = req.get("top_p") {
        anthropic_req["top_p"] = top_p.clone();
    }

    anthropic_req
}

/// Translate a streaming chunk from Anthropic to OpenAI format.
pub fn translate_anthropic_chunk_to_openai(chunk: &Value) -> Value {
    let event_type = chunk["type"].as_str().unwrap_or("");

    match event_type {
        "message_start" => {
            json!({
                "id": chunk["message"]["id"],
                "object": "chat.completion.chunk",
                "created": chrono::Utc::now().timestamp(),
                "model": chunk["message"]["model"],
                "choices": [{
                    "index": 0,
                    "delta": {"role": "assistant"},
                    "finish_reason": null,
                }],
            })
        }
        "content_block_delta" => {
            let delta = &chunk["delta"];
            let delta_type = delta["type"].as_str().unwrap_or("");

            match delta_type {
                "text_delta" => json!({
                    "id": chunk.get("message_id").unwrap_or(&json!("")),
                    "object": "chat.completion.chunk",
                    "created": chrono::Utc::now().timestamp(),
                    "model": "",
                    "choices": [{
                        "index": 0,
                        "delta": {"content": delta["text"]},
                        "finish_reason": null,
                    }],
                }),
                "input_json_delta" => json!({
                    "id": chunk.get("message_id").unwrap_or(&json!("")),
                    "object": "chat.completion.chunk",
                    "created": chrono::Utc::now().timestamp(),
                    "model": "",
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "tool_calls": [{
                                "index": chunk["index"],
                                "function": {"arguments": delta["partial_json"]},
                            }]
                        },
                        "finish_reason": null,
                    }],
                }),
                // thinking_delta -> reasoning_content (OpenAI non-standard field)
                "thinking_delta" => json!({
                    "id": chunk.get("message_id").unwrap_or(&json!("")),
                    "object": "chat.completion.chunk",
                    "created": chrono::Utc::now().timestamp(),
                    "model": "",
                    "choices": [{
                        "index": 0,
                        "delta": {"reasoning_content": delta["thinking"]},
                        "finish_reason": null,
                    }],
                }),
                _ => Value::Null,
            }
        }
        "message_delta" => {
            json!({
                "id": chunk.get("message_id").unwrap_or(&json!("")),
                "object": "chat.completion.chunk",
                "created": chrono::Utc::now().timestamp(),
                "model": "",
                "choices": [{
                    "index": 0,
                    "delta": {},
                    "finish_reason": match chunk["delta"]["stop_reason"].as_str() {
                        Some("end_turn") => "stop",
                        Some("tool_use") => "tool_calls",
                        Some("max_tokens") => "length",
                        _ => "stop",
                    },
                }],
            })
        }
        _ => Value::Null,
    }
}

/// Extract token usage from an Anthropic upstream SSE event.
/// Returns `(input_tokens, output_tokens, output_chars_delta)`:
/// - `message_start` carries `input_tokens` (fires once at stream open);
/// - `message_delta` carries the authoritative final `output_tokens`;
/// - `content_block_delta` contributes text/thinking chars for the fallback
///   estimate used when the upstream reports no token counts.
pub fn extract_anthropic_usage(chunk: &Value) -> (u64, u64, u64) {
    let event_type = chunk["type"].as_str().unwrap_or("");
    match event_type {
        "message_start" => {
            let it = chunk["message"]["usage"]["input_tokens"]
                .as_u64()
                .unwrap_or(0);
            (it, 0, 0)
        }
        "message_delta" => {
            let ot = chunk["usage"]["output_tokens"]
                .as_u64()
                .unwrap_or(0);
            (0, ot, 0)
        }
        "content_block_delta" => {
            let delta = &chunk["delta"];
            let chars = match delta["type"].as_str() {
                Some("text_delta") => {
                    delta["text"].as_str().map(|s| s.chars().count() as u64).unwrap_or(0)
                }
                Some("thinking_delta") => {
                    delta["thinking"].as_str().map(|s| s.chars().count() as u64).unwrap_or(0)
                }
                _ => 0,
            };
            (0, 0, chars)
        }
        _ => (0, 0, 0),
    }
}

/// Translate a streaming chunk from OpenAI to Anthropic format.
///
/// Returns a `Vec` because one OpenAI chunk may map to several Anthropic SSE
/// events (e.g. the first text delta emits message_start + content_block_start +
/// content_block_delta). The caller maintains `state` across chunks so the full
/// envelope (message_start ... message_stop) is produced.
pub fn translate_openai_chunk_to_anthropic(
    chunk: &Value,
    model: &str,
    state: &mut StreamState,
) -> Vec<Value> {
    let mut events = Vec::new();
    let msg_id = chunk.get("id").and_then(|v| v.as_str()).unwrap_or("msg_unknown");

    // Capture real usage from the upstream's final chunk (present when the
    // request asked for stream_options.include_usage). Overwrites on each
    // occurrence so the final value wins.
    if let Some(usage) = chunk.get("usage") {
        if let Some(pt) = usage.get("prompt_tokens").and_then(|v| v.as_u64()) {
            state.input_tokens = pt;
        }
        if let Some(ct) = usage.get("completion_tokens").and_then(|v| v.as_u64()) {
            state.output_tokens = ct;
        }
    }

    if !state.started {
        state.started = true;
        events.push(json!({
            "type": "message_start",
            "message": {
                "id": msg_id,
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": {"input_tokens": state.input_tokens, "output_tokens": 0},
            }
        }));
    }

    let choice = chunk
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first());

    if let Some(choice) = choice {
        let delta = &choice["delta"];

        // reasoning_content -> thinking block. Only emit while the thinking
        // block is still open (i.e. before any text/tool has started). Once the
        // body has begun, Anthropic cannot represent interleaved thinking, so
        // later reasoning fragments are dropped to keep the stream valid.
        if let Some(rc) = delta.get("reasoning_content").and_then(|r| r.as_str()) {
            if !state.thinking_used && !state.body_started() {
                state.thinking_used = true;
                state.thinking_open = true;
                state.thinking_index = state.alloc_index();
                events.push(json!({
                    "type": "content_block_start",
                    "index": state.thinking_index,
                    "content_block": {"type": "thinking", "thinking": ""},
                }));
            }
            if state.thinking_open {
                events.push(json!({
                    "type": "content_block_delta",
                    "index": state.thinking_index,
                    "delta": {"type": "thinking_delta", "thinking": rc},
                }));
            }
            // Count toward the fallback estimate even if the reasoning was
            // dropped (it still consumed upstream output tokens).
            state.output_chars += rc.chars().count() as u64;
        }

        // text content -> close any open thinking block, then a text block at
        // the next sequential index.
        if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
            if state.thinking_open {
                events.push(json!({"type": "content_block_stop", "index": state.thinking_index}));
                state.thinking_open = false;
            }
            if !state.text_open {
                state.text_open = true;
                state.text_index = state.alloc_index();
                events.push(json!({
                    "type": "content_block_start",
                    "index": state.text_index,
                    "content_block": {"type": "text", "text": ""},
                }));
            }
            events.push(json!({
                "type": "content_block_delta",
                "index": state.text_index,
                "delta": {"type": "text_delta", "text": content},
            }));
            state.output_chars += content.chars().count() as u64;
        }

        // tool calls -> close any open thinking/text block (transition to the
        // tool phase), then one tool_use block per call at a unique index.
        // OpenAI's tool `index` is mapped to our Anthropic index so subsequent
        // argument fragments route back to the same block (OpenAI reuses 0,
        // which would otherwise collide with the thinking/text block).
        if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
            for tc in tool_calls {
                let oai_index = tc.get("index").and_then(|i| i.as_i64()).unwrap_or(0);
                if tc.get("id").is_some() {
                    // New tool: close any open thinking/text block first.
                    if state.thinking_open {
                        events.push(json!({"type": "content_block_stop", "index": state.thinking_index}));
                        state.thinking_open = false;
                    }
                    if state.text_open {
                        events.push(json!({"type": "content_block_stop", "index": state.text_index}));
                        state.text_open = false;
                    }
                    let anth_index = state.alloc_index();
                    state.tool_index_map.insert(oai_index, anth_index);
                    state.open_tool_blocks.push(anth_index);
                    events.push(json!({
                        "type": "content_block_start",
                        "index": anth_index,
                        "content_block": {
                            "type": "tool_use",
                            "id": tc["id"],
                            "name": tc["function"]["name"],
                            "input": {},
                        }
                    }));
                } else if let Some(args) = tc
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(|a| a.as_str())
                {
                    if let Some(&anth_index) = state.tool_index_map.get(&oai_index) {
                        events.push(json!({
                            "type": "content_block_delta",
                            "index": anth_index,
                            "delta": {"type": "input_json_delta", "partial_json": args},
                        }));
                    }
                }
            }
        }

        // finish reason -> close every open block + message_delta + message_stop.
        if let Some(finish_reason) = choice.get("finish_reason").and_then(|f| f.as_str()) {
            events.extend(state.close_open_blocks());
            events.push(json!({
                "type": "message_delta",
                "delta": {
                    "stop_reason": match finish_reason {
                        "stop" => "end_turn",
                        "tool_calls" => "tool_use",
                        "length" => "max_tokens",
                        _ => "end_turn",
                    }
                },
                "usage": {"output_tokens": if state.output_tokens > 0 { state.output_tokens } else { state.output_chars / 4 }},
            }));
            events.push(json!({"type": "message_stop"}));
        }
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- D3: thinking <-> reasoning_content ----

    #[test]
    fn test_thinking_block_to_reasoning_content() {
        let req = json!({
            "model": "gpt-4o",
            "max_tokens": 100,
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "let me think"},
                    {"type": "text", "text": "answer"},
                ]
            }]
        });
        let openai = anthropic_req_to_openai(&req);
        let msg = &openai["messages"][0];
        assert_eq!(msg["reasoning_content"], "let me think");
        assert_eq!(msg["content"], "answer");
    }

    #[test]
    fn test_reasoning_content_to_thinking_block() {
        let req = json!({
            "model": "claude",
            "max_tokens": 100,
            "messages": [{
                "role": "assistant",
                "content": "answer",
                "reasoning_content": "reasoning here"
            }]
        });
        let anthropic = openai_req_to_anthropic(&req);
        let blocks = anthropic["messages"][0]["content"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "thinking");
        assert_eq!(blocks[0]["thinking"], "reasoning here");
    }

    #[test]
    fn test_thinking_config_to_reasoning_effort() {
        let req = json!({
            "model": "x", "max_tokens": 1, "messages": [],
            "thinking": {"type": "enabled", "budget_tokens": 5000}
        });
        let openai = anthropic_req_to_openai(&req);
        assert_eq!(openai["reasoning_effort"], "high");
    }

    #[test]
    fn test_reasoning_effort_to_thinking_config() {
        let req = json!({"model":"x","max_tokens":1,"messages":[],"reasoning_effort":"high"});
        let anthropic = openai_req_to_anthropic(&req);
        assert_eq!(anthropic["thinking"]["type"], "enabled");
    }

    // ---- D4: streaming SSE envelope (OpenAI -> Anthropic) ----

    #[test]
    fn test_sse_envelope_full_sequence() {
        let mut state = StreamState::new();

        // first text delta -> message_start + content_block_start + content_block_delta
        let chunk1 = json!({
            "id": "chatcmpl-1",
            "choices": [{"index": 0, "delta": {"content": "Hello"}, "finish_reason": null}]
        });
        let ev1 = translate_openai_chunk_to_anthropic(&chunk1, "claude", &mut state);
        assert!(ev1.iter().any(|e| e["type"] == "message_start"));
        assert!(ev1.iter().any(|e| e["type"] == "content_block_start"));
        assert!(ev1.iter().any(|e| e["type"] == "content_block_delta"));

        // second text delta -> only content_block_delta (no second message_start)
        let chunk2 = json!({
            "id": "chatcmpl-1",
            "choices": [{"index": 0, "delta": {"content": " world"}, "finish_reason": null}]
        });
        let ev2 = translate_openai_chunk_to_anthropic(&chunk2, "claude", &mut state);
        assert!(!ev2.iter().any(|e| e["type"] == "message_start"));
        assert!(ev2.iter().any(|e| e["type"] == "content_block_delta"));

        // finish chunk -> content_block_stop + message_delta(stop_reason) + message_stop
        let chunk3 = json!({
            "id": "chatcmpl-1",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
        });
        let ev3 = translate_openai_chunk_to_anthropic(&chunk3, "claude", &mut state);
        assert!(ev3.iter().any(|e| e["type"] == "content_block_stop"));
        let md = ev3.iter().find(|e| e["type"] == "message_delta").expect("message_delta");
        assert_eq!(md["delta"]["stop_reason"], "end_turn");
        assert!(ev3.iter().any(|e| e["type"] == "message_stop"));
    }

    #[test]
    fn test_thinking_delta_a_to_o() {
        let chunk = json!({
            "type": "content_block_delta",
            "index": 0,
            "message_id": "msg1",
            "delta": {"type": "thinking_delta", "thinking": "partial reasoning"}
        });
        let out = translate_anthropic_chunk_to_openai(&chunk);
        assert_eq!(out["choices"][0]["delta"]["reasoning_content"], "partial reasoning");
    }

    #[test]
    fn test_reasoning_content_o_to_a_thinking_delta() {
        let mut state = StreamState::new();

        // reasoning chunk -> must open a thinking block (content_block_start)
        // BEFORE the thinking_delta, else Claude Code rejects the stream.
        let chunk = json!({
            "id": "chatcmpl-1",
            "choices": [{"index": 0, "delta": {"reasoning_content": "thinking..."}, "finish_reason": null}]
        });
        let evs = translate_openai_chunk_to_anthropic(&chunk, "claude", &mut state);
        assert!(evs.iter().any(|e| e["type"] == "message_start"));
        let start_pos = evs
            .iter()
            .position(|e| e["type"] == "content_block_start" && e["content_block"]["type"] == "thinking")
            .expect("thinking content_block_start");
        let delta_pos = evs
            .iter()
            .position(|e| e["delta"]["type"] == "thinking_delta")
            .expect("thinking_delta");
        assert!(start_pos < delta_pos, "content_block_start must precede thinking_delta");
        assert_eq!(evs[start_pos]["index"], 0);

        // subsequent text chunk -> close thinking (index 0), open text at index 1
        let chunk2 = json!({
            "id": "chatcmpl-1",
            "choices": [{"index": 0, "delta": {"content": "answer"}, "finish_reason": null}]
        });
        let evs2 = translate_openai_chunk_to_anthropic(&chunk2, "claude", &mut state);
        assert!(
            evs2.iter().any(|e| e["type"] == "content_block_stop" && e["index"] == 0),
            "thinking block must be closed when text begins"
        );
        let text_start = evs2
            .iter()
            .find(|e| e["type"] == "content_block_start" && e["content_block"]["type"] == "text")
            .expect("text content_block_start");
        assert_eq!(text_start["index"], 1, "text block must be at index 1 after thinking");
        assert!(evs2.iter().any(|e| e["delta"]["type"] == "text_delta" && e["index"] == 1));
    }

    #[test]
    fn test_interleaved_reasoning_dropped_after_text() {
        let mut state = StreamState::new();
        // reasoning -> thinking opens at index 0
        let c1 = json!({"id":"x","choices":[{"index":0,"delta":{"reasoning_content":"first"},"finish_reason":null}]});
        let ev1 = translate_openai_chunk_to_anthropic(&c1, "m", &mut state);
        assert!(ev1.iter().any(|e| e["delta"]["type"] == "thinking_delta"));
        // text -> closes thinking
        let c2 = json!({"id":"x","choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":null}]});
        translate_openai_chunk_to_anthropic(&c2, "m", &mut state);
        // more reasoning AFTER text started -> must be dropped (Anthropic can't
        // interleave thinking with text); the chunk produces no events.
        let c3 = json!({"id":"x","choices":[{"index":0,"delta":{"reasoning_content":"late"},"finish_reason":null}]});
        let ev3 = translate_openai_chunk_to_anthropic(&c3, "m", &mut state);
        assert!(
            !ev3.iter().any(|e| e["delta"]["type"] == "thinking_delta"),
            "reasoning after text must be dropped to keep the stream valid"
        );
        assert!(ev3.is_empty(), "a pure late-reasoning chunk should emit nothing");
    }

    #[test]
    fn test_tool_use_unique_index_and_close() {
        let mut state = StreamState::new();
        // reasoning -> thinking at 0
        let c1 = json!({"id":"x","choices":[{"index":0,"delta":{"reasoning_content":"think"},"finish_reason":null}]});
        translate_openai_chunk_to_anthropic(&c1, "m", &mut state);
        // text -> text at 1 (thinking closed)
        let c2 = json!({"id":"x","choices":[{"index":0,"delta":{"content":"ans"},"finish_reason":null}]});
        translate_openai_chunk_to_anthropic(&c2, "m", &mut state);
        // new tool (OpenAI index 0) -> must land at Anthropic index 2, NOT 0
        let c3 = json!({"id":"x","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"foo","arguments":""}}]},"finish_reason":null}]});
        let ev3 = translate_openai_chunk_to_anthropic(&c3, "m", &mut state);
        let ts = ev3
            .iter()
            .find(|e| e["type"] == "content_block_start" && e["content_block"]["type"] == "tool_use")
            .expect("tool_use content_block_start");
        assert_eq!(ts["index"], 2, "tool block must avoid colliding with thinking(0)/text(1)");
        assert_eq!(ts["content_block"]["id"], "call_1");
        // argument continuation -> routes back to the same Anthropic index 2
        let c4 = json!({"id":"x","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"a\":1}"}}]},"finish_reason":null}]});
        let ev4 = translate_openai_chunk_to_anthropic(&c4, "m", &mut state);
        let d = ev4
            .iter()
            .find(|e| e["delta"]["type"] == "input_json_delta")
            .expect("input_json_delta");
        assert_eq!(d["index"], 2);
        // finish -> tool block closed (stop idx 2), stop_reason tool_use, message_stop
        let c5 = json!({"id":"x","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]});
        let ev5 = translate_openai_chunk_to_anthropic(&c5, "m", &mut state);
        assert!(ev5.iter().any(|e| e["type"] == "content_block_stop" && e["index"] == 2));
        let md = ev5.iter().find(|e| e["type"] == "message_delta").expect("message_delta");
        assert_eq!(md["delta"]["stop_reason"], "tool_use");
        assert!(ev5.iter().any(|e| e["type"] == "message_stop"));
    }
}
