#![allow(dead_code)]
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::SqlUuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Tenant {
    pub id: SqlUuid,
    pub name: String,
    pub slug: String,
    pub status: String,
    pub plan_type: String,
    pub billing_mode: String,
    pub settings: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTenant {
    pub name: String,
    pub slug: String,
    pub plan_type: String,
    pub billing_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateTenant {
    pub name: Option<String>,
    pub status: Option<String>,
    pub plan_type: Option<String>,
    pub billing_mode: Option<String>,
    pub settings: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TenantQuota {
    pub rpm_limit: i32,
    pub tpm_limit: i32,
    pub daily_limit: i64,
    pub monthly_limit: i64,
}
