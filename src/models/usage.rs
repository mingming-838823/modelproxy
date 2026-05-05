#![allow(dead_code)]
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::SqlUuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UsageRecord {
    pub id: SqlUuid,
    pub tenant_id: SqlUuid,
    pub user_id: SqlUuid,
    pub api_key_id: SqlUuid,
    pub conversation_id: Option<SqlUuid>,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub billing_mode: String,
    pub recorded_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UsageSummary {
    pub total_requests: i64,
    pub total_tokens: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageQuery {
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
    pub user_id: Option<Uuid>,
    pub api_key_id: Option<Uuid>,
    pub model: Option<String>,
    pub model_keyword: Option<String>,
    pub group_by: Option<String>,
    pub top_n: Option<i32>,
    pub page: i32,
    pub page_size: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct DailyUsage {
    pub date: String,
    pub requests: i64,
    pub tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ModelUsage {
    pub model: String,
    pub requests: i64,
    pub tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UserUsage {
    pub user_id: SqlUuid,
    pub username: Option<String>,
    pub requests: i64,
    pub tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UsageAnalysis {
    pub distinct_requested_models: i64,
    pub distinct_upstream_models: i64,
    pub fallback_to_requested_model_requests: i64,
}
