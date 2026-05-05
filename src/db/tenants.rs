#![allow(dead_code)]
use crate::db::DbPool;
use uuid::Uuid;

use crate::models::tenant::{CreateTenant, Tenant, TenantQuota, UpdateTenant};
use crate::utils::error::AppError;

pub async fn find_by_id(pool: &DbPool, id: Uuid) -> Result<Option<Tenant>, sqlx::Error> {
    sqlx::query_as::<_, Tenant>("SELECT * FROM tenants WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await
}

pub async fn find_by_slug(pool: &DbPool, slug: &str) -> Result<Option<Tenant>, sqlx::Error> {
    sqlx::query_as::<_, Tenant>("SELECT * FROM tenants WHERE slug = ?")
        .bind(slug)
        .fetch_optional(pool)
        .await
}

pub async fn find_all(
    pool: &DbPool,
    page: i32,
    page_size: i32,
) -> Result<Vec<Tenant>, sqlx::Error> {
    let offset = (page - 1) * page_size;
    sqlx::query_as::<_, Tenant>("SELECT * FROM tenants ORDER BY created_at DESC LIMIT ? OFFSET ?")
        .bind(page_size as i64)
        .bind(offset as i64)
        .fetch_all(pool)
        .await
}

pub async fn create(pool: &DbPool, input: CreateTenant) -> Result<Tenant, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO tenants (id, name, slug, plan_type, billing_mode)
        VALUES (?, ?, ?, ?, ?)
        "#,
    )
    .bind(id.to_string())
    .bind(input.name)
    .bind(input.slug)
    .bind(input.plan_type)
    .bind(input.billing_mode)
    .execute(pool)
    .await?;

    find_by_id(pool, id).await?.ok_or(sqlx::Error::RowNotFound)
}

pub async fn update(pool: &DbPool, id: Uuid, input: UpdateTenant) -> Result<Tenant, AppError> {
    let _tenant = find_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Tenant not found".to_string()))?;

    sqlx::query(
        r#"
        UPDATE tenants SET
            name = COALESCE(?, name),
            status = COALESCE(?, status),
            plan_type = COALESCE(?, plan_type),
            billing_mode = COALESCE(?, billing_mode),
            settings = COALESCE(?, settings),
            updated_at = datetime('now')
        WHERE id = ?
        "#,
    )
    .bind(input.name)
    .bind(input.status)
    .bind(input.plan_type)
    .bind(input.billing_mode)
    .bind(input.settings)
    .bind(id.to_string())
    .execute(pool)
    .await
    .map_err(AppError::from)?;

    find_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Tenant not found".to_string()))
}

pub async fn delete(pool: &DbPool, id: Uuid) -> Result<(), AppError> {
    let result = sqlx::query("DELETE FROM tenants WHERE id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("Tenant not found".to_string()));
    }

    Ok(())
}

pub async fn get_quota(pool: &DbPool, id: Uuid) -> Result<TenantQuota, sqlx::Error> {
    let row: TenantQuota = sqlx::query_as::<_, TenantQuota>(
        r#"
        SELECT 
            COALESCE(SUM(ak.rpm_limit), 0) as rpm_limit,
            COALESCE(SUM(ak.tpm_limit), 0) as tpm_limit,
            COALESCE(SUM(ak.daily_limit), 0) as daily_limit,
            0 as monthly_limit
        FROM api_keys ak
        JOIN users u ON ak.user_id = u.id
        WHERE u.tenant_id = ? AND ak.status = 'active'
        "#,
    )
    .bind(id.to_string())
    .fetch_one(pool)
    .await?;

    Ok(row)
}
