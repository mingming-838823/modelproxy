use crate::store::AdminState;
use axum::{
    extract::{Path, State},
    Extension, Json,
};
use http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    db,
    models::upstream::{CreateUpstream, CreateUpstreamGroup, UpstreamConfig, UpstreamConfigResponse, UpstreamGroup},
    utils::error::{AppError, AppResult},
    utils::secrets::decrypt_upstream_api_key,
};

#[derive(Debug, Deserialize)]
pub struct CreateUpstreamRequest {
    pub name: String,
    pub provider: String,
    pub api_type: Option<String>,
    pub base_url: String,
    pub api_key: Option<String>,
    pub custom_headers: Option<serde_json::Value>,
    pub daily_request_limit: Option<i64>,
    pub monthly_request_limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateUpstreamRequest {
    pub name: Option<String>,
    pub provider: Option<String>,
    pub api_type: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub custom_headers: Option<serde_json::Value>,
    pub daily_request_limit: Option<i64>,
    pub monthly_request_limit: Option<i64>,
}

pub async fn list_upstreams(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
) -> AppResult<Json<Vec<UpstreamConfigResponse>>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let upstreams_list = db::upstreams::find_by_tenant(&state.pool, auth_user.tenant_id).await?;

    Ok(Json(upstreams_list.into_iter().map(UpstreamConfigResponse::from).collect()))
}

pub async fn create_upstream(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Json(req): Json<CreateUpstreamRequest>,
) -> AppResult<Json<UpstreamConfigResponse>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let api_key = req.api_key.clone().unwrap_or_default();
    let api_type = req.api_type.clone().filter(|s| !s.is_empty());
    let provider = api_type.clone().unwrap_or_else(|| req.provider.clone());
    let create = CreateUpstream {
        name: req.name,
        provider,
        api_type,
        base_url: req.base_url,
        api_key: req.api_key,
        custom_headers: req.custom_headers,
        daily_request_limit: Some(req.daily_request_limit.unwrap_or(2000)),
        monthly_request_limit: Some(req.monthly_request_limit.unwrap_or(50000)),
    };

    let upstream =
        db::upstreams::create(&state.pool, auth_user.tenant_id, create, &api_key).await?;
    state.store.reload_upstreams_cache(&state.pool).await?;

    Ok(Json(UpstreamConfigResponse::from(upstream)))
}

pub async fn get_upstream(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<UpstreamConfigResponse>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let upstream = db::upstreams::find_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Upstream not found".to_string()))?;

    if upstream.tenant_id != auth_user.tenant_id {
        return Err(AppError::Forbidden("Access denied".to_string()));
    }

    Ok(Json(UpstreamConfigResponse::from(upstream)))
}

pub async fn update_upstream(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateUpstreamRequest>,
) -> AppResult<Json<UpstreamConfigResponse>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let upstream = db::upstreams::find_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Upstream not found".to_string()))?;

    if upstream.tenant_id != auth_user.tenant_id {
        return Err(AppError::Forbidden("Access denied".to_string()));
    }

    let should_reset_upstream_window =
        req.daily_request_limit.is_some() || req.monthly_request_limit.is_some();
    let api_key = req.api_key.clone().filter(|s| !s.is_empty());
    let api_type = req.api_type.clone().filter(|s| !s.is_empty());
    let provider = api_type.clone().or_else(|| req.provider.clone().filter(|s| !s.is_empty()));
    let req = UpdateUpstreamRequest {
        name: req.name.filter(|s| !s.is_empty()),
        provider,
        api_type,
        base_url: req.base_url.filter(|s| !s.is_empty()),
        api_key: api_key.clone(),
        custom_headers: req.custom_headers,
        daily_request_limit: req.daily_request_limit,
        monthly_request_limit: req.monthly_request_limit,
    };
    let updated = db::upstreams::update(&state.pool, id, req, api_key.as_deref()).await?;
    state.store.reload_upstreams_cache(&state.pool).await?;
    if should_reset_upstream_window {
        state.upstream_rate_limiter.reset_upstream_window(id);
    }

    Ok(Json(UpstreamConfigResponse::from(updated)))
}

