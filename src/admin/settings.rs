use axum::{extract::State, Extension, Json};
use serde::{Deserialize, Serialize};

use crate::{
    auth::AuthUser,
    store::AdminState,
    utils::error::{AppError, AppResult},
};

#[derive(Debug, Serialize, Deserialize)]
pub struct SystemSettings {
    pub base_url: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSettings {
    pub base_url: String,
}

#[derive(Debug, Serialize)]
pub struct PublicSettings {
    pub base_url: String,
}

pub async fn get_settings(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
) -> AppResult<Json<SystemSettings>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let base_url: String =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = 'base_url'")
            .fetch_optional(&state.pool)
            .await?
            .unwrap_or_else(|| "http://localhost:3000/v1".to_string());

    Ok(Json(SystemSettings { base_url }))
}

pub async fn update_settings(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Json(payload): Json<UpdateSettings>,
) -> AppResult<Json<SystemSettings>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    sqlx::query(
        r#"
        INSERT INTO system_settings (key, value) 
        VALUES ('base_url', ?)
        ON CONFLICT(key) DO UPDATE SET value = excluded.value
        "#,
    )
    .bind(&payload.base_url)
    .execute(&state.pool)
    .await?;

    Ok(Json(SystemSettings {
        base_url: payload.base_url,
    }))
}

pub async fn get_public_settings(
    State(state): State<AdminState>,
) -> AppResult<Json<PublicSettings>> {
    let base_url: String =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = 'base_url'")
            .fetch_optional(&state.pool)
            .await?
            .unwrap_or_else(|| "http://localhost:3000/v1".to_string());

    Ok(Json(PublicSettings { base_url }))
}
