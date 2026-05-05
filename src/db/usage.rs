#![allow(dead_code)]
use crate::db::DbPool;
use crate::models::usage::{
    DailyUsage, ModelUsage, UsageAnalysis, UsageQuery, UsageRecord, UsageSummary, UserUsage,
};
use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, FromRow)]
pub struct TodayUsageSummary {
    pub requests: i64,
    pub tokens: i64,
}

pub async fn create(
    pool: &DbPool,
    tenant_id: Uuid,
    user_id: Uuid,
    api_key_id: Uuid,
    conversation_id: Option<Uuid>,
    model: &str,
    input_tokens: i64,
    output_tokens: i64,
    billing_mode: &str,
) -> Result<UsageRecord, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO usage_records (id, tenant_id, user_id, api_key_id, conversation_id, model, input_tokens, output_tokens, total_tokens, billing_mode)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(id.to_string())
    .bind(tenant_id.to_string())
    .bind(user_id.to_string())
    .bind(api_key_id.to_string())
    .bind(conversation_id.map(|u| u.to_string()))
    .bind(model)
    .bind(input_tokens)
    .bind(output_tokens)
    .bind(input_tokens + output_tokens)
    .bind(billing_mode)
    .execute(pool)
    .await?;

    sqlx::query_as::<_, UsageRecord>("SELECT * FROM usage_records WHERE id = ?")
        .bind(id.to_string())
        .fetch_one(pool)
        .await
}

pub async fn get_summary(
    pool: &DbPool,
    tenant_id: Uuid,
    query: UsageQuery,
) -> Result<UsageSummary, sqlx::Error> {
    let model = query.model.as_ref();
    sqlx::query_as::<_, UsageSummary>(
        r#"
        SELECT 
            COUNT(*) as total_requests,
            COALESCE(SUM(pl.total_tokens), 0) as total_tokens,
            COALESCE(SUM(pl.input_tokens), 0) as total_input_tokens,
            COALESCE(SUM(pl.output_tokens), 0) as total_output_tokens
        FROM proxy_logs pl
        WHERE pl.tenant_id = ?
            AND (? IS NULL OR pl.created_at >= ?)
            AND (? IS NULL OR pl.created_at <= ?)
            AND (? IS NULL OR pl.user_id = ?)
            AND (? IS NULL OR pl.api_key_id = ?)
            AND (? IS NULL OR pl.model = ?)
        "#,
    )
    .bind(tenant_id.to_string())
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .bind(query.user_id.map(|u| u.to_string()))
    .bind(query.user_id.map(|u| u.to_string()))
    .bind(query.api_key_id.map(|u| u.to_string()))
    .bind(query.api_key_id.map(|u| u.to_string()))
    .bind(model)
    .bind(model)
    .fetch_one(pool)
    .await
}

pub async fn get_user_summary(
    pool: &DbPool,
    tenant_id: Uuid,
    user_id: Uuid,
    query: UsageQuery,
) -> Result<UsageSummary, sqlx::Error> {
    let model = query.model.as_ref();
    sqlx::query_as::<_, UsageSummary>(
        r#"
        SELECT 
            COUNT(*) as total_requests,
            COALESCE(SUM(pl.total_tokens), 0) as total_tokens,
            COALESCE(SUM(pl.input_tokens), 0) as total_input_tokens,
            COALESCE(SUM(pl.output_tokens), 0) as total_output_tokens
        FROM proxy_logs pl
        WHERE pl.tenant_id = ?
            AND pl.user_id = ?
            AND (? IS NULL OR pl.created_at >= ?)
            AND (? IS NULL OR pl.created_at <= ?)
            AND (? IS NULL OR pl.model = ?)
        "#,
    )
    .bind(tenant_id.to_string())
    .bind(user_id.to_string())
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .bind(model)
    .bind(model)
    .fetch_one(pool)
    .await
}

