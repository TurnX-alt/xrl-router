//! WebSearch 劫持：本地 Bing 搜索 → 注入 IR system → 委托正常流式转发。
//!
//! 跳过 LLM tool-calling loop：代理自己提取搜索关键词（取最后一条 user 消息文本），
//! 本地跑 Bing 搜索，把结果作为 system block 注入 IR，清除 tools/tool_choice，
//! 然后交回 proxy_stream 正常流式转发给上游 LLM。
//!
//! 优势：省掉一轮 LLM 非流式调用 + key failover 由 proxy_stream 天然支持。

use tracing::warn;

use super::ir::types::*;

/// IR 请求的 tools 里是否含 server-side web_search 工具。
pub(super) fn has_websearch_tool_ir(req: &IrRequest) -> bool {
    tracing::debug!(tools_count = req.tools.len(), "Checking for websearch tool");
    for tool in &req.tools {
        tracing::debug!(tool_name = %tool.name, "  Found tool");
    }
    req.tools
        .iter()
        .any(|t| t.name.starts_with("web_search"))
}

/// 把 Bing 结果格式化成喂给 LLM 的文本。
fn format_search_text(results: &[crate::search::SearchResult]) -> String {
    results
        .iter()
        .enumerate()
        .map(|(i, r)| format!("[{}] {}\n{}\n{}", i + 1, r.title, r.url, r.snippet))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 本地 Bing 搜索 → 注入 IR system → 清除 tools/tool_choice。
///
/// 返回修改后的 IrRequest，直接传给 proxy_stream 正常流式转发。
/// 不再需要 tool-calling loop：搜索决策完全本地完成。
pub(super) async fn enrich_ir_with_search(mut ir_request: IrRequest) -> IrRequest {
    // 1. 提取搜索关键词
    let query = extract_search_query(&ir_request.messages);
    tracing::info!(query = %query, messages_count = ir_request.messages.len(), "websearch: extracted query");

    // 2. 本地 Bing 搜索
    let search_text = match crate::search::bing::search(&query).await {
        Ok(results) if results.is_empty() => {
            tracing::warn!(query = %query, "websearch: Bing returned 0 results");
            format!("No web search results found for: {}", query)
        }
        Ok(results) => {
            tracing::info!(query = %query, results_count = results.len(), "websearch: Bing search succeeded");
            format_search_text(&results)
        }
        Err(e) => {
            tracing::warn!(query = %query, error = %e, "websearch: Bing search failed");
            format!("Web search unavailable: {}. Do NOT make up information. Inform the user that the search is temporarily unavailable.", e)
        }
    };

    // 3. 搜索结果注入 system prompt
    let search_block = IrSystemBlock {
        text: format!(
            "[Web Search Results for: {}]\n{}\n\nUse the above search results to answer the user's question. Cite sources using [N] notation.",
            query, search_text
        ),
        cache_control: None,
    };
    ir_request.system = match ir_request.system.take() {
        Some(IrSystemContent::Text(t)) => Some(IrSystemContent::Blocks(vec![
            IrSystemBlock { text: t, cache_control: None },
            search_block,
        ])),
        Some(IrSystemContent::Blocks(mut blocks)) => {
            blocks.push(search_block);
            Some(IrSystemContent::Blocks(blocks))
        }
        None => Some(IrSystemContent::Blocks(vec![search_block])),
    };

    // 4. 清除工具
    let tools_count = ir_request.tools.len();
    ir_request.tools = Vec::new();
    ir_request.tool_choice = None;
    tracing::info!(cleared_tools = tools_count, "websearch: IR enriched, tools cleared");

    ir_request
}

/// 从 messages 中提取最后一条 user 消息文本作为搜索关键词。
fn extract_search_query(messages: &[IrMessage]) -> String {
    for msg in messages.iter().rev() {
        if msg.role == IrRole::User {
            let text: String = msg.content.iter().filter_map(|b| match b {
                IrContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            }).collect::<Vec<_>>().join(" ");
            if !text.trim().is_empty() {
                return text.trim().to_string();
            }
        }
    }
    "search".to_string() // fallback
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── 构造 IR 请求辅助 ─────────────────────────────────────
    fn make_ir_request_with_websearch(user_message: &str) -> IrRequest {
        IrRequest {
            model: "claude-opus-4-8".to_string(),
            system: Some(IrSystemContent::Text("You are a helpful assistant.".to_string())),
            messages: vec![IrMessage {
                role: IrRole::User,
                content: vec![IrContentBlock::Text {
                    text: user_message.to_string(),
                    cache_control: None,
                }],
            }],
            tools: vec![IrTool {
                name: "web_search".to_string(),
                description: None,
                input_schema: json!({}),
            }],
            tool_choice: Some(IrToolChoice::Auto),
            max_tokens: Some(4096),
            temperature: None,
            top_p: None,
            thinking: None,
            stream: true,
        }
    }

    // ── 基础单元测试 ──────────────────────────────────────────

    #[test]
    fn test_has_websearch_tool_ir() {
        let req = IrRequest {
            model: "test".to_string(),
            system: None,
            messages: vec![],
            tools: vec![IrTool {
                name: "web_search".to_string(),
                description: None,
                input_schema: json!({}),
            }],
            tool_choice: None,
            max_tokens: None,
            temperature: None,
            top_p: None,
            thinking: None,
            stream: false,
        };
        assert!(has_websearch_tool_ir(&req));

        let req_no_tools = IrRequest {
            model: "test".to_string(),
            system: None,
            messages: vec![],
            tools: vec![],
            tool_choice: None,
            max_tokens: None,
            temperature: None,
            top_p: None,
            thinking: None,
            stream: false,
        };
        assert!(!has_websearch_tool_ir(&req_no_tools));
    }

    #[test]
    fn test_extract_search_query() {
        let messages = vec![
            IrMessage {
                role: IrRole::User,
                content: vec![IrContentBlock::Text {
                    text: "Hello".to_string(),
                    cache_control: None,
                }],
            },
            IrMessage {
                role: IrRole::Assistant,
                content: vec![IrContentBlock::Text {
                    text: "Hi!".to_string(),
                    cache_control: None,
                }],
            },
            IrMessage {
                role: IrRole::User,
                content: vec![IrContentBlock::Text {
                    text: "今日金价多少？".to_string(),
                    cache_control: None,
                }],
            },
        ];
        assert_eq!(extract_search_query(&messages), "今日金价多少？");
    }

    #[test]
    fn test_extract_search_query_empty() {
        let messages: Vec<IrMessage> = vec![];
        assert_eq!(extract_search_query(&messages), "search");
    }

    #[test]
    fn test_format_search_text() {
        let results = vec![
            crate::search::SearchResult {
                title: "Test".to_string(),
                url: "https://example.com".to_string(),
                snippet: "A snippet".to_string(),
            },
        ];
        let text = format_search_text(&results);
        assert!(text.contains("[1] Test"));
        assert!(text.contains("https://example.com"));
        assert!(text.contains("A snippet"));
    }

    // ═══════════════════════════════════════════════════════════════
    // 端到端管道测试：IR → Bing 搜索 → IR 注入 → 上游 JSON
    // ═══════════════════════════════════════════════════════════════

    fn make_ir_with_websearch(user_msg: &str) -> IrRequest {
        IrRequest {
            model: "test-model".to_string(),
            system: Some(IrSystemContent::Text("You are a helpful assistant.".to_string())),
            messages: vec![IrMessage {
                role: IrRole::User,
                content: vec![IrContentBlock::Text {
                    text: user_msg.to_string(),
                    cache_control: None,
                }],
            }],
            tools: vec![IrTool {
                name: "web_search".to_string(),
                description: None,
                input_schema: json!({}),
            }],
            tool_choice: Some(IrToolChoice::Auto),
            max_tokens: None,
            temperature: None,
            top_p: None,
            thinking: None,
            stream: false,
        }
    }

    /// 管道 Step 1: IR 请求检测到 web_search 工具
    #[test]
    fn test_e2e_step1_detect_websearch_tool() {
        let ir = make_ir_with_websearch("张雪峰的死因");
        assert!(has_websearch_tool_ir(&ir), "IR should detect web_search tool");
        assert_eq!(ir.tools.len(), 1);
        assert!(matches!(ir.tool_choice, Some(IrToolChoice::Auto)));
    }

    /// 管道 Step 2: 模拟 Bing 结果注入 system prompt（不走网络）
    #[test]
    fn test_e2e_step2_inject_search_results_into_system() {
        let mut ir = make_ir_with_websearch("张雪峰的死因");

        // 模拟 Bing 返回 3 条结果
        let fake_results = vec![
            crate::search::SearchResult {
                title: "41岁张雪峰去世，死因曝光".to_string(),
                url: "https://www.sohu.com/a/1001465544".to_string(),
                snippet: "医院随后公布死因——心源性猝死".to_string(),
            },
            crate::search::SearchResult {
                title: "张雪峰离世细节曝光".to_string(),
                url: "https://zhuanlan.zhihu.com/p/202025575".to_string(),
                snippet: "因心源性猝死永远停在了41岁".to_string(),
            },
            crate::search::SearchResult {
                title: "张雪峰老师心源性猝死的原因解析".to_string(),
                url: "https://zhuanlan.zhihu.com/p/2019915159".to_string(),
                snippet: "张雪峰老师在公司跑步后突发不适".to_string(),
            },
        ];

        let search_text = format_search_text(&fake_results);

        // 模拟 enrich_ir_with_search 的注入逻辑
        let search_block = IrSystemBlock {
            text: format!(
                "[Web Search Results for: {}]\n{}\n\nUse the above search results to answer the user's question. Cite sources using [N] notation.",
                "张雪峰的死因", search_text
            ),
            cache_control: None,
        };

        // 原 system 是 Text → 应升级为 Blocks
        ir.system = match ir.system.take() {
            Some(IrSystemContent::Text(t)) => Some(IrSystemContent::Blocks(vec![
                IrSystemBlock { text: t, cache_control: None },
                search_block,
            ])),
            _ => unreachable!(),
        };
        ir.tools = Vec::new();
        ir.tool_choice = None;

        // 验证 system 已升级
        match ir.system.as_ref().unwrap() {
            IrSystemContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 2, "Should have original + search block");
                assert_eq!(blocks[0].text, "You are a helpful assistant.");
                assert!(blocks[1].text.contains("[Web Search Results for: 张雪峰的死因]"));
                assert!(blocks[1].text.contains("心源性猝死"));
                assert!(blocks[1].text.contains("Cite sources using [N] notation"));
            }
            _ => panic!("System should be Blocks after injection"),
        }

        // 验证 tools 已清除
        assert!(ir.tools.is_empty());
        assert!(ir.tool_choice.is_none());
    }

    /// 管道 Step 3: 注入后的 IR 序列化为 Anthropic Messages 格式
    #[test]
    fn test_e2e_step3_serialize_to_messages() {
        let ir = build_enriched_ir();

        let json = crate::api::proxy::ir::to_messages::ir_req_to_messages(&ir);

        // system 应为数组（原始 + 搜索结果两个 block）
        let system = json.get("system").expect("should have system");
        let system_arr = system.as_array().expect("system should be array");
        assert_eq!(system_arr.len(), 2);
        assert_eq!(system_arr[0]["text"].as_str().unwrap(), "You are a helpful assistant.");
        assert!(system_arr[1]["text"].as_str().unwrap().contains("[Web Search Results"));

        // tools 应不存在
        assert!(json.get("tools").is_none(), "tools should be cleared");

        // messages 正常
        let msgs = json["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"].as_str().unwrap(), "user");
    }

    /// 管道 Step 3b: 注入后的 IR 序列化为 Chat Completions 格式
    #[test]
    fn test_e2e_step3b_serialize_to_chat_completions() {
        let ir = build_enriched_ir();

        let json = crate::api::proxy::ir::to_chat_completions::ir_req_to_chat_completions(&ir);

        // system 应合并为 messages[0]
        let msgs = json["messages"].as_array().unwrap();
        assert!(msgs.len() >= 2, "should have system + user messages");
        assert_eq!(msgs[0]["role"].as_str().unwrap(), "system");
        let sys_content = msgs[0]["content"].as_str().unwrap();
        assert!(sys_content.contains("You are a helpful assistant."));
        assert!(sys_content.contains("[Web Search Results"));

        // tools 应不存在
        assert!(json.get("tools").is_none(), "tools should be cleared");
    }

    /// 管道 Step 3c: 注入后的 IR 序列化为 Responses 格式
    #[test]
    fn test_e2e_step3c_serialize_to_responses() {
        let ir = build_enriched_ir();

        let json = crate::api::proxy::ir::to_responses::ir_req_to_responses(&ir);

        // system 应在 input[0]
        let input = json["input"].as_array().unwrap();
        assert!(input.len() >= 2, "should have system + user input items");
        assert_eq!(input[0]["role"].as_str().unwrap(), "system");
        let content = input[0]["content"].as_array().unwrap();
        // 两个 system block → 两个 input_text part
        assert_eq!(content.len(), 2);
        assert!(content[1]["text"].as_str().unwrap().contains("[Web Search Results"));

        // tools 应不存在
        assert!(json.get("tools").is_none(), "tools should be cleared");
    }

    /// 管道 Step 4: 完整 E2E — 最终发给上游的 JSON 应包含搜索结果且不含 tools
    #[test]
    fn test_e2e_step4_final_upstream_json() {
        let ir = build_enriched_ir();

        // 模拟 stream.rs 中 pick provider 后的序列化逻辑
        let body = crate::api::proxy::ir::to_messages::ir_req_to_messages(&ir);

        let json_str = serde_json::to_string(&body).unwrap();
        println!("Final upstream JSON (truncated):\n{}", &json_str[..json_str.len().min(500)]);

        // 关键断言
        assert!(json_str.contains("心源性猝死"), "JSON should contain search result content");
        assert!(json_str.contains("[Web Search Results"), "JSON should contain search marker");
        assert!(json_str.contains("Cite sources using [N]"), "JSON should contain citation instruction");
        assert!(!json_str.contains("\"tools\""), "JSON should NOT contain tools field");
        assert!(json_str.contains("\"stream\":true"), "JSON should have stream=true");
    }

    /// 管道异常路径: Bing 返回空结果时 LLM 被告知无结果
    #[test]
    fn test_e2e_empty_search_results() {
        let search_text = format!("No web search results found for: {}", "随机无意义字符串xyz123");

        let search_block = IrSystemBlock {
            text: format!(
                "[Web Search Results for: {}]\n{}\n\nUse the above search results to answer the user's question. Cite sources using [N] notation.",
                "随机无意义字符串xyz123", search_text
            ),
            cache_control: None,
        };

        let ir = IrRequest {
            model: "test".to_string(),
            system: Some(IrSystemContent::Blocks(vec![
                IrSystemBlock { text: "You are a helpful assistant.".to_string(), cache_control: None },
                search_block,
            ])),
            messages: vec![IrMessage {
                role: IrRole::User,
                content: vec![IrContentBlock::Text { text: "随机无意义字符串xyz123".to_string(), cache_control: None }],
            }],
            tools: vec![],
            tool_choice: None,
            max_tokens: None, temperature: None, top_p: None, thinking: None, stream: false,
        };

        let json = crate::api::proxy::ir::to_messages::ir_req_to_messages(&ir);
        let sys = json["system"].as_array().unwrap();
        let search_text = sys[1]["text"].as_str().unwrap();
        assert!(search_text.contains("No web search results found"));
    }

    /// 管道异常路径: Bing 搜索失败时 LLM 被告知不可用
    #[test]
    fn test_e2e_search_error_fallback() {
        let error_msg = "Web search unavailable: timeout. Do NOT make up information. Inform the user that the search is temporarily unavailable.";

        let search_block = IrSystemBlock {
            text: format!(
                "[Web Search Results for: {}]\n{}\n\nUse the above search results to answer the user's question. Cite sources using [N] notation.",
                "test query", error_msg
            ),
            cache_control: None,
        };

        let ir = IrRequest {
            model: "test".to_string(),
            system: Some(IrSystemContent::Blocks(vec![
                IrSystemBlock { text: "original system".to_string(), cache_control: None },
                search_block,
            ])),
            messages: vec![],
            tools: vec![],
            tool_choice: None,
            max_tokens: None, temperature: None, top_p: None, thinking: None, stream: false,
        };

        let json = crate::api::proxy::ir::to_messages::ir_req_to_messages(&ir);
        let sys = json["system"].as_array().unwrap();
        assert!(sys[1]["text"].as_str().unwrap().contains("Do NOT make up information"));
    }

    /// 管道边界: 已有 cache_control 的 system blocks 应保留
    #[test]
    fn test_e2e_preserve_cache_control() {
        let mut ir = make_ir_with_websearch("test");
        ir.system = Some(IrSystemContent::Blocks(vec![
            IrSystemBlock {
                text: "cached system".to_string(),
                cache_control: Some(json!({"type": "ephemeral"})),
            },
        ]));

        // 模拟注入
        let search_block = IrSystemBlock {
            text: "[Web Search Results] fake results".to_string(),
            cache_control: None,
        };
        match ir.system {
            Some(IrSystemContent::Blocks(ref mut blocks)) => blocks.push(search_block),
            _ => unreachable!(),
        }
        ir.tools = Vec::new();
        ir.tool_choice = None;

        let json = crate::api::proxy::ir::to_messages::ir_req_to_messages(&ir);
        let sys = json["system"].as_array().unwrap();
        assert_eq!(sys.len(), 2);
        // 第一个 block 保留 cache_control
        assert!(sys[0].get("cache_control").is_some());
        assert_eq!(sys[0]["cache_control"]["type"].as_str().unwrap(), "ephemeral");
        // 第二个 block（搜索结果）无 cache_control
        assert!(sys[1].get("cache_control").is_none());
    }

    /// 管道边界: 无 system 时注入搜索结果应正常工作
    #[test]
    fn test_e2e_no_existing_system() {
        let mut ir = IrRequest {
            model: "test".to_string(),
            system: None,
            messages: vec![IrMessage {
                role: IrRole::User,
                content: vec![IrContentBlock::Text { text: "search query".to_string(), cache_control: None }],
            }],
            tools: vec![IrTool {
                name: "web_search".to_string(),
                description: None,
                input_schema: json!({}),
            }],
            tool_choice: None,
            max_tokens: None, temperature: None, top_p: None, thinking: None, stream: false,
        };

        let search_block = IrSystemBlock {
            text: "[Web Search Results for: search query]\nfake results".to_string(),
            cache_control: None,
        };
        ir.system = Some(IrSystemContent::Blocks(vec![search_block]));
        ir.tools = Vec::new();

        let json = crate::api::proxy::ir::to_messages::ir_req_to_messages(&ir);
        let sys = json["system"].as_array().unwrap();
        assert_eq!(sys.len(), 1);
        assert!(sys[0]["text"].as_str().unwrap().contains("[Web Search Results"));
    }

    // ── 辅助函数 ──────────────────────────────────────────────

    /// 构造一个已注入搜索结果的 IR（模拟 enrich_ir_with_search 的输出）
    fn build_enriched_ir() -> IrRequest {
        let fake_results = vec![
            crate::search::SearchResult {
                title: "41岁张雪峰去世，死因曝光".to_string(),
                url: "https://www.sohu.com/a/1001465544".to_string(),
                snippet: "医院随后公布死因——心源性猝死".to_string(),
            },
            crate::search::SearchResult {
                title: "张雪峰离世细节曝光".to_string(),
                url: "https://zhuanlan.zhihu.com/p/202025575".to_string(),
                snippet: "因心源性猝死永远停在了41岁".to_string(),
            },
        ];

        let search_text = format_search_text(&fake_results);
        let search_block = IrSystemBlock {
            text: format!(
                "[Web Search Results for: {}]\n{}\n\nUse the above search results to answer the user's question. Cite sources using [N] notation.",
                "张雪峰的死因", search_text
            ),
            cache_control: None,
        };

        IrRequest {
            model: "claude-sonnet-4-20250514".to_string(),
            system: Some(IrSystemContent::Blocks(vec![
                IrSystemBlock { text: "You are a helpful assistant.".to_string(), cache_control: None },
                search_block,
            ])),
            messages: vec![IrMessage {
                role: IrRole::User,
                content: vec![IrContentBlock::Text {
                    text: "张雪峰的死因".to_string(),
                    cache_control: None,
                }],
            }],
            tools: vec![],  // 已清除
            tool_choice: None,  // 已清除
            max_tokens: None,
            temperature: None,
            top_p: None,
            thinking: None,
            stream: true,
        }
    }
}
