use std::sync::Arc;

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{header, StatusCode},
    response::{sse::Event, IntoResponse, Response, Sse},
};
use chrono::Utc;
use futures::StreamExt;
use serde_json::Value;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::{
    config::ProxyConfig,
    models::SqlUuid,
    proxy::{
        blocker::UpstreamBlocker,
        error_log::UpstreamErrorLogger,
        rate_limiter::{RateLimiter, UpstreamRateLimitConfig, UpstreamRateLimiter},
        upstream::{
            convert_anthropic_stream_to_openai, convert_anthropic_to_openai_response,
            convert_ollama_stream_to_openai, convert_ollama_to_openai_response,
            convert_openai_to_anthropic_request, convert_openai_to_ollama_request,
            estimate_tokens_by_rules,
            ChatCompletionRequest, ChatCompletionResponse, LoadBalancer, OllamaChatResponse,
            OllamaStreamChunk, UpstreamClient, Usage,
        },
    },
    store::{ConversationRecord, ConversationUpdate, MessageRecord, ProxyLogContent, ProxyLogRecord, StoreManager},
    utils::error::{AppError, AppResult},
};

#[derive(Clone)]
#[allow(dead_code)]
pub struct ProxyState {
    pub store: Arc<StoreManager>,
    pub client: UpstreamClient,
    pub rate_limiter: Arc<RateLimiter>,
    pub upstream_rate_limiter: Arc<UpstreamRateLimiter>,
    pub load_balancer: LoadBalancer,
    pub config: ProxyConfig,
    pub blocker: UpstreamBlocker,
    pub error_logger: Option<UpstreamErrorLogger>,
}

pub async fn list_models(
    State(state): State<ProxyState>,
    request: Request,
) -> AppResult<Response> {
    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| {
            tracing::warn!("List models request rejected: missing authorization header");
            AppError::Unauthorized("Missing authorization header".to_string())
        })?;

    let api_key_str = auth_header
        .strip_prefix("Bearer ")
        .ok_or_else(|| {
            tracing::warn!("List models request rejected: invalid authorization format");
            AppError::Unauthorized("Invalid authorization format".to_string())
        })?;

    let api_key = state.store.verify_api_key(api_key_str).await.map_err(|e| {
        tracing::warn!("List models request rejected: API key verification failed - {}", e);
        e
    })?;

    let models = state.store.get_visible_models(api_key.user_id, api_key.tenant_id).await;

    let response = crate::store::ModelsResponse {
        object: "list".to_string(),
        data: models,
    };

    Ok(axum::Json(response).into_response())
}

