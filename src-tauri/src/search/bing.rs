//! Bing 网页搜索包装（cn.bing.com 中国版 + 完整浏览器指纹 + cookie 复用）。
//! cn.bing.com 对服务器端反爬比 www.bing.com 宽松；带 cookie + Sec-CH-UA 指纹可拿到真实结果。
//! 给 WebSearch 劫持 loop 用。

use once_cell::sync::Lazy;
use scraper::{Html, Selector};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

// 全局复用的 client：cookie_store 把首页拿到的 cookie 留存，后续搜索请求自动带上，
// 省掉每次搜索都重新 GET 首页的 ~2s。
static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    crate::http::build_http_client()
        .user_agent(UA)
        .cookie_store(true)
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("failed to build bing client")
});

// 首页只需暖一次（拿到初始 cookie 会话）。失败可重试。
static HOME_WARMED: AtomicBool = AtomicBool::new(false);

async fn warm_home() {
    if HOME_WARMED.load(Ordering::Relaxed) {
        return;
    }
    let ok = CLIENT
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
    if ok {
        HOME_WARMED.store(true, Ordering::Relaxed);
    }
}

/// 搜索：复用全局 client 的 cookie 会话，直接发搜索请求（完整浏览器指纹头）。
pub async fn search(query: &str) -> anyhow::Result<Vec<SearchResult>> {
    warm_home().await;

    let html = CLIENT
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
        .await?
        .error_for_status()?
        .text()
        .await?;

    Ok(parse(&html))
}

/// cn.bing.com 结果：标题在 `h2 a`（外部 URL），摘要在 `.b_caption` 下的 `.b_lineclamp2`/`p`。
fn parse(html: &str) -> Vec<SearchResult> {
    let document = Html::parse_document(html);
    let link_sel = Selector::parse("h2 a").unwrap();
    let snippet_sel = Selector::parse(".b_caption .b_lineclamp2, .b_caption p, .b_lineclamp4, .b_lineclamp2").unwrap();

    let links: Vec<(String, String)> = document
        .select(&link_sel)
        .filter_map(|a| {
            let href = a.value().attr("href").unwrap_or("").to_string();
            if !href.starts_with("http")
                || href.contains("bing.com")
                || href.contains("microsoft.com")
                || href.contains("msn.com")
                || href.contains("go.microsoft")
            {
                return None;
            }
            let title = a.text().collect::<String>().trim().to_string();
            if title.is_empty() {
                None
            } else {
                Some((title, href))
            }
        })
        .collect();

    let snippets: Vec<String> = document
        .select(&snippet_sel)
        .map(|e| e.text().collect::<String>().trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    links
        .into_iter()
        .enumerate()
        .map(|(i, (title, url))| SearchResult {
            title,
            url,
            snippet: snippets.get(i).cloned().unwrap_or_default(),
        })
        .take(8)
        .collect()
}
