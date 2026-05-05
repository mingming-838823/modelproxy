#![allow(dead_code)]
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::SqlUuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UpstreamConfig {
    pub id: SqlUuid,
    pub tenant_id: SqlUuid,
    pub name: String,
    pub provider: String,
    pub api_type: String,
    pub base_url: String,
    #[serde(skip_serializing)]
    pub api_key_encrypted: String,
    pub models: String,
    pub custom_headers: serde_json::Value,
    pub priority: i32,
    pub weight: i32,
    pub rate_limit: Option<i32>,
    pub daily_request_limit: i64,
    pub monthly_request_limit: i64,
    pub daily_request_used: i64,
    pub monthly_request_used: i64,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UpstreamGroup {
    pub id: SqlUuid,
    pub tenant_id: SqlUuid,
    pub name: String,
    pub balance_strategy: String,
    pub failover_enabled: bool,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UpstreamGroupMember {
    pub id: SqlUuid,
    pub group_id: SqlUuid,
    pub upstream_id: SqlUuid,
    pub priority: i32,
    pub weight: i32,
    pub is_backup: bool,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateUpstream {
    pub name: String,
    pub provider: String,
    pub api_type: Option<String>,
    pub base_url: String,
    pub api_key: Option<String>,
    pub custom_headers: Option<serde_json::Value>,
    pub daily_request_limit: Option<i64>,
    pub monthly_request_limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateUpstreamGroup {
    pub name: String,
    pub upstream_ids: Vec<Uuid>,
    pub balance_strategy: Option<String>,
    pub failover_enabled: Option<bool>,
}

pub const PROVIDER_OPENAI: &str = "openai";
pub const PROVIDER_ANTHROPIC: &str = "anthropic";
pub const PROVIDER_OLLAMA: &str = "ollama";
pub const PROVIDER_VLLM: &str = "vllm";
pub const PROVIDER_SGLANG: &str = "sglang";

pub const API_TYPE_OPENAI: &str = "openai";
pub const API_TYPE_ANTHROPIC: &str = "anthropic";
pub const API_TYPE_OLLAMA: &str = "ollama";

pub const STATUS_ACTIVE: &str = "active";
pub const STATUS_DISABLED: &str = "disabled";

pub const STRATEGY_ROUND_ROBIN: &str = "round_robin";
pub const STRATEGY_WEIGHTED: &str = "weighted";
pub const STRATEGY_PRIORITY: &str = "priority";

#[derive(Debug, Clone, Serialize)]
pub struct UpstreamConfigResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub provider: String,
    pub api_type: String,
    pub base_url: String,
    pub api_key_masked: String,
    pub models: String,
    pub custom_headers: serde_json::Value,
    pub priority: i32,
    pub weight: i32,
    pub rate_limit: Option<i32>,
    pub daily_request_limit: i64,
    pub monthly_request_limit: i64,
    pub daily_request_used: i64,
    pub monthly_request_used: i64,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<UpstreamConfig> for UpstreamConfigResponse {
    fn from(config: UpstreamConfig) -> Self {
        Self {
            id: config.id.into(),
            tenant_id: config.tenant_id.into(),
            name: config.name,
            provider: config.provider,
            api_type: config.api_type,
            base_url: config.base_url,
            api_key_masked: mask_api_key(&config.api_key_encrypted),
            models: config.models,
            custom_headers: config.custom_headers,
            priority: config.priority,
            weight: config.weight,
            rate_limit: config.rate_limit,
            daily_request_limit: config.daily_request_limit,
            monthly_request_limit: config.monthly_request_limit,
            daily_request_used: config.daily_request_used,
            monthly_request_used: config.monthly_request_used,
            status: config.status,
            created_at: config.created_at,
            updated_at: config.updated_at,
        }
    }
}

pub fn mask_api_key(key: &str) -> String {
    if key.is_empty() {
        return String::new();
    }
    if key.len() <= 8 {
        return "*".repeat(key.len());
    }
    format!("{}****{}", &key[..4], &key[key.len() - 4..])
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ModelVisibility {
    pub id: SqlUuid,
    pub upstream_id: SqlUuid,
    pub model_name: String,
    pub model_alias: Option<String>,
    pub model_headers: serde_json::Value,
    pub all_users_visible: bool,
    pub retry_count: i64,
    pub retry_interval_seconds: i64,
    pub retry_backoff_strategy: String,
    pub retry_max_interval_seconds: i64,
    pub retry_failure_strategy: String,
    pub retry_fallback_upstream_id: Option<String>,
    pub retry_fallback_model_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ModelVisibilityUser {
    pub id: SqlUuid,
    pub visibility_id: SqlUuid,
    pub user_id: SqlUuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelWithVisibility {
    pub upstream_id: Uuid,
    pub upstream_name: String,
    pub model_name: String,
    pub original_model_name: String,
    pub model_alias: Option<String>,
    pub model_aliases: Vec<String>,
    pub model_headers: serde_json::Value,
    pub provider: String,
    pub all_users_visible: bool,
    pub allowed_users: Vec<Uuid>,
    pub retry_count: i64,
    pub retry_interval_seconds: i64,
    pub retry_backoff_strategy: String,
    pub retry_max_interval_seconds: i64,
    pub retry_failure_strategy: String,
    pub retry_fallback_upstream_id: Option<Uuid>,
    pub retry_fallback_model_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateModelVisibilityRequest {
    pub all_users_visible: bool,
    pub user_ids: Vec<Uuid>,
    pub model_alias: Option<String>,
    pub model_aliases: Option<Vec<String>>,
    pub model_headers: Option<serde_json::Value>,
    pub retry_count: Option<i64>,
    pub retry_interval_seconds: Option<i64>,
    pub retry_backoff_strategy: Option<String>,
    pub retry_max_interval_seconds: Option<i64>,
    pub retry_failure_strategy: Option<String>,
    pub retry_fallback_upstream_id: Option<Uuid>,
    pub retry_fallback_model_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionalAliasRuleInput {
    pub upstream_id: Uuid,
    pub model_name: String,
    pub min_input_tokens: Option<i64>,
    pub max_input_tokens: Option<i64>,
    pub keywords: Option<Vec<String>>,
    #[serde(default)]
    pub has_image: bool,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionalAliasFallbackInput {
    pub upstream_id: Uuid,
    pub model_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertConditionalAliasRequest {
    pub rules: Vec<ConditionalAliasRuleInput>,
    pub fallback: ConditionalAliasFallbackInput,
    #[serde(default = "default_true")]
    pub all_users_visible: bool,
    #[serde(default)]
    pub user_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionalAliasRule {
    pub priority: i32,
    pub upstream_id: Uuid,
    pub model_name: String,
    pub min_input_tokens: Option<i64>,
    pub max_input_tokens: Option<i64>,
    pub keywords: Vec<String>,
    #[serde(default)]
    pub has_image: bool,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionalAliasConfig {
    pub alias: String,
    pub rules: Vec<ConditionalAliasRule>,
    pub fallback: ConditionalAliasFallbackInput,
    #[serde(default = "default_true")]
    pub all_users_visible: bool,
    #[serde(default)]
    pub user_ids: Vec<Uuid>,
}

fn default_true() -> bool {
    true
}