pub async fn proxy_handler(
    State(state): State<ProxyState>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    request: Request,
) -> AppResult<Response> {
    let path = request.uri().path().to_string();
    let method = request.method().clone();
    tracing::info!("Proxy request: {} {} from {}", method, path, addr);

    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| {
            tracing::warn!("Proxy request rejected: missing authorization header from {}", addr);
            AppError::Unauthorized("Missing authorization header".to_string())
        })?;

    let api_key_str = auth_header
        .strip_prefix("Bearer ")
        .ok_or_else(|| {
            tracing::warn!("Proxy request rejected: invalid authorization format from {}", addr);
            AppError::Unauthorized("Invalid authorization format".to_string())
        })?;

    let api_key = state.store.verify_api_key(api_key_str).await.map_err(|e| {
        tracing::warn!("Proxy request rejected: API key verification failed from {} - {}", addr, e);
        e
    })?;

    let user = state
        .store
        .get_user(api_key.user_id)
        .await
        .ok_or_else(|| {
            tracing::error!("User not found for API key {}", api_key.id);
            AppError::NotFound("User not found".to_string())
        })?;

    let rate_config = crate::proxy::rate_limiter::RateLimitConfig {
        rpm_limit: api_key.rpm_limit,
        tpm_limit: api_key.tpm_limit,
        daily_limit: api_key.daily_limit as i64,
    };

    state
        .rate_limiter
        .check_rate_limit(api_key.id, &rate_config)
        .map_err(|e| {
            tracing::warn!("Rate limit exceeded for user {}: {}", user.username, e);
            AppError::RateLimitExceeded(e)
        })?;

    state.store.check_user_request_quota(&user).await.map_err(|e| {
        tracing::warn!("User {} quota exceeded: {}", user.username, e);
        e
    })?;

    let incoming_headers = request.headers().clone();

    let hard_limit = state.config.max_request_body_bytes;
    let body_bytes = axum::body::to_bytes(request.into_body(), hard_limit)
        .await
        .map_err(|e| {
            let message = e.to_string();
            if message.contains("length limit exceeded") {
                return AppError::PayloadTooLarge(format!(
                    "Request body exceeds hard limit: {} bytes",
                    hard_limit
                ));
            }
            AppError::Internal(format!("Failed to read body: {}", message))
        })?;

    let body: Option<Value> = if body_bytes.is_empty() {
        None
    } else {
        serde_json::from_slice(&body_bytes)
            .map_err(|e| AppError::BadRequest(format!("Invalid JSON body: {}", e)))?
    };

    let is_multimodal = is_multimodal_request(&body);
    let configured_limit = if is_multimodal {
        state.config.max_multimodal_request_body_bytes
    } else {
        state.config.max_text_request_body_bytes
    };
    let effective_limit = configured_limit.min(hard_limit);
    if body_bytes.len() > effective_limit {
        let mode = if is_multimodal { "multimodal" } else { "text" };
        return Err(AppError::PayloadTooLarge(format!(
            "Request body exceeds {} limit: {} bytes (current: {} bytes)",
            mode,
            effective_limit,
            body_bytes.len()
        )));
    }

    let is_stream = body
        .as_ref()
        .and_then(|b: &Value| b.get("stream").and_then(|s| s.as_bool()))
        .unwrap_or(false);

    let model = body
        .as_ref()
        .and_then(|b: &Value| b.get("model").and_then(|m| m.as_str()))
        .unwrap_or("gpt-3.5-turbo");

    tracing::info!(
        "Proxy request details: model={}, stream={}, user={}, multimodal={}",
        model, is_stream, user.username, is_multimodal
    );

    let mut available_upstreams: Vec<(crate::store::UpstreamCache, String, Option<crate::store::ModelVisibilityCache>)> =
        Vec::new();
    let mut used_upstream_ids: Vec<Uuid> = Vec::new();
    let routing_text = extract_routing_text_from_request(&body);
    let estimated_input_tokens = estimate_input_tokens(&routing_text);
    let conditional_routes = state
        .store
        .resolve_conditional_alias_routes(
            api_key.user_id,
            api_key.tenant_id,
            model,
            &routing_text,
            estimated_input_tokens,
            is_multimodal,
        )
        .await;

    if !conditional_routes.is_empty() {
        tracing::debug!("Model '{}' matched {} conditional route(s)", model, conditional_routes.len());
        for (upstream_id, routed_model) in conditional_routes {
            let Some(upstream) = state.store.get_upstream(upstream_id).await else {
                tracing::warn!("Conditional route upstream {} not found in cache", upstream_id);
                continue;
            };
            if upstream.status != "active" {
                tracing::debug!("Skipping conditional route upstream '{}' (status={})", upstream.name, upstream.status);
                continue;
            }
            if state.blocker.is_blocked(upstream.id) {
                tracing::debug!("Skipping conditional route upstream '{}' (blocked)", upstream.name);
                continue;
            }
            let vis = state
                .store
                .get_model_visibility(upstream.id, &routed_model)
                .await;
            used_upstream_ids.push(upstream.id);
            available_upstreams.push((upstream, routed_model, vis));
        }
    }

    let upstreams_list = state.store.get_upstreams_by_tenant(api_key.tenant_id).await;
    for u in upstreams_list {
        if u.status != "active" {
            continue;
        }
        if state.blocker.is_blocked(u.id) {
            continue;
        }
        if used_upstream_ids.contains(&u.id) {
            continue;
        }
        if let Some(routed_model) = state
            .store
            .resolve_requested_model(api_key.user_id, u.id, model)
            .await
        {
            tracing::debug!("Upstream '{}' resolved model '{}' -> '{}'", u.name, model, routed_model);
            let vis = state
                .store
                .get_model_visibility(u.id, &routed_model)
                .await;
            available_upstreams.push((u, routed_model, vis));
        }
    }

    if available_upstreams.is_empty() {
        tracing::warn!(
            "No active upstream found for model='{}', user={}",
            model, user.username
        );
        return Err(AppError::Forbidden(format!(
            "No active upstream found for model: {}",
            model
        )));
    }

    let conversation_id = Uuid::new_v4().to_string();
    let client_ip = addr.ip().to_string();

    let conversation_uuid = Uuid::new_v4();
    let conversation = ConversationRecord {
        id: conversation_uuid,
        conversation_id: conversation_id.clone(),
        tenant_id: api_key.tenant_id,
        user_id: api_key.user_id,
        api_key_id: api_key.id,
        model: model.to_string(),
        provider: String::new(),
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        status: Some(Utc::now()),
        client_ip: client_ip.clone(),
        started_at: Utc::now(),
        ended_at: None,
    };

    if let Err(e) = state.store.create_conversation(conversation.clone()) {
        return Err(e);
    }

    let mut last_error = None;
    let mut tried_upstream_ids: Vec<Uuid> = Vec::new();
    let mut last_provider = String::new();
    let mut last_routed_model = String::new();
    let mut upstream_idx = 0;

    while upstream_idx < available_upstreams.len() {
        let (upstream, routed_model, vis) = &available_upstreams[upstream_idx];
        let upstream_id = upstream.id;
        let routed_model_clone = routed_model.clone();
        last_provider = upstream.provider.clone();
        last_routed_model = routed_model.clone();

        let model_headers = vis.as_ref()
            .map(|v| v.model_headers.clone())
            .unwrap_or_else(|| serde_json::json!({}));
        let retry_count = vis.as_ref().map(|v| v.retry_count).unwrap_or(0);
        let retry_interval_ms = vis.as_ref().map(|v| v.retry_interval_seconds).unwrap_or(0);
        let retry_backoff_strategy = vis.as_ref()
            .map(|v| v.retry_backoff_strategy.as_str())
            .unwrap_or("fixed");
        let retry_max_interval_ms = vis.as_ref().map(|v| v.retry_max_interval_seconds).unwrap_or(0);
        let current_retry_failure_strategy = vis.as_ref()
            .map(|v| v.retry_failure_strategy.clone())
            .unwrap_or_else(|| "error".to_string());
        let current_retry_fallback_upstream_id = vis.as_ref().and_then(|v| v.retry_fallback_upstream_id);
        let current_retry_fallback_model_name = vis.as_ref().and_then(|v| v.retry_fallback_model_name.clone());

        tracing::info!(
            "Trying upstream '{}' model='{}': retries={}, fallback_upstream={:?}, fallback_model={:?}",
            upstream.name, routed_model, retry_count,
            current_retry_fallback_upstream_id, current_retry_fallback_model_name
        );

        let max_attempts = if retry_count > 0 { retry_count as usize + 1 } else { 1 };

        'retry: for attempt in 0..max_attempts {
            if attempt > 0 {
                let delay_ms = calculate_retry_delay(
                    attempt,
                    retry_interval_ms,
                    retry_backoff_strategy,
                    retry_max_interval_ms,
                );
                if delay_ms > 0 {
                    tracing::info!(
                        "Retry attempt {}/{} for upstream '{}' model='{}', waiting {}ms",
                        attempt, max_attempts - 1, upstream.name, routed_model, delay_ms
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms as u64)).await;
                }
            }

            let upstream_rate_config = UpstreamRateLimitConfig {
                daily_request_limit: upstream.daily_request_limit,
                monthly_request_limit: upstream.monthly_request_limit,
            };

            if let Err(e) = state
                .upstream_rate_limiter
                .check_limit(upstream.id, &upstream_rate_config)
            {
                tracing::warn!("Upstream '{}' rate limit exceeded: {}", upstream.name, e);
                last_error = Some(AppError::RateLimitExceeded(e));
                break 'retry;
            }

            let is_ollama = upstream.api_type.eq_ignore_ascii_case("ollama") || upstream.provider.eq_ignore_ascii_case("ollama");
            let is_anthropic = upstream.api_type.eq_ignore_ascii_case("anthropic") || upstream.provider.eq_ignore_ascii_case("anthropic");
            let is_minimax = upstream.provider == "minimax";
            let request_body_str: Option<String> = body.as_ref().map(|b: &Value| b.to_string());
            let messages = extract_messages_from_request(&body);

            let (upstream_path, upstream_body) = match build_upstream_request(
                &body, routed_model, is_ollama, is_anthropic, is_minimax, &upstream.base_url, &path,
                &state, &mut last_error,
            ).await {
                Some(result) => result,
                None => {
                    tracing::warn!("Failed to build upstream request for '{}' model='{}'", upstream.name, routed_model);
                    break 'retry;
                }
            };

            let upstream_config = upstream.to_config();
            match state
                .client
                .proxy_request(
                    &upstream_config,
                    &upstream_path,
                    method.clone(),
                    upstream_body,
                    &incoming_headers,
                    Some(&model_headers),
                )
                .await
            {
                Ok(response) => {
                    let status = response.status();

                    if status == StatusCode::TOO_MANY_REQUESTS {
                        let error_body = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
                        let reason = "Rate limit exceeded from upstream".to_string();
                        state.blocker.block_upstream(upstream.id, reason);
                        let error_message = format!("Upstream rate limit: {} - {}", status, error_body);
                        log_upstream_error(&state.error_logger, &upstream.base_url, routed_model, &error_message, Some(status.as_u16())).await;
                        last_error = Some(AppError::RateLimitExceeded(
                            "Upstream rate limit exceeded".to_string(),
                        ));
                        continue 'retry;
                    }

                    if status.is_client_error() || status.is_server_error() {
                        let error_body = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());

                        if is_rate_limit_error(&error_body, status) {
                            let reason = format!("Rate limit error: {}", error_body);
                            state.blocker.block_upstream(upstream.id, reason);
                            let error_message = format!("Upstream rate limit: {} - {}", status, error_body);
                            log_upstream_error(&state.error_logger, &upstream.base_url, routed_model, &error_message, Some(status.as_u16())).await;
                            last_error = Some(AppError::RateLimitExceeded(
                                "Upstream rate limit exceeded".to_string(),
                            ));
                            continue 'retry;
                        }

                        let error_message = format!("Upstream error: {} - {}", status, error_body);
                        log_upstream_error(&state.error_logger, &upstream.base_url, routed_model, &error_message, Some(status.as_u16())).await;
                        last_error = Some(AppError::Internal(error_message));
                        if status.is_server_error() || status == StatusCode::FORBIDDEN {
                            continue 'retry;
                        }
                        break 'retry;
                    }

                    if is_stream {
                        return handle_stream_response(
                            state, response, conversation, api_key.id, upstream.id,
                            is_ollama, is_anthropic, model.to_string(), routed_model.to_string(),
                            upstream.provider.clone(),
                            request_body_str, messages,
                        ).await;
                    } else {
                        match handle_normal_response(
                            state.clone(), response, conversation.clone(), api_key.id, upstream.id,
                            is_ollama, is_anthropic, model.to_string(), routed_model.to_string(),
                            upstream.provider.clone(),
                            request_body_str.clone(), messages.clone(),
                        ).await {
                            Ok(resp) => return Ok(resp),
                            Err(e) => {
                                let error_message = e.to_string();
                                log_upstream_error(&state.error_logger, &upstream.base_url, routed_model, &error_message, None).await;
                                last_error = Some(e);
                                continue 'retry;
                            }
                        }
                    }
                }
                Err(e) => {
                    let error_msg = e.to_string().to_lowercase();
                    if error_msg.contains("rate limit") || error_msg.contains("too many") || error_msg.contains("429") {
                        let reason = format!("Connection error with rate limit: {}", e);
                        state.blocker.block_upstream(upstream.id, reason);
                        let error_message = format!("Upstream request failed: {}", e);
                        log_upstream_error(&state.error_logger, &upstream.base_url, routed_model, &error_message, Some(429)).await;
                        last_error = Some(AppError::RateLimitExceeded(
                            "Upstream rate limit exceeded".to_string(),
                        ));
                        continue 'retry;
                    }
                    let error_message = format!("Upstream request failed: {}", e);
                    log_upstream_error(&state.error_logger, &upstream.base_url, routed_model, &error_message, None).await;
                    last_error = Some(AppError::Internal(error_message));
                    continue 'retry;
                }
            }
        }

        tracing::info!(
            "Upstream '{}' model='{}' all attempts exhausted, checking fallback: fallback_upstream={:?}",
            upstream.name, routed_model_clone, current_retry_fallback_upstream_id
        );

        tried_upstream_ids.push(upstream_id);

        if current_retry_failure_strategy == "route" {
            if let Some(fallback_upstream_id) = current_retry_fallback_upstream_id {
                if let Some(fallback_upstream) = state.store.get_upstream(fallback_upstream_id).await {
                    if fallback_upstream.status == "active" && !state.blocker.is_blocked(fallback_upstream.id) {
                        let fallback_model = current_retry_fallback_model_name.as_deref().unwrap_or(model);
                        if state.store.check_model_access(api_key.user_id, fallback_upstream_id, fallback_model).await {
                            if !tried_upstream_ids.contains(&fallback_upstream_id) {
                                tracing::info!(
                                    "Routing to fallback: upstream='{}', model='{}' (original upstream '{}' failed)",
                                    fallback_upstream.name, fallback_model, upstream.name
                                );
                                let fallback_vis = state.store.get_model_visibility(fallback_upstream_id, fallback_model).await;
                                let insert_pos = upstream_idx + 1;
                                available_upstreams.insert(insert_pos, (fallback_upstream, fallback_model.to_string(), fallback_vis));
                            } else {
                                tracing::debug!("Fallback upstream '{}' already tried, skipping", fallback_upstream.name);
                            }
                        } else {
                            tracing::warn!("Fallback upstream '{}' model '{}' access denied for user {}", fallback_upstream.name, fallback_model, user.username);
                        }
                    } else {
                        tracing::warn!("Fallback upstream '{}' is not available (status={}, blocked={})", fallback_upstream.name, fallback_upstream.status, state.blocker.is_blocked(fallback_upstream.id));
                    }
                } else {
                    tracing::warn!("Fallback upstream {} not found in cache", fallback_upstream_id);
                }
            } else {
                tracing::warn!("Retry failure strategy is 'route' but no fallback upstream configured for upstream '{}' model '{}'", upstream.name, routed_model_clone);
            }
        }

        upstream_idx += 1;
    }

    let request_body_str: Option<String> = body.as_ref().map(|b: &Value| b.to_string());
    let messages = extract_messages_from_request(&body);
    let error_message = last_error.as_ref().map(|e| e.to_string()).unwrap_or_default();

    let _ = write_failed_proxy_log(
        &state.store, &conversation, api_key.id,
        Some(last_routed_model), last_provider.clone(), request_body_str, messages,
        error_message, None,
    );

    let _ = state.store.update_conversation(
        conversation.id,
        ConversationUpdate {
            provider: Some(last_provider),
            input_tokens: Some(0),
            output_tokens: Some(0),
            total_tokens: Some(0),
            ended_at: Some(Utc::now()),
        },
    );

    Err(last_error.unwrap_or_else(|| {
        AppError::ServiceUnavailable("All upstreams are unavailable".to_string())
    }))
}

