use crate::store::AdminState;
use axum::{
    extract::{Path, Query, State},
    Extension, Json,
};
use http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    db,
    models::user::{CreateUser, UpdateUser, User, ROLE_ADMIN, ROLE_USER},
    utils::error::{AppError, AppResult},
};

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub page: Option<i32>,
    pub page_size: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct UserList {
    pub items: Vec<User>,
    pub total: i64,
}

pub async fn list_users(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<UserList>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).max(1);

    let items =
        db::users::find_by_tenant(&state.pool, auth_user.tenant_id, page, page_size).await?;

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE tenant_id = ?")
        .bind(auth_user.tenant_id.to_string())
        .fetch_optional(&state.pool)
        .await?
        .unwrap_or(0);

    Ok(Json(UserList { items, total }))
}

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub email: String,
    pub role: Option<String>,
    pub daily_request_limit: Option<i64>,
    pub monthly_request_limit: Option<i64>,
}

pub async fn create_user(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Json(req): Json<CreateUserRequest>,
) -> AppResult<Json<User>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    if db::users::find_by_username_global(&state.pool, &req.username)
        .await?
        .is_some()
    {
        return Err(AppError::Conflict("Username already exists".to_string()));
    }

    if db::users::find_by_email(&state.pool, auth_user.tenant_id, &req.email)
        .await?
        .is_some()
    {
        return Err(AppError::Conflict("Email already exists".to_string()));
    }

    let password_hash = bcrypt::hash(&req.password, bcrypt::DEFAULT_COST)
        .map_err(|_| AppError::Internal("Failed to hash password".to_string()))?;

    let create_user = CreateUser {
        tenant_id: auth_user.tenant_id,
        username: req.username,
        email: req.email,
        role: req.role.unwrap_or_else(|| ROLE_USER.to_string()),
        daily_request_limit: req.daily_request_limit,
        monthly_request_limit: req.monthly_request_limit,
    };

    let user = db::users::create(&state.pool, create_user, password_hash).await?;
    state.store.reload_users_cache(&state.pool).await?;

    Ok(Json(user))
}

fn verify_user_tenant(user: &User, auth_user: &AuthUser) -> AppResult<()> {
    let user_tenant_id: Uuid = user.tenant_id.into();
    if user_tenant_id != auth_user.tenant_id {
        return Err(AppError::Forbidden("Access denied".to_string()));
    }
    Ok(())
}

pub async fn get_user(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<User>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let user = db::users::find_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    verify_user_tenant(&user, &auth_user)?;

    Ok(Json(user))
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserRequest {
    pub username: Option<String>,
    pub email: Option<String>,
    pub role: Option<String>,
    pub status: Option<String>,
    pub quota_limit: Option<i64>,
    pub daily_request_limit: Option<i64>,
    pub monthly_request_limit: Option<i64>,
}

pub async fn update_user(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateUserRequest>,
) -> AppResult<Json<User>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let existing = db::users::find_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    verify_user_tenant(&existing, &auth_user)?;

    if id == auth_user.user_id {
        if let Some(ref new_role) = req.role {
            if new_role != ROLE_ADMIN {
                return Err(AppError::BadRequest(
                    "Cannot demote yourself from admin".to_string(),
                ));
            }
        }
        if let Some(ref new_status) = req.status {
            if new_status != "active" {
                return Err(AppError::BadRequest(
                    "Cannot disable your own account".to_string(),
                ));
            }
        }
    }

    let update = UpdateUser {
        username: req.username.filter(|s| !s.is_empty()),
        email: req.email.filter(|s| !s.is_empty()),
        role: req.role.filter(|s| !s.is_empty()),
        status: req.status.filter(|s| !s.is_empty()),
        quota_limit: req.quota_limit,
        daily_request_limit: req.daily_request_limit,
        monthly_request_limit: req.monthly_request_limit,
    };

    let user = db::users::update(&state.pool, id, update).await?;
    state.store.reload_users_cache(&state.pool).await?;

    Ok(Json(user))
}

pub async fn delete_user(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    if id == auth_user.user_id {
        return Err(AppError::BadRequest("Cannot delete yourself".to_string()));
    }

    let existing = db::users::find_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    verify_user_tenant(&existing, &auth_user)?;

    db::users::delete(&state.pool, id).await?;
    state.store.reload_users_cache(&state.pool).await?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct ResetPasswordRequest {
    pub new_password: String,
}

pub async fn reset_user_password(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
    Json(req): Json<ResetPasswordRequest>,
) -> AppResult<StatusCode> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let existing = db::users::find_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    verify_user_tenant(&existing, &auth_user)?;

    let password_hash = bcrypt::hash(&req.new_password, bcrypt::DEFAULT_COST)
        .map_err(|_| AppError::Internal("Failed to hash password".to_string()))?;

    db::users::update_password(&state.pool, id, password_hash).await?;
    state.store.reload_users_cache(&state.pool).await?;

    Ok(StatusCode::NO_CONTENT)
}
