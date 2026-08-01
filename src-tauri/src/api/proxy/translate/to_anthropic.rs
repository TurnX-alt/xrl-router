//! OpenAI -> Anthropic 方向的格式翻译：请求体、流式 chunk、收尾与状态机。

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
    /// finish_reason captured from the OpenAI stream (None until finish chunk).
    /// message_delta/message_stop are deferred to finalize() so they can carry
    /// the real usage, which arrives in a later chunk than finish_reason.
    finish_reason: Option<String>,
    /// Token usage accumulated from the upstream stream. input_tokens 含全部
    /// 新输入（未缓存 + 首次写缓存——写缓存只是首次处理输入，并入 input）。
    /// cache_read_input_tokens 是真正的「缓存」（命中复用）。output_chars
    /// accumulates emitted text/thinking char counts as a fallback (chars / 4)
    /// when the upstream reports no token counts.
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
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
            finish_reason: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_input_tokens: 0,
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
            // 各种「缓存命中」字段的统一处理：把命中部分拆成 cache_read，
            // 剩余算未缓存输入，符合 Anthropic 语义，避免重复计入 input_tokens。
            // 支持：DeepSeek/Kimi 的 prompt_cache_hit_tokens、OpenAI 标准的
            // prompt_tokens_details.cached_tokens。
            let cache_hit = usage
                .get("prompt_cache_hit_tokens")
                .and_then(|v| v.as_u64())
                .or_else(|| {
                    usage
                        .get("prompt_tokens_details")
                        .and_then(|d| d.get("cached_tokens"))
                        .and_then(|v| v.as_u64())
                });
            if let Some(hit) = cache_hit {
                state.cache_read_input_tokens = hit;
                if let Some(miss) = usage
                    .get("prompt_cache_miss_tokens")
                    .and_then(|v| v.as_u64())
                {
                    state.input_tokens = miss;
                } else {
                    state.input_tokens = pt.saturating_sub(hit);
                }
            } else {
                state.input_tokens = pt;
            }
        }
        if let Some(ct) = usage.get("completion_tokens").and_then(|v| v.as_u64()) {
            state.output_tokens = ct;
        }
        // 透传上游自报的 cache_read（若有）。
        if let Some(cr) = usage
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_u64())
        {
            state.cache_read_input_tokens = cr;
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

        // finish reason -> close every open block + remember stop_reason.
        // message_delta/message_stop are deferred to finalize() (called after
        // the stream ends), because OpenAI's usage arrives in a LATER chunk
        // than finish_reason — emitting here would report input/cache as 0.
        if let Some(finish_reason) = choice.get("finish_reason").and_then(|f| f.as_str()) {
            events.extend(state.close_open_blocks());
            state.finish_reason = Some(
                match finish_reason {
                    "stop" => "end_turn",
                    "tool_calls" => "tool_use",
                    "length" => "max_tokens",
                    _ => "end_turn",
                }
                .to_string(),
            );
        }
    }

    events
}