async fn build_upstream_request(
    body: &Option<Value>,
    routed_model: &str,
    is_ollama: bool,
    is_anthropic: bool,
    is_minimax: bool,
    base_url: &str,
    path: &str,
    state: &ProxyState,
    last_error: &mut Option<AppError>,
) -> Option<(String, Option<Value>)> {
    if is_ollama {
        let ollama_path = "/api/chat".to_string();
        let ollama_body = if let Some(ref b) = body {
            let mut openai_req: ChatCompletionRequest = match serde_json::from_value(b.clone()) {
                Ok(req) => req,
                Err(e) => {
                    let error_message = format!("Invalid request: {}", e);
                    log_upstream_error(&state.error_logger, base_url, routed_model, &error_message, None).await;
                    *last_error = Some(AppError::BadRequest(error_message));
                    return None;
                }
            };
            openai_req.model = routed_model.to_string();
            match serde_json::to_value(convert_openai_to_ollama_request(&openai_req)) {
                Ok(v) => Some(v),
                Err(e) => {
                    let error_message = format!("Failed to convert request: {}", e);
                    log_upstream_error(&state.error_logger, base_url, routed_model, &error_message, None).await;
                    *last_error = Some(AppError::Internal(error_message));
                    return None;
                }
            }
        } else {
            None
        };
        Some((ollama_path, ollama_body))
    } else if is_anthropic {
        let anthropic_path = "/v1/messages".to_string();
        let anthropic_body = if let Some(ref b) = body {
            let mut openai_req: ChatCompletionRequest = match serde_json::from_value(b.clone()) {
                Ok(req) => req,
                Err(e) => {
                    let error_message = format!("Invalid request: {}", e);
                    log_upstream_error(&state.error_logger, base_url, routed_model, &error_message, None).await;
                    *last_error = Some(AppError::BadRequest(error_message));
                    return None;
                }
            };
            openai_req.model = routed_model.to_string();
            match serde_json::to_value(convert_openai_to_anthropic_request(&openai_req)) {
                Ok(v) => Some(v),
                Err(e) => {
                    let error_message = format!("Failed to convert request: {}", e);
                    log_upstream_error(&state.error_logger, base_url, routed_model, &error_message, None).await;
                    *last_error = Some(AppError::Internal(error_message));
                    return None;
                }
            }
        } else {
            None
        };
        Some((anthropic_path, anthropic_body))
    } else if is_minimax {
        let minimax_path = "/v1/chat/completions".to_string();
        Some((minimax_path, replace_request_model(body, routed_model)))
    } else {
        Some((
            normalize_upstream_chat_path(base_url, path),
            replace_request_model(body, routed_model),
        ))
    }
}

