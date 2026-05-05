use axum::{
    extract::{Path, Query, State},
    Extension, Json,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    db::model_visibility,
    models::upstream::{
        ConditionalAliasConfig, ModelWithVisibility, UpdateModelVisibilityRequest,
        UpsertConditionalAliasRequest, UpstreamConfig,
    },
    proxy::upstream::UpstreamClient,
    store::ModelVisibilityCache,
    utils::error::{AppError, AppResult},
    utils::secrets::decrypt_upstream_api_key,
};

// ModelState 包含 pool、client 和 store
type ModelState = (
    crate::db::DbPool,
    Arc<UpstreamClient>,
    Arc<crate::store::StoreManager>,
);

#[derive(Debug, Deserialize)]
pub struct FetchModelsQuery {
    upstream_id: Option<Uuid>,
}

#[derive(Debug, serde::Serialize)]
pub struct UpstreamWithModels {
    pub upstream_id: Uuid,
    pub upstream_name: String,
    pub base_url: String,
    pub provider: String,
    pub models: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct UpdateConditionalAliasVisibilityRequest {
    pub all_users_visible: bool,
    pub user_ids: Vec<Uuid>,
}

pub async fn fetch_upstream_models(
    State((pool, client, _store)): State<ModelState>,
    Extension(auth_user): Extension<AuthUser>,
    Query(query): Query<FetchModelsQuery>,
) -> AppResult<Json<Vec<UpstreamWithModels>>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let upstreams: Vec<UpstreamConfig> = if let Some(id) = query.upstream_id {
        sqlx::query_as::<_, UpstreamConfig>(
            "SELECT * FROM upstream_configs WHERE id = ? AND tenant_id = ?",
        )
        .bind(id.to_string())
        .bind(auth_user.tenant_id.to_string())
        .fetch_all(&pool)
        .await?
    } else {
        sqlx::query_as::<_, UpstreamConfig>(
            "SELECT * FROM upstream_configs WHERE tenant_id = ? AND status = 'active'",
        )
        .bind(auth_user.tenant_id.to_string())
        .fetch_all(&pool)
        .await?
    };

    let mut result = Vec::new();

    for upstream in upstreams {
        let models = match fetch_models_from_upstream(&client, &upstream).await {
            Ok(m) => m,
            Err(_) => Vec::new(),
        };

        result.push(UpstreamWithModels {
            upstream_id: upstream.id.into(),
            upstream_name: upstream.name,
            base_url: upstream.base_url,
            provider: upstream.provider,
            models,
        });
    }

    Ok(Json(result))
}