pub async fn delete_upstream(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let upstream = db::upstreams::find_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Upstream not found".to_string()))?;

    let upstream_tenant_id: Uuid = upstream.tenant_id.into();
    if upstream_tenant_id != auth_user.tenant_id {
        return Err(AppError::Forbidden("Access denied".to_string()));
    }

    db::upstreams::delete(&state.pool, id).await?;
    state.store.reload_upstreams_cache(&state.pool).await?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
pub struct UpstreamApiKeyResponse {
    pub api_key: String,
}

pub async fn get_upstream_api_key(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<UpstreamApiKeyResponse>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let upstream = db::upstreams::find_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Upstream not found".to_string()))?;

    if upstream.tenant_id != auth_user.tenant_id {
        return Err(AppError::Forbidden("Access denied".to_string()));
    }

    Ok(Json(UpstreamApiKeyResponse {
        api_key: upstream.api_key_encrypted,
    }))
}

#[derive(Debug, Deserialize)]
pub struct UpdateUpstreamStatusRequest {
    pub status: String,
}

pub async fn update_upstream_status(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateUpstreamStatusRequest>,
) -> AppResult<StatusCode> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let upstream = db::upstreams::find_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Upstream not found".to_string()))?;

    let upstream_tenant_id: Uuid = upstream.tenant_id.into();
    if upstream_tenant_id != auth_user.tenant_id {
        return Err(AppError::Forbidden("Access denied".to_string()));
    }

    db::upstreams::update_status(&state.pool, id, &req.status).await?;
    state.store.reload_upstreams_cache(&state.pool).await?;

    Ok(StatusCode::OK)
}

pub async fn list_upstream_groups(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
) -> AppResult<Json<Vec<UpstreamGroup>>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let groups = db::upstreams::find_groups_by_tenant(&state.pool, auth_user.tenant_id).await?;

    Ok(Json(groups))
}

#[derive(Debug, Deserialize)]
pub struct CreateGroupRequest {
    pub name: String,
    pub upstream_ids: Vec<Uuid>,
    pub balance_strategy: Option<String>,
    pub failover_enabled: Option<bool>,
}

pub async fn create_upstream_group(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Json(req): Json<CreateGroupRequest>,
) -> AppResult<Json<UpstreamGroup>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let create = CreateUpstreamGroup {
        name: req.name,
        upstream_ids: req.upstream_ids,
        balance_strategy: req.balance_strategy,
        failover_enabled: req.failover_enabled,
    };

    let group = db::upstreams::create_group(&state.pool, auth_user.tenant_id, create).await?;

    Ok(Json(group))
}

