pub mod translate;
pub mod sniff;

use crate::gateway::server::AppState;
use axum::{
    extract::{Json, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
};
use futures::stream::StreamExt;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Instant;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{error, info, warn};
use uuid::Uuid;

/// POST /v1/messages - Anthropic Messages API proxy (streaming only).
pub async fn proxy_anthropic_messages(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, (StatusCode, Json<Value>)> {
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

    let (service_key_id, allowed_models) = match verify_service_key(&state, api_key).await {
        Some((id, allowed)) => {
            info!(trace_id = %trace_id, service_key_id = %id, "Service key verified");
            (id, allowed)
        }
        None => {
            warn!(trace_id = %trace_id, "Authentication failed: invalid API key");
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": {"type": "authentication_error", "message": "Invalid API key"}})),
            ))
        }
    };
    // Enforce allowed_models whitelist (empty = allow all). Clients must use the alias.
    if !allowed_models.is_empty() && !allowed_models.iter().any(|m| m == &model_name) {
        warn!(trace_id = %trace_id, model = %model_name, "Model not allowed for this service key");
        return Err((
            StatusCode::FORBIDDEN,
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
        return run_websearch_loop(&state, &body, &resolved, provider_is_anthropic, &trace_id, &service_key_id).await;
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
    let client = reqwest::Client::builder()
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
                        Json(json!({"error": {"type": "api_error", "message": "No available upstream keys"}})),
                    ));
                }
            }
        }
        let (api_key, key_id) = match pick_key_for(&state, &provider_id) {
            Some(k) => k,
            None => match last_resp {
                // All keys exhausted: forward the last failed upstream response.
                Some(r) => break r,
                None => {
                    return Err((
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({"error": {"type": "api_error", "message": "No available upstream keys"}})),
                    ));
                }
            },
        };
        last_key_id = Some(key_id.clone());

        let mut req_builder = client.post(&upstream_url);
        if provider_is_anthropic {
            req_builder = req_builder
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01");
        } else {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", api_key));
        }

        let resp = match req_builder
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                // Network error (not a key problem) — don't retry, record and bail.
                let duration_ms = start_time.elapsed().as_millis() as i64;
                error!(trace_id = %trace_id, duration_ms, error = %e, "Upstream call failed");
                let _ = state.database.insert_usage_log(
                    chrono::Utc::now().timestamp(),
                    &provider_id, &model_row_id,
                    Some(&key_id), Some(service_key_id.as_str()),
                    "/v1/messages",
                    0, 0, duration_ms, false, Some(&e.to_string()), None,
                );
                return Err((
                    StatusCode::BAD_GATEWAY,
                    Json(json!({"error": {"type": "api_error", "message": e.to_string()}})),
                ));
            }
        };

        let status = resp.status().as_u16();
        update_key_health(&state.keys, &provider_id, &api_key, status);

        if matches!(status, 401 | 402 | 403 | 429) {
            warn!(trace_id = %trace_id, status, key_id = %key_id, "upstream rejected key, rotating");
            last_resp = Some(resp);
            continue;
        }
        break resp;
    };

    let upstream_status = response.status().as_u16();

    if upstream_status >= 400 {
        return Ok(forward_upstream_error(
            &state.database, &provider_id, &model_row_id,
            last_key_id.as_deref(), Some(service_key_id.as_str()), "/v1/messages",
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
        let model_id_log = resolved.model_row_id.clone();
        let key_id_log = last_key_id.clone();
        let service_key_id_log = service_key_id.clone();

        tokio::spawn(async move {
            let mut buffer = String::new();
            let mut chunk_count = 0u64;
            let mut stream_state = translate::StreamState::new();
            // Whether the Anthropic envelope has been closed with message_stop.
            let mut stopped = false;
            // Whether the OpenAI upstream sent its [DONE] terminator.
            let mut saw_done = false;
            // TEMP DEBUG: capture upstream behavior + translated event sequence to
            // diagnose why Claude Code sees a malformed stream. Remove once stable.
            let mut saw_reasoning = false;
            let mut saw_finish: Option<String> = None;
            let mut event_seq: Vec<String> = Vec::new();

            'outer: while let Some(chunk) = stream.next().await {
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
                                // TEMP DEBUG: record upstream delta composition.
                                let first_delta = chunk_json
                                    .get("choices")
                                    .and_then(|c| c.as_array())
                                    .and_then(|a| a.first())
                                    .and_then(|c| c.get("delta"));
                                if first_delta
                                    .and_then(|d| d.get("reasoning_content"))
                                    .is_some()
                                {
                                    saw_reasoning = true;
                                }
                                if let Some(fr) = chunk_json
                                    .get("choices")
                                    .and_then(|c| c.as_array())
                                    .and_then(|a| a.first())
                                    .and_then(|c| c.get("finish_reason"))
                                    .and_then(|f| f.as_str())
                                {
                                    saw_finish = Some(fr.to_string());
                                }

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
                                        if event_type == "message_stop" {
                                            stopped = true;
                                        }
                                        // TEMP DEBUG: record type (and block index when present).
                                        if let Some(idx) = ev.get("index").and_then(|i| i.as_i64()) {
                                            event_seq.push(format!("{}:{}", event_type, idx));
                                        } else {
                                            event_seq.push(event_type.to_string());
                                        }
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

            // If the upstream ended without emitting message_stop (no finish_reason and
            // no [DONE]), close any still-open content block, then synthesize a clean
            // message_delta + message_stop so the client sees a well-formed close
            // instead of a truncated stream with an unclosed block.
            if !stopped {
                for ev in stream_state.close_open_blocks() {
                    let event_type = ev["type"].as_str().unwrap_or("message");
                    event_seq.push(format!("{}:0(synth)", event_type));
                    let json_str = serde_json::to_string(&ev).unwrap();
                    let _ = tx.send(
                        Ok(Event::default().event(event_type).data(json_str)),
                    )
                    .await;
                }
                let md = json!({
                    "type": "message_delta",
                    "delta": {"stop_reason": "end_turn", "stop_sequence": null},
                    "usage": {"output_tokens": 0},
                });
                event_seq.push("message_delta(synth)".to_string());
                let _ = tx.send(
                    Ok(Event::default()
                        .event("message_delta")
                        .data(serde_json::to_string(&md).unwrap())),
                )
                .await;
                event_seq.push("message_stop(synth)".to_string());
                let _ = tx.send(
                    Ok(Event::default()
                        .event("message_stop")
                        .data(serde_json::to_string(&json!({"type": "message_stop"})).unwrap())),
                )
                .await;
            }

            info!(
                trace_id = %trace_id_clone,
                total_chunks = chunk_count,
                done = saw_done,
                saw_reasoning,
                saw_finish = ?saw_finish,
                seq = ?event_seq,
                "Stream ended"
            );

            // Record usage. Prefer real token counts; fall back to chars/4 when
            // the upstream reported none.
            let output_tokens = if stream_state.output_tokens > 0 {
                stream_state.output_tokens as i64
            } else {
                (stream_state.output_chars / 4) as i64
            };
            let _ = db.insert_usage_log(
                chrono::Utc::now().timestamp(),
                &provider_id_log,
                &model_id_log,
                key_id_log.as_deref(),
                Some(service_key_id_log.as_str()),
                "/v1/messages",
                stream_state.input_tokens as i64,
                output_tokens,
                start_time.elapsed().as_millis() as i64,
                true,
                None,
                None,
            );
        });

        Ok(Sse::new(ReceiverStream::new(rx))
            .keep_alive(KeepAlive::default())
            .into_response())
    } else {
        // Upstream is Anthropic: sniff usage while forwarding bytes verbatim.
        let provider_kind = resolved.provider_kind.clone();
        let provider_id_log = provider_id.clone();
        let model_id_log = resolved.model_row_id.clone();
        let key_id_log = last_key_id.clone();
        let service_key_id_log = service_key_id.clone();
        let db = state.database.clone();
        let mut sniff = sniff::SniffStream::new(response.bytes_stream(), &provider_kind);
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, reqwest::Error>>(100);

        tokio::spawn(async move {
            while let Some(item) = sniff.next().await {
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
            let _ = db.insert_usage_log(
                chrono::Utc::now().timestamp(),
                &provider_id_log,
                &model_id_log,
                key_id_log.as_deref(),
                Some(service_key_id_log.as_str()),
                "/v1/messages",
                usage.input_tokens as i64,
                output_tokens,
                start_time.elapsed().as_millis() as i64,
                true,
                None,
                None,
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
) -> Result<Response, (StatusCode, Json<Value>)> {
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

    let (service_key_id, allowed_models) = match verify_service_key(&state, api_key).await {
        Some((id, allowed)) => {
            info!(trace_id = %trace_id, service_key_id = %id, "Service key verified");
            (id, allowed)
        }
        None => {
            warn!(trace_id = %trace_id, "Authentication failed: invalid API key");
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": {"type": "authentication_error", "message": "Invalid API key"}})),
            ))
        }
    };
    // Enforce allowed_models whitelist (empty = allow all). Clients must use the alias.
    if !allowed_models.is_empty() && !allowed_models.iter().any(|m| m == &model_name) {
        warn!(trace_id = %trace_id, model = %model_name, "Model not allowed for this service key");
        return Err((
            StatusCode::FORBIDDEN,
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
    let client = reqwest::Client::builder()
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
                        Json(json!({"error": {"type": "api_error", "message": "No available upstream keys"}})),
                    ));
                }
            }
        }
        let (api_key, key_id) = match pick_key_for(&state, &provider_id) {
            Some(k) => k,
            None => match last_resp {
                Some(r) => break r,
                None => {
                    return Err((
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({"error": {"type": "api_error", "message": "No available upstream keys"}})),
                    ));
                }
            },
        };
        last_key_id = Some(key_id.clone());

        let mut req_builder = client.post(&upstream_url);
        if provider_is_anthropic {
            req_builder = req_builder
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01");
        } else {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", api_key));
        }

        let resp = match req_builder
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let duration_ms = start_time.elapsed().as_millis() as i64;
                error!(trace_id = %trace_id, duration_ms, error = %e, "Upstream call failed");
                let _ = state.database.insert_usage_log(
                    chrono::Utc::now().timestamp(),
                    &provider_id, &model_row_id,
                    Some(&key_id), Some(service_key_id.as_str()),
                    "/v1/chat/completions",
                    0, 0, duration_ms, false, Some(&e.to_string()), None,
                );
                return Err((
                    StatusCode::BAD_GATEWAY,
                    Json(json!({"error": {"type": "api_error", "message": e.to_string()}})),
                ));
            }
        };

        let status = resp.status().as_u16();
        update_key_health(&state.keys, &provider_id, &api_key, status);

        if matches!(status, 401 | 402 | 403 | 429) {
            warn!(trace_id = %trace_id, status, key_id = %key_id, "upstream rejected key, rotating");
            last_resp = Some(resp);
            continue;
        }
        break resp;
    };

    let upstream_status = response.status().as_u16();

    if upstream_status >= 400 {
        return Ok(forward_upstream_error(
            &state.database, &provider_id, &model_row_id,
            last_key_id.as_deref(), Some(service_key_id.as_str()), "/v1/chat/completions",
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
        let provider_id_log = provider_id.clone();
        let model_id_log = resolved.model_row_id.clone();
        let key_id_log = last_key_id.clone();
        let service_key_id_log = service_key_id.clone();

        tokio::spawn(async move {
            let mut buffer = String::new();
            let mut chunk_count = 0u64;
            let mut accum_input: u64 = 0;
            let mut accum_output: u64 = 0;
            let mut accum_chars: u64 = 0;

            // Record usage with the chars/4 fallback. Borrows the log fields.
            let record_usage = |input_tokens: u64, output_tokens: u64, output_chars: u64| {
                let output_tokens = if output_tokens > 0 {
                    output_tokens as i64
                } else {
                    (output_chars / 4) as i64
                };
                let _ = db.insert_usage_log(
                    chrono::Utc::now().timestamp(),
                    &provider_id_log,
                    &model_id_log,
                    key_id_log.as_deref(),
                    Some(service_key_id_log.as_str()),
                    "/v1/chat/completions",
                    input_tokens as i64,
                    output_tokens,
                    start_time.elapsed().as_millis() as i64,
                    true,
                    None,
                    None,
                );
            };

            while let Some(chunk) = stream.next().await {
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
                                record_usage(accum_input, accum_output, accum_chars);
                                return;
                            }

                            if let Ok(chunk_json) = serde_json::from_str::<Value>(data) {
                                let (it, ot, ch) = translate::extract_anthropic_usage(&chunk_json);
                                accum_input = accum_input.max(it);
                                if ot > 0 {
                                    accum_output = ot;
                                }
                                accum_chars += ch;

                                let translated = translate::translate_anthropic_chunk_to_openai(&chunk_json);
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
            record_usage(accum_input, accum_output, accum_chars);
        });

        Ok(Sse::new(ReceiverStream::new(rx))
            .keep_alive(KeepAlive::default())
            .into_response())
    } else {
        // Upstream is OpenAI: sniff usage while forwarding bytes verbatim.
        let provider_kind = resolved.provider_kind.clone();
        let provider_id_log = provider_id.clone();
        let model_id_log = resolved.model_row_id.clone();
        let key_id_log = last_key_id.clone();
        let service_key_id_log = service_key_id.clone();
        let db = state.database.clone();
        let mut sniff = sniff::SniffStream::new(response.bytes_stream(), &provider_kind);
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, reqwest::Error>>(100);

        tokio::spawn(async move {
            while let Some(item) = sniff.next().await {
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
            let _ = db.insert_usage_log(
                chrono::Utc::now().timestamp(),
                &provider_id_log,
                &model_id_log,
                key_id_log.as_deref(),
                Some(service_key_id_log.as_str()),
                "/v1/chat/completions",
                usage.input_tokens as i64,
                output_tokens,
                start_time.elapsed().as_millis() as i64,
                true,
                None,
                None,
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
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let api_key = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| headers.get("x-api-key").and_then(|v| v.to_str().ok()))
        .unwrap_or("");

    let (_service_key_id, allowed_models) = match verify_service_key(&state, api_key).await {
        Some((id, allowed)) => (id, allowed),
        None => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": {"type": "authentication_error", "message": "Invalid API key"}})),
            ))
        }
    };

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
                Json(json!({"error": {"type": "api_error", "message": e.to_string()}})),
            )
        })?
        .filter_map(|r| r.ok())
        .collect();

    // Apply allowed_models whitelist (empty = return all)
    let data: Vec<Value> = if allowed_models.is_empty() {
        models
    } else {
        models
            .into_iter()
            .filter(|m| {
                m["display_name"]
                    .as_str()
                    .map(|dn| allowed_models.iter().any(|a| a == dn))
                    .unwrap_or(false)
            })
            .collect()
    };

    Ok(Json(json!({
        "object": "list",
        "data": data,
    })))
}

