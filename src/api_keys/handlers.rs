use axum::{
    extract::{Path, Query, State},
    Extension, Json,
};
use http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    db::{self, api_keys},
    models::api_key::{ApiKey, ApiKeyWithSecret, ApiKeyWithUser, CreateApiKey, UpdateApiKey},
    store::AdminState,
    utils::error::{AppError, AppResult},
};

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub page: Option<i32>,
    pub page_size: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct ApiKeyList {
    pub items: Vec<ApiKey>,
    pub total: i64,
}

#[derive(Debug, Serialize)]
pub struct ApiKeyListWithUser {
    pub items: Vec<ApiKeyWithUser>,
    pub total: i64,
}

pub async fn list_my_keys(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<ApiKeyList>> {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).max(1);

    let items = api_keys::find_by_user(&state.pool, auth_user.user_id, page, page_size).await?;

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_keys WHERE user_id = ?")
        .bind(auth_user.user_id.to_string())
        .fetch_optional(&state.pool)
        .await?
        .unwrap_or(0);

    Ok(Json(ApiKeyList { items, total }))
}

pub async fn list_all_keys(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<ApiKeyListWithUser>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).max(1);

    let items =
        api_keys::find_by_tenant_with_user(&state.pool, auth_user.tenant_id, page, page_size)
            .await?;

    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM api_keys ak JOIN users u ON ak.user_id = u.id WHERE u.tenant_id = ?",
    )
    .bind(auth_user.tenant_id.to_string())
    .fetch_optional(&state.pool)
    .await?
    .unwrap_or(0);

    Ok(Json(ApiKeyListWithUser { items, total }))
}

pub async fn create_key(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Json(input): Json<CreateApiKey>,
) -> AppResult<Json<ApiKeyWithSecret>> {
    let user_id = if auth_user.is_admin() {
        let target_user = db::users::find_by_id(&state.pool, input.user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Target user not found".to_string()))?;
        let target_tenant_id: Uuid = target_user.tenant_id.into();
        if target_tenant_id != auth_user.tenant_id {
            return Err(AppError::Forbidden(
                "Cannot create API key for user in another tenant".to_string(),
            ));
        }
        input.user_id
    } else {
        auth_user.user_id
    };

    let (api_key, full_key) = api_keys::create(
        &state.pool,
        auth_user.tenant_id,
        CreateApiKey {
            user_id,
            name: input.name,
            rpm_limit: input.rpm_limit,
            tpm_limit: input.tpm_limit,
            daily_limit: input.daily_limit,
            expires_at: input.expires_at,
        },
    )
    .await?;

    let api_key_id = api_key.id;
    let api_key_name = api_key.name.clone();
    let api_key_created_at = api_key.created_at;
    state.store.reload_api_keys_cache(&state.pool).await?;
    tracing::info!("API Key cache reloaded after create: {}", api_key_id);

    Ok(Json(ApiKeyWithSecret {
        id: api_key_id.into(),
        key: full_key,
        name: api_key_name,
        created_at: api_key_created_at,
    }))
}

pub async fn update_key(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateApiKey>,
) -> AppResult<Json<ApiKey>> {
    let api_key = api_keys::find_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("API Key not found".to_string()))?;

    if !auth_user.is_admin() && api_key.user_id != auth_user.user_id {
        return Err(AppError::Forbidden("Access denied".to_string()));
    }

    let input = UpdateApiKey {
        name: input.name.filter(|s| !s.is_empty()),
        status: input.status.filter(|s| !s.is_empty()),
        rpm_limit: input.rpm_limit,
        tpm_limit: input.tpm_limit,
        daily_limit: input.daily_limit,
    };
    let updated = api_keys::update(&state.pool, id, input).await?;
    state.store.reload_api_keys_cache(&state.pool).await?;

    Ok(Json(updated))
}

pub async fn delete_key(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    tracing::info!("Delete API Key request: id={}, auth_user_id={}, role={}", id, auth_user.user_id, auth_user.role);

    let api_key = api_keys::find_by_id(&state.pool, id)
        .await
        .map_err(|e| {
            tracing::error!("Find API Key by id failed: id={}, error={}", id, e);
            AppError::from(e)
        })?
        .ok_or_else(|| {
            tracing::warn!("API Key not found: id={}", id);
            AppError::NotFound("API Key not found".to_string())
        })?;

    tracing::info!("Found API Key: key_id={}, key_user_id={}, auth_user_id={}, is_admin={}", 
        id, api_key.user_id, auth_user.user_id, auth_user.is_admin());

    if !auth_user.is_admin() && api_key.user_id != auth_user.user_id {
        tracing::warn!("Access denied: key_user_id={}, auth_user_id={}", api_key.user_id, auth_user.user_id);
        return Err(AppError::Forbidden("Access denied".to_string()));
    }

    api_keys::delete(&state.pool, id).await.map_err(|e| {
        tracing::error!("Delete API Key failed: id={}, error={}", id, e);
        AppError::from(e)
    })?;
    state.store.reload_api_keys_cache(&state.pool).await?;
    tracing::info!("API Key deleted successfully: {}", id);

    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_key(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<ApiKey>> {
    let api_key = api_keys::find_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("API Key not found".to_string()))?;

    if !auth_user.is_admin() && api_key.user_id != auth_user.user_id {
        return Err(AppError::Forbidden("Access denied".to_string()));
    }

    Ok(Json(api_key))
}