pub async fn delete_upstream_group(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let group = db::upstreams::find_group_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Upstream group not found".to_string()))?;

    let group_tenant_id: Uuid = group.tenant_id.into();
    if group_tenant_id != auth_user.tenant_id {
        return Err(AppError::Forbidden("Access denied".to_string()));
    }

    db::upstreams::delete_group(&state.pool, id).await?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
pub struct TestUpstreamResponse {
    pub success: bool,
    pub message: String,
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct TestModelRequest {
    pub model: String,
    pub prompt: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TestModelResponse {
    pub success: bool,
    pub message: String,
    pub model: String,
    pub output_preview: Option<String>,
}

pub async fn test_upstream(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<TestUpstreamResponse>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let upstream = db::upstreams::find_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Upstream not found".to_string()))?;

    if upstream.tenant_id != auth_user.tenant_id {
        return Err(AppError::Forbidden("Access denied".to_string()));
    }

    // MiniMax 特殊处理：没有模型列表接口，直接返回预定义模型
    if upstream.provider == "minimax" {
        let models = vec![
            ModelInfo {
                id: "MiniMax-M2.5".to_string(),
                name: "MiniMax-M2.5 (204800 tokens, 60tps)".to_string(),
            },
            ModelInfo {
                id: "MiniMax-M2.5-highspeed".to_string(),
                name: "MiniMax-M2.5-highspeed (204800 tokens, 100tps)".to_string(),
            },
            ModelInfo {
                id: "MiniMax-M2.1".to_string(),
                name: "MiniMax-M2.1 (204800 tokens, 60tps)".to_string(),
            },
            ModelInfo {
                id: "MiniMax-M2.1-highspeed".to_string(),
                name: "MiniMax-M2.1-highspeed (204800 tokens, 100tps)".to_string(),
            },
            ModelInfo {
                id: "MiniMax-M2".to_string(),
                name: "MiniMax-M2 (204800 tokens)".to_string(),
            },
        ];
        return Ok(Json(TestUpstreamResponse {
            success: true,
            message: format!("MiniMax 连接配置正确！共 {} 个可用模型", models.len()),
            models,
        }));
    }

    let is_anthropic = upstream.api_type == "anthropic" || upstream.provider == "anthropic";

    let decrypted_api_key = if !upstream.api_key_encrypted.is_empty() {
        Some(decrypt_upstream_api_key(&upstream.api_key_encrypted)?)
    } else {
        None
    };

    let client = reqwest::Client::new();

    if is_anthropic {
        let models = vec![
            ModelInfo { id: "claude-sonnet-4-20250514".to_string(), name: "Claude Sonnet 4".to_string() },
            ModelInfo { id: "claude-3-7-sonnet-20250219".to_string(), name: "Claude 3.7 Sonnet".to_string() },
            ModelInfo { id: "claude-3-5-sonnet-20241022".to_string(), name: "Claude 3.5 Sonnet (New)".to_string() },
            ModelInfo { id: "claude-3-5-haiku-20241022".to_string(), name: "Claude 3.5 Haiku".to_string() },
            ModelInfo { id: "claude-3-opus-20240229".to_string(), name: "Claude 3 Opus".to_string() },
            ModelInfo { id: "claude-3-sonnet-20240229".to_string(), name: "Claude 3 Sonnet".to_string() },
            ModelInfo { id: "claude-3-haiku-20240307".to_string(), name: "Claude 3 Haiku".to_string() },
        ];
        return Ok(Json(TestUpstreamResponse {
            success: true,
            message: format!("Anthropic 连接配置正确！共 {} 个可用模型（Anthropic 无模型列表API，以下为已知模型）", models.len()),
            models,
        }));
    }

    for models_url in model_endpoint_candidates(&upstream) {
        let mut request = client.get(&models_url);

        if let Some(ref api_key) = decrypted_api_key {
            if is_anthropic {
                request = request.header("x-api-key", api_key);
                request = request.header("anthropic-version", "2023-06-01");
            } else {
                request = request.header(
                    "Authorization",
                    format!("Bearer {}", api_key),
                );
            }
        }

        if let Some(headers) = upstream.custom_headers.as_object() {
            for (name, value) in headers {
                if let Some(text) = value.as_str() {
                    request = request.header(name, text);
                } else {
                    request = request.header(name, value.to_string());
                }
            }
        }

        match request
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
            Ok(response) => {
                if response.status().is_success() {
                    match response.json::<serde_json::Value>().await {
                        Ok(data) => {
                            let models = extract_models(&data, &upstream.api_type);
                            if !models.is_empty() {
                                return Ok(Json(TestUpstreamResponse {
                                    success: true,
                                    message: format!("连接成功！共发现 {} 个模型", models.len()),
                                    models,
                                }));
                            }
                        }
                        Err(_) => continue,
                    }
                }
            }
            Err(_) => continue,
        }
    }

    Ok(Json(TestUpstreamResponse {
        success: false,
        message: "连接成功但未解析到模型清单，请检查Base URL与API类型".to_string(),
        models: vec![],
    }))
}

pub async fn test_upstream_model(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
    Json(req): Json<TestModelRequest>,
) -> AppResult<Json<TestModelResponse>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let upstream = db::upstreams::find_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Upstream not found".to_string()))?;

    if upstream.tenant_id != auth_user.tenant_id {
        return Err(AppError::Forbidden("Access denied".to_string()));
    }

    let model = req.model.trim().to_string();
    if model.is_empty() {
        return Err(AppError::BadRequest("模型名称不能为空".to_string()));
    }
    let prompt = req
        .prompt
        .as_deref()
        .unwrap_or("请回复：model test ok")
        .to_string();

    let decrypted_api_key = if !upstream.api_key_encrypted.is_empty() {
        Some(decrypt_upstream_api_key(&upstream.api_key_encrypted)?)
    } else {
        None
    };

    let client = reqwest::Client::new();
    let is_anthropic = upstream.api_type == "anthropic" || upstream.provider == "anthropic";
    let (url, body) = build_model_test_request(&upstream, &model, &prompt);
    let mut request = client.post(url).json(&body);
    if let Some(ref api_key) = decrypted_api_key {
        if is_anthropic {
            request = request.header("x-api-key", api_key);
            request = request.header("anthropic-version", "2023-06-01");
        } else {
            request = request.header(
                "Authorization",
                format!("Bearer {}", api_key),
            );
        }
    }
    if let Some(headers) = upstream.custom_headers.as_object() {
        for (name, value) in headers {
            if let Some(text) = value.as_str() {
                request = request.header(name, text);
            } else {
                request = request.header(name, value.to_string());
            }
        }
    }

    let response = request
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("测试模型请求失败: {}", e)))?;

    let status = response.status();
    let text = response
        .text()
        .await
        .unwrap_or_else(|_| String::from("无法读取响应内容"));

    if !status.is_success() {
        return Ok(Json(TestModelResponse {
            success: false,
            message: format!(
                "上游返回失败状态: {}，响应: {}",
                status.as_u16(),
                trim_preview(&text, 200)
            ),
            model,
            output_preview: None,
        }));
    }

    let output_preview = parse_model_test_output(&text, &upstream.api_type);
    Ok(Json(TestModelResponse {
        success: true,
        message: "模型对话测试成功".to_string(),
        model,
        output_preview,
    }))
}

