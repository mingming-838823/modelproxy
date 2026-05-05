use chrono::Utc;
use uuid::Uuid;

use crate::audit::handlers::AuditLogListQuery;
use crate::models::audit::{AuditLog, AuditLogQuery, CreateAuditLog};
use crate::models::SqlUuid;
use crate::store::{AuditLogRecord, StoreManager};
use std::sync::Arc;

pub async fn create(
    store: Arc<StoreManager>,
    input: CreateAuditLog,
) -> Result<AuditLog, crate::utils::error::AppError> {
    let id = Uuid::new_v4();
    let now = Utc::now();

    let record = AuditLogRecord {
        id: SqlUuid::from(id),
        tenant_id: SqlUuid::from(input.tenant_id),
        user_id: SqlUuid::from(input.user_id),
        action: input.action.clone(),
        resource_type: input.resource_type.clone(),
        resource_id: input.resource_id.clone(),
        details: input.details.clone(),
        ip_address: input.ip_address.clone(),
        user_agent: input.user_agent.clone(),
        created_at: now,
    };

    store.write_audit_log(record)?;

    Ok(AuditLog {
        id: SqlUuid::from(id),
        tenant_id: SqlUuid::from(input.tenant_id),
        user_id: SqlUuid::from(input.user_id),
        action: input.action,
        resource_type: input.resource_type,
        resource_id: input.resource_id,
        details: input.details,
        ip_address: input.ip_address,
        user_agent: input.user_agent,
        created_at: now,
    })
}

pub async fn list(
    store: Arc<StoreManager>,
    tenant_id: Option<Uuid>,
    query: AuditLogQuery,
) -> Result<Vec<AuditLog>, crate::utils::error::AppError> {
    let limit = query.page_size as usize;
    let offset = ((query.page - 1).max(0) as usize) * limit;

    let records = store
        .list_audit_logs_filtered(
            tenant_id,
            query.start_time,
            query.end_time,
            query.user_id,
            query.action.as_deref(),
            query.resource_type.as_deref(),
            limit,
            offset,
        )
        .await?;

    Ok(records
        .into_iter()
        .map(|record| AuditLog {
            id: record.id,
            tenant_id: record.tenant_id,
            user_id: record.user_id,
            action: record.action,
            resource_type: record.resource_type,
            resource_id: record.resource_id,
            details: record.details,
            ip_address: record.ip_address,
            user_agent: record.user_agent,
            created_at: record.created_at,
        })
        .collect())
}

pub async fn count(
    store: Arc<StoreManager>,
    tenant_id: Option<Uuid>,
    query: &AuditLogListQuery,
) -> Result<i64, crate::utils::error::AppError> {
    store
        .count_audit_logs_filtered(
            tenant_id,
            query.start_time,
            query.end_time,
            query.user_id,
            query.action.as_deref(),
            query.resource_type.as_deref(),
        )
        .await
}

pub async fn find_by_id(
    store: Arc<StoreManager>,
    id: Uuid,
) -> Result<Option<AuditLog>, crate::utils::error::AppError> {
    let record = store.find_audit_log_by_id(id).await?;

    Ok(record.map(|record| AuditLog {
        id: record.id,
        tenant_id: record.tenant_id,
        user_id: record.user_id,
        action: record.action,
        resource_type: record.resource_type,
        resource_id: record.resource_id,
        details: record.details,
        ip_address: record.ip_address,
        user_agent: record.user_agent,
        created_at: record.created_at,
    }))
}
