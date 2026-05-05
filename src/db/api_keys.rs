#![allow(dead_code)]
use crate::db::DbPool;
use chrono::Utc;
use uuid::Uuid;

use crate::models::api_key::{
    ApiKey, ApiKeyWithUser, CreateApiKey, UpdateApiKey, KEY_PREFIX_LENGTH, KEY_SECRET_LENGTH,
    KEY_SUFFIX_LENGTH, STATUS_ACTIVE,
};
use crate::utils::error::AppError;

pub async fn find_by_id(pool: &DbPool, id: Uuid) -> Result<Option<ApiKey>, sqlx::Error> {
    sqlx::query_as::<_, ApiKey>("SELECT * FROM api_keys WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await
}

pub async fn find_by_key_hash(
    pool: &DbPool,
    key_hash: &str,
) -> Result<Option<ApiKey>, sqlx::Error> {
    sqlx::query_as::<_, ApiKey>("SELECT * FROM api_keys WHERE key_hash = ?")
        .bind(key_hash)
        .fetch_optional(pool)
        .await
}

pub async fn find_by_user(
    pool: &DbPool,
    user_id: Uuid,
    page: i32,
    page_size: i32,
) -> Result<Vec<ApiKey>, sqlx::Error> {
    let offset = (page - 1) * page_size;
    sqlx::query_as::<_, ApiKey>(
        "SELECT * FROM api_keys WHERE user_id = ? ORDER BY created_at DESC LIMIT ? OFFSET ?",
    )
    .bind(user_id.to_string())
    .bind(page_size as i64)
    .bind(offset as i64)
    .fetch_all(pool)
    .await
}

pub async fn find_by_tenant(
    pool: &DbPool,
    tenant_id: Uuid,
    page: i32,
    page_size: i32,
) -> Result<Vec<ApiKey>, sqlx::Error> {
    let offset = (page - 1) * page_size;
    sqlx::query_as::<_, ApiKey>(
        r#"
        SELECT ak.* FROM api_keys ak
        JOIN users u ON ak.user_id = u.id
        WHERE u.tenant_id = ?
        ORDER BY ak.created_at DESC
        LIMIT ? OFFSET ?
        "#,
    )
    .bind(tenant_id.to_string())
    .bind(page_size as i64)
    .bind(offset as i64)
    .fetch_all(pool)
    .await
}

pub async fn find_by_tenant_with_user(
    pool: &DbPool,
    tenant_id: Uuid,
    page: i32,
    page_size: i32,
) -> Result<Vec<ApiKeyWithUser>, sqlx::Error> {
    let offset = (page - 1) * page_size;
    sqlx::query_as::<_, ApiKeyWithUser>(
        r#"
        SELECT 
            ak.id,
            ak.tenant_id,
            ak.user_id,
            u.username,
            ak.key_prefix,
            ak.key_suffix,
            ak.key_full,
            ak.name,
            ak.status,
            ak.rpm_limit,
            ak.tpm_limit,
            ak.daily_limit,
            ak.expires_at,
            ak.last_used_at,
            ak.created_at
        FROM api_keys ak
        JOIN users u ON ak.user_id = u.id
        WHERE u.tenant_id = ?
        ORDER BY ak.created_at DESC
        LIMIT ? OFFSET ?
        "#,
    )
    .bind(tenant_id.to_string())
    .bind(page_size as i64)
    .bind(offset as i64)
    .fetch_all(pool)
    .await
}

pub async fn create(
    pool: &DbPool,
    tenant_id: Uuid,
    input: CreateApiKey,
) -> Result<(ApiKey, String), AppError> {
    let secret = generate_secret();
    let key_prefix = format!("sk-{}", &secret[..KEY_PREFIX_LENGTH]);
    let key_suffix = secret[KEY_SECRET_LENGTH - KEY_SUFFIX_LENGTH..].to_string();
    let key_hash = hash_key(&secret);
    let full_key = format!("sk-{}", secret);
    let id = Uuid::new_v4();

    sqlx::query(
        r#"
        INSERT INTO api_keys (id, tenant_id, user_id, key_prefix, key_suffix, key_full, key_hash, name, rpm_limit, tpm_limit, daily_limit, expires_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(id.to_string())
    .bind(tenant_id.to_string())
    .bind(input.user_id.to_string())
    .bind(key_prefix)
    .bind(key_suffix)
    .bind(full_key.clone())
    .bind(key_hash)
    .bind(input.name)
    .bind(input.rpm_limit.unwrap_or(0))
    .bind(input.tpm_limit.unwrap_or(0))
    .bind(input.daily_limit.unwrap_or(0))
    .bind(input.expires_at)
    .execute(pool)
    .await?;

    let api_key = find_by_id(pool, id).await?.ok_or(sqlx::Error::RowNotFound)?;
    Ok((api_key, full_key))
}

pub async fn update(pool: &DbPool, id: Uuid, input: UpdateApiKey) -> Result<ApiKey, AppError> {
    sqlx::query(
        r#"
        UPDATE api_keys SET
            name = COALESCE(?, name),
            status = COALESCE(?, status),
            rpm_limit = COALESCE(?, rpm_limit),
            tpm_limit = COALESCE(?, tpm_limit),
            daily_limit = COALESCE(?, daily_limit)
        WHERE id = ?
        "#,
    )
    .bind(input.name)
    .bind(input.status)
    .bind(input.rpm_limit)
    .bind(input.tpm_limit)
    .bind(input.daily_limit)
    .bind(id.to_string())
    .execute(pool)
    .await
    .map_err(|e| AppError::from(e))?;

    find_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("API Key not found".to_string()))
}

pub async fn update_last_used(pool: &DbPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE api_keys SET last_used_at = datetime('now') WHERE id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete(pool: &DbPool, id: Uuid) -> Result<(), AppError> {
    let id_str = id.to_string();

    sqlx::query("DELETE FROM proxy_logs WHERE api_key_id = ?")
        .bind(&id_str)
        .execute(pool)
        .await?;

    sqlx::query("DELETE FROM conversations WHERE api_key_id = ?")
        .bind(&id_str)
        .execute(pool)
        .await?;

    let result = sqlx::query("DELETE FROM api_keys WHERE id = ?")
        .bind(&id_str)
        .execute(pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("API Key not found".to_string()));
    }

    Ok(())
}

pub async fn verify_key(pool: &DbPool, key: &str) -> Result<ApiKey, AppError> {
    let key = key.strip_prefix("sk-").unwrap_or(key);
    let key_hash = hash_key(key);

    let api_key = find_by_key_hash(pool, &key_hash)
        .await?
        .ok_or_else(|| AppError::Unauthorized("Invalid API key".to_string()))?;

    if api_key.status != STATUS_ACTIVE {
        return Err(AppError::Unauthorized("API key is disabled".to_string()));
    }

    if let Some(expires_at) = api_key.expires_at {
        if expires_at < Utc::now() {
            return Err(AppError::Unauthorized("API key has expired".to_string()));
        }
    }

    update_last_used(pool, api_key.id.into()).await?;

    Ok(api_key)
}

fn generate_secret() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    (0..KEY_SECRET_LENGTH)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

pub fn hash_key(key: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    hex::encode(hasher.finalize())
}
