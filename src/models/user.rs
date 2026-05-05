#![allow(dead_code)]
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::SqlUuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: SqlUuid,
    pub tenant_id: SqlUuid,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub email: String,
    pub role: String,
    pub status: String,
    pub quota_limit: i64,
    pub quota_used: i64,
    pub daily_request_limit: i64,
    pub monthly_request_limit: i64,
    pub last_login: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateUser {
    pub tenant_id: Uuid,
    pub username: String,
    pub email: String,
    pub role: String,
    pub daily_request_limit: Option<i64>,
    pub monthly_request_limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateUser {
    pub username: Option<String>,
    pub email: Option<String>,
    pub role: Option<String>,
    pub status: Option<String>,
    pub quota_limit: Option<i64>,
    pub daily_request_limit: Option<i64>,
    pub monthly_request_limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    pub token: String,
    pub user: UserInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub username: String,
    pub email: String,
    pub role: String,
}

impl From<User> for UserInfo {
    fn from(user: User) -> Self {
        Self {
            id: user.id.into(),
            tenant_id: user.tenant_id.into(),
            username: user.username,
            email: user.email,
            role: user.role,
        }
    }
}

pub const ROLE_ADMIN: &str = "admin";
pub const ROLE_USER: &str = "user";

pub const STATUS_ACTIVE: &str = "active";
pub const STATUS_DISABLED: &str = "disabled";
