#![allow(dead_code)]
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::SqlUuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AuditLog {
    pub id: SqlUuid,
    pub tenant_id: SqlUuid,
    pub user_id: SqlUuid,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub details: serde_json::Value,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAuditLog {
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub details: serde_json::Value,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogQuery {
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
    pub user_id: Option<Uuid>,
    pub action: Option<String>,
    pub resource_type: Option<String>,
    pub page: i32,
    pub page_size: i32,
}

pub const ACTION_LOGIN: &str = "login";
pub const ACTION_LOGOUT: &str = "logout";
pub const ACTION_API_KEY_CREATE: &str = "api_key.create";
pub const ACTION_API_KEY_DELETE: &str = "api_key.delete";
pub const ACTION_USER_CREATE: &str = "user.create";
pub const ACTION_USER_UPDATE: &str = "user.update";
pub const ACTION_TENANT_UPDATE: &str = "tenant.update";
pub const ACTION_PROXY_REQUEST: &str = "proxy.request";
