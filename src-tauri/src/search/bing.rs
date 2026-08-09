//! Bing 网页搜索包装（cn.bing.com 中国版 + 完整浏览器指纹）。
//! cn.bing.com 对服务器端反爬比 www.bing.com 宽松。
//!
//! 关键策略：
//! - **绕过代理直连**：cn.bing.com 是国内站点，走代理会导致出口 IP 在海外，
//!   Bing 会降级为"热门站点推荐"模式（返回今日头条/百度热搜等非相关结果）。
//! - **每次搜索用独立 cookie 会话**：避免全局 cookie 累积污染搜索结果。

use scraper::{Html, Selector};
use serde::Serialize;
use std::sync::Arc;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// 构建一个带独立 cookie jar 的 client（每次搜索一个干净会话）。
/// **绕过代理直连**：cn.bing.com 是国内站点，走代理反而会因为出口 IP 在海外
/// 而被 Bing 降级为"热门站点推荐"模式（返回今日头条/百度热搜等非相关结果）。
fn build_fresh_client() -> anyhow::Result<reqwest::Client> {
    let jar = Arc::new(reqwest::cookie::Jar::default());
    Ok(reqwest::Client::builder()
        .user_agent(UA)
        .cookie_provider(jar)
        .no_proxy()
        .timeout(std::time::Duration::from_secs(10))
        .build()?)
}