fn calculate_retry_delay(attempt: usize, base_interval_ms: i64, strategy: &str, max_interval_ms: i64) -> i64 {
    if base_interval_ms <= 0 {
        return 0;
    }
    let delay = match strategy {
        "exponential" => {
            let exp_delay = base_interval_ms * (2_i64.pow(attempt as u32 - 1));
            if max_interval_ms > 0 {
                exp_delay.min(max_interval_ms)
            } else {
                exp_delay
            }
        }
        "exponential_jitter" => {
            let exp_delay = base_interval_ms * (2_i64.pow(attempt as u32 - 1));
            let capped = if max_interval_ms > 0 {
                exp_delay.min(max_interval_ms)
            } else {
                exp_delay
            };
            let jitter = (capped as f64 * 0.25 * rand_random_factor()).round() as i64;
            capped + jitter
        }
        _ => base_interval_ms,
    };
    delay.max(0)
}

fn rand_random_factor() -> f64 {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    (nanos as f64 / u32::MAX as f64) * 2.0 - 1.0
}

fn is_rate_limit_error(body: &str, status: StatusCode) -> bool {
    if status == StatusCode::TOO_MANY_REQUESTS {
        return true;
    }

    if let Ok(json) = serde_json::from_str::<Value>(body) {
        if let Some(error) = json.get("error") {
            if let Some(error_obj) = error.as_object() {
                if let Some(error_type) = error_obj.get("type").and_then(|t| t.as_str()) {
                    let error_type_lower = error_type.to_lowercase();
                    if error_type_lower.contains("rate_limit")
                        || error_type_lower.contains("quota")
                        || error_type_lower == "insufficient_quota"
                    {
                        return true;
                    }
                }

                if let Some(code) = error_obj.get("code").and_then(|c| c.as_str()) {
                    let code_lower = code.to_lowercase();
                    if code_lower.contains("rate_limit")
                        || code_lower.contains("quota")
                        || code_lower == "insufficient_quota"
                    {
                        return true;
                    }
                }

                if let Some(code_num) = error_obj.get("code").and_then(|c| c.as_i64()) {
                    if code_num == 429 {
                        return true;
                    }
                }
            }

            if let Some(error_str) = error.as_str() {
                let error_lower = error_str.to_lowercase();
                if is_rate_limit_message(&error_lower) {
                    return true;
                }
            }
        }

        if let Some(code) = json.get("code").and_then(|c| c.as_i64()) {
            if code == 429 {
                return true;
            }
        }

        if let Some(message) = json.get("message").and_then(|m| m.as_str()) {
            let msg_lower = message.to_lowercase();
            if is_rate_limit_message(&msg_lower) {
                return true;
            }
        }
    }

    false
}

fn is_rate_limit_message(msg: &str) -> bool {
    let patterns = [
        "rate limit exceeded",
        "rate limit reached",
        "too many requests",
        "quota exceeded",
        "insufficient quota",
        "requests per minute",
        "tokens per minute",
        "api rate limit",
    ];

    for pattern in &patterns {
        if msg.contains(pattern) {
            return true;
        }
    }

    false
}

fn replace_request_model(body: &Option<Value>, model: &str) -> Option<Value> {
    let mut next = body.clone();
    if let Some(ref mut v) = next {
        if let Some(obj) = v.as_object_mut() {
            obj.insert("model".to_string(), Value::String(model.to_string()));
        }
    }
    next
}