async fn fetch_models_from_upstream(
    client: &UpstreamClient,
    upstream: &UpstreamConfig,
) -> AppResult<Vec<String>> {
    match upstream.provider.as_str() {
        "ollama" => {
            let url = format!("{}/api/tags", upstream.base_url.trim_end_matches('/'));

            let response = client
                .http_client()
                .get(&url)
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
                .map_err(|e| AppError::Internal(format!("Failed to fetch models: {}", e)))?;

            #[derive(serde::Deserialize)]
            struct OllamaModelsResponse {
                models: Vec<OllamaModel>,
            }

            #[derive(serde::Deserialize)]
            struct OllamaModel {
                name: String,
            }

            let body = response
                .text()
                .await
                .map_err(|e| AppError::Internal(format!("Failed to read response: {}", e)))?;

            let ollama_resp: OllamaModelsResponse = serde_json::from_str(&body)
                .map_err(|e| AppError::Internal(format!("Failed to parse response: {}", e)))?;

            Ok(ollama_resp.models.into_iter().map(|m| m.name).collect())
        }
        "minimax" => {
            // MiniMax 支持的模型列表 (来自官方文档)
            Ok(vec![
                "MiniMax-M2.5".to_string(),
                "MiniMax-M2.5-highspeed".to_string(),
                "MiniMax-M2.1".to_string(),
                "MiniMax-M2.1-highspeed".to_string(),
                "MiniMax-M2".to_string(),
            ])
        }
        _ => {
            let is_anthropic = upstream.api_type == "anthropic" || upstream.provider == "anthropic";
            if is_anthropic {
                return Ok(vec![
                    "claude-sonnet-4-20250514".to_string(),
                    "claude-3-7-sonnet-20250219".to_string(),
                    "claude-3-5-sonnet-20241022".to_string(),
                    "claude-3-5-haiku-20241022".to_string(),
                    "claude-3-opus-20240229".to_string(),
                    "claude-3-sonnet-20240229".to_string(),
                    "claude-3-haiku-20240307".to_string(),
                ]);
            }
            let decrypted_api_key = if !upstream.api_key_encrypted.is_empty() {
                Some(decrypt_upstream_api_key(&upstream.api_key_encrypted)?)
            } else {
                None
            };
            for url in model_endpoint_candidates(&upstream.base_url) {
                let mut request = client.http_client().get(&url);
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

                let response = match request
                    .timeout(std::time::Duration::from_secs(10))
                    .send()
                    .await
                {
                    Ok(resp) => resp,
                    Err(_) => continue,
                };
                if !response.status().is_success() {
                    continue;
                }
                let data = match response.json::<serde_json::Value>().await {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if let Some(arr) = data.get("data").and_then(|v| v.as_array()) {
                    let models: Vec<String> = arr
                        .iter()
                        .filter_map(|item| item.get("id").and_then(|v| v.as_str()))
                        .map(|s| s.to_string())
                        .collect();
                    if !models.is_empty() {
                        return Ok(models);
                    }
                }
            }
            Ok(Vec::new())
        }
    }
}

fn model_endpoint_candidates(base_url: &str) -> Vec<String> {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1") {
        vec![
            format!("{}/models", base),
            format!("{}/models", base.trim_end_matches("/v1")),
        ]
    } else {
        vec![format!("{}/v1/models", base), format!("{}/models", base)]
    }
}

pub async fn list_models(
    State((pool, _, _)): State<ModelState>,
    Extension(auth_user): Extension<AuthUser>,
) -> AppResult<Json<Vec<ModelWithVisibility>>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let models = model_visibility::get_all_visibility_settings(&pool, auth_user.tenant_id).await?;

    Ok(Json(models))
}

pub async fn set_visibility(
    State((pool, _, store)): State<ModelState>,
    Extension(auth_user): Extension<AuthUser>,
    Path((upstream_id, model_name)): Path<(Uuid, String)>,
    Json(input): Json<UpdateModelVisibilityRequest>,
) -> AppResult<Json<ModelWithVisibility>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let visibility =
        model_visibility::set_model_visibility(&pool, upstream_id, &model_name, input.clone())
            .await?;

    // 更新内存缓存
    let cache = ModelVisibilityCache {
        id: visibility.id.into(),
        upstream_id: visibility.upstream_id.into(),
        model_name: visibility.model_name.clone(),
        model_aliases: visibility
            .model_alias
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect(),
        model_headers: visibility.model_headers,
        all_users_visible: visibility.all_users_visible,
        allowed_users: input.user_ids.clone(),
        retry_count: visibility.retry_count,
        retry_interval_seconds: visibility.retry_interval_seconds,
        retry_backoff_strategy: visibility.retry_backoff_strategy,
        retry_max_interval_seconds: visibility.retry_max_interval_seconds,
        retry_failure_strategy: visibility.retry_failure_strategy,
        retry_fallback_upstream_id: visibility.retry_fallback_upstream_id.and_then(|s| Uuid::parse_str(&s).ok()),
        retry_fallback_model_name: visibility.retry_fallback_model_name,
        created_at: visibility.created_at,
        updated_at: visibility.updated_at,
    };
    store.update_model_visibility_entry((upstream_id, model_name.clone()), cache);

    let all = model_visibility::get_all_visibility_settings(&pool, auth_user.tenant_id).await?;

    let result = all
        .into_iter()
        .find(|m| m.upstream_id == upstream_id && m.model_name == model_name)
        .ok_or_else(|| AppError::NotFound("Model visibility not found".to_string()))?;

    Ok(Json(result))
}