// ============================================================================
// Helper functions (V5 Schema)
// ============================================================================

struct ResolvedRoute {
    upstream_url: String,
    provider_kind: String,
    provider_id: String,
    real_model_id: String,
    /// models.id (UUID primary key) — needed for usage_log.model_id FK.
    model_row_id: String,
}

/// Verify a service key against the service_keys table (argon2 hash).
/// Returns the service_key id on success, None on failure.
async fn verify_service_key(state: &AppState, api_key: &str) -> Option<(String, Vec<String>)> {
    if api_key.is_empty() {
        return None;
    }

    // argon2 hashes are salted and not directly comparable, so enumerate and verify each.
    let conn = state.database.conn();
    let mut stmt = conn.prepare("SELECT id, key_hash, allowed_models FROM service_keys").ok()?;

    let rows: Vec<(String, String, String)> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .ok()?
        .filter_map(|r| r.ok())
        .collect();

    for (id, stored, allowed_str) in rows {
        if crate::api::verify_service_key(api_key, &stored) {
            let allowed: Vec<String> = serde_json::from_str(&allowed_str).unwrap_or_default();
            return Some((id, allowed));
        }
    }
    None
}

/// Resolve a route for the given model name using V5 normalized schema.
/// 1. Look up model by model_id or display_name (alias) in models table
/// 2. JOIN providers to get base_url, api_path, kind
/// 3. Get API key from KeyPool (plaintext) or DB fallback (decrypted)
async fn resolve_route(state: &AppState, model_name: &str) -> Option<ResolvedRoute> {
    let conn = state.database.conn();

    // Find model by display_name (alias) ONLY — calling with the real model_id
    // is rejected; clients must use the alias.
    let mut stmt = conn
        .prepare(
            "SELECT m.id, m.model_id, m.provider_id, p.base_url, p.api_path, p.kind
             FROM models m
             JOIN providers p ON m.provider_id = p.id
             WHERE m.display_name = ?1
               AND m.enabled = 1
               AND p.enabled = 1
             LIMIT 1",
        )
        .ok()?;

    let (model_row_id, real_model_id, provider_id, base_url, api_path, kind): (
        String,
        String,
        String,
        String,
        String,
        String,
    ) = stmt
        .query_row([&model_name.to_string()], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .ok()?;

    let upstream_url = format!("{}{}", base_url, api_path);

    Some(ResolvedRoute {
        upstream_url,
        provider_kind: kind,
        provider_id,
        real_model_id,
        model_row_id,
    })
}

/// Pick the next available key for a provider from the pool (round-robin,
/// skips Red/Yellow). Returns (plaintext_key, api_keys.id) or None when no
/// usable key remains. Called in the retry loop so 401/402/403/429 rotate keys.
fn pick_key_for(state: &AppState, provider_id: &str) -> Option<(String, String)> {
    match state.keys.get_next_key(provider_id) {
        Ok(entry) => Some((entry.key_hash, entry.id)),
        Err(_) => None,
    }
}

/// Drive the key pool health based on an upstream HTTP status code.
/// 401/403 -> red (invalid key), 402/429 -> yellow (quota/rate limit),
/// 2xx -> green (success). 5xx and other 4xx (400/404…) are NOT key problems,
/// so they leave the key state untouched.
fn update_key_health(pool: &crate::keys::KeyPool, provider_id: &str, key: &str, status: u16) {
    match status {
        401 | 403 => { let _ = pool.mark_key_invalid(provider_id, key); }
        402 | 429 => { let _ = pool.mark_key_low_quota(provider_id, key); }
        200..=299 => { let _ = pool.record_key_success(provider_id, key, 0); }
        _ => {}
    }
}

/// Forward an upstream error response (status >= 400) to the client as-is,
/// rather than attempting to stream a non-SSE body. Also records a failed
/// usage_log row (success=false, zero tokens).
async fn forward_upstream_error(
    database: &crate::db::Database,
    provider_id: &str,
    model_id: &str,
    key_id: Option<&str>,
    service_key_id: Option<&str>,
    request_type: &str,
    response: reqwest::Response,
    upstream_status: u16,
    trace_id: &str,
    start_time: &Instant,
) -> Response {
    let duration_ms = start_time.elapsed().as_millis();
    warn!(
        trace_id = %trace_id,
        upstream_status = upstream_status,
        duration_ms = duration_ms,
        "Upstream returned error, forwarding to client"
    );
    let err_body: Value = response
        .json()
        .await
        .unwrap_or_else(|_| json!({"error": {"type": "api_error", "message": "upstream error"}}));
    let code = StatusCode::from_u16(upstream_status).unwrap_or(StatusCode::BAD_GATEWAY);
    let _ = database.insert_usage_log(
        chrono::Utc::now().timestamp(),
        provider_id,
        model_id,
        key_id,
        service_key_id,
        request_type,
        0,
        0,
        duration_ms as i64,
        false,
        Some(&format!("upstream status {}", upstream_status)),
        None,
    );
    (code, Json(err_body)).into_response()
}

// ============================================================================
// WebSearch 劫持（本地 Bing 包装）
// ============================================================================

/// 请求 body 的 tools 里是否含 server-side web_search 工具。
fn has_websearch_tool(body: &Value) -> bool {
    body.get("tools")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter().any(|t| {
                t.get("type")
                    .and_then(|v| v.as_str())
                    .map(|s| s.starts_with("web_search"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// 把 Bing 结果格式化成喂给 LLM 的 tool_result 文本。
fn format_search_text(results: &[crate::search::SearchResult]) -> String {
    results
        .iter()
        .enumerate()
        .map(|(i, r)| format!("[{}] {}\n{}\n{}", i + 1, r.title, r.url, r.snippet))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 把 Bing 结果转成 Anthropic web_search_tool_result 的 content 数组。
fn search_result_blocks(results: &[crate::search::SearchResult]) -> Vec<Value> {
    results
        .iter()
        .map(|r| json!({"title": r.title, "url": r.url, "encrypted_content": ""}))
        .collect()
}

/// 把最终累积的 content blocks + stop_reason + usage 转成 Anthropic SSE 事件序列。
fn build_sse_events(msg_id: &str, model: &str, content: &[Value], stop_reason: &str, usage: &Value) -> Vec<Event> {
    let mk = |event_type: &str, payload: Value| {
        Event::default()
            .event(event_type)
            .data(serde_json::to_string(&payload).unwrap_or_default())
    };
    let mut events = vec![mk(
        "message_start",
        json!({
            "type": "message_start",
            "message": {
                "id": msg_id, "type": "message", "role": "assistant", "model": model,
                "content": [], "stop_reason": null, "stop_sequence": null,
                "usage": {"input_tokens": 0, "output_tokens": 0}
            }
        }),
    )];

    for (i, block) in content.iter().enumerate() {
        let bt = block["type"].as_str().unwrap_or("text");
        let start_block = match bt {
            "text" => json!({"type": "text", "text": ""}),
            "tool_use" => json!({"type": "tool_use", "id": block["id"], "name": block["name"], "input": {}}),
            _ => block.clone(),
        };
        events.push(mk("content_block_start", json!({"type": "content_block_start", "index": i, "content_block": start_block})));
        match bt {
            "text" => {
                let text = block["text"].as_str().unwrap_or("");
                events.push(mk("content_block_delta", json!({"type": "content_block_delta", "index": i, "delta": {"type": "text_delta", "text": text}})));
            }
            "tool_use" => {
                let input_json = serde_json::to_string(&block["input"]).unwrap_or_default();
                events.push(mk("content_block_delta", json!({"type": "content_block_delta", "index": i, "delta": {"type": "input_json_delta", "partial_json": input_json}})));
            }
            _ => {} // web_search_tool_result: 无 delta
        }
        events.push(mk("content_block_stop", json!({"type": "content_block_stop", "index": i})));
    }

    events.push(mk(
        "message_delta",
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": stop_reason, "stop_sequence": null},
            "usage": {"output_tokens": usage["output_tokens"]}
        }),
    ));
    events.push(mk("message_stop", json!({"type": "message_stop"})));
    events
}

/// WebSearch 劫持 loop：把 server-side web_search 改写成自定义 tool，
/// WebSearch 劫持 loop 入口：根据上游类型走 Anthropic 或 OpenAI 格式，
/// 在代理内跑 tool-calling loop（本地 Bing 搜索），累积内容转 SSE 返回客户端。
async fn run_websearch_loop(
    state: &Arc<AppState>,
    body: &Value,
    resolved: &ResolvedRoute,
    provider_is_anthropic: bool,
    _trace_id: &str,
    service_key_id: &str,
) -> Result<Response, (StatusCode, Json<Value>)> {
    let (api_key, key_id) = pick_key_for(state, &resolved.provider_id).ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": {"type": "api_error", "message": "No available upstream keys"}})),
        )
    })?;

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();

    let upstream_url = &resolved.upstream_url;
    let model = &resolved.real_model_id;
    let max_tokens = body["max_tokens"].as_u64().unwrap_or(4096);
    let msg_id = format!("msg_{}", uuid::Uuid::new_v4().simple());

    let mut accumulated: Vec<Value> = Vec::new();
    let mut final_stop = "end_turn".to_string();
    let mut final_usage = json!({"input_tokens": 0, "output_tokens": 0});

    // Some(Response) = 上游错误，直接返客户端；None = loop 正常结束
    let early: Option<Response> = if provider_is_anthropic {
        hijack_anthropic(
            &client, upstream_url, model, &api_key, body, max_tokens,
            &state.keys, &resolved.provider_id,
            &mut accumulated, &mut final_stop, &mut final_usage,
        )
        .await?
    } else {
        hijack_openai(
            &client, upstream_url, model, &api_key, body, max_tokens,
            &state.keys, &resolved.provider_id,
            &mut accumulated, &mut final_stop, &mut final_usage,
        )
        .await?
    };
    if let Some(resp) = early {
        return Ok(resp);
    }

    let in_t = final_usage["input_tokens"].as_i64().unwrap_or(0);
    let out_t = final_usage["output_tokens"].as_i64().unwrap_or(0);
    let _ = state.database.insert_usage_log(
        chrono::Utc::now().timestamp(),
        &resolved.provider_id,
        &resolved.model_row_id,
        Some(&key_id),
        Some(service_key_id),
        "/v1/messages",
        in_t,
        out_t,
        0,
        true,
        None,
        None,
    );

    let events = build_sse_events(&msg_id, model, &accumulated, &final_stop, &final_usage);
    let stream = futures::stream::iter(events.into_iter().map(Ok::<_, std::io::Error>));
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()).into_response())
}