fn normalize_upstream_chat_path(base_url: &str, path: &str) -> String {
    let normalized_base = base_url.trim_end_matches('/').to_ascii_lowercase();
    if normalized_base.ends_with("/v2/coding") && path == "/v1/chat/completions" {
        return "/chat/completions".to_string();
    }
    if normalized_base.ends_with("/v1") && path.starts_with("/v1/") {
        return path[3..].to_string();
    }
    path.to_string()
}

fn extract_messages_from_request(body: &Option<Value>) -> Vec<MessageRecord> {
    let mut messages = Vec::new();

    if let Some(b) = body {
        if let Some(msgs) = b.get("messages").and_then(|m| m.as_array()) {
            for msg in msgs {
                if let (Some(role), Some(content)) = (
                    msg.get("role").and_then(|r| r.as_str()),
                    extract_message_content(msg),
                ) {
                    messages.push(MessageRecord {
                        role: role.to_string(),
                        content,
                    });
                }
            }
        }
    }

    messages
}

fn extract_routing_text_from_request(body: &Option<Value>) -> String {
    let mut chunks: Vec<String> = Vec::new();
    if let Some(payload) = body {
        if let Some(prompt) = payload.get("prompt").and_then(|v| v.as_str()) {
            chunks.push(prompt.to_string());
        }
        if let Some(input) = payload.get("input").and_then(|v| v.as_str()) {
            chunks.push(input.to_string());
        }
        if let Some(msgs) = payload.get("messages").and_then(|m| m.as_array()) {
            for msg in msgs {
                if let Some(content) = extract_message_content(msg) {
                    chunks.push(content);
                }
            }
        }
    }
    chunks.join("\n")
}

fn estimate_input_tokens(text: &str) -> i64 {
    estimate_tokens_by_rules(text)
}

async fn handle_normal_response(
    state: ProxyState,
    response: reqwest::Response,
    conversation: ConversationRecord,
    api_key_id: Uuid,
    upstream_id: Uuid,
    is_ollama: bool,
    is_anthropic: bool,
    model: String,
    routed_model: String,
    provider: String,
    request_body_str: Option<String>,
    messages: Vec<MessageRecord>,
) -> AppResult<Response> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| AppError::Internal(format!("Failed to read response: {}", e)))?;
    if !status.is_success() {
        return Err(AppError::Internal(format!(
            "Upstream error: {} - {}",
            status, body
        )));
    }
    let normalized_body = normalize_response_body(&body);

    let (usage, response_content, response_body): (Option<Usage>, Option<String>, String) =
        if is_ollama {
            let mut final_response: Option<OllamaChatResponse> = None;
            let mut full_content = String::new();
            let mut full_thinking = String::new();

            for line in body.lines() {
                if line.is_empty() {
                    continue;
                }
                if let Ok(chunk) = serde_json::from_str::<OllamaChatResponse>(line) {
                    if let Some(ref message) = chunk.message {
                        full_content.push_str(&message.content);
                        if let Some(ref thinking) = message.thinking {
                            full_thinking.push_str(thinking);
                        }
                    }

                    if chunk.done {
                        final_response = Some(chunk);
                        break;
                    }
                }
            }

            let mut ollama_response = final_response
                .ok_or_else(|| AppError::Internal("No final Ollama response found".to_string()))
                .or_else(|_| {
                    serde_json::from_str::<OllamaChatResponse>(normalized_body)
                        .map_err(|_| AppError::Internal("No final Ollama response found".to_string()))
                })?;

            if let Some(ref mut message) = ollama_response.message {
                message.content = full_content;
                if !full_thinking.is_empty() {
                    message.thinking = Some(full_thinking);
                }
            }

            let openai_response = convert_ollama_to_openai_response(ollama_response, &model);
            let response_body = serde_json::to_string(&openai_response)
                .map_err(|e| AppError::Internal(format!("Failed to serialize response: {}", e)))?;
            (
                openai_response.usage.clone(),
                extract_content_from_response(&openai_response),
                response_body,
            )
        } else if is_anthropic {
            let response_json: Value = serde_json::from_str(normalized_body)
                .map_err(|e| AppError::Internal(format!("Failed to parse response: {}", e)))?;
            let openai_response = convert_anthropic_to_openai_response(&response_json, &model);
            let response_body = serde_json::to_string(&openai_response)
                .map_err(|e| AppError::Internal(format!("Failed to serialize response: {}", e)))?;
            (
                openai_response.usage.clone(),
                extract_content_from_response(&openai_response),
                response_body,
            )
        } else {
            let chat_response: ChatCompletionResponse = match serde_json::from_str(normalized_body)
            {
                Ok(resp) => resp,
                Err(_) => parse_openai_sse_body(normalized_body).ok_or_else(|| {
                    let preview: String = normalized_body.chars().take(200).collect();
                    AppError::Internal(format!(
                        "Failed to parse response: expected JSON object, body_len={}, preview={}",
                        normalized_body.len(),
                        preview
                    ))
                })?,
            };
            (
                chat_response.usage.clone(),
                extract_content_from_response(&chat_response),
                normalized_body.to_string(),
            )
        };

    if let Some(usage) = usage {
        let tokens = usage.total_tokens as i64;

        state.rate_limiter.record_usage(api_key_id, tokens);
        state
            .upstream_rate_limiter
            .record_usage(upstream_id, tokens);

        let _ = state
            .store
            .update_conversation(
                conversation.id,
                ConversationUpdate {
                    provider: Some(provider.clone()),
                    input_tokens: Some(usage.prompt_tokens as i64),
                    output_tokens: Some(usage.completion_tokens as i64),
                    total_tokens: Some(tokens),
                    ended_at: Some(Utc::now()),
                },
            );

        let mut all_messages = messages.clone();
        if let Some(content) = response_content {
            all_messages.push(MessageRecord {
                role: "assistant".to_string(),
                content,
            });
        }

        let proxy_log = ProxyLogRecord {
            id: SqlUuid::new_v4(),
            tenant_id: SqlUuid::from(conversation.tenant_id),
            user_id: SqlUuid::from(conversation.user_id),
            api_key_id: SqlUuid::from(api_key_id),
            conversation_id: Some(conversation.conversation_id.clone()),
            model: conversation.model.clone(),
            routed_model: Some(routed_model),
            provider: provider.clone(),
            input_tokens: usage.prompt_tokens as i64,
            output_tokens: usage.completion_tokens as i64,
            total_tokens: tokens,
            status: "success".to_string(),
            error_message: None,
            log_file: None,
            client_ip: conversation.client_ip.clone(),
            created_at: Utc::now(),
        };
        let content = ProxyLogContent {
            id: proxy_log.id.into(),
            request_body: request_body_str,
            response_body: Some(response_body.clone()),
            messages: all_messages,
        };
        state.store.write_proxy_log(proxy_log, content)?;
    } else {
        let _ = state
            .store
            .update_conversation(
                conversation.id,
                ConversationUpdate {
                    provider: Some(provider.clone()),
                    input_tokens: Some(0),
                    output_tokens: Some(0),
                    total_tokens: Some(0),
                    ended_at: Some(Utc::now()),
                },
            )
            ;

        let mut all_messages = messages.clone();
        if let Some(content) = response_content {
            all_messages.push(MessageRecord {
                role: "assistant".to_string(),
                content,
            });
        }

        let proxy_log = ProxyLogRecord {
            id: SqlUuid::new_v4(),
            tenant_id: SqlUuid::from(conversation.tenant_id),
            user_id: SqlUuid::from(conversation.user_id),
            api_key_id: SqlUuid::from(api_key_id),
            conversation_id: Some(conversation.conversation_id.clone()),
            model: conversation.model.clone(),
            routed_model: Some(routed_model),
            provider: provider.clone(),
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            status: "success".to_string(),
            error_message: Some("No usage data from upstream".to_string()),
            log_file: None,
            client_ip: conversation.client_ip.clone(),
            created_at: Utc::now(),
        };
        let content = ProxyLogContent {
            id: proxy_log.id.into(),
            request_body: request_body_str,
            response_body: Some(response_body.clone()),
            messages: all_messages,
        };
        state.store.write_proxy_log(proxy_log, content)?;
    }

    Ok(Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(response_body.into())
        .map_err(|e| AppError::Internal(format!("Failed to build response: {}", e)))?)
}

