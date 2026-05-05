use axum::{extract::State, http::StatusCode, Extension, Json};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    auth::jwt::JwtService,
    db::{self, audit},
    models::{
        audit::{CreateAuditLog, ACTION_LOGOUT},
        user::{CreateUser, LoginRequest, LoginResponse, UserInfo},
    },
    store::StoreManager,
    utils::error::{AppError, AppResult},
};

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    pub email: String,
}

pub async fn login(
    State((pool, jwt_service, _store)): State<(
        crate::db::DbPool,
        Arc<JwtService>,
        Arc<StoreManager>,
    )>,
    Json(req): Json<LoginRequest>,
) -> AppResult<Json<LoginResponse>> {
    let user = db::users::authenticate_any_tenant(&pool, &req.username, &req.password).await?;

    let token = jwt_service.generate_token(user.id.into(), user.tenant_id.into(), &user.role)?;

    Ok(Json(LoginResponse {
        token,
        user: UserInfo::from(user),
    }))
}

pub async fn register(
    State((pool, jwt_service, store)): State<(
        crate::db::DbPool,
        Arc<JwtService>,
        Arc<StoreManager>,
    )>,
    Json(req): Json<RegisterRequest>,
) -> AppResult<Json<LoginResponse>> {
    let default_tenant_id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();

    if db::users::find_by_username_global(&pool, &req.username)
        .await?
        .is_some()
    {
        return Err(AppError::Conflict("Username already exists".to_string()));
    }

    if db::users::find_by_email(&pool, default_tenant_id, &req.email)
        .await?
        .is_some()
    {
        return Err(AppError::Conflict("Email already exists".to_string()));
    }

    let password_hash = bcrypt::hash(&req.password, bcrypt::DEFAULT_COST)
        .map_err(|_| AppError::Internal("Failed to hash password".to_string()))?;

    let create_user = CreateUser {
        tenant_id: default_tenant_id,
        username: req.username,
        email: req.email,
        role: "user".to_string(),
        daily_request_limit: None,
        monthly_request_limit: None,
    };

    let user = db::users::create(&pool, create_user, password_hash).await?;
    store.reload_users_cache(&pool).await?;

    let token = jwt_service.generate_token(user.id.into(), user.tenant_id.into(), &user.role)?;

    Ok(Json(LoginResponse {
        token,
        user: UserInfo::from(user),
    }))
}

pub async fn logout(
    State((_pool, _jwt_service, store)): State<(
        crate::db::DbPool,
        Arc<JwtService>,
        Arc<StoreManager>,
    )>,
    Extension(auth_user): Extension<crate::auth::jwt::AuthUser>,
) -> AppResult<StatusCode> {
    let audit_log = CreateAuditLog {
        tenant_id: auth_user.tenant_id,
        user_id: auth_user.user_id,
        action: ACTION_LOGOUT.to_string(),
        resource_type: "session".to_string(),
        resource_id: None,
        details: serde_json::json!({}),
        ip_address: Some("0.0.0.0".to_string()),
        user_agent: None,
    };

    audit::create(store, audit_log).await?;

    Ok(StatusCode::OK)
}

pub async fn get_current_user(
    State((pool, _jwt_service, _store)): State<(
        crate::db::DbPool,
        Arc<JwtService>,
        Arc<StoreManager>,
    )>,
    Extension(auth_user): Extension<crate::auth::jwt::AuthUser>,
) -> AppResult<Json<UserInfo>> {
    let user = db::users::find_by_id(&pool, auth_user.user_id)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    Ok(Json(UserInfo::from(user)))
}

pub async fn change_password(
    State((pool, _jwt_service, _store)): State<(
        crate::db::DbPool,
        Arc<JwtService>,
        Arc<StoreManager>,
    )>,
    Extension(auth_user): Extension<crate::auth::jwt::AuthUser>,
    Json(req): Json<ChangePasswordRequest>,
) -> AppResult<StatusCode> {
    let user = db::users::find_by_id(&pool, auth_user.user_id)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    let valid = bcrypt::verify(&req.old_password, &user.password_hash)
        .map_err(|_| AppError::Internal("Password verification failed".to_string()))?;

    if !valid {
        return Err(AppError::BadRequest("Invalid old password".to_string()));
    }

    let password_hash = bcrypt::hash(&req.new_password, bcrypt::DEFAULT_COST)
        .map_err(|_| AppError::Internal("Failed to hash password".to_string()))?;

    db::users::update_password(&pool, user.id.into(), password_hash).await?;

    Ok(StatusCode::OK)
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub old_password: String,
    pub new_password: String,
}