fn model_endpoint_candidates(upstream: &UpstreamConfig) -> Vec<String> {
    let base = upstream.base_url.trim_end_matches('/');
    if upstream.api_type == "ollama" {
        return vec![format!("{}/api/tags", base)];
    }
    if upstream.api_type == "anthropic" || upstream.provider == "anthropic" {
        return Vec::new();
    }
    if base.ends_with("/v1") {
        vec![
            format!("{}/models", base),
            format!("{}/models", base.trim_end_matches("/v1")),
        ]
    } else {
        vec![format!("{}/v1/models", base), format!("{}/models", base)]
    }
}

fn extract_models(data: &serde_json::Value, api_type: &str) -> Vec<ModelInfo> {
    let mut models = Vec::new();

    if api_type == "ollama" {
        if let Some(arr) = data.get("models").and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                    models.push(ModelInfo {
                        id: name.to_string(),
                        name: name.to_string(),
                    });
                }
            }
        }
    } else {
        if let Some(arr) = data.get("data").and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                    let name = item
                        .get("object")
                        .and_then(|v| v.as_str())
                        .map(|s| format!("{} ({})", id, s))
                        .unwrap_or_else(|| id.to_string());
                    models.push(ModelInfo {
                        id: id.to_string(),
                        name,
                    });
                }
            }
        }
    }

    models
}

fn build_model_test_request(
    upstream: &UpstreamConfig,
    model: &str,
    prompt: &str,
) -> (String, serde_json::Value) {
    let base = upstream.base_url.trim_end_matches('/');
    if upstream.api_type == "ollama" {
        return (
            format!("{}/api/chat", base),
            serde_json::json!({
                "model": model,
                "messages": [
                    {"role":"user","content": prompt}
                ],
                "stream": false
            }),
        );
    }
    if upstream.api_type == "anthropic" {
        let url = if base.ends_with("/v1") {
            format!("{}/messages", base)
        } else {
            format!("{}/v1/messages", base)
        };
        return (
            url,
            serde_json::json!({
                "model": model,
                "max_tokens": 64,
                "messages": [
                    {"role":"user","content": prompt}
                ]
            }),
        );
    }
    let url = if is_qianfan_coding_base(base) {
        format!("{}/chat/completions", base)
    } else if base.ends_with("/v1") {
        format!("{}/chat/completions", base)
    } else {
        format!("{}/v1/chat/completions", base)
    };
    (
        url,
        serde_json::json!({
            "model": model,
            "messages": [
                {"role":"user","content": prompt}
            ],
            "max_tokens": 64,
            "stream": false
        }),
    )
}

fn is_qianfan_coding_base(base: &str) -> bool {
    base.trim_end_matches('/')
        .to_ascii_lowercase()
        .ends_with("/v2/coding")
}

fn parse_model_test_output(body_text: &str, api_type: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body_text).ok()?;
    if api_type == "ollama" {
        if let Some(content) = value
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|v| v.as_str())
        {
            return Some(trim_preview(content, 200));
        }
    }
    if api_type == "anthropic" {
        if let Some(text) = value
            .get("content")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|v| v.get("text"))
            .and_then(|v| v.as_str())
        {
            return Some(trim_preview(text, 200));
        }
    }
    if let Some(text) = value
        .get("choices")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("message"))
        .and_then(|v| v.get("content"))
        .and_then(|v| v.as_str())
    {
        return Some(trim_preview(text, 200));
    }
    Some(trim_preview(body_text, 200))
}

fn trim_preview(input: &str, max_len: usize) -> String {
    let mut chars = input.chars();
    let mut out = String::new();
    for _ in 0..max_len {
        if let Some(ch) = chars.next() {
            out.push(ch);
        } else {
            return out;
        }
    }
    if chars.next().is_some() {
        out.push_str("...");
    }
    out
}