fn extract_content_from_response(response: &ChatCompletionResponse) -> Option<String> {
    response
        .choices
        .first()
        .and_then(|c| c.message.as_ref())
        .and_then(|m| m.content.clone())
}

fn normalize_response_body(body: &str) -> &str {
    body.trim_start_matches('\u{feff}').trim()
}

fn parse_openai_sse_body(body: &str) -> Option<ChatCompletionResponse> {
    let mut chunks = Vec::new();
    for line in body.lines() {
        if let Some(data) = line.strip_prefix("data: ") {
            if data.trim() == "[DONE]" {
                continue;
            }
            if let Ok(json) = serde_json::from_str::<Value>(data) {
                chunks.push(json);
            }
        }
    }
    if chunks.is_empty() {
        return None;
    }

    let usage = Usage::from_stream_chunks(&chunks);
    let mut content = String::new();
    let mut reasoning_content = String::new();
    let mut id = "chatcmpl-stream".to_string();
    let mut object = "chat.completion".to_string();
    let mut created = Utc::now().timestamp();
    let mut model = String::new();
    let mut finish_reason: Option<String> = None;

    for chunk in &chunks {
        if let Some(v) = chunk.get("id").and_then(|v| v.as_str()) {
            id = v.to_string();
        }
        if let Some(v) = chunk.get("object").and_then(|v| v.as_str()) {
            object = v.to_string();
        }
        if let Some(v) = chunk.get("created").and_then(|v| v.as_i64()) {
            created = v;
        }
        if let Some(v) = chunk.get("model").and_then(|v| v.as_str()) {
            model = v.to_string();
        }
        if let Some(choice) = chunk
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
        {
            if let Some(delta) = choice.get("delta") {
                if let Some(v) = delta.get("content").and_then(|v| v.as_str()) {
                    content.push_str(v);
                }
                if let Some(v) = delta.get("reasoning_content").and_then(|v| v.as_str()) {
                    reasoning_content.push_str(v);
                }
            }
            if let Some(v) = choice.get("finish_reason").and_then(|v| v.as_str()) {
                finish_reason = Some(v.to_string());
            }
        }
    }

    if model.is_empty() {
        model = "unknown".to_string();
    }

    Some(ChatCompletionResponse {
        id,
        object,
        created,
        model,
        choices: vec![crate::proxy::upstream::Choice {
            index: 0,
            message: Some(crate::proxy::upstream::ResponseMessage {
                role: Some("assistant".to_string()),
                content: if content.is_empty() {
                    None
                } else {
                    Some(content)
                },
                reasoning_content: if reasoning_content.is_empty() {
                    None
                } else {
                    Some(reasoning_content)
                },
            }),
            delta: None,
            finish_reason,
        }],
        usage: Some(usage),
    })
}

fn extract_message_content(msg: &Value) -> Option<String> {
    if let Some(content) = msg.get("content") {
        if let Some(text) = content.as_str() {
            return Some(text.to_string());
        }
        if let Some(items) = content.as_array() {
            let mut text_parts: Vec<String> = Vec::new();
            for item in items {
                if let Some(item_type) = item.get("type").and_then(|t| t.as_str()) {
                    if item_type == "text" {
                        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                            text_parts.push(text.to_string());
                        }
                    } else if item_type == "image_url" {
                        text_parts.push("[image]".to_string());
                    } else if item_type == "input_audio" || item_type == "input_image" {
                        text_parts.push(format!("[{}]", item_type));
                    }
                }
            }
            if !text_parts.is_empty() {
                return Some(text_parts.join(" "));
            }
        }
        if content.is_object() {
            return Some(content.to_string());
        }
    }
    None
}

fn is_multimodal_request(body: &Option<Value>) -> bool {
    let Some(payload) = body else {
        return false;
    };
    let Some(messages) = payload.get("messages").and_then(|v| v.as_array()) else {
        return false;
    };

    for message in messages {
        let Some(content) = message.get("content") else {
            continue;
        };
        if let Some(items) = content.as_array() {
            for item in items {
                if let Some(kind) = item.get("type").and_then(|v| v.as_str()) {
                    let kind = kind.to_ascii_lowercase();
                    if kind != "text" {
                        return true;
                    }
                }
                if item.get("image_url").is_some()
                    || item.get("input_image").is_some()
                    || item.get("audio_url").is_some()
                    || item.get("input_audio").is_some()
                    || item.get("video_url").is_some()
                    || item.get("input_video").is_some()
                {
                    return true;
                }
            }
        }
    }
    false
}