/// Anthropic 上游的劫持 loop。
async fn hijack_anthropic(
    client: &reqwest::Client,
    upstream_url: &str,
    model: &str,
    api_key: &str,
    body: &Value,
    max_tokens: u64,
    pool: &crate::keys::KeyPool,
    provider_id: &str,
    accumulated: &mut Vec<Value>,
    final_stop: &mut String,
    final_usage: &mut Value,
) -> Result<Option<Response>, (StatusCode, Json<Value>)> {
    let custom_tool = json!({
        "name": "web_search",
        "description": "Search the web (Bing) for up-to-date information.",
        "input_schema": {
            "type": "object",
            "properties": {"query": {"type": "string", "description": "The search query"}},
            "required": ["query"]
        }
    });
    let req_system = body.get("system").cloned();
    let mut messages = body["messages"].as_array().cloned().unwrap_or_default();

    for _ in 0..5 {
        let mut req = serde_json::Map::new();
        req.insert("model".into(), json!(model));
        req.insert("messages".into(), json!(messages.clone()));
        req.insert("tools".into(), json!([custom_tool.clone()]));
        req.insert("tool_choice".into(), json!({"type": "auto"}));
        req.insert("stream".into(), json!(false));
        req.insert("max_tokens".into(), json!(max_tokens));
        if let Some(s) = &req_system {
            req.insert("system".into(), s.clone());
        }

        let resp = client
            .post(upstream_url)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&Value::Object(req))
            .send()
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, Json(json!({"error": {"type": "api_error", "message": e.to_string()}}))))?;
        let status = resp.status().as_u16();
        let msg_val: Value = resp.json().await
            .map_err(|e| (StatusCode::BAD_GATEWAY, Json(json!({"error": {"type": "api_error", "message": e.to_string()}}))))?;
        if status >= 400 {
            update_key_health(pool, provider_id, api_key, status);
            let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
            return Ok(Some((code, Json(msg_val)).into_response()));
        }
        update_key_health(pool, provider_id, api_key, status);

        let stop = msg_val["stop_reason"].as_str().unwrap_or("end_turn").to_string();
        let content = msg_val["content"].as_array().cloned().unwrap_or_default();
        *final_usage = msg_val["usage"].clone();
        accumulated.extend(content.clone());

        if stop != "tool_use" {
            *final_stop = stop;
            break;
        }

        let tool_uses: Vec<Value> = content
            .iter()
            .filter(|b| b["type"] == "tool_use" && b["name"] == "web_search")
            .cloned()
            .collect();
        messages.push(json!({"role": "assistant", "content": content}));

        let mut results: Vec<Value> = Vec::new();
        for tu in &tool_uses {
            let query = tu["input"]["query"].as_str().unwrap_or("");
            let bing = crate::search::bing::search(query).await.unwrap_or_default();
            accumulated.push(json!({"type": "web_search_tool_result", "tool_use_id": tu["id"], "content": search_result_blocks(&bing)}));
            results.push(json!({"type": "tool_result", "tool_use_id": tu["id"], "content": format_search_text(&bing)}));
        }
        messages.push(json!({"role": "user", "content": results}));
    }
    Ok(None)
}

