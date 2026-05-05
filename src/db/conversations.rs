#![allow(dead_code)]
use crate::db::DbPool;
use sqlx::Row;
use uuid::Uuid;

use crate::models::conversation::{
    Conversation, ConversationList, ConversationMessage, ConversationQuery,
};

pub async fn find_by_id(pool: &DbPool, id: Uuid) -> Result<Option<Conversation>, sqlx::Error> {
    sqlx::query_as::<_, Conversation>("SELECT * FROM conversations WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await
}

pub async fn find_by_conversation_id(
    pool: &DbPool,
    conversation_id: &str,
) -> Result<Option<Conversation>, sqlx::Error> {
    sqlx::query_as::<_, Conversation>("SELECT * FROM conversations WHERE conversation_id = ?")
        .bind(conversation_id)
        .fetch_optional(pool)
        .await
}

pub async fn find_by_user(
    pool: &DbPool,
    user_id: Uuid,
    query: ConversationQuery,
) -> Result<Vec<ConversationList>, sqlx::Error> {
    let offset = (query.page - 1) * query.page_size;
    let model = query.model.as_ref();

    sqlx::query_as::<_, ConversationList>(
        r#"
        SELECT 
            id, conversation_id, user_id, model, provider, total_tokens,
            started_at, ended_at
        FROM conversations
        WHERE user_id = ?
            AND (? IS NULL OR started_at >= ?)
            AND (? IS NULL OR started_at <= ?)
            AND (? IS NULL OR model = ?)
        ORDER BY started_at DESC 
        LIMIT ? OFFSET ?
        "#,
    )
    .bind(user_id.to_string())
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .bind(model)
    .bind(model)
    .bind(query.page_size as i64)
    .bind(offset as i64)
    .fetch_all(pool)
    .await
}

pub async fn find_all(
    pool: &DbPool,
    tenant_id: Uuid,
    query: ConversationQuery,
) -> Result<Vec<ConversationList>, sqlx::Error> {
    let offset = (query.page - 1) * query.page_size;
    let model = query.model.as_ref();

    sqlx::query_as::<_, ConversationList>(
        r#"
        SELECT 
            id, conversation_id, user_id, model, provider, total_tokens,
            started_at, ended_at
        FROM conversations
        WHERE tenant_id = ?
            AND (? IS NULL OR started_at >= ?)
            AND (? IS NULL OR started_at <= ?)
            AND (? IS NULL OR model = ?)
        ORDER BY started_at DESC 
        LIMIT ? OFFSET ?
        "#,
    )
    .bind(tenant_id.to_string())
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .bind(model)
    .bind(model)
    .bind(query.page_size as i64)
    .bind(offset as i64)
    .fetch_all(pool)
    .await
}

pub async fn count_by_user(
    pool: &DbPool,
    user_id: Uuid,
    query: ConversationQuery,
) -> Result<i64, sqlx::Error> {
    let model = query.model.as_ref();
    let row = sqlx::query(
        r#"
        SELECT COUNT(*) AS total
        FROM conversations
        WHERE user_id = ?
            AND (? IS NULL OR started_at >= ?)
            AND (? IS NULL OR started_at <= ?)
            AND (? IS NULL OR model = ?)
        "#,
    )
    .bind(user_id.to_string())
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .bind(model)
    .bind(model)
    .fetch_one(pool)
    .await?;

    Ok(row.try_get("total")?)
}

pub async fn count_all(
    pool: &DbPool,
    tenant_id: Uuid,
    query: ConversationQuery,
) -> Result<i64, sqlx::Error> {
    let model = query.model.as_ref();
    let row = sqlx::query(
        r#"
        SELECT COUNT(*) AS total
        FROM conversations
        WHERE tenant_id = ?
            AND (? IS NULL OR started_at >= ?)
            AND (? IS NULL OR started_at <= ?)
            AND (? IS NULL OR model = ?)
        "#,
    )
    .bind(tenant_id.to_string())
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.start_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .bind(query.end_time.map(|t| t.to_rfc3339()))
    .bind(model)
    .bind(model)
    .fetch_one(pool)
    .await?;

    Ok(row.try_get("total")?)
}

pub async fn create(
    pool: &DbPool,
    tenant_id: Uuid,
    user_id: Uuid,
    api_key_id: Uuid,
    conversation_id: &str,
    model: &str,
    provider: &str,
    client_ip: &str,
) -> Result<Conversation, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO conversations (id, tenant_id, user_id, api_key_id, conversation_id, model, provider, client_ip)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(id.to_string())
    .bind(tenant_id.to_string())
    .bind(user_id.to_string())
    .bind(api_key_id.to_string())
    .bind(conversation_id)
    .bind(model)
    .bind(provider)
    .bind(client_ip)
    .execute(pool)
    .await?;

    find_by_id(pool, id).await?.ok_or(sqlx::Error::RowNotFound)
}

pub async fn update_tokens(
    pool: &DbPool,
    id: Uuid,
    input_tokens: i64,
    output_tokens: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE conversations SET
            total_input_tokens = total_input_tokens + ?,
            total_output_tokens = total_output_tokens + ?,
            total_tokens = total_tokens + ? + ?
        WHERE id = ?
        "#,
    )
    .bind(input_tokens)
    .bind(output_tokens)
    .bind(input_tokens)
    .bind(output_tokens)
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn end_conversation(pool: &DbPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE conversations SET ended_at = datetime('now') WHERE id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn add_message(
    pool: &DbPool,
    conversation_id: Uuid,
    input_content: &str,
    output_content: &str,
    input_tokens: i64,
    output_tokens: i64,
    model: &str,
    finish_reason: Option<&str>,
) -> Result<ConversationMessage, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO conversation_messages (id, conversation_id, input_content, output_content, input_tokens, output_tokens, model, finish_reason)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(id.to_string())
    .bind(conversation_id.to_string())
    .bind(input_content)
    .bind(output_content)
    .bind(input_tokens)
    .bind(output_tokens)
    .bind(model)
    .bind(finish_reason)
    .execute(pool)
    .await?;

    sqlx::query_as::<_, ConversationMessage>("SELECT * FROM conversation_messages WHERE id = ?")
        .bind(id.to_string())
        .fetch_one(pool)
        .await
}

pub async fn find_messages(
    pool: &DbPool,
    conversation_id: Uuid,
) -> Result<Vec<ConversationMessage>, sqlx::Error> {
    sqlx::query_as::<_, ConversationMessage>(
        "SELECT * FROM conversation_messages WHERE conversation_id = ? ORDER BY created_at ASC",
    )
    .bind(conversation_id.to_string())
    .fetch_all(pool)
    .await
}

pub async fn get_messages_without_content(
    pool: &DbPool,
    conversation_id: Uuid,
) -> Result<Vec<ConversationMessage>, sqlx::Error> {
    sqlx::query_as::<_, ConversationMessage>(
        r#"
        SELECT 
            id, conversation_id, 
            '*****' as input_content,
            '*****' as output_content,
            input_tokens, output_tokens, model, finish_reason, created_at
        FROM conversation_messages 
        WHERE conversation_id = ? 
        ORDER BY created_at ASC
        "#,
    )
    .bind(conversation_id.to_string())
    .fetch_all(pool)
    .await
}
