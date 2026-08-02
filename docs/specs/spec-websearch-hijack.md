# Spec: WebSearch 劫持

## 目标

拦截包含 `web_search` 工具的 LLM 请求，用本地 Bing 搜索替代上游 API 的搜索功能。

## 触发条件

1. `settings.websearch_hijack` 为 `true`
2. 请求的 `tools` 数组包含 `web_search` 工具

## 流程

```
客户端请求（含 web_search 工具）
    ↓
检查触发条件
    ↓
进入 tool-calling loop（最多 5 轮）
    ↓
每轮：
  1. 发送请求到上游（stream=false）
  2. 检查响应是否包含 tool_use
  3. 如果有 web_search 调用：
     - 本地 Bing 搜索
     - 构造 tool_result
     - 追加到 messages
     - 继续下一轮
  4. 如果无 tool_use：
     - 返回最终响应
    ↓
转换响应为 SSE 流
    ↓
返回客户端
```

## 输入契约

### 请求检测

```rust
pub fn has_websearch_tool(body: &Value) -> bool {
    body.get("tools")
        .and_then(|t| t.as_array())
        .map(|tools| {
            tools.iter().any(|t| {
                t.get("type")
                    .and_then(|s| s.as_str())
                    .map(|s| s.starts_with("web_search"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}
```

### Bing 搜索

```rust
pub fn search(query: &str) -> anyhow::Result<Vec<SearchResult>>
```

**SearchResult**:

```rust
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}
```

## 输出契约

### tool_result 格式

```json
{
  "role": "user",
  "content": [
    {
      "type": "tool_result",
      "tool_use_id": "toolu_xxx",
      "content": [
        {
          "type": "text",
          "text": "搜索结果：\n\n1. 标题1\nURL1\n摘要1\n\n2. 标题2\nURL2\n摘要2"
        }
      ]
    }
  ]
}
```

### SSE 响应

将最终响应转换为 Anthropic SSE 格式：

```
event: message_start
data: {"type":"message_start","message":{...}}

event: content_block_start
data: {"type":"content_block_start","index":0,...}

event: content_block_delta
data: {"type":"content_block_delta","index":0,...}

event: message_stop
data: {"type":"message_stop"}
```

## 关键约束

1. **最多 5 轮**: 防止无限循环
2. **非流式上游**: tool-calling loop 中 `stream=false`
3. **本地搜索**: 使用 `cn.bing.com`（反爬较宽松）
4. **浏览器指纹**: 模拟 Chrome 浏览器请求
5. **Cookie 复用**: 全局 `reqwest::Client` 复用 cookie

## Bing 搜索实现

```rust
static CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        .cookie_store(true)
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
});

pub fn search(query: &str) -> anyhow::Result<Vec<SearchResult>> {
    let resp = CLIENT.get("https://cn.bing.com/search")
        .query(&[("q", query), ("ensearch", "0")])
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .header("Sec-CH-UA", "\"Chromium\";v=\"131\", \"Not_A Brand\";v=\"24\"")
        .send()?;
    
    let html = resp.text()?;
    let document = Html::parse_document(&html);
    
    // 解析搜索结果
    let titles = document.select(&Selector::parse("h2 a").unwrap());
    let snippets = document.select(&Selector::parse(".b_caption .b_lineclamp2, .b_caption p, .b_lineclamp4, .b_lineclamp2").unwrap());
    
    // 提取最多 8 条结果（排除 bing.com/microsoft.com/msn.com 内部链接）
    let mut results = Vec::new();
    for (title, snippet) in titles.zip(snippets).take(8) {
        results.push(SearchResult {
            title: title.text().collect::<String>(),
            url: extract_url(title),
            snippet: snippet.text().collect::<String>(),
        });
    }
    
    Ok(results)
}
```

## 错误处理

| 场景 | 行为 |
|------|------|
| Bing 搜索失败 | 返回空结果，继续 loop |
| 上游 API 错误 | 直接返回错误响应 |
| 超过 5 轮 | 返回最后一次响应 |
| 无 web_search 调用 | 正常转发，不劫持 |

## 实现位置

- `src-tauri/src/api/proxy/websearch.rs` - 劫持逻辑
- `src-tauri/src/search/bing.rs` - Bing 搜索
- `src-tauri/src/api/proxy/handler.rs` - 触发检测

## 测试要求

1. **单元测试**: tool 检测、Bing 搜索解析
2. **集成测试**: 完整 tool-calling loop
3. **边界测试**: 5 轮上限、搜索失败、无 tool 调用
4. **反爬测试**: 验证 Bing 不会封禁

## 完成标准

- [x] 检测 `web_search` 工具
- [x] tool-calling loop（最多 5 轮）
- [x] 本地 Bing 搜索（cn.bing.com）
- [x] 浏览器指纹（User-Agent、Sec-CH-UA）
- [x] Cookie 复用（全局 Client）
- [x] 结果格式化（最多 8 条）
- [x] SSE 响应转换
- [x] 通过所有单元测试