/// OpenAI 兼容上游（如 qwen / 钉钉 DEAP）的劫持 loop。
async fn hijack_openai(
    client: &reqwest::Client,
    upstream_url: &str,
    model: &str,
    api_key: &str,
    body: &Value,
    max_tokens: u64,
    pool: &crate::keys::KeyPool,
    provider_id: &str,
    accumulated: &mut Vec<Value>,
    final_stop: &mut String,
    final_usage: &mut Value,
) -> Result<Option<Response>, (StatusCode, Json<Value>)> {
    let custom_fn = json!({
        "type": "function",
        "function": {
            "name": "web_search",
            "description": "Search the web (Bing) for up-to-date information.",
            "parameters": {"type": "object", "properties": {"query": {"type": "string", "description": "The search query"}}, "required": ["query"]}
        }
    });
    // 翻译客户端 Anthropic 请求为 OpenAI 格式（messages + system）
    let init = translate::anthropic_req_to_openai(body);
    let mut messages = init["messages"].as_array().cloned().unwrap_or_default();

    for _ in 0..5 {
        let req = json!({
            "model": model,
            "messages": messages,
            "tools": [custom_fn.clone()],
            "tool_choice": "auto",
            "stream": false,
            "max_tokens": max_tokens,
        });
        let resp = client
            .post(upstream_url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&req)
            .send()
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, Json(json!({"error": {"type": "api_error", "message": e.to_string()}}))))?;
        let status = resp.status().as_u16();
        let msg_val: Value = resp.json().await
            .map_err(|e| (StatusCode::BAD_GATEWAY, Json(json!({"error": {"type": "api_error", "message": e.to_string()}}))))?;
        if status >= 400 {
            update_key_health(pool, provider_id, api_key, status);
            let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
            return Ok(Some((code, Json(msg_val)).into_response()));
        }
        update_key_health(pool, provider_id, api_key, status);

        let choice = &msg_val["choices"][0];
        let finish = choice["finish_reason"].as_str().unwrap_or("stop");
        let content_text = choice["message"]["content"].as_str().unwrap_or("");
        let tool_calls = choice["message"]["tool_calls"].as_array().cloned().unwrap_or_default();
        *final_usage = json!({
            "input_tokens": msg_val["usage"]["prompt_tokens"].as_i64().unwrap_or(0),
            "output_tokens": msg_val["usage"]["completion_tokens"].as_i64().unwrap_or(0),
        });

        if !content_text.is_empty() {
            accumulated.push(json!({"type": "text", "text": content_text}));
        }

        if finish != "tool_calls" || tool_calls.is_empty() {
            *final_stop = match finish { "length" => "max_tokens", _ => "end_turn" }.to_string();
            break;
        }

        // 追加 assistant（OpenAI 格式，含 tool_calls）
        messages.push(json!({"role": "assistant", "content": content_text, "tool_calls": tool_calls.clone()}));

        // 并行搜索所有 web_search tool_call（一轮多个时省时间）
        let ws_calls: Vec<Value> = tool_calls
            .iter()
            .filter(|tc| tc["function"]["name"].as_str() == Some("web_search"))
            .cloned()
            .collect();
        let ws_results = futures::future::join_all(ws_calls.iter().map(|tc| async move {
            let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
            let input: Value = serde_json::from_str(args_str).unwrap_or(json!({}));
            let query = input["query"].as_str().unwrap_or("").to_string();
            let bing = crate::search::bing::search(&query).await.unwrap_or_default();
            (tc.clone(), input, bing)
        }))
        .await;
        for (tc, input, bing) in ws_results {
            accumulated.push(json!({"type": "tool_use", "id": tc["id"], "name": "web_search", "input": input}));
            accumulated.push(json!({"type": "web_search_tool_result", "tool_use_id": tc["id"], "content": search_result_blocks(&bing)}));
            messages.push(json!({"role": "tool", "tool_call_id": tc["id"], "content": format_search_text(&bing)}));
        }
    }
    Ok(None)
}