/// 搜索：每次创建新 client + cookie 会话，避免累积 cookie 污染。
pub async fn search(query: &str) -> anyhow::Result<Vec<SearchResult>> {
    let client = build_fresh_client()?;

    // 暖首页（拿初始 cookie）
    let warm_ok = client
        .get("https://cn.bing.com/")
        .header("Accept", "text/html,application/xhtml+xml,*/*;q=0.8")
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .header("Sec-Fetch-Dest", "document")
        .header("Sec-Fetch-Mode", "navigate")
        .header("Sec-Fetch-Site", "none")
        .header("Upgrade-Insecure-Requests", "1")
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    if !warm_ok {
        tracing::warn!("bing: warm_home failed, proceeding anyway");
    }

    let resp = client
        .get("https://cn.bing.com/search")
        .query(&[("q", query), ("ensearch", "0")])
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .header("Sec-CH-UA", r#""Google Chrome";v="131", "Chromium";v="131", "Not_A Brand";v="24""#)
        .header("Sec-CH-UA-Mobile", "?0")
        .header("Sec-CH-UA-Platform", r#""Windows""#)
        .header("Sec-Fetch-Dest", "document")
        .header("Sec-Fetch-Mode", "navigate")
        .header("Sec-Fetch-Site", "same-origin")
        .header("Sec-Fetch-User", "?1")
        .header("Upgrade-Insecure-Requests", "1")
        .header("Referer", "https://cn.bing.com/")
        .send()
        .await?;

    let html = resp.error_for_status()?.text().await?;

    // 反爬检测
    if html.contains("captcha") || html.contains("Challenge") {
        tracing::warn!(query = %query, "bing: anti-bot page detected!");
    }

    let results = parse(&html);
    tracing::info!(query = %query, results_count = results.len(), "bing: search complete");
    Ok(results)
}

/// cn.bing.com 搜索结果解析。
///
/// 选择器策略：
/// - `li.b_algo` 是 Bing 标准搜索结果容器（排除侧边栏/推荐区域）
/// - `h2 a` 在 b_algo 内 → 真正的搜索结果链接
/// - `.b_caption` → 摘要文本
fn parse(html: &str) -> Vec<SearchResult> {
    let document = Html::parse_document(html);
    let algo_sel = Selector::parse("li.b_algo").unwrap();
    let link_sel = Selector::parse("h2 a").unwrap();
    let snippet_sel = Selector::parse(".b_caption p, .b_caption .b_lineclamp2, .b_lineclamp4, .b_lineclamp2").unwrap();

    let mut results = Vec::new();

    for algo in document.select(&algo_sel) {
        // 从 b_algo 容器内提取链接
        let (title, url) = match algo.select(&link_sel).next() {
            Some(a) => {
                let href = a.value().attr("href").unwrap_or("").to_string();
                if !href.starts_with("http")
                    || href.contains("bing.com")
                    || href.contains("microsoft.com")
                    || href.contains("msn.com")
                {
                    continue;
                }
                let title = a.text().collect::<String>().trim().to_string();
                if title.is_empty() {
                    continue;
                }
                (title, href)
            }
            None => continue,
        };

        // 从同一 b_algo 容器内提取摘要
        let snippet = algo
            .select(&snippet_sel)
            .map(|e| e.text().collect::<String>().trim().to_string())
            .find(|s| !s.is_empty())
            .unwrap_or_default();

        results.push(SearchResult { title, url, snippet });
        if results.len() >= 8 {
            break;
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "real network call"]
    async fn test_real_search_zhangxuefeng() {
        let query = "张雪峰的死因";
        println!("\n=== Bing Search: {} ===\n", query);
        match search(query).await {
            Ok(results) => {
                println!("Got {} results:\n", results.len());
                for (i, r) in results.iter().enumerate() {
                    println!("[{}] {}", i + 1, r.title);
                    println!("    URL: {}", r.url);
                    println!("    Snippet: {}", r.snippet);
                    println!();
                }
            }
            Err(e) => {
                println!("Search failed: {}", e);
            }
        }
    }

    #[tokio::test]
    #[ignore = "real network call"]
    async fn test_real_search_gold_price() {
        let query = "今日金价";
        println!("\n=== Bing Search: {} ===\n", query);
        match search(query).await {
            Ok(results) => {
                println!("Got {} results:\n", results.len());
                for (i, r) in results.iter().enumerate() {
                    println!("[{}] {}", i + 1, r.title);
                    println!("    URL: {}", r.url);
                    println!("    Snippet: {}", r.snippet);
                    println!();
                }
            }
            Err(e) => {
                println!("Search failed: {}", e);
            }
        }
    }

    /// 连续多次搜索验证：cookie 不累积，每次都是干净结果
    #[tokio::test]
    #[ignore = "real network call"]
    async fn test_consecutive_searches_no_cookie_pollution() {
        let queries = ["今日金价", "张雪峰", "Python 教程"];
        for query in queries {
            println!("\n=== Search: {} ===", query);
            match search(query).await {
                Ok(results) => {
                    println!("Got {} results", results.len());
                    for (i, r) in results.iter().enumerate() {
                        println!("  [{}] {} — {}", i + 1, r.title, r.url);
                    }
                }
                Err(e) => println!("FAILED: {}", e),
            }
        }
    }

    #[test]
    fn test_parse_b_algo_container() {
        let html = r#"
        <html><body>
        <li class="b_algo">
            <h2><a href="https://example.com/page">Test Title</a></h2>
            <div class="b_caption"><p>Test snippet here</p></div>
        </li>
        <li class="b_algo">
            <h2><a href="https://other.com">Other Title</a></h2>
            <div class="b_caption"><p>Other snippet</p></div>
        </li>
        </body></html>
        "#;
        let results = parse(html);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Test Title");
        assert_eq!(results[0].url, "https://example.com/page");
        assert_eq!(results[0].snippet, "Test snippet here");
        assert_eq!(results[1].title, "Other Title");
    }

    #[test]
    fn test_parse_filters_bing_internal() {
        let html = r#"
        <html><body>
        <li class="b_algo">
            <h2><a href="https://www.bing.com/images">Bing Images</a></h2>
            <div class="b_caption"><p>Internal</p></div>
        </li>
        <li class="b_algo">
            <h2><a href="https://example.com">Real Result</a></h2>
            <div class="b_caption"><p>Good</p></div>
        </li>
        </body></html>
        "#;
        let results = parse(html);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Real Result");
    }

    #[test]
    fn test_parse_empty_html() {
        let html = "<html><body></body></html>";
        let results = parse(html);
        assert!(results.is_empty());
    }
}
