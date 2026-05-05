#![allow(dead_code)]
use crate::db::DbPool;
use uuid::Uuid;

use crate::models::user::{CreateUser, LoginRequest, UpdateUser, User, ROLE_ADMIN, STATUS_ACTIVE};
use crate::utils::error::AppError;

pub async fn find_by_id(pool: &DbPool, id: Uuid) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await
}

pub async fn find_by_username(
    pool: &DbPool,
    tenant_id: Uuid,
    username: &str,
) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>("SELECT * FROM users WHERE tenant_id = ? AND username = ?")
        .bind(tenant_id.to_string())
        .bind(username)
        .fetch_optional(pool)
        .await
}

pub async fn find_by_username_global(
    pool: &DbPool,
    username: &str,
) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>("SELECT * FROM users WHERE username = ?")
        .bind(username)
        .fetch_optional(pool)
        .await
}

pub async fn find_by_email(
    pool: &DbPool,
    tenant_id: Uuid,
    email: &str,
) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>("SELECT * FROM users WHERE tenant_id = ? AND email = ?")
        .bind(tenant_id.to_string())
        .bind(email)
        .fetch_optional(pool)
        .await
}

pub async fn find_by_tenant(
    pool: &DbPool,
    tenant_id: Uuid,
    page: i32,
    page_size: i32,
) -> Result<Vec<User>, sqlx::Error> {
    let offset = (page - 1) * page_size;
    sqlx::query_as::<_, User>(
        "SELECT * FROM users WHERE tenant_id = ? ORDER BY created_at DESC LIMIT ? OFFSET ?",
    )
    .bind(tenant_id.to_string())
    .bind(page_size as i64)
    .bind(offset as i64)
    .fetch_all(pool)
    .await
}

pub async fn create(
    pool: &DbPool,
    input: CreateUser,
    password_hash: String,
) -> Result<User, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO users (id, tenant_id, username, password_hash, email, role, daily_request_limit, monthly_request_limit)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(id.to_string())
    .bind(input.tenant_id.to_string())
    .bind(input.username)
    .bind(password_hash)
    .bind(input.email)
    .bind(input.role)
    .bind(input.daily_request_limit.unwrap_or(0))
    .bind(input.monthly_request_limit.unwrap_or(0))
    .execute(pool)
    .await?;

    find_by_id(pool, id).await?.ok_or(sqlx::Error::RowNotFound)
}

pub async fn update(pool: &DbPool, id: Uuid, input: UpdateUser) -> Result<User, AppError> {
    sqlx::query(
        r#"
        UPDATE users SET
            username = COALESCE(?, username),
            email = COALESCE(?, email),
            role = COALESCE(?, role),
            status = COALESCE(?, status),
            quota_limit = COALESCE(?, quota_limit),
            daily_request_limit = COALESCE(?, daily_request_limit),
            monthly_request_limit = COALESCE(?, monthly_request_limit),
            updated_at = datetime('now')
        WHERE id = ?
        "#,
    )
    .bind(input.username)
    .bind(input.email)
    .bind(input.role)
    .bind(input.status)
    .bind(input.quota_limit)
    .bind(input.daily_request_limit)
    .bind(input.monthly_request_limit)
    .bind(id.to_string())
    .execute(pool)
    .await
    .map_err(|e| AppError::from(e))?;

    find_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))
}

pub async fn update_password(
    pool: &DbPool,
    id: Uuid,
    password_hash: String,
) -> Result<(), AppError> {
    let result =
        sqlx::query("UPDATE users SET password_hash = ?, updated_at = datetime('now') WHERE id = ?")
            .bind(password_hash)
            .bind(id.to_string())
            .execute(pool)
            .await
            .map_err(|e| AppError::from(e))?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("User not found".to_string()));
    }

    Ok(())
}

pub async fn update_last_login(pool: &DbPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET last_login = datetime('now') WHERE id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_quota_used(pool: &DbPool, id: Uuid, delta: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET quota_used = quota_used + ? WHERE id = ?")
        .bind(delta)
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete(pool: &DbPool, id: Uuid) -> Result<(), AppError> {
    let user = find_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    if user.role == ROLE_ADMIN {
        return Err(AppError::Forbidden("Cannot delete admin user".to_string()));
    }

    let result = sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("User not found".to_string()));
    }

    Ok(())
}

pub async fn authenticate(
    pool: &DbPool,
    tenant_id: Uuid,
    req: LoginRequest,
) -> Result<User, AppError> {
    let user = find_by_username(pool, tenant_id, &req.username)
        .await?
        .ok_or_else(|| AppError::Unauthorized("Invalid credentials".to_string()))?;

    if user.status != STATUS_ACTIVE {
        return Err(AppError::Unauthorized("User is disabled".to_string()));
    }

    let valid = bcrypt::verify(&req.password, &user.password_hash)
        .map_err(|_| AppError::Internal("Password verification failed".to_string()))?;

    if !valid {
        return Err(AppError::Unauthorized("Invalid credentials".to_string()));
    }

    update_last_login(pool, user.id.into()).await?;

    Ok(user)
}

pub async fn authenticate_any_tenant(
    pool: &DbPool,
    username: &str,
    password: &str,
) -> Result<User, AppError> {
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE username = ?")
        .bind(username)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::Unauthorized("Invalid credentials".to_string()))?;

    if user.status != STATUS_ACTIVE {
        return Err(AppError::Unauthorized("User is disabled".to_string()));
    }

    let valid = bcrypt::verify(password, &user.password_hash)
        .map_err(|_| AppError::Internal("Password verification failed".to_string()))?;

    if !valid {
        return Err(AppError::Unauthorized("Invalid credentials".to_string()));
    }

    update_last_login(pool, user.id.into()).await?;

    Ok(user)
}

pub async fn is_admin(pool: &DbPool, user_id: Uuid) -> Result<bool, sqlx::Error> {
    let user = find_by_id(pool, user_id).await?;
    Ok(user.map(|u| u.role == ROLE_ADMIN).unwrap_or(false))
}

pub async fn admin_exists(pool: &DbPool) -> Result<bool, sqlx::Error> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users WHERE role = 'admin' AND status = 'active')",
    )
    .fetch_one(pool)
    .await?;
    Ok(exists)
}

pub async fn create_admin(pool: &DbPool, username: &str, password_hash: &str, email: &str) -> Result<User, AppError> {
    let default_tenant_id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
    let id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO users (id, tenant_id, username, password_hash, email, role, status, quota_limit, quota_used, daily_request_limit, monthly_request_limit)
        VALUES (?, ?, ?, ?, ?, 'admin', 'active', 0, 0, 0, 0)
        "#,
    )
    .bind(id.to_string())
    .bind(default_tenant_id.to_string())
    .bind(username)
    .bind(password_hash)
    .bind(email)
    .execute(pool)
    .await
    .map_err(|e| AppError::from(e))?;

    find_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))
}
