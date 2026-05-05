#![allow(dead_code)]
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::SqlUuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ApiKey {
    pub id: SqlUuid,
    pub tenant_id: SqlUuid,
    pub user_id: SqlUuid,
    pub key_prefix: String,
    pub key_suffix: Option<String>,
    pub key_full: Option<String>,
    pub key_hash: String,
    pub name: String,
    pub status: String,
    pub rpm_limit: i32,
    pub tpm_limit: i32,
    pub daily_limit: i32,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateApiKey {
    pub user_id: Uuid,
    pub name: String,
    pub rpm_limit: Option<i32>,
    pub tpm_limit: Option<i32>,
    pub daily_limit: Option<i32>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateApiKey {
    pub name: Option<String>,
    pub status: Option<String>,
    pub rpm_limit: Option<i32>,
    pub tpm_limit: Option<i32>,
    pub daily_limit: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyWithSecret {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ApiKeyWithUser {
    pub id: SqlUuid,
    pub tenant_id: SqlUuid,
    pub user_id: SqlUuid,
    pub username: String,
    pub key_prefix: String,
    pub key_suffix: Option<String>,
    pub key_full: Option<String>,
    pub name: String,
    pub status: String,
    pub rpm_limit: i32,
    pub tpm_limit: i32,
    pub daily_limit: i32,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub const KEY_PREFIX_LENGTH: usize = 16;
pub const KEY_SECRET_LENGTH: usize = 32;
pub const KEY_SUFFIX_LENGTH: usize = 4;

pub const STATUS_ACTIVE: &str = "active";
pub const STATUS_DISABLED: &str = "disabled";
pub const STATUS_EXPIRED: &str = "expired";