pub async fn get_user_models(
    State((pool, _, _)): State<ModelState>,
    Extension(auth_user): Extension<AuthUser>,
) -> AppResult<Json<Vec<ModelWithVisibility>>> {
    let mut models =
        model_visibility::get_visible_models(&pool, auth_user.tenant_id, auth_user.user_id).await?;
    for m in &mut models {
        m.original_model_name = m.model_name.clone();
        if let Some(alias) = m.model_aliases.first().cloned() {
            m.model_name = alias;
        }
    }

    let alias_configs =
        model_visibility::list_conditional_aliases(&pool, auth_user.tenant_id).await?;
    let upstream_rows: Vec<UpstreamConfig> =
        sqlx::query_as::<_, UpstreamConfig>("SELECT * FROM upstream_configs WHERE tenant_id = ?")
            .bind(auth_user.tenant_id.to_string())
            .fetch_all(&pool)
            .await?;
    let upstream_map: HashMap<Uuid, UpstreamConfig> =
        upstream_rows.into_iter().map(|u| (u.id.into(), u)).collect();

    for cfg in alias_configs {
        if !cfg.all_users_visible && !cfg.user_ids.contains(&auth_user.user_id) {
            continue;
        }
        let fallback_upstream = upstream_map.get(&cfg.fallback.upstream_id);
        let upstream_name = fallback_upstream
            .map(|u| format!("智能路由 / {}", u.name))
            .unwrap_or_else(|| "智能路由".to_string());
        let provider = fallback_upstream
            .map(|u| u.provider.clone())
            .unwrap_or_else(|| "openai".to_string());
        models.push(ModelWithVisibility {
            upstream_id: cfg.fallback.upstream_id,
            upstream_name,
            model_name: cfg.alias.clone(),
            original_model_name: cfg.alias.clone(),
            model_alias: Some(cfg.alias.clone()),
            model_aliases: vec![cfg.alias.clone()],
            model_headers: serde_json::json!({}),
            provider,
            all_users_visible: cfg.all_users_visible,
            allowed_users: cfg.user_ids.clone(),
            retry_count: 0,
            retry_interval_seconds: 0,
            retry_backoff_strategy: "fixed".to_string(),
            retry_max_interval_seconds: 0,
            retry_failure_strategy: "error".to_string(),
            retry_fallback_upstream_id: None,
            retry_fallback_model_name: None,
        });
    }

    Ok(Json(models))
}

pub async fn list_conditional_aliases(
    State((pool, _, _)): State<ModelState>,
    Extension(auth_user): Extension<AuthUser>,
) -> AppResult<Json<Vec<ConditionalAliasConfig>>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }
    let aliases = model_visibility::list_conditional_aliases(&pool, auth_user.tenant_id).await?;
    Ok(Json(aliases))
}

pub async fn set_conditional_alias(
    State((pool, _, store)): State<ModelState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(alias): Path<String>,
    Json(input): Json<UpsertConditionalAliasRequest>,
) -> AppResult<Json<ConditionalAliasConfig>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let config =
        model_visibility::upsert_conditional_alias(&pool, auth_user.tenant_id, &alias, input)
            .await?;
    store.reload_conditional_alias_routes(&pool).await?;
    Ok(Json(config))
}

pub async fn set_conditional_alias_visibility(
    State((pool, _, store)): State<ModelState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(alias): Path<String>,
    Json(input): Json<UpdateConditionalAliasVisibilityRequest>,
) -> AppResult<Json<ConditionalAliasConfig>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }
    let config = model_visibility::update_conditional_alias_visibility(
        &pool,
        auth_user.tenant_id,
        &alias,
        input.all_users_visible,
        input.user_ids,
    )
    .await?;
    store.reload_conditional_alias_routes(&pool).await?;
    Ok(Json(config))
}

pub async fn delete_conditional_alias(
    State((pool, _, store)): State<ModelState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(alias): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }
    model_visibility::delete_conditional_alias(&pool, auth_user.tenant_id, &alias).await?;
    store.reload_conditional_alias_routes(&pool).await?;
    Ok(Json(serde_json::json!({
        "success": true,
        "alias": alias
    })))
}

/// 刷新模型可见性缓存
#[axum::debug_handler]
pub async fn refresh_cache(
    State((pool, _, store)): State<ModelState>,
    Extension(auth_user): Extension<AuthUser>,
) -> AppResult<Json<serde_json::Value>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    store.reload_model_visibility(&pool).await?;
    store.reload_conditional_alias_routes(&pool).await?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Model visibility cache refreshed"
    })))
}