async fn handle_stream_response(
    state: ProxyState,
    response: reqwest::Response,
    conversation: ConversationRecord,
    api_key_id: Uuid,
    upstream_id: Uuid,
    is_ollama: bool,
    is_anthropic: bool,
    model: String,
    routed_model: String,
    provider: String,
    request_body_str: Option<String>,
    messages: Vec<MessageRecord>,
) -> AppResult<Response> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, axum::Error>>(100);

    let store = state.store.clone();
    let rate_limiter = state.rate_limiter.clone();
    let upstream_rate_limiter = state.upstream_rate_limiter.clone();
    let conversation_id = conversation.id;
    let tenant_id = conversation.tenant_id;
    let user_id = conversation.user_id;
    let client_ip = conversation.client_ip.clone();
    let model_clone = model.clone();
    let routed_model_clone = routed_model.clone();
    let provider_clone = provider.clone();

    let chat_id = format!("chatcmpl-{}", Uuid::new_v4().simple());

    tokio::spawn(async move {
        let mut stream = response.bytes_stream();
        let mut total_input_tokens = 0i64;
        let mut total_output_tokens = 0i64;
        let mut chunks: Vec<Value> = Vec::new();
        let mut buffer = String::new();
        let mut sse_event_name: Option<String> = None;
        let mut sse_data_lines: Vec<String> = Vec::new();
        let mut full_response_content = String::new();
        let mut full_reasoning_content = String::new();
        let mut stream_error: Option<String> = None;

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);

                    if is_ollama {
                        buffer.push_str(&text);

                        while let Some(newline_pos) = buffer.find('\n') {
                            let line = buffer[..newline_pos].to_string();
                            buffer = buffer[newline_pos + 1..].to_string();

                            if line.is_empty() {
                                continue;
                            }

                            if let Ok(ollama_chunk) =
                                serde_json::from_str::<OllamaStreamChunk>(&line)
                            {
                                if ollama_chunk.done {
                                    if let (Some(prompt_tokens), Some(completion_tokens)) =
                                        (ollama_chunk.prompt_eval_count, ollama_chunk.eval_count)
                                    {
                                        total_input_tokens = prompt_tokens;
                                        total_output_tokens = completion_tokens;
                                    }
                                }

                                if let Some(ref message) = ollama_chunk.message {
                                    full_response_content.push_str(&message.content);
                                    if let Some(ref thinking) = message.thinking {
                                        full_reasoning_content.push_str(thinking);
                                    }
                                }

                                if let Some(openai_chunk) = convert_ollama_stream_to_openai(
                                    ollama_chunk,
                                    &model_clone,
                                    &chat_id,
                                ) {
                                    chunks.push(openai_chunk.clone());
                                    let _ = tx
                                        .send(Ok(Event::default().data(openai_chunk.to_string())))
                                        .await;
                                }
                            }
                        }
                    } else if is_anthropic {
                        buffer.push_str(&text);

                        while let Some(newline_pos) = buffer.find('\n') {
                            let mut line = buffer[..newline_pos].to_string();
                            buffer = buffer[newline_pos + 1..].to_string();
                            if line.ends_with('\r') {
                                line.pop();
                            }

                            if let Some(event_name) = line.strip_prefix("event: ") {
                                sse_event_name = Some(event_name.to_string());
                                continue;
                            }
                            if let Some(data_line) = line.strip_prefix("data: ") {
                                sse_data_lines.push(data_line.to_string());
                                continue;
                            }
                            if !line.is_empty() {
                                continue;
                            }
                            if sse_data_lines.is_empty() {
                                sse_event_name = None;
                                continue;
                            }

                            let data = sse_data_lines.join("\n");
                            if data == "[DONE]" {
                                let _ = tx.send(Ok(Event::default().data("[DONE]"))).await;
                            } else if let Ok(json) = serde_json::from_str::<Value>(&data) {
                                if let Some(event_name) = sse_event_name.as_deref() {
                                    if event_name == "message_start" {
                                        if let Some(input_tokens) = json
                                            .get("message")
                                            .and_then(|m| m.get("usage"))
                                            .and_then(|u| u.get("input_tokens"))
                                            .and_then(|v| v.as_i64())
                                        {
                                            total_input_tokens = input_tokens;
                                        }
                                    } else if event_name == "message_delta" {
                                        if let Some(output_tokens) = json
                                            .get("usage")
                                            .and_then(|u| u.get("output_tokens"))
                                            .and_then(|v| v.as_i64())
                                        {
                                            total_output_tokens = output_tokens;
                                        }
                                    }
                                }
                                if let Some(text_delta) = json
                                    .get("delta")
                                    .and_then(|d| d.get("text"))
                                    .and_then(|v| v.as_str())
                                {
                                    full_response_content.push_str(text_delta);
                                }
                                if let Some(thinking_delta) = json
                                    .get("delta")
                                    .and_then(|d| d.get("thinking"))
                                    .and_then(|v| v.as_str())
                                {
                                    full_reasoning_content.push_str(thinking_delta);
                                }
                                if let Some(text_start) = json
                                    .get("content_block")
                                    .and_then(|d| d.get("text"))
                                    .and_then(|v| v.as_str())
                                {
                                    full_response_content.push_str(text_start);
                                }
                                if let Some(thinking_start) = json
                                    .get("content_block")
                                    .and_then(|d| d.get("thinking"))
                                    .and_then(|v| v.as_str())
                                {
                                    full_reasoning_content.push_str(thinking_start);
                                }
                                chunks.push(json.clone());

                                if let Some(event_name) = sse_event_name.as_deref() {
                                    let openai_chunks = convert_anthropic_stream_to_openai(
                                        event_name,
                                        &json,
                                        &model_clone,
                                        &chat_id,
                                    );
                                    for openai_chunk in openai_chunks {
                                        if let Some(chunk_data) = openai_chunk {
                                            let _ = tx
                                                .send(Ok(Event::default().data(chunk_data.to_string())))
                                                .await;
                                        }
                                    }
                                }
                            }

                            sse_event_name = None;
                            sse_data_lines.clear();
                        }
                    } else {
                        buffer.push_str(&text);

                        while let Some(newline_pos) = buffer.find('\n') {
                            let mut line = buffer[..newline_pos].to_string();
                            buffer = buffer[newline_pos + 1..].to_string();
                            if line.ends_with('\r') {
                                line.pop();
                            }
                            if line.is_empty() {
                                continue;
                            }
                            if let Some(data) = line.strip_prefix("data: ") {
                                if data == "[DONE]" {
                                    let _ = tx.send(Ok(Event::default().data("[DONE]"))).await;
                                    continue;
                                }

                                if let Ok(json) = serde_json::from_str::<Value>(data) {
                                    if let Some(content) = json
                                        .get("choices")
                                        .and_then(|c| c.get(0))
                                        .and_then(|c| c.get("delta"))
                                        .and_then(|d| d.get("content"))
                                        .and_then(|c| c.as_str())
                                    {
                                        full_response_content.push_str(content);
                                    }
                                    if let Some(reasoning) = json
                                        .get("choices")
                                        .and_then(|c| c.get(0))
                                        .and_then(|c| c.get("delta"))
                                        .and_then(|d| d.get("reasoning_content"))
                                        .and_then(|c| c.as_str())
                                    {
                                        full_reasoning_content.push_str(reasoning);
                                    }
                                    chunks.push(json.clone());
                                    let _ =
                                        tx.send(Ok(Event::default().data(data.to_string()))).await;
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    let err = format!("Stream error: {}", e);
                    tracing::error!("{}", err);
                    stream_error = Some(err);
                    break;
                }
            }
        }

        if is_ollama {
            let _ = tx.send(Ok(Event::default().data("[DONE]"))).await;
        }

        let usage = if is_ollama && total_input_tokens > 0 {
            Usage {
                prompt_tokens: total_input_tokens as i32,
                completion_tokens: total_output_tokens as i32,
                total_tokens: (total_input_tokens + total_output_tokens) as i32,
            }
        } else if is_anthropic && (total_input_tokens > 0 || total_output_tokens > 0) {
            Usage {
                prompt_tokens: total_input_tokens as i32,
                completion_tokens: total_output_tokens as i32,
                total_tokens: (total_input_tokens + total_output_tokens) as i32,
            }
        } else {
            Usage::from_stream_chunks(&chunks)
        };

        total_input_tokens = usage.prompt_tokens as i64;
        total_output_tokens = usage.completion_tokens as i64;
        let total_tokens = total_input_tokens + total_output_tokens;
        let failed_reason = if let Some(err) = stream_error {
            Some(err)
        } else if chunks.is_empty() && total_tokens == 0 && full_response_content.is_empty() {
            Some("Empty stream response".to_string())
        } else {
            None
        };
        let status = if failed_reason.is_some() {
            "failed"
        } else {
            "success"
        }
        .to_string();

        if failed_reason.is_none() {
            rate_limiter.record_usage(api_key_id, total_tokens);
            upstream_rate_limiter.record_usage(upstream_id, total_tokens);
        }

        let _ = store
            .update_conversation(
                conversation_id,
                ConversationUpdate {
                    provider: Some(provider_clone.clone()),
                    input_tokens: Some(total_input_tokens),
                    output_tokens: Some(total_output_tokens),
                    total_tokens: Some(total_tokens),
                    ended_at: Some(Utc::now()),
                },
            );

        let mut all_messages = messages.clone();
        if !full_response_content.is_empty() {
            all_messages.push(MessageRecord {
                role: "assistant".to_string(),
                content: full_response_content.clone(),
            });
        }

        let response_body = if chunks.is_empty() {
            None
        } else if is_anthropic {
            Some(Value::Array(chunks.clone()).to_string())
        } else {
            let mut message = serde_json::json!({
                "role": "assistant",
                "content": full_response_content
            });
            if !full_reasoning_content.is_empty() {
                message["reasoning_content"] = serde_json::json!(full_reasoning_content);
            }
            let full_response = serde_json::json!({
                "id": chat_id,
                "object": "chat.completion",
                "created": chrono::Utc::now().timestamp(),
                "model": model_clone,
                "choices": [{
                    "index": 0,
                    "message": message,
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": total_input_tokens,
                    "completion_tokens": total_output_tokens,
                    "total_tokens": total_tokens
                }
            });
            Some(full_response.to_string())
        };

        // 写入合并的代理日志（包含会话和日志信息）
        let proxy_log = ProxyLogRecord {
            id: SqlUuid::new_v4(),
            tenant_id: SqlUuid::from(tenant_id),
            user_id: SqlUuid::from(user_id),
            api_key_id: SqlUuid::from(api_key_id),
            conversation_id: Some(conversation_id.to_string()),
            model: model_clone.clone(),
            routed_model: Some(routed_model_clone.clone()),
            provider,
            input_tokens: total_input_tokens,
            output_tokens: total_output_tokens,
            total_tokens,
            status,
            error_message: failed_reason,
            log_file: None,
            client_ip,
            created_at: Utc::now(),
        };
        let content = ProxyLogContent {
            id: proxy_log.id.into(),
            request_body: request_body_str,
            response_body,
            messages: all_messages,
        };
        let _ = store.write_proxy_log(proxy_log, content);
    });

    let stream = ReceiverStream::new(rx);

    Ok(Sse::new(stream).into_response())
}