pub async fn get_daily_usage(
    pool: &DbPool,
    tenant_id: Uuid,
    query: UsageQuery,
) -> Result<Vec<DailyUsage>, sqlx::Error> {
    sqlx::query_as::<_, DailyUsage>(
        r#"
        SELECT 
            date(pl.created_at) as date,
            COUNT(*) as requests,
            COALESCE(SUM(pl.total_tokens), 0) as tokens
        FROM proxy_logs pl
        WHERE pl.tenant_id = ?
            AND (? IS NULL OR pl.created_at >= ?)
            AND (? IS NULL OR pl.created_at <= ?)
            AND (? IS NULL OR pl.user_id = ?)
        GROUP BY date(pl.created_at)
        ORDER BY date DESC
        LIMIT 30
        "#,
    )
    .bind(tenant_id.to_string())
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .bind(query.user_id.map(|u| u.to_string()))
    .bind(query.user_id.map(|u| u.to_string()))
    .fetch_all(pool)
    .await
}

pub async fn get_model_usage(
    pool: &DbPool,
    tenant_id: Uuid,
    query: UsageQuery,
) -> Result<Vec<ModelUsage>, sqlx::Error> {
    let model_keyword = query.model_keyword.as_ref();
    sqlx::query_as::<_, ModelUsage>(
        r#"
        SELECT 
            pl.model as model,
            COUNT(*) as requests,
            COALESCE(SUM(pl.total_tokens), 0) as tokens
        FROM proxy_logs pl
        WHERE pl.tenant_id = ?
            AND (? IS NULL OR pl.created_at >= ?)
            AND (? IS NULL OR pl.created_at <= ?)
            AND (? IS NULL OR pl.user_id = ?)
            AND (? IS NULL OR pl.model LIKE '%' || ? || '%')
        GROUP BY pl.model
        ORDER BY tokens DESC
        LIMIT ?
        "#,
    )
    .bind(tenant_id.to_string())
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .bind(query.user_id.map(|u| u.to_string()))
    .bind(query.user_id.map(|u| u.to_string()))
    .bind(model_keyword)
    .bind(model_keyword)
    .bind(query.top_n.unwrap_or(20) as i64)
    .fetch_all(pool)
    .await
}

pub async fn get_upstream_model_usage(
    pool: &DbPool,
    tenant_id: Uuid,
    query: UsageQuery,
) -> Result<Vec<ModelUsage>, sqlx::Error> {
    let model_keyword = query.model_keyword.as_ref();
    sqlx::query_as::<_, ModelUsage>(
        r#"
        SELECT
            COALESCE(NULLIF(pl.routed_model, ''), pl.model) AS model,
            COUNT(*) as requests,
            COALESCE(SUM(pl.total_tokens), 0) as tokens
        FROM proxy_logs pl
        WHERE pl.tenant_id = ?
            AND (? IS NULL OR pl.created_at >= ?)
            AND (? IS NULL OR pl.created_at <= ?)
            AND (? IS NULL OR pl.user_id = ?)
            AND (? IS NULL OR COALESCE(NULLIF(pl.routed_model, ''), pl.model) LIKE '%' || ? || '%')
        GROUP BY COALESCE(NULLIF(pl.routed_model, ''), pl.model)
        ORDER BY tokens DESC
        LIMIT ?
        "#,
    )
    .bind(tenant_id.to_string())
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .bind(query.user_id.map(|u| u.to_string()))
    .bind(query.user_id.map(|u| u.to_string()))
    .bind(model_keyword)
    .bind(model_keyword)
    .bind(query.top_n.unwrap_or(20) as i64)
    .fetch_all(pool)
    .await
}

