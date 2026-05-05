#![allow(dead_code)]
use crate::db::DbPool;
use uuid::Uuid;

use crate::models::upstream::{
    CreateUpstream, CreateUpstreamGroup, UpstreamConfig, UpstreamGroup, UpstreamGroupMember,
    STATUS_ACTIVE,
};
use crate::utils::error::AppError;
use crate::utils::secrets::{
    decrypt_upstream_api_key, encrypt_upstream_api_key, is_encrypted_upstream_api_key,
};

pub async fn find_by_id(pool: &DbPool, id: Uuid) -> Result<Option<UpstreamConfig>, AppError> {
    let upstream =
        sqlx::query_as::<_, UpstreamConfig>("SELECT * FROM upstream_configs WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool)
            .await?;
    upstream.map(decrypt_upstream_config).transpose()
}

pub async fn find_by_tenant(
    pool: &DbPool,
    tenant_id: Uuid,
) -> Result<Vec<UpstreamConfig>, AppError> {
    let upstreams = sqlx::query_as::<_, UpstreamConfig>(
        "SELECT * FROM upstream_configs WHERE tenant_id = ? ORDER BY created_at DESC",
    )
    .bind(tenant_id.to_string())
    .fetch_all(pool)
    .await?;
    upstreams.into_iter().map(decrypt_upstream_config).collect()
}

pub async fn create(
    pool: &DbPool,
    tenant_id: Uuid,
    input: CreateUpstream,
    encrypted_key: &str,
) -> Result<UpstreamConfig, AppError> {
    let api_type = input.api_type.unwrap_or_else(|| "openai".to_string());
    let id = Uuid::new_v4();
    let encrypted_key = encrypt_upstream_api_key(encrypted_key)?;
    sqlx::query(
        r#"
        INSERT INTO upstream_configs (id, tenant_id, name, provider, api_type, base_url, api_key_encrypted, custom_headers, daily_request_limit, monthly_request_limit)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(id.to_string())
    .bind(tenant_id.to_string())
    .bind(input.name)
    .bind(input.provider)
    .bind(api_type)
    .bind(input.base_url)
    .bind(encrypted_key)
    .bind(input.custom_headers.unwrap_or_else(|| serde_json::json!({})))
    .bind(input.daily_request_limit.unwrap_or(2000))
    .bind(input.monthly_request_limit.unwrap_or(50000))
    .execute(pool)
    .await?;
    
    find_by_id(pool, id).await?.ok_or(sqlx::Error::RowNotFound.into())
}

pub async fn update(
    pool: &DbPool,
    id: Uuid,
    input: crate::admin::upstreams::UpdateUpstreamRequest,
    api_key: Option<&str>,
) -> Result<UpstreamConfig, AppError> {
    let encrypted_api_key = match api_key {
        Some(api_key) => Some(encrypt_upstream_api_key(api_key)?),
        None => None,
    };
    sqlx::query(
        r#"
        UPDATE upstream_configs SET
            name = COALESCE(?, name),
            provider = COALESCE(?, provider),
            api_type = COALESCE(?, api_type),
            base_url = COALESCE(?, base_url),
            api_key_encrypted = COALESCE(?, api_key_encrypted),
            custom_headers = COALESCE(?, custom_headers),
            daily_request_limit = COALESCE(?, daily_request_limit),
            monthly_request_limit = COALESCE(?, monthly_request_limit),
            updated_at = datetime('now')
        WHERE id = ?
        "#,
    )
    .bind(input.name)
    .bind(input.provider)
    .bind(input.api_type)
    .bind(input.base_url)
    .bind(encrypted_api_key)
    .bind(input.custom_headers)
    .bind(input.daily_request_limit)
    .bind(input.monthly_request_limit)
    .bind(id.to_string())
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("Failed to update upstream: {}", e)))?;

    find_by_id(pool, id).await?.ok_or(AppError::NotFound("Upstream config not found".to_string()))
}

pub async fn update_status(pool: &DbPool, id: Uuid, status: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE upstream_configs SET status = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(status)
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_usage(
    pool: &DbPool,
    id: Uuid,
    daily_delta: i32,
    monthly_delta: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE upstream_configs SET daily_request_used = daily_request_used + ?, monthly_request_used = monthly_request_used + ? WHERE id = ?")
        .bind(daily_delta)
        .bind(monthly_delta)
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn migrate_plaintext_api_keys(pool: &DbPool) -> Result<usize, AppError> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT id, api_key_encrypted FROM upstream_configs WHERE api_key_encrypted IS NOT NULL AND api_key_encrypted != ''",
    )
    .fetch_all(pool)
    .await?;

    let mut migrated = 0usize;
    for (id, api_key_encrypted) in rows {
        if is_encrypted_upstream_api_key(&api_key_encrypted) {
            continue;
        }
        let encrypted = encrypt_upstream_api_key(&api_key_encrypted)?;
        sqlx::query(
            "UPDATE upstream_configs SET api_key_encrypted = ?, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(encrypted)
        .bind(id)
        .execute(pool)
        .await?;
        migrated += 1;
    }

    Ok(migrated)
}

pub async fn reencrypt_api_keys(
    pool: &DbPool,
    old_secret: &str,
    new_secret: &str,
) -> Result<usize, AppError> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT id, api_key_encrypted FROM upstream_configs WHERE api_key_encrypted IS NOT NULL AND api_key_encrypted != ''",
    )
    .fetch_all(pool)
    .await?;

    let mut migrated = 0usize;
    for (id, api_key_encrypted) in rows {
        if !is_encrypted_upstream_api_key(&api_key_encrypted) {
            continue;
        }

        let plaintext = match crate::utils::secrets::decrypt_upstream_api_key_with_secret(
            &api_key_encrypted,
            old_secret,
        ) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("Failed to decrypt upstream key {} with old secret: {}", id, e);
                continue;
            }
        };

        let reencrypted =
            crate::utils::secrets::encrypt_upstream_api_key_with_secret(&plaintext, new_secret)?;

        sqlx::query(
            "UPDATE upstream_configs SET api_key_encrypted = ?, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(reencrypted)
        .bind(id)
        .execute(pool)
        .await?;
        migrated += 1;
    }

    Ok(migrated)
}

