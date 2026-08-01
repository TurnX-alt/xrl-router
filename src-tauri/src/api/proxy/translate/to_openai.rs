//! Anthropic -> OpenAI 方向的格式翻译：请求体、流式 chunk 与 usage 提取。

use serde_json::{json, Value};
use std::collections::HashMap;

/// Streaming conversion state for Anthropic -> OpenAI chunk translation.
///
/// Anthropic tool_use blocks carry their `id`/`name` on `content_block_start`
/// and stream arguments via subsequent `input_json_delta` events, all keyed by
/// the Anthropic block `index`. OpenAI's `tool_calls` are keyed by a separate
/// 0-based `index` and expect `id`/`function.name` on the first delta. This
/// state maps Anthropic block indices onto sequential OpenAI tool_call indices
/// so a tool_use block reassembles into a valid OpenAI tool_call stream.
pub struct OaStreamState {
    /// Anthropic content_block index -> OpenAI tool_calls index (0-based).
    tool_index_map: HashMap<i64, i64>,
    /// Next OpenAI tool_calls index to hand out.
    next_tool_index: i64,
    /// Message id / model captured from message_start, reused on later chunks.
    msg_id: String,
    model: String,
}

impl OaStreamState {
    pub fn new() -> Self {
        Self {
            tool_index_map: HashMap::new(),
            next_tool_index: 0,
            msg_id: String::new(),
            model: String::new(),
        }
    }
}

impl Default for OaStreamState {
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
        let (system_content, cache_control) = match system {
            Value::String(s) => (s.clone(), None),
            Value::Array(blocks) => {
                let text = blocks
                    .iter()
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                // Anthropic 惯例：缓存断点标记在最后一个 block 上。透传给
                // 兼容该字段的 OpenAI 上游（标准 OpenAI 会忽略，无害）。
                let cc = blocks.iter().rev().find_map(|b| b.get("cache_control").cloned());
                (text, cc)
            }
            _ => (String::new(), None),
        };
        if !system_content.is_empty() {
            let mut sys = json!({"role": "system", "content": system_content});
            if let Some(cc) = cache_control {
                sys["cache_control"] = cc;
            }
            openai_req["messages"]
                .as_array_mut()
                .unwrap()
                .push(sys);
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
                }
            }

            // 只有当消息还含实质内容（text/thinking/tool_use）时才作为一条
            // 消息发出；纯 tool_result 已作为 tool role 消息加入，避免既丢文本
            // 又重复发空消息。纯字符串 content 的消息此时 has_text=true，照常发出。
            let has_text = openai_msg["content"]
                .as_str()
                .map(|s| !s.is_empty())
                .unwrap_or(false);
            let has_tool_calls = openai_msg.get("tool_calls").is_some();
            let has_reasoning = openai_msg.get("reasoning_content").is_some();
            if has_text || has_tool_calls || has_reasoning {
                openai_req["messages"]
                    .as_array_mut()
                    .unwrap()
                    .push(openai_msg);
            }
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

/// Translate a streaming chunk from Anthropic to OpenAI format.
///
/// Stateful: `content_block_start` for a tool_use emits the OpenAI tool_call
/// header (id + function.name), and later `input_json_delta` fragments route
/// back to the same OpenAI tool_call index via `state.tool_index_map`.
/// `message_delta` carries the final usage so the OpenAI client sees token
/// counts (prompt_tokens = non-cached input + cache read + cache creation,
/// matching how Anthropic bills).
pub fn translate_anthropic_chunk_to_openai(chunk: &Value, state: &mut OaStreamState) -> Value {
    let event_type = chunk["type"].as_str().unwrap_or("");
    let created = chrono::Utc::now().timestamp();
    // 宏而非闭包：闭包会持续借用 state，导致 message_start 分支无法先写入
    // state.msg_id/model 再调用。宏每次展开时借用，可先赋值后展开。
    macro_rules! mk {
        ($delta:expr, $finish:expr) => {{
            let finish: Option<&str> = $finish;
            json!({
                "id": state.msg_id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": state.model,
                "choices": [{
                    "index": 0,
                    "delta": $delta,
                    "finish_reason": finish,
                }],
            })
        }};
    }

    match event_type {
        "message_start" => {
            if let Some(id) = chunk["message"]["id"].as_str() {
                state.msg_id = id.to_string();
            }
            if let Some(m) = chunk["message"]["model"].as_str() {
                state.model = m.to_string();
            }
            mk!(json!({"role": "assistant"}), None)
        }
        "content_block_start" => {
            let idx = chunk["index"].as_i64().unwrap_or(0);
            let block = &chunk["content_block"];
            match block["type"].as_str() {
                // Emit the tool_call header (id + name); arguments stream in later.
                Some("tool_use") => {
                    let oai_idx = state.next_tool_index;
                    state.next_tool_index += 1;
                    state.tool_index_map.insert(idx, oai_idx);
                    mk!(
                        json!({
                            "tool_calls": [{
                                "index": oai_idx,
                                "id": block["id"],
                                "type": "function",
                                "function": {
                                    "name": block["name"],
                                    "arguments": "",
                                }
                            }]
                        }),
                        None
                    )
                }
                // text/thinking blocks begin implicitly in OpenAI; no event needed.
                _ => Value::Null,
            }
        }
        "content_block_delta" => {
            let idx = chunk["index"].as_i64().unwrap_or(0);
            let delta = &chunk["delta"];
            match delta["type"].as_str() {
                Some("text_delta") => mk!(json!({"content": delta["text"]}), None),
                // thinking_delta -> reasoning_content (OpenAI non-standard field)
                Some("thinking_delta") => {
                    mk!(json!({"reasoning_content": delta["thinking"]}), None)
                }
                Some("input_json_delta") => {
                    let oai_idx = state.tool_index_map.get(&idx).copied().unwrap_or(0);
                    mk!(
                        json!({
                            "tool_calls": [{
                                "index": oai_idx,
                                "function": {"arguments": delta["partial_json"]},
                            }]
                        }),
                        None
                    )
                }
                _ => Value::Null,
            }
        }
        "message_delta" => {
            let usage = &chunk["usage"];
            // prompt_tokens = 全部输入（未缓存 input + 写缓存 creation + 读缓存 read）。
            // cache_creation 并入输入（首次处理即输入）；cache_read 单列，体现缓存命中收益。
            let input_t = usage["input_tokens"].as_i64().unwrap_or(0);
            let cache_r = usage["cache_read_input_tokens"].as_i64().unwrap_or(0);
            let cache_w = usage["cache_creation_input_tokens"].as_i64().unwrap_or(0);
            let output_t = usage["output_tokens"].as_i64().unwrap_or(0);
            json!({
                "id": state.msg_id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": state.model,
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
                "usage": {
                    "prompt_tokens": input_t + cache_w + cache_r,
                    "completion_tokens": output_t,
                    "cache_read_input_tokens": cache_r,
                },
            })
        }
        _ => Value::Null,
    }
}