pub async fn get_usage_analysis(
    pool: &DbPool,
    tenant_id: Uuid,
    user_id: Option<Uuid>,
    start_time: Option<DateTime<Utc>>,
    end_time: Option<DateTime<Utc>>,
) -> Result<UsageAnalysis, sqlx::Error> {
    sqlx::query_as::<_, UsageAnalysis>(
        r#"
        SELECT
            COUNT(DISTINCT pl.model) AS distinct_requested_models,
            COUNT(DISTINCT COALESCE(NULLIF(pl.routed_model, ''), pl.model)) AS distinct_upstream_models,
            SUM(CASE WHEN pl.routed_model IS NULL OR pl.routed_model = '' THEN 1 ELSE 0 END) AS fallback_to_requested_model_requests
        FROM proxy_logs pl
        WHERE pl.tenant_id = ?
            AND (? IS NULL OR pl.created_at >= ?)
            AND (? IS NULL OR pl.created_at <= ?)
            AND (? IS NULL OR pl.user_id = ?)
        "#,
    )
    .bind(tenant_id.to_string())
    .bind(start_time.map(|t| t.to_rfc3339()))
    .bind(start_time.map(|t| t.to_rfc3339()))
    .bind(end_time.map(|t| t.to_rfc3339()))
    .bind(end_time.map(|t| t.to_rfc3339()))
    .bind(user_id.map(|u| u.to_string()))
    .bind(user_id.map(|u| u.to_string()))
    .fetch_one(pool)
    .await
}

pub async fn get_user_usage_stats(
    pool: &DbPool,
    tenant_id: Uuid,
    query: UsageQuery,
) -> Result<Vec<UserUsage>, sqlx::Error> {
    sqlx::query_as::<_, UserUsage>(
        r#"
        SELECT 
            pl.user_id,
            u.username,
            COUNT(*) as requests,
            COALESCE(SUM(pl.total_tokens), 0) as tokens
        FROM proxy_logs pl
        LEFT JOIN users u ON pl.user_id = u.id
        WHERE pl.tenant_id = ?
            AND (? IS NULL OR pl.created_at >= ?)
            AND (? IS NULL OR pl.created_at <= ?)
        GROUP BY pl.user_id, u.username
        ORDER BY tokens DESC
        LIMIT 20
        "#,
    )
    .bind(tenant_id.to_string())
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .fetch_all(pool)
    .await
}

pub async fn get_today_summary(
    pool: &DbPool,
    tenant_id: Uuid,
    user_id: Option<Uuid>,
) -> Result<TodayUsageSummary, sqlx::Error> {
    sqlx::query_as::<_, TodayUsageSummary>(
        r#"
        SELECT
            COUNT(*) AS requests,
            COALESCE(SUM(pl.total_tokens), 0) AS tokens
        FROM proxy_logs pl
        WHERE pl.tenant_id = ?
          AND pl.created_at >= date('now', 'start of day')
          AND (? IS NULL OR pl.user_id = ?)
        "#,
    )
    .bind(tenant_id.to_string())
    .bind(user_id.map(|u| u.to_string()))
    .bind(user_id.map(|u| u.to_string()))
    .fetch_one(pool)
    .await
}

pub async fn list_records(
    pool: &DbPool,
    tenant_id: Uuid,
    query: UsageQuery,
) -> Result<Vec<UsageRecord>, sqlx::Error> {
    let offset = (query.page - 1) * query.page_size;
    let model = query.model.as_ref();
    sqlx::query_as::<_, UsageRecord>(
        r#"
        SELECT
            pl.id,
            pl.tenant_id,
            pl.user_id,
            pl.api_key_id,
            pl.conversation_id,
            pl.model,
            pl.input_tokens,
            pl.output_tokens,
            pl.total_tokens,
            'token' AS billing_mode,
            pl.created_at AS recorded_at,
            pl.created_at
        FROM proxy_logs pl
        WHERE pl.tenant_id = ?
            AND (? IS NULL OR pl.created_at >= ?)
            AND (? IS NULL OR pl.created_at <= ?)
            AND (? IS NULL OR pl.user_id = ?)
            AND (? IS NULL OR pl.api_key_id = ?)
            AND (? IS NULL OR pl.model = ?)
        ORDER BY pl.created_at DESC
        LIMIT ? OFFSET ?
        "#,
    )
    .bind(tenant_id.to_string())
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .bind(query.user_id.map(|u| u.to_string()))
    .bind(query.user_id.map(|u| u.to_string()))
    .bind(query.api_key_id.map(|u| u.to_string()))
    .bind(query.api_key_id.map(|u| u.to_string()))
    .bind(model)
    .bind(model)
    .bind(query.page_size as i64)
    .bind(offset as i64)
    .fetch_all(pool)
    .await
}
