//! 统一 HTTP 客户端工厂：自动继承系统代理（环境变量 → Windows 注册表）。

use std::sync::OnceLock;

/// 当前解析出的系统代理 URL（如 `http://127.0.0.1:7897`）。无代理时为 None。
///
/// 用 OnceLock 缓存一次：代理在应用运行期间几乎不会变（Clash 端口固定），
/// 省掉每次建 client 都读注册表。
pub fn system_proxy() -> Option<&'static str> {
    static PROXY: OnceLock<Option<String>> = OnceLock::new();
    PROXY.get_or_init(resolve_system_proxy).as_deref()
}

fn resolve_system_proxy() -> Option<String> {
    // 1. 环境变量优先（跨平台标准，也便于开发时覆盖）。
    for var in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"] {
        if let Ok(v) = std::env::var(var) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    // 2. Windows：读注册表 Internet Settings 的系统代理。
    //    ProxyEnable=1 且 ProxyServer 非空才生效；跳过 PAC (AutoConfigURL)。
    resolve_windows_registry_proxy()
}

#[cfg(windows)]
fn resolve_windows_registry_proxy() -> Option<String> {
    const HKCU: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";
    let query = |name: &str| -> Option<String> {
        let out = std::process::Command::new("reg")
            .args(["query", HKCU, "/v", name])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        String::from_utf8_lossy(&out.stdout).lines().find_map(|l| {
            let s = l.trim();
            let (_, v) = s.split_once("REG_SZ")?;
            Some(v.trim().trim_matches('"').to_string())
        })
    };

    let enabled = query("ProxyEnable")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0)
        != 0;
    if !enabled {
        return None;
    }
    let server = query("ProxyServer")?;
    if server.is_empty() {
        return None;
    }
    // 形如 "127.0.0.1:7897" 或 "http://127.0.0.1:7897"。
    Some(if server.contains("://") {
        server
    } else {
        format!("http://{}", server)
    })
}

#[cfg(not(windows))]
fn resolve_windows_registry_proxy() -> Option<String> {
    None
}

/// 构建带系统代理的 reqwest 客户端。
///
/// 调用方可继续链式覆盖 timeout / cookie_store 等。
pub fn build_http_client() -> reqwest::ClientBuilder {
    let mut builder = reqwest::Client::builder();
    if let Some(proxy) = system_proxy() {
        if let Ok(p) = reqwest::Proxy::all(proxy) {
            let p = if no_proxy_list().is_empty() {
                p
            } else {
                p.no_proxy(reqwest::NoProxy::from_string(&no_proxy_list()))
            };
            builder = builder.proxy(p);
        }
    }
    builder
}

/// NO_PROXY 列表：默认豁免本机回环（插件系统的 upstream 可能在 localhost），
/// 并附加环境变量 NO_PROXY / no_proxy 的额外项。
fn no_proxy_list() -> String {
    let mut list: Vec<String> = ["localhost", "127.0.0.1", "[::1]"]
        .into_iter()
        .map(String::from)
        .collect();
    if let Ok(extra) = std::env::var("NO_PROXY").or_else(|_| std::env::var("no_proxy")) {
        for part in extra.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if !list.iter().any(|p| p == part) {
                list.push(part.to_string());
            }
        }
    }
    list.join(",")
}

/// 便捷方法：带系统代理 + 默认构建。
pub fn http_client() -> reqwest::Client {
    build_http_client()
        .build()
        .expect("failed to build http client")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_builds_with_or_without_proxy() {
        // 无论环境有无代理，客户端都能正常构建（不 panic）。
        let _ = build_http_client().build().unwrap();
        let _ = http_client();
    }
}