/// Extract token usage from an Anthropic upstream SSE event.
/// Returns `(input_tokens, output_tokens, cache_read, output_chars_delta)`:
/// - `message_start` carries `input_tokens` + cache_creation（写缓存并入输入）;
/// - `message_delta` carries the authoritative final `output_tokens` + cache read;
/// - `content_block_delta` contributes text/thinking chars for the fallback
///   estimate used when the upstream reports no token counts.
pub fn extract_anthropic_usage(chunk: &Value) -> (u64, u64, u64, u64) {
    let event_type = chunk["type"].as_str().unwrap_or("");
    match event_type {
        "message_start" => {
            let usage = &chunk["message"]["usage"];
            // input 含写缓存（cache_creation 只是首次处理输入）
            let it = usage["input_tokens"].as_u64().unwrap_or(0)
                + usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
            let cr = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
            (it, 0, cr, 0)
        }
        "message_delta" => {
            let usage = &chunk["usage"];
            let ot = usage["output_tokens"].as_u64().unwrap_or(0);
            let cr = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
            (0, ot, cr, 0)
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
            (0, 0, 0, chars)
        }
        _ => (0, 0, 0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_thinking_config_to_reasoning_effort() {
        let req = json!({
            "model": "x", "max_tokens": 1, "messages": [],
            "thinking": {"type": "enabled", "budget_tokens": 5000}
        });
        let openai = anthropic_req_to_openai(&req);
        assert_eq!(openai["reasoning_effort"], "high");
    }

    #[test]
    fn test_thinking_delta_a_to_o() {
        let chunk = json!({
            "type": "content_block_delta",
            "index": 0,
            "message_id": "msg1",
            "delta": {"type": "thinking_delta", "thinking": "partial reasoning"}
        });
        let mut state = OaStreamState::new();
        let out = translate_anthropic_chunk_to_openai(&chunk, &mut state);
        assert_eq!(out["choices"][0]["delta"]["reasoning_content"], "partial reasoning");
    }

    #[test]
    fn test_tool_use_a_to_o_header_and_args() {
        // content_block_start(tool_use) -> OpenAI tool_call header (id + name)
        let start = json!({
            "type": "content_block_start",
            "index": 2,
            "content_block": {"type": "tool_use", "id": "call_1", "name": "get_weather", "input": {}}
        });
        let mut state = OaStreamState::new();
        let out = translate_anthropic_chunk_to_openai(&start, &mut state);
        let tc = &out["choices"][0]["delta"]["tool_calls"][0];
        assert_eq!(tc["index"], 0, "OpenAI tool_call index must be 0-based");
        assert_eq!(tc["id"], "call_1");
        assert_eq!(tc["function"]["name"], "get_weather");

        // input_json_delta routes back to the same OpenAI index via the map
        let d = json!({
            "type": "content_block_delta",
            "index": 2,
            "delta": {"type": "input_json_delta", "partial_json": "{\"loc\":"}
        });
        let out2 = translate_anthropic_chunk_to_openai(&d, &mut state);
        assert_eq!(out2["choices"][0]["delta"]["tool_calls"][0]["index"], 0);
        assert_eq!(out2["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"], "{\"loc\":");
    }

    #[test]
    fn test_message_delta_a_to_o_carries_usage() {
        let md = json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn"},
            "usage": {
                "input_tokens": 200,
                "output_tokens": 500,
                "cache_read_input_tokens": 8000,
                "cache_creation_input_tokens": 1500
            }
        });
        let mut state = OaStreamState::new();
        let out = translate_anthropic_chunk_to_openai(&md, &mut state);
        // prompt_tokens = input + cache_creation(并入输入) + cache_read（全部计费输入）
        assert_eq!(out["usage"]["prompt_tokens"], 200 + 1500 + 8000);
        assert_eq!(out["usage"]["completion_tokens"], 500);
        assert_eq!(out["usage"]["cache_read_input_tokens"], 8000);
        // cache_creation 不再单列（已并入 prompt_tokens）
        assert!(out["usage"].get("cache_creation_input_tokens").is_none());
        assert_eq!(out["choices"][0]["finish_reason"], "stop");
    }
}
