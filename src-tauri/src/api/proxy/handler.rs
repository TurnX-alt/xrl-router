//! 三个代理入口 handler：Anthropic / OpenAI Chat 流式代理 + 模型列表。
//!
//! 保持原 proxy.rs 中的实现原样（含重试循环与流式转发逻辑），仅做文件迁移；
//! 认证 / 路由 / 密钥轮换 / 上游错误 / WebSearch 已下沉到对应子模块。

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Json, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::stream::StreamExt;
use serde_json::{json, Value};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::gateway::server::AppState;

use super::auth::verify_service_key;
use super::key_rotation::{pick_key_for, update_key_health};
use super::quota::check_quota;
use super::route::resolve_route;
use super::upstream::forward_upstream_error;
use super::websearch::{has_websearch_tool, run_websearch_loop};
use super::{sniff, translate, UPSTREAM_CHUNK_TIMEOUT_SECS, UPSTREAM_HEADER_TIMEOUT_SECS};

/// POST /v1/messages - Anthropic Messages API proxy (streaming only).
pub async fn proxy_anthropic_messages(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, (StatusCode, HeaderMap, Json<Value>)> {
    let trace_id = Uuid::new_v4().to_string();
    let start_time = Instant::now();

    let api_key = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .or_else(|| {
            headers
                .get("Authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
        })
        .unwrap_or("");

    let model_name = body["model"].as_str().unwrap_or("").to_owned();

    info!(
        trace_id = %trace_id,
        model = %model_name,
        endpoint = "/v1/messages",
        "Proxy request received"
    );

    let service_key = match verify_service_key(&state, api_key).await {
        Some(info) => {
            info!(trace_id = %trace_id, service_key_id = %info.id, "Service key verified");
            info
        }
        None => {
            warn!(trace_id = %trace_id, "Authentication failed: invalid API key");
            return Err((
                StatusCode::UNAUTHORIZED,
                HeaderMap::new(),
                Json(json!({"error": {"type": "authentication_error", "message": "Invalid API key"}})),
            ))
        }
    };
    // 滚动窗口 token 配额（5h / 7d），任一窗口触顶即 429（quota_error + retry-after，message 含重置时间）。
    if let Err((code, headers, body)) = check_quota(&state, &service_key).await {
        warn!(trace_id = %trace_id, service_key_id = %service_key.id, "Quota exceeded for service key");
        return Err((code, headers, body));
    }
    // Enforce allowed_models whitelist (empty = allow all). Clients must use the alias.
    if !service_key.allowed_models.is_empty() && !service_key.allowed_models.iter().any(|m| m == &model_name) {
        warn!(trace_id = %trace_id, model = %model_name, "Model not allowed for this service key");
        return Err((
            StatusCode::FORBIDDEN,
            HeaderMap::new(),
            Json(json!({"error": {"type": "forbidden", "message": "Model not allowed for this service key"}})),
        ));
    }

    let resolved = match resolve_route(&state, &model_name).await {
        Some(r) => {
            info!(
                trace_id = %trace_id,
                provider_kind = %r.provider_kind,
                real_model = %r.real_model_id,
                "Route resolved"
            );
            r
        }
        None => {
            warn!(trace_id = %trace_id, model = %model_name, "Model not found or not available");
            return Err((
                StatusCode::BAD_REQUEST,
                HeaderMap::new(),
                Json(json!({"error": {"type": "invalid_request_error", "message": "Model not found or not available"}})),
            ))
        }
    };

    let provider_is_anthropic = resolved.provider_kind == "anthropic";
    let needs_translation = !provider_is_anthropic;

    // WebSearch 劫持：开关开 + 请求带 web_search tool → 走本地 Bing loop（Anthropic/OpenAI 上游均可）
    if state.websearch_hijack.load(std::sync::atomic::Ordering::Relaxed)
        && has_websearch_tool(&body)
    {
        info!(trace_id = %trace_id, anthropic_upstream = provider_is_anthropic, "web_search hijacked → local Bing loop");
        return run_websearch_loop(&state, &body, &resolved, provider_is_anthropic, &trace_id, &service_key).await;
    }

    let mut request_body = if needs_translation {
        translate::anthropic_req_to_openai(&body)
    } else {
        body.clone()
    };

    // Force streaming and substitute real model name for upstream.
    if let Some(obj) = request_body.as_object_mut() {
        obj.insert("model".to_string(), json!(resolved.real_model_id));
        obj.insert("stream".to_string(), json!(true));
        // Ask OpenAI-compatible upstreams to include token usage in the final
        // stream chunk so we can record it. Anthropic upstreams always include it.
        if !provider_is_anthropic {
            obj.insert("stream_options".to_string(), json!({"include_usage": true}));
        }
    }

    let upstream_url = resolved.upstream_url.clone();
    let provider_id = resolved.provider_id.clone();
    let model_row_id = resolved.model_row_id.clone();
    let client = crate::http::build_http_client()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();

    info!(
        trace_id = %trace_id,
        upstream_url = %upstream_url,
        "Calling upstream API (streaming)"
    );

    // Retry loop: 401/402/403/429 → mark current key, rotate to next, replay.
    let mut last_resp: Option<reqwest::Response> = None;
    let mut last_key_id: Option<String> = None;
    let mut last_key_name: Option<String> = None;
    let mut last_key_masked: Option<String> = None;
    // 兜底：最多重试 key 总数次，防止任何意外死循环。
    let max_attempts = state.keys.get_stats(&provider_id).map(|s| s.total as u32).unwrap_or(1);
    let mut attempts: u32 = 0;
    let response = loop {
        attempts += 1;
        if attempts > max_attempts {
            match last_resp {
                Some(r) => break r,
                None => {
                    return Err((
                        StatusCode::SERVICE_UNAVAILABLE,
                        HeaderMap::new(),
                        Json(json!({"error": {"type": "api_error", "message": "No available upstream keys"}})),
                    ));
                }
            }
        }
        let picked = match pick_key_for(&state, &provider_id) {
            Some(p) => p,
            None => match last_resp {
                // All keys exhausted: forward the last failed upstream response.
                Some(r) => break r,
                None => {
                    return Err((
                        StatusCode::SERVICE_UNAVAILABLE,
                        HeaderMap::new(),
                        Json(json!({"error": {"type": "api_error", "message": "No available upstream keys"}})),
                    ));
                }
            },
        };
        last_key_id = Some(picked.id.clone());
        last_key_name = Some(picked.name.clone());
        last_key_masked = Some(picked.key_masked.clone());
        let key_name = picked.name.clone();
        let key_masked = picked.key_masked.clone();

        let mut req_builder = client.post(&upstream_url);
        if provider_is_anthropic {
            req_builder = req_builder
                .header("x-api-key", &picked.key_hash)
                .header("anthropic-version", "2023-06-01");
        } else {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", picked.key_hash));
        }

        let resp = match tokio::time::timeout(
            Duration::from_secs(UPSTREAM_HEADER_TIMEOUT_SECS),
            req_builder
                .header("Content-Type", "application/json")
                .json(&request_body)
                .send(),
        )
        .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                // Network error (not a key problem) — don't retry, record and bail.
                let duration_ms = start_time.elapsed().as_millis() as i64;
                error!(trace_id = %trace_id, duration_ms, error = %e, "Upstream call failed");
                let _ = state.database.insert_usage_log(
                    chrono::Utc::now().timestamp(),
                    &provider_id, resolved.provider_name.as_str(), &model_row_id, model_name.as_str(),
                    Some(&picked.id), key_name.as_str(), key_masked.as_str(),
                    Some(service_key.id.as_str()), service_key.name.as_str(), service_key.key_masked.as_str(),
                    "/v1/messages",
                    0, 0, duration_ms, false, Some(&e.to_string()), 0,
                );
                return Err((
                    StatusCode::BAD_GATEWAY,
                    HeaderMap::new(),
                    Json(json!({"error": {"type": "api_error", "message": e.to_string()}})),
                ));
            }
            Err(_) => {
                // 上游建连后挂起不响应时，send() 会卡死；这里用超时兜底，避免整个重试循环被拖住。
                let duration_ms = start_time.elapsed().as_millis() as i64;
                let msg = format!(
                    "upstream timed out after {}s waiting for response headers",
                    UPSTREAM_HEADER_TIMEOUT_SECS
                );
                warn!(trace_id = %trace_id, duration_ms, key_id = %picked.id, "{}", msg);
                let _ = state.database.insert_usage_log(
                    chrono::Utc::now().timestamp(),
                    &provider_id, resolved.provider_name.as_str(), &model_row_id, model_name.as_str(),
                    Some(&picked.id), key_name.as_str(), key_masked.as_str(),
                    Some(service_key.id.as_str()), service_key.name.as_str(), service_key.key_masked.as_str(),
                    "/v1/messages",
                    0, 0, duration_ms, false, Some(&msg), 0,
                );
                return Err((
                    StatusCode::GATEWAY_TIMEOUT,
                    HeaderMap::new(),
                    Json(json!({"error": {"type": "api_error", "message": msg}})),
                ));
            }
        };

        let status = resp.status().as_u16();
        update_key_health(&state.keys, &provider_id, &picked.key_hash, status);

        if matches!(status, 401 | 402 | 403 | 429) {
            warn!(trace_id = %trace_id, status, key_id = %picked.id, "upstream rejected key, rotating");
            last_resp = Some(resp);
            continue;
        }
        break resp;
    };

    let upstream_status = response.status().as_u16();

    if upstream_status >= 400 {
        return Ok(forward_upstream_error(
            &state.database, &provider_id, resolved.provider_name.as_str(), &model_row_id, model_name.as_str(),
            last_key_id.as_deref(), last_key_name.as_deref(), last_key_masked.as_deref(),
            Some(service_key.id.as_str()), service_key.name.as_str(), service_key.key_masked.as_str(),
            "/v1/messages",
            response, upstream_status, &trace_id, &start_time,
        )
        .await);
    }

    info!(
        trace_id = %trace_id,
        status = upstream_status,
        duration_ms = start_time.elapsed().as_millis(),
        "Upstream response received, starting stream"
    );

    if needs_translation {
        // Upstream is OpenAI: parse SSE chunks, translate to Anthropic, re-emit.
        let mut stream = response.bytes_stream();
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::io::Error>>(100);
        let model_name_clone = model_name.clone();
        let trace_id_clone = trace_id.clone();
        let db = state.database.clone();
        let provider_id_log = provider_id.clone();
        let provider_name_log = resolved.provider_name.clone();
        let model_id_log = resolved.model_row_id.clone();
        let model_name_log = model_name.clone();
        let key_id_log = last_key_id.clone();
        let key_name_log = last_key_name.clone();
        let key_masked_log = last_key_masked.clone();
        let service_key_id_log = service_key.id.clone();
        let service_key_name_log = service_key.name.clone();
        let service_key_masked_log = service_key.key_masked.clone();
        let state_clone = state.clone();
        // message_start 阶段上游尚未返回 usage，先给一个非零估算占位，
        // 避免客户端（CCSwitch 等）把 input 记成 0。真实值在流末尾覆盖。
        let est_input = translate::estimate_input_tokens(&body);

        tokio::spawn(async move {
            let mut buffer = String::new();
            let mut chunk_count = 0u64;
            let mut stream_state = translate::StreamState::new();
            stream_state.input_tokens = est_input;
            // Whether the OpenAI upstream sent its [DONE] terminator.
            let mut saw_done = false;

            'outer: loop {
                // 包装 chunk 读取：上游中途断流不关连接时，next() 会永久挂起。
                let chunk = match tokio::time::timeout(
                    Duration::from_secs(UPSTREAM_CHUNK_TIMEOUT_SECS),
                    stream.next(),
                )
                .await
                {
                    Ok(Some(c)) => c,
                    Ok(None) => break,
                    Err(_) => {
                        warn!(
                            trace_id = %trace_id_clone,
                            "upstream stream silent for {}s, closing",
                            UPSTREAM_CHUNK_TIMEOUT_SECS
                        );
                        break;
                    }
                };
                if let Ok(bytes) = chunk {
                    buffer.push_str(&String::from_utf8_lossy(&bytes));

                    while let Some(newline_pos) = buffer.find("\n\n") {
                        let event = buffer[..newline_pos].to_string();
                        buffer = buffer[newline_pos + 2..].to_string();

                        if let Some(data) = event.strip_prefix("data: ") {
                            if data == "[DONE]" {
                                // [DONE] is OpenAI's terminator; the Anthropic stream is
                                // closed by a message_stop event, not a bare data line.
                                saw_done = true;
                                break 'outer;
                            }

                            if let Ok(chunk_json) = serde_json::from_str::<Value>(data) {
                                let events: Vec<Value> = translate::translate_openai_chunk_to_anthropic(
                                    &chunk_json,
                                    &model_name_clone,
                                    &mut stream_state,
                                );
                                for ev in events {
                                    if ev != Value::Null {
                                        chunk_count += 1;
                                        // Anthropic SSE requires an `event: <type>` line;
                                        // without it Claude Code treats the frame as malformed.
                                        let event_type = ev["type"].as_str().unwrap_or("message");
                                        let json_str = serde_json::to_string(&ev).unwrap();
                                        let _ = tx.send(
                                            Ok(Event::default().event(event_type).data(json_str)),
                                        )
                                        .await;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // message_delta/message_stop 统一在流结束后通过 finalize 发出。
            // OpenAI 上游的 usage 在最后一个 chunk 才到（晚于 finish_reason），
            // 若提前发 message_delta 会把 input/cache 报成 0。这里等流处理完，
            // 用 stream_state 里已累积的真实 usage 收尾。
            for ev in translate::finalize_openai_to_anthropic(&mut stream_state) {
                let event_type = ev["type"].as_str().unwrap_or("message");
                let json_str = serde_json::to_string(&ev).unwrap();
                let _ = tx.send(
                    Ok(Event::default().event(event_type).data(json_str)),
                )
                .await;
            }

            info!(
                trace_id = %trace_id_clone,
                total_chunks = chunk_count,
                done = saw_done,
                "Stream ended"
            );

            // Record usage. Prefer real token counts; fall back to chars/4 when
            // the upstream reported none.
            let output_tokens = if stream_state.output_tokens > 0 {
                stream_state.output_tokens as i64
            } else {
                (stream_state.output_chars / 4) as i64
            };
            let input_t = stream_state.input_tokens as i64;
            let cr = stream_state.cache_read_input_tokens as i64;
            let _ = db.insert_usage_log(
                chrono::Utc::now().timestamp(),
                &provider_id_log,
                provider_name_log.as_str(),
                &model_id_log,
                model_name_log.as_str(),
                key_id_log.as_deref(),
                key_name_log.as_deref().unwrap_or(""),
                key_masked_log.as_deref().unwrap_or(""),
                Some(service_key_id_log.as_str()),
                service_key_name_log.as_str(),
                service_key_masked_log.as_str(),
                "/v1/messages",
                input_t,
                output_tokens,
                start_time.elapsed().as_millis() as i64,
                true,
                None,
                cr,
            );
        });

        Ok(Sse::new(ReceiverStream::new(rx))
            .keep_alive(KeepAlive::default())
            .into_response())
    } else {
        // Upstream is Anthropic: sniff usage while forwarding bytes verbatim.
        let provider_kind = resolved.provider_kind.clone();
        let provider_id_log = provider_id.clone();
        let provider_name_log = resolved.provider_name.clone();
        let model_id_log = resolved.model_row_id.clone();
        let model_name_log = model_name.clone();
        let key_id_log = last_key_id.clone();
        let key_name_log = last_key_name.clone();
        let key_masked_log = last_key_masked.clone();
        let service_key_id_log = service_key.id.clone();
        let service_key_name_log = service_key.name.clone();
        let service_key_masked_log = service_key.key_masked.clone();
        let db = state.database.clone();
        let state_clone = state.clone();
        let mut sniff = sniff::SniffStream::new(response.bytes_stream(), &provider_kind);
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, reqwest::Error>>(100);

        tokio::spawn(async move {
            loop {
                let item = match tokio::time::timeout(
                    Duration::from_secs(UPSTREAM_CHUNK_TIMEOUT_SECS),
                    sniff.next(),
                )
                .await
                {
                    Ok(Some(i)) => i,
                    Ok(None) => break,
                    Err(_) => {
                        warn!(
                            "upstream stream silent for {}s, closing",
                            UPSTREAM_CHUNK_TIMEOUT_SECS
                        );
                        break;
                    }
                };
                if tx.send(item).await.is_err() {
                    break;
                }
            }
            let usage = sniff.into_usage();
            let output_tokens = if usage.output_tokens > 0 {
                usage.output_tokens as i64
            } else {
                (usage.output_chars / 4) as i64
            };
            let input_t = usage.input_tokens as i64;
            let cr = usage.cache_read_input_tokens as i64;
            let _ = db.insert_usage_log(
                chrono::Utc::now().timestamp(),
                &provider_id_log,
                provider_name_log.as_str(),
                &model_id_log,
                model_name_log.as_str(),
                key_id_log.as_deref(),
                key_name_log.as_deref().unwrap_or(""),
                key_masked_log.as_deref().unwrap_or(""),
                Some(service_key_id_log.as_str()),
                service_key_name_log.as_str(),
                service_key_masked_log.as_str(),
                "/v1/messages",
                input_t,
                output_tokens,
                start_time.elapsed().as_millis() as i64,
                true,
                None,
                cr,
            );
        });

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .body(axum::body::Body::from_stream(
                tokio_stream::wrappers::ReceiverStream::new(rx),
            ))
            .unwrap())
    }
}

/// POST /v1/chat/completions - OpenAI Chat API proxy (streaming only).
pub async fn proxy_openai_chat(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, (StatusCode, HeaderMap, Json<Value>)> {
    let trace_id = Uuid::new_v4().to_string();
    let start_time = Instant::now();

    let api_key = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| headers.get("x-api-key").and_then(|v| v.to_str().ok()))
        .unwrap_or("");

    let model_name = body["model"].as_str().unwrap_or("").to_owned();

    info!(
        trace_id = %trace_id,
        model = %model_name,
        endpoint = "/v1/chat/completions",
        "Proxy request received"
    );

    let service_key = match verify_service_key(&state, api_key).await {
        Some(info) => {
            info!(trace_id = %trace_id, service_key_id = %info.id, "Service key verified");
            info
        }
        None => {
            warn!(trace_id = %trace_id, "Authentication failed: invalid API key");
            return Err((
                StatusCode::UNAUTHORIZED,
                HeaderMap::new(),
                Json(json!({"error": {"type": "authentication_error", "message": "Invalid API key"}})),
            ))
        }
    };
    // 滚动窗口 token 配额（5h / 7d），任一窗口触顶即 429（quota_error + retry-after，message 含重置时间）。
    if let Err((code, headers, body)) = check_quota(&state, &service_key).await {
        warn!(trace_id = %trace_id, service_key_id = %service_key.id, "Quota exceeded for service key");
        return Err((code, headers, body));
    }
    // Enforce allowed_models whitelist (empty = allow all). Clients must use the alias.
    if !service_key.allowed_models.is_empty() && !service_key.allowed_models.iter().any(|m| m == &model_name) {
        warn!(trace_id = %trace_id, model = %model_name, "Model not allowed for this service key");
        return Err((
            StatusCode::FORBIDDEN,
            HeaderMap::new(),
            Json(json!({"error": {"type": "forbidden", "message": "Model not allowed for this service key"}})),
        ));
    }

    let resolved = match resolve_route(&state, &model_name).await {
        Some(r) => {
            info!(
                trace_id = %trace_id,
                provider_kind = %r.provider_kind,
                real_model = %r.real_model_id,
                "Route resolved"
            );
            r
        }
        None => {
            warn!(trace_id = %trace_id, model = %model_name, "Model not found");
            return Err((
                StatusCode::BAD_REQUEST,
                HeaderMap::new(),
                Json(json!({"error": {"type": "invalid_request_error", "message": "Model not found"}})),
            ))
        }
    };

    let provider_is_anthropic = resolved.provider_kind == "anthropic";
    let needs_translation = provider_is_anthropic;

    let mut request_body = if needs_translation {
        translate::openai_req_to_anthropic(&body)
    } else {
        body.clone()
    };

    // Force streaming and substitute real model name for upstream.
    if let Some(obj) = request_body.as_object_mut() {
        obj.insert("model".to_string(), json!(resolved.real_model_id));
        obj.insert("stream".to_string(), json!(true));
        // Ask OpenAI-compatible upstreams to include token usage in the final
        // stream chunk so we can record it. Anthropic upstreams always include it.
        if !provider_is_anthropic {
            obj.insert("stream_options".to_string(), json!({"include_usage": true}));
        }
    }

    let upstream_url = resolved.upstream_url.clone();
    let provider_id = resolved.provider_id.clone();
    let model_row_id = resolved.model_row_id.clone();
    let client = crate::http::build_http_client()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();

    info!(
        trace_id = %trace_id,
        upstream_url = %upstream_url,
        "Calling upstream API (streaming)"
    );

    // Retry loop: 401/402/403/429 → mark current key, rotate to next, replay.
    let mut last_resp: Option<reqwest::Response> = None;
    let mut last_key_id: Option<String> = None;
    let mut last_key_name: Option<String> = None;
    let mut last_key_masked: Option<String> = None;
    // 兜底：最多重试 key 总数次，防止任何意外死循环。
    let max_attempts = state.keys.get_stats(&provider_id).map(|s| s.total as u32).unwrap_or(1);
    let mut attempts: u32 = 0;
    let response = loop {
        attempts += 1;
        if attempts > max_attempts {
            match last_resp {
                Some(r) => break r,
                None => {
                    return Err((
                        StatusCode::SERVICE_UNAVAILABLE,
                        HeaderMap::new(),
                        Json(json!({"error": {"type": "api_error", "message": "No available upstream keys"}})),
                    ));
                }
            }
        }
        let picked = match pick_key_for(&state, &provider_id) {
            Some(p) => p,
            None => match last_resp {
                Some(r) => break r,
                None => {
                    return Err((
                        StatusCode::SERVICE_UNAVAILABLE,
                        HeaderMap::new(),
                        Json(json!({"error": {"type": "api_error", "message": "No available upstream keys"}})),
                    ));
                }
            },
        };
        last_key_id = Some(picked.id.clone());
        last_key_name = Some(picked.name.clone());
        last_key_masked = Some(picked.key_masked.clone());
        let key_name = picked.name.clone();
        let key_masked = picked.key_masked.clone();

        let mut req_builder = client.post(&upstream_url);
        if provider_is_anthropic {
            req_builder = req_builder
                .header("x-api-key", &picked.key_hash)
                .header("anthropic-version", "2023-06-01");
        } else {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", picked.key_hash));
        }

        let resp = match tokio::time::timeout(
            Duration::from_secs(UPSTREAM_HEADER_TIMEOUT_SECS),
            req_builder
                .header("Content-Type", "application/json")
                .json(&request_body)
                .send(),
        )
        .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                let duration_ms = start_time.elapsed().as_millis() as i64;
                error!(trace_id = %trace_id, duration_ms, error = %e, "Upstream call failed");
                let _ = state.database.insert_usage_log(
                    chrono::Utc::now().timestamp(),
                    &provider_id, resolved.provider_name.as_str(), &model_row_id, model_name.as_str(),
                    Some(&picked.id), key_name.as_str(), key_masked.as_str(),
                    Some(service_key.id.as_str()), service_key.name.as_str(), service_key.key_masked.as_str(),
                    "/v1/chat/completions",
                    0, 0, duration_ms, false, Some(&e.to_string()), 0,
                );
                return Err((
                    StatusCode::BAD_GATEWAY,
                    HeaderMap::new(),
                    Json(json!({"error": {"type": "api_error", "message": e.to_string()}})),
                ));
            }
            Err(_) => {
                let duration_ms = start_time.elapsed().as_millis() as i64;
                let msg = format!(
                    "upstream timed out after {}s waiting for response headers",
                    UPSTREAM_HEADER_TIMEOUT_SECS
                );
                warn!(trace_id = %trace_id, duration_ms, key_id = %picked.id, "{}", msg);
                let _ = state.database.insert_usage_log(
                    chrono::Utc::now().timestamp(),
                    &provider_id, resolved.provider_name.as_str(), &model_row_id, model_name.as_str(),
                    Some(&picked.id), key_name.as_str(), key_masked.as_str(),
                    Some(service_key.id.as_str()), service_key.name.as_str(), service_key.key_masked.as_str(),
                    "/v1/chat/completions",
                    0, 0, duration_ms, false, Some(&msg), 0,
                );
                return Err((
                    StatusCode::GATEWAY_TIMEOUT,
                    HeaderMap::new(),
                    Json(json!({"error": {"type": "api_error", "message": msg}})),
                ));
            }
        };

        let status = resp.status().as_u16();
        update_key_health(&state.keys, &provider_id, &picked.key_hash, status);

        if matches!(status, 401 | 402 | 403 | 429) {
            warn!(trace_id = %trace_id, status, key_id = %picked.id, "upstream rejected key, rotating");
            last_resp = Some(resp);
            continue;
        }
        break resp;
    };

    let upstream_status = response.status().as_u16();

    if upstream_status >= 400 {
        return Ok(forward_upstream_error(
            &state.database, &provider_id, resolved.provider_name.as_str(), &model_row_id, model_name.as_str(),
            last_key_id.as_deref(), last_key_name.as_deref(), last_key_masked.as_deref(),
            Some(service_key.id.as_str()), service_key.name.as_str(), service_key.key_masked.as_str(),
            "/v1/chat/completions",
            response, upstream_status, &trace_id, &start_time,
        )
        .await);
    }

    info!(
        trace_id = %trace_id,
        status = upstream_status,
        duration_ms = start_time.elapsed().as_millis(),
        "Upstream response received, starting stream"
    );

    if needs_translation {
        // Upstream is Anthropic: parse + translate to OpenAI format.
        let mut stream = response.bytes_stream();
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::io::Error>>(100);
        let trace_id_clone = trace_id.clone();
        let db = state.database.clone();
        let state_clone = state.clone();
        let provider_id_log = provider_id.clone();
        let provider_name_log = resolved.provider_name.clone();
        let model_id_log = resolved.model_row_id.clone();
        let model_name_log = model_name.clone();
        let key_id_log = last_key_id.clone();
        let key_name_log = last_key_name.clone();
        let key_masked_log = last_key_masked.clone();
        let service_key_id_log = service_key.id.clone();
        let service_key_name_log = service_key.name.clone();
        let service_key_masked_log = service_key.key_masked.clone();

        tokio::spawn(async move {
            let mut buffer = String::new();
            let mut chunk_count = 0u64;
            let mut accum_input: u64 = 0;
            let mut accum_output: u64 = 0;
            let mut accum_cache_read: u64 = 0;
            let mut accum_chars: u64 = 0;
            let mut oa_state = translate::OaStreamState::new();

            // Record usage with the chars/4 fallback. Borrows the log fields.
            let record_usage = |input_tokens: u64, output_tokens: u64, output_chars: u64, cache_read: u64| {
                let output_tokens = if output_tokens > 0 {
                    output_tokens as i64
                } else {
                    (output_chars / 4) as i64
                };
                let cr = cache_read as i64;
                let _ = db.insert_usage_log(
                    chrono::Utc::now().timestamp(),
                    &provider_id_log,
                    provider_name_log.as_str(),
                    &model_id_log,
                    model_name_log.as_str(),
                    key_id_log.as_deref(),
                    key_name_log.as_deref().unwrap_or(""),
                    key_masked_log.as_deref().unwrap_or(""),
                    Some(service_key_id_log.as_str()),
                    service_key_name_log.as_str(),
                    service_key_masked_log.as_str(),
                    "/v1/chat/completions",
                    input_tokens as i64,
                    output_tokens,
                    start_time.elapsed().as_millis() as i64,
                    true,
                    None,
                    cr,
                );
            };

            loop {
                // 包装 chunk 读取：上游中途断流不关连接时，next() 会永久挂起。
                let chunk = match tokio::time::timeout(
                    Duration::from_secs(UPSTREAM_CHUNK_TIMEOUT_SECS),
                    stream.next(),
                )
                .await
                {
                    Ok(Some(c)) => c,
                    Ok(None) => break,
                    Err(_) => {
                        warn!(
                            trace_id = %trace_id_clone,
                            "upstream stream silent for {}s, closing",
                            UPSTREAM_CHUNK_TIMEOUT_SECS
                        );
                        break;
                    }
                };
                if let Ok(bytes) = chunk {
                    buffer.push_str(&String::from_utf8_lossy(&bytes));

                    while let Some(newline_pos) = buffer.find("\n\n") {
                        let event = buffer[..newline_pos].to_string();
                        buffer = buffer[newline_pos + 2..].to_string();

                        if let Some(data) = event.strip_prefix("data: ") {
                            if data == "[DONE]" {
                                info!(
                                    trace_id = %trace_id_clone,
                                    total_chunks = chunk_count,
                                    "Stream completed"
                                );
                                let _ = tx.send(Ok(Event::default().data("[DONE]"))).await;
                                record_usage(accum_input, accum_output, accum_chars, accum_cache_read);
                                return;
                            }

                            if let Ok(chunk_json) = serde_json::from_str::<Value>(data) {
                                let (it, ot, cr, ch) = translate::extract_anthropic_usage(&chunk_json);
                                accum_input = accum_input.max(it);
                                if ot > 0 {
                                    accum_output = ot;
                                }
                                accum_cache_read = accum_cache_read.max(cr);
                                accum_chars += ch;

                                let translated = translate::translate_anthropic_chunk_to_openai(&chunk_json, &mut oa_state);
                                if translated != Value::Null {
                                    chunk_count += 1;
                                    let json_str = serde_json::to_string(&translated).unwrap();
                                    let _ = tx.send(Ok(Event::default().data(json_str))).await;
                                }
                            }
                        }
                    }
                }
            }
            info!(
                trace_id = %trace_id_clone,
                total_chunks = chunk_count,
                "Stream ended (no [DONE] received)"
            );
            // Anthropic 流以 message_stop 结束（不会发 OpenAI 的 [DONE]），
            // 这里补发一个 [DONE] 让 OpenAI 客户端正常结束读取。
            let _ = tx.send(Ok(Event::default().data("[DONE]"))).await;
            record_usage(accum_input, accum_output, accum_chars, accum_cache_read);
        });

        Ok(Sse::new(ReceiverStream::new(rx))
            .keep_alive(KeepAlive::default())
            .into_response())
    } else {
        // Upstream is OpenAI: sniff usage while forwarding bytes verbatim.
        let provider_kind = resolved.provider_kind.clone();
        let provider_id_log = provider_id.clone();
        let provider_name_log = resolved.provider_name.clone();
        let model_id_log = resolved.model_row_id.clone();
        let model_name_log = model_name.clone();
        let key_id_log = last_key_id.clone();
        let key_name_log = last_key_name.clone();
        let key_masked_log = last_key_masked.clone();
        let service_key_id_log = service_key.id.clone();
        let service_key_name_log = service_key.name.clone();
        let service_key_masked_log = service_key.key_masked.clone();
        let db = state.database.clone();
        let state_clone = state.clone();
        let mut sniff = sniff::SniffStream::new(response.bytes_stream(), &provider_kind);
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, reqwest::Error>>(100);

        tokio::spawn(async move {
            loop {
                let item = match tokio::time::timeout(
                    Duration::from_secs(UPSTREAM_CHUNK_TIMEOUT_SECS),
                    sniff.next(),
                )
                .await
                {
                    Ok(Some(i)) => i,
                    Ok(None) => break,
                    Err(_) => {
                        warn!(
                            "upstream stream silent for {}s, closing",
                            UPSTREAM_CHUNK_TIMEOUT_SECS
                        );
                        break;
                    }
                };
                if tx.send(item).await.is_err() {
                    break;
                }
            }
            let usage = sniff.into_usage();
            let output_tokens = if usage.output_tokens > 0 {
                usage.output_tokens as i64
            } else {
                (usage.output_chars / 4) as i64
            };
            let input_t = usage.input_tokens as i64;
            let cr = usage.cache_read_input_tokens as i64;
            let _ = db.insert_usage_log(
                chrono::Utc::now().timestamp(),
                &provider_id_log,
                provider_name_log.as_str(),
                &model_id_log,
                model_name_log.as_str(),
                key_id_log.as_deref(),
                key_name_log.as_deref().unwrap_or(""),
                key_masked_log.as_deref().unwrap_or(""),
                Some(service_key_id_log.as_str()),
                service_key_name_log.as_str(),
                service_key_masked_log.as_str(),
                "/v1/chat/completions",
                input_t,
                output_tokens,
                start_time.elapsed().as_millis() as i64,
                true,
                None,
                cr,
            );
        });

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .body(axum::body::Body::from_stream(
                tokio_stream::wrappers::ReceiverStream::new(rx),
            ))
            .unwrap())
    }
}

/// GET /v1/models - List available models.
pub async fn proxy_list_models(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, HeaderMap, Json<Value>)> {
    let api_key = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| headers.get("x-api-key").and_then(|v| v.to_str().ok()))
        .unwrap_or("");

    let service_key = match verify_service_key(&state, api_key).await {
        Some(info) => info,
        None => {
            return Err((
                StatusCode::UNAUTHORIZED,
                HeaderMap::new(),
                Json(json!({"error": {"type": "authentication_error", "message": "Invalid API key"}})),
            ))
        }
    };
    // 列表端点同样受配额约束：超限时模型列表不可用。
    if let Err((code, headers, body)) = check_quota(&state, &service_key).await {
        return Err((code, headers, body));
    }

    let conn = state.database.conn();

    let mut stmt = conn
        .prepare(
            "SELECT m.model_id, m.display_name, m.tier, p.name, m.context_window
             FROM models m
             JOIN providers p ON m.provider_id = p.id
             WHERE m.enabled = 1 AND p.enabled = 1
             ORDER BY m.tier, m.display_name",
        )
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                HeaderMap::new(),
                Json(json!({"error": {"type": "api_error", "message": e.to_string()}})),
            )
        })?;

    let models: Vec<Value> = stmt
        .query_map([], |row| {
            Ok(json!({
                "id": row.get::<_, String>(1)?,
                "object": "model",
                "created": 1699000000,
                "owned_by": row.get::<_, String>(3)?,
                "display_name": row.get::<_, String>(1)?,
                "tier": row.get::<_, String>(2)?,
                "context_window": row.get::<_, i64>(4)?,
            }))
        })
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                HeaderMap::new(),
                Json(json!({"error": {"type": "api_error", "message": e.to_string()}})),
            )
        })?
        .filter_map(|r| r.ok())
        .collect();

    // Apply allowed_models whitelist (empty = return all)
    let data: Vec<Value> = if service_key.allowed_models.is_empty() {
        models
    } else {
        models
            .into_iter()
            .filter(|m| {
                m["display_name"]
                    .as_str()
                    .map(|dn| service_key.allowed_models.iter().any(|a| a == dn))
                    .unwrap_or(false)
            })
            .collect()
    };

    Ok(Json(json!({
        "object": "list",
        "data": data,
    })))
}