/// Emit the final message_delta + message_stop using the accumulated usage.
/// Call after the upstream stream has fully ended so input/cache/output are
/// authoritative (OpenAI delivers usage in the last chunk, after finish_reason).
pub fn finalize_openai_to_anthropic(state: &mut StreamState) -> Vec<Value> {
    let mut events = Vec::new();
    // Close any block still open (e.g. upstream cut off with no finish_reason).
    events.extend(state.close_open_blocks());
    let stop_reason = state
        .finish_reason
        .clone()
        .unwrap_or_else(|| "end_turn".to_string());
    let output_tokens =
        if state.output_tokens > 0 { state.output_tokens } else { state.output_chars / 4 };
    events.push(json!({
        "type": "message_delta",
        "delta": {"stop_reason": stop_reason},
        "usage": {
            "input_tokens": state.input_tokens,
            "output_tokens": output_tokens,
            "cache_read_input_tokens": state.cache_read_input_tokens,
        },
    }));
    events.push(json!({"type": "message_stop"}));
    events
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_reasoning_effort_to_thinking_config() {
        let req = json!({"model":"x","max_tokens":1,"messages":[],"reasoning_effort":"high"});
        let anthropic = openai_req_to_anthropic(&req);
        assert_eq!(anthropic["thinking"]["type"], "enabled");
    }

    // ---- streaming SSE envelope (OpenAI -> Anthropic) ----

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

        // finish chunk -> only content_block_stop (message_delta/stop deferred to finalize)
        let chunk3 = json!({
            "id": "chatcmpl-1",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
        });
        let ev3 = translate_openai_chunk_to_anthropic(&chunk3, "claude", &mut state);
        assert!(ev3.iter().any(|e| e["type"] == "content_block_stop"));
        assert!(!ev3.iter().any(|e| e["type"] == "message_delta"));
        assert!(!ev3.iter().any(|e| e["type"] == "message_stop"));

        // finalize -> message_delta(stop_reason) + message_stop, with usage
        let fin = finalize_openai_to_anthropic(&mut state);
        let md = fin.iter().find(|e| e["type"] == "message_delta").expect("message_delta");
        assert_eq!(md["delta"]["stop_reason"], "end_turn");
        assert!(fin.iter().any(|e| e["type"] == "message_stop"));
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
    fn test_usage_chunk_after_finish_reflected_in_finalize() {
        // Regression: OpenAI delivers usage in a chunk AFTER finish_reason.
        // message_delta must not be emitted at finish; finalize carries the
        // real usage that arrives later, so input/cache are not reported as 0.
        let mut state = StreamState::new();
        // finish chunk (no usage yet)
        let fin_chunk = json!({"id":"x","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
        let ev = translate_openai_chunk_to_anthropic(&fin_chunk, "m", &mut state);
        assert!(!ev.iter().any(|e| e["type"] == "message_delta"));
        assert!(!ev.iter().any(|e| e["type"] == "message_stop"));

        // usage chunk arrives after finish (choices empty, usage present)
        let usage_chunk = json!({
            "id":"x","choices":[],
            "usage":{
                "prompt_tokens": 9700,
                "completion_tokens": 300,
                "prompt_tokens_details": {"cached_tokens": 8000}
            }
        });
        translate_openai_chunk_to_anthropic(&usage_chunk, "m", &mut state);

        let fin = finalize_openai_to_anthropic(&mut state);
        let md = fin.iter().find(|e| e["type"] == "message_delta").expect("message_delta");
        // DeepSeek-style: prompt_cache_hit=8000 -> cache_read=8000, input=miss=1700
        assert_eq!(md["usage"]["cache_read_input_tokens"], 8000);
        assert_eq!(md["usage"]["input_tokens"], 1700);
        assert_eq!(md["usage"]["output_tokens"], 300);
        assert_eq!(md["delta"]["stop_reason"], "end_turn");
    }

    #[test]
    fn test_message_start_carries_estimate_not_zero() {
        let mut state = StreamState::new();
        state.input_tokens = 42; // proxy sets an estimate before streaming
        let chunk = json!({"id":"x","choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":null}]});
        let ev = translate_openai_chunk_to_anthropic(&chunk, "m", &mut state);
        let ms = ev.iter().find(|e| e["type"] == "message_start").expect("message_start");
        assert_eq!(ms["message"]["usage"]["input_tokens"], 42, "message_start must use the estimate, not 0");
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
        // finish -> tool block closed (stop idx 2); message_delta/stop deferred to finalize
        let c5 = json!({"id":"x","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]});
        let ev5 = translate_openai_chunk_to_anthropic(&c5, "m", &mut state);
        assert!(ev5.iter().any(|e| e["type"] == "content_block_stop" && e["index"] == 2));
        let fin = finalize_openai_to_anthropic(&mut state);
        let md = fin.iter().find(|e| e["type"] == "message_delta").expect("message_delta");
        assert_eq!(md["delta"]["stop_reason"], "tool_use");
        assert!(fin.iter().any(|e| e["type"] == "message_stop"));
    }
}
