#![allow(dead_code)]
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::SqlUuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Conversation {
    pub id: SqlUuid,
    pub tenant_id: SqlUuid,
    pub user_id: SqlUuid,
    pub api_key_id: SqlUuid,
    pub conversation_id: String,
    pub model: String,
    pub provider: String,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_tokens: i64,
    pub client_ip: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ConversationMessage {
    pub id: SqlUuid,
    pub conversation_id: SqlUuid,
    pub input_content: String,
    pub output_content: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub model: String,
    pub finish_reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ConversationList {
    pub id: SqlUuid,
    pub conversation_id: String,
    pub user_id: SqlUuid,
    pub model: String,
    pub provider: String,
    pub total_tokens: i64,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationDetail {
    pub conversation: Conversation,
    pub messages: Vec<ConversationMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationQuery {
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
    pub model: Option<String>,
    pub page: i32,
    pub page_size: i32,
}
