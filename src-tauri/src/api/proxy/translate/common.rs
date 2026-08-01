//! 两个方向共享的小工具。

use serde_json::{json, Value};

/// 粗估 Anthropic 请求的输入 token 数，用于 message_start 的占位 usage。
/// OpenAI 上游的真实 usage 在流末尾才到；在此之前先给一个非零估算值，
/// 避免客户端（如 CCSwitch）在 message_start 阶段把 input 记成 0。
/// 估算口径：system + messages 的文本字符数 / 4（粗略，仅占位用）。
pub fn estimate_input_tokens(req: &Value) -> u64 {
    let mut chars: usize = 0;
    if let Some(s) = req.get("system") {
        match s {
            Value::String(t) => chars += t.chars().count(),
            Value::Array(blocks) => {
                for b in blocks {
                    if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                        chars += t.chars().count();
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(messages) = req.get("messages").and_then(|m| m.as_array()) {
        for msg in messages {
            match &msg["content"] {
                Value::String(t) => chars += t.chars().count(),
                Value::Array(blocks) => {
                    for b in blocks {
                        if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                            chars += t.chars().count();
                        }
                        if let Some(t) = b.get("thinking").and_then(|v| v.as_str()) {
                            chars += t.chars().count();
                        }
                    }
                }
                _ => {}
            }
        }
    }
    // 4 字符 ≈ 1 token；至少返回 1，避免占位为 0。
    ((chars / 4) as u64).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_input_tokens_nonzero() {
        let req = json!({
            "system": "You are helpful.",
            "messages": [{"role":"user","content":"Hello world, this is a test message."}]
        });
        let est = estimate_input_tokens(&req);
        assert!(est > 0, "estimate must be non-zero to avoid message_start showing 0");
    }
}