pub async fn delete(pool: &DbPool, id: Uuid) -> Result<(), AppError> {
    let result = sqlx::query("DELETE FROM upstream_configs WHERE id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("Upstream config not found".to_string()));
    }

    Ok(())
}

pub async fn find_group_by_id(
    pool: &DbPool,
    id: Uuid,
) -> Result<Option<UpstreamGroup>, sqlx::Error> {
    sqlx::query_as::<_, UpstreamGroup>("SELECT * FROM upstream_groups WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await
}

pub async fn find_groups_by_tenant(
    pool: &DbPool,
    tenant_id: Uuid,
) -> Result<Vec<UpstreamGroup>, sqlx::Error> {
    sqlx::query_as::<_, UpstreamGroup>(
        "SELECT * FROM upstream_groups WHERE tenant_id = ? ORDER BY created_at DESC",
    )
    .bind(tenant_id.to_string())
    .fetch_all(pool)
    .await
}

pub async fn create_group(
    pool: &DbPool,
    tenant_id: Uuid,
    input: CreateUpstreamGroup,
) -> Result<UpstreamGroup, AppError> {
    let id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO upstream_groups (id, tenant_id, name, balance_strategy, failover_enabled)
        VALUES (?, ?, ?, ?, ?)
        "#,
    )
    .bind(id.to_string())
    .bind(tenant_id.to_string())
    .bind(input.name)
    .bind(
        input
            .balance_strategy
            .unwrap_or_else(|| "priority".to_string()),
    )
    .bind(input.failover_enabled.unwrap_or(true))
    .execute(pool)
    .await?;

    for (idx, upstream_id) in input.upstream_ids.iter().enumerate() {
        add_group_member(pool, id, *upstream_id, (idx + 1) as i32, 100, false).await?;
    }

    find_group_by_id(pool, id).await?.ok_or(sqlx::Error::RowNotFound.into())
}

pub async fn delete_group(pool: &DbPool, id: Uuid) -> Result<(), AppError> {
    let result = sqlx::query("DELETE FROM upstream_groups WHERE id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("Upstream group not found".to_string()));
    }

    Ok(())
}

pub async fn add_group_member(
    pool: &DbPool,
    group_id: Uuid,
    upstream_id: Uuid,
    priority: i32,
    weight: i32,
    is_backup: bool,
) -> Result<UpstreamGroupMember, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO upstream_group_members (id, group_id, upstream_id, priority, weight, is_backup)
        VALUES (?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(id.to_string())
    .bind(group_id.to_string())
    .bind(upstream_id.to_string())
    .bind(priority)
    .bind(weight)
    .bind(is_backup)
    .execute(pool)
    .await?;

    sqlx::query_as::<_, UpstreamGroupMember>("SELECT * FROM upstream_group_members WHERE id = ?")
        .bind(id.to_string())
        .fetch_one(pool)
        .await
}

pub async fn get_group_members(
    pool: &DbPool,
    group_id: Uuid,
) -> Result<Vec<UpstreamGroupMember>, sqlx::Error> {
    sqlx::query_as::<_, UpstreamGroupMember>(
        "SELECT * FROM upstream_group_members WHERE group_id = ? ORDER BY priority ASC",
    )
    .bind(group_id.to_string())
    .fetch_all(pool)
    .await
}

pub async fn get_active_upstreams_for_group(
    pool: &DbPool,
    group_id: Uuid,
) -> Result<Vec<UpstreamConfig>, AppError> {
    let upstreams = sqlx::query_as::<_, UpstreamConfig>(
        r#"
        SELECT uc.* FROM upstream_configs uc
        JOIN upstream_group_members ugm ON uc.id = ugm.upstream_id
        WHERE ugm.group_id = ? AND uc.status = ? AND ugm.status = ?
        ORDER BY ugm.priority ASC, ugm.weight DESC
        "#,
    )
    .bind(group_id.to_string())
    .bind(STATUS_ACTIVE)
    .bind(STATUS_ACTIVE)
    .fetch_all(pool)
    .await?;
    upstreams.into_iter().map(decrypt_upstream_config).collect()
}

fn decrypt_upstream_config(mut upstream: UpstreamConfig) -> Result<UpstreamConfig, AppError> {
    if upstream.api_key_encrypted.is_empty() {
        return Ok(upstream);
    }
    match decrypt_upstream_api_key(&upstream.api_key_encrypted) {
        Ok(key) => {
            upstream.api_key_encrypted = key;
        }
        Err(e) => {
            tracing::error!(
                "Failed to decrypt upstream API key for '{}' ({}): {}. Clearing key to prevent invalid requests.",
                upstream.name, upstream.id, e
            );
            upstream.api_key_encrypted = String::new();
        }
    }
    Ok(upstream)
}