fn write_failed_proxy_log(
    store: &Arc<StoreManager>,
    conversation: &ConversationRecord,
    api_key_id: Uuid,
    routed_model: Option<String>,
    provider: String,
    request_body: Option<String>,
    messages: Vec<MessageRecord>,
    error_message: String,
    response_body: Option<String>,
) -> Result<(), AppError> {
    let proxy_log = ProxyLogRecord {
        id: SqlUuid::new_v4(),
        tenant_id: SqlUuid::from(conversation.tenant_id),
        user_id: SqlUuid::from(conversation.user_id),
        api_key_id: SqlUuid::from(api_key_id),
        conversation_id: Some(conversation.conversation_id.clone()),
        model: conversation.model.clone(),
        routed_model,
        provider,
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        status: "failed".to_string(),
        error_message: Some(error_message),
        log_file: None,
        client_ip: conversation.client_ip.clone(),
        created_at: Utc::now(),
    };
    let content = ProxyLogContent {
        id: proxy_log.id.into(),
        request_body,
        response_body,
        messages,
    };
    store.write_proxy_log(proxy_log, content)
}

async fn log_upstream_error(
    error_logger: &Option<UpstreamErrorLogger>,
    upstream_url: &str,
    model_name: &str,
    error_message: &str,
    status_code: Option<u16>,
) {
    if let Some(logger) = error_logger {
        logger.log(upstream_url, model_name, error_message, status_code).await;
    }
}
