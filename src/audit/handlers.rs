use crate::store::{AdminState, ProxyLogContent};
use axum::{
    extract::{Query, State},
    http::header,
    response::Response,
    Extension, Json,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    db,
    models::audit::{AuditLog, AuditLogQuery},
    models::SqlUuid,
    store::ProxyLogRecord,
    utils::error::{AppError, AppResult},
};

#[derive(Debug, Deserialize)]
pub struct AuditLogListQuery {
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
    pub user_id: Option<Uuid>,
    pub username: Option<String>,
    pub keyword: Option<String>,
    pub min_tokens: Option<i64>,
    pub max_tokens: Option<i64>,
    pub format: Option<String>,
    pub action: Option<String>,
    pub resource_type: Option<String>,
    pub page: Option<i32>,
    pub page_size: Option<i32>,
}

#[derive(Debug, serde::Serialize)]
pub struct AuditLogListResponse {
    pub items: Vec<AuditLog>,
    pub total: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct ProxyAuditLogItem {
    pub id: SqlUuid,
    pub user_id: SqlUuid,
    pub username: Option<String>,
    pub conversation_id: Option<String>,
    pub model: String,
    pub model_alias: Option<String>,
    pub routed_model: Option<String>,
    pub original_model_name: String,
    pub provider: String,
    pub total_tokens: i64,
    pub status: String,
    pub error_message: Option<String>,
    pub client_ip: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, serde::Serialize)]
pub struct ProxyAuditLogDetail {
    pub id: SqlUuid,
    pub user_id: SqlUuid,
    pub username: Option<String>,
    pub conversation_id: Option<String>,
    pub api_key_id: SqlUuid,
    pub model: String,
    pub model_alias: Option<String>,
    pub routed_model: Option<String>,
    pub original_model_name: String,
    pub provider: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub status: String,
    pub error_message: Option<String>,
    pub request_body: Option<String>,
    pub response_body: Option<String>,
    pub messages: Vec<crate::store::MessageRecord>,
    pub content_deleted: bool,
    pub client_ip: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, serde::Serialize)]
pub struct ProxyAuditLogListResponse {
    pub items: Vec<ProxyAuditLogItem>,
    pub total: i64,
}

struct EnrichedProxyLog {
    log: ProxyLogRecord,
    content: Option<ProxyLogContent>,
    username: Option<String>,
    model_alias: Option<String>,
    original_model_name: String,
}

#[derive(Debug, Serialize)]
struct ExportDatasetMetadata {
    id: SqlUuid,
    conversation_id: Option<String>,
    user_id: SqlUuid,
    username: Option<String>,
    model: String,
    model_alias: Option<String>,
    original_model_name: String,
    provider: String,
    total_tokens: i64,
    input_tokens: i64,
    output_tokens: i64,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct ExportChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct ExportOpenAiRecord {
    messages: Vec<ExportChatMessage>,
    metadata: ExportDatasetMetadata,
}

#[derive(Debug, Serialize)]
struct ExportShareGptMessage {
    from: String,
    value: String,
}

#[derive(Debug, Serialize)]
struct ExportShareGptRecord {
    id: String,
    conversations: Vec<ExportShareGptMessage>,
    metadata: ExportDatasetMetadata,
}

#[derive(Debug, Serialize)]
struct ExportQueryResponseRecord {
    query: String,
    response: String,
    metadata: ExportDatasetMetadata,
}

#[derive(Debug, Serialize)]
struct ExportAlpacaRecord {
    instruction: String,
    input: String,
    output: String,
    metadata: ExportDatasetMetadata,
}

fn matches_proxy_keyword(
    log: &ProxyLogRecord,
    content: &Option<ProxyLogContent>,
    username: Option<&str>,
    model_alias: Option<&str>,
    original_model_name: &str,
    keyword: &str,
) -> bool {
    let needle = keyword.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }

    let message_match = content.as_ref().map(|c| {
        c.messages.iter().any(|message| {
            message.role.to_lowercase().contains(&needle)
                || message.content.to_lowercase().contains(&needle)
        })
    }).unwrap_or(false);

    [
        Some(log.id.to_string()),
        Some(log.user_id.to_string()),
        log.conversation_id.clone(),
        username.map(|value| value.to_string()),
        Some(log.model.clone()),
        model_alias.map(|value| value.to_string()),
        Some(original_model_name.to_string()),
        log.routed_model.clone(),
        Some(log.provider.clone()),
        Some(log.status.clone()),
        log.error_message.clone(),
        Some(log.client_ip.clone()),
    ]
    .into_iter()
    .flatten()
    .any(|value| value.to_lowercase().contains(&needle))
        || message_match
}

fn normalize_export_role(role: &str) -> String {
    match role {
        "user" => "human".to_string(),
        "assistant" => "gpt".to_string(),
        "system" => "system".to_string(),
        other => other.to_string(),
    }
}

fn is_exportable_training_log(content: &Option<ProxyLogContent>) -> bool {
    content.as_ref().map(|c| {
        !c.messages.is_empty()
            && c.messages.iter().any(|message| message.role == "assistant")
            && c.messages.iter().any(|message| message.role == "user")
    }).unwrap_or(false)
}

fn build_export_metadata(item: &EnrichedProxyLog) -> ExportDatasetMetadata {
    ExportDatasetMetadata {
        id: item.log.id,
        conversation_id: item.log.conversation_id.clone(),
        user_id: item.log.user_id,
        username: item.username.clone(),
        model: item.log.model.clone(),
        model_alias: item.model_alias.clone(),
        original_model_name: item.original_model_name.clone(),
        provider: item.log.provider.clone(),
        total_tokens: item.log.total_tokens,
        input_tokens: item.log.input_tokens,
        output_tokens: item.log.output_tokens,
        created_at: item.log.created_at,
    }
}

fn join_non_empty_messages(contents: Vec<String>) -> String {
    contents
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn build_query_response_payload(content: &ProxyLogContent) -> Option<(String, String)> {
    let query = join_non_empty_messages(
        content.messages
            .iter()
            .filter(|message| message.role == "system" || message.role == "user")
            .map(|message| {
                if message.role == "system" {
                    format!("System:\n{}", message.content)
                } else {
                    message.content.clone()
                }
            })
            .collect(),
    );
    let response = join_non_empty_messages(
        content.messages
            .iter()
            .filter(|message| message.role == "assistant")
            .map(|message| message.content.clone())
            .collect(),
    );

    if query.is_empty() || response.is_empty() {
        None
    } else {
        Some((query, response))
    }
}

fn build_alpaca_payload(content: &ProxyLogContent) -> Option<(String, String, String)> {
    let instruction = join_non_empty_messages(
        content.messages
            .iter()
            .filter(|message| message.role == "system")
            .map(|message| message.content.clone())
            .collect(),
    );
    let input = join_non_empty_messages(
        content.messages
            .iter()
            .filter(|message| message.role == "user")
            .map(|message| message.content.clone())
            .collect(),
    );
    let output = join_non_empty_messages(
        content.messages
            .iter()
            .filter(|message| message.role == "assistant")
            .map(|message| message.content.clone())
            .collect(),
    );

    if output.is_empty() || (instruction.is_empty() && input.is_empty()) {
        None
    } else {
        Some((instruction, input, output))
    }
}

fn resolve_effective_time_range(
    query: &AuditLogListQuery,
) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    if query.start_time.is_none() && query.end_time.is_none() {
        let end = Utc::now();
        let start = end - Duration::days(30);
        (Some(start), Some(end))
    } else {
        (query.start_time, query.end_time)
    }
}

fn resolve_effective_user_id(auth_user: &AuthUser, query: &AuditLogListQuery) -> Option<Uuid> {
    if auth_user.is_admin() {
        query.user_id
    } else {
        Some(auth_user.user_id)
    }
}

fn has_text_filters(query: &AuditLogListQuery) -> bool {
    query
        .username
        .as_ref()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
        || query
            .keyword
            .as_ref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
}

async fn enrich_proxy_log(
    state: &AdminState,
    log: ProxyLogRecord,
    username_cache: &mut HashMap<Uuid, Option<String>>,
    model_cache: &mut HashMap<String, (Option<String>, String)>,
    load_content: bool,
) -> EnrichedProxyLog {
    let user_id: Uuid = log.user_id.into();
    let username = if let Some(cached) = username_cache.get(&user_id) {
        cached.clone()
    } else {
        let value = state.store.get_user(user_id).await.map(|u| u.username);
        username_cache.insert(user_id, value.clone());
        value
    };

    let (model_alias, original_model_name) = if let Some(cached) = model_cache.get(&log.model) {
        cached.clone()
    } else {
        let value = state.store.resolve_model_display(&log.model).await;
        model_cache.insert(log.model.clone(), value.clone());
        value
    };

    let content = if load_content {
        if let Some(ref log_file) = log.log_file {
            if let Some(ref writer) = state.store.get_proxy_log_writer() {
                let log_id: Uuid = log.id.into();
                writer.read_content(log_file, log_id).ok().flatten()
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    EnrichedProxyLog {
        log,
        content,
        username,
        model_alias,
        original_model_name,
    }
}

async fn scan_filtered_proxy_logs(
    state: &AdminState,
    auth_user: &AuthUser,
    query: &AuditLogListQuery,
    page: Option<(usize, usize)>,
) -> AppResult<(i64, Vec<EnrichedProxyLog>)> {
    let (effective_start_time, effective_end_time) = resolve_effective_time_range(query);
    let effective_user_id = resolve_effective_user_id(auth_user, query);
    let username_filter = query
        .username
        .as_ref()
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty());
    let keyword_filter = query
        .keyword
        .as_ref()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    let (page_start, page_end) = if let Some((page, page_size)) = page {
        let start = page.saturating_sub(1).saturating_mul(page_size);
        (Some(start), Some(start + page_size))
    } else {
        (None, None)
    };

    let mut username_cache: HashMap<Uuid, Option<String>> = HashMap::new();
    let mut model_cache: HashMap<String, (Option<String>, String)> = HashMap::new();
    let mut total_matched: i64 = 0;
    let mut items: Vec<EnrichedProxyLog> = Vec::new();
    let mut offset: usize = 0;
    let batch_size: usize = 1000;

    loop {
        let batch = state
            .store
            .list_proxy_logs_filtered(
                Some(auth_user.tenant_id),
                effective_start_time,
                effective_end_time,
                effective_user_id,
                query.min_tokens,
                query.max_tokens,
                offset,
                batch_size,
            )
            .await?;

        if batch.is_empty() {
            break;
        }

        for log in batch.iter().cloned() {
            let enriched =
                enrich_proxy_log(state, log, &mut username_cache, &mut model_cache, true).await;

            if let Some(ref user_filter) = username_filter {
                let candidate = enriched.username.clone().unwrap_or_default().to_lowercase();
                if candidate != *user_filter {
                    continue;
                }
            }

            if let Some(ref keyword) = keyword_filter {
                if !matches_proxy_keyword(
                    &enriched.log,
                    &enriched.content,
                    enriched.username.as_deref(),
                    enriched.model_alias.as_deref(),
                    &enriched.original_model_name,
                    keyword,
                ) {
                    continue;
                }
            }

            let current_index = total_matched as usize;
            total_matched += 1;

            match (page_start, page_end) {
                (Some(start), Some(end)) => {
                    if current_index >= start && current_index < end {
                        items.push(enriched);
                    }
                }
                _ => items.push(enriched),
            }
        }

        offset += batch.len();
        if batch.len() < batch_size {
            break;
        }
    }

    Ok((total_matched, items))
}

pub async fn list_audit_logs(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Query(query): Query<AuditLogListQuery>,
) -> AppResult<Json<AuditLogListResponse>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(50).max(1);

    let total = db::audit::count(state.store.clone(), Some(auth_user.tenant_id), &query).await?;

    let audit_query = AuditLogQuery {
        start_time: query.start_time,
        end_time: query.end_time,
        user_id: query.user_id,
        action: query.action,
        resource_type: query.resource_type,
        page,
        page_size,
    };

    let items: Vec<AuditLog> =
        db::audit::list(state.store.clone(), Some(auth_user.tenant_id), audit_query).await?;

    Ok(Json(AuditLogListResponse { items, total }))
}

pub async fn get_audit_log(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    path: axum::extract::Path<Uuid>,
) -> AppResult<Json<AuditLog>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let id = path.0;
    let log: AuditLog = db::audit::find_by_id(state.store.clone(), id)
        .await?
        .ok_or_else(|| AppError::NotFound("Audit log not found".to_string()))?;

    Ok(Json(log))
}

pub async fn list_proxy_audit_logs(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Query(query): Query<AuditLogListQuery>,
) -> AppResult<Json<ProxyAuditLogListResponse>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }
    let page_size = query.page_size.unwrap_or(50).max(1) as usize;
    let page = query.page.unwrap_or(1).max(1) as usize;
    let mut items: Vec<ProxyAuditLogItem> = Vec::new();

    if has_text_filters(&query) {
        let (total, filtered) =
            scan_filtered_proxy_logs(&state, &auth_user, &query, Some((page, page_size))).await?;
        for item in filtered.iter() {
            let log = item.log.clone();
            items.push(ProxyAuditLogItem {
                id: log.id,
                user_id: log.user_id,
                username: item.username.clone(),
                conversation_id: log.conversation_id,
                model: log.model,
                model_alias: item.model_alias.clone(),
                routed_model: log.routed_model,
                original_model_name: item.original_model_name.clone(),
                provider: log.provider,
                total_tokens: log.total_tokens,
                status: log.status,
                error_message: log.error_message,
                client_ip: log.client_ip,
                created_at: log.created_at,
            });
        }
        return Ok(Json(ProxyAuditLogListResponse { total, items }));
    }

    let (effective_start_time, effective_end_time) = resolve_effective_time_range(&query);
    let effective_user_id = resolve_effective_user_id(&auth_user, &query);
    let offset = page.saturating_sub(1).saturating_mul(page_size);

    tracing::info!(
        "list_proxy_audit_logs: tenant_id={}, start={:?}, end={:?}, user_id={:?}, page={}, page_size={}",
        auth_user.tenant_id, effective_start_time, effective_end_time, effective_user_id, page, page_size
    );

    let total = state
        .store
        .count_proxy_logs_filtered(
            Some(auth_user.tenant_id),
            effective_start_time,
            effective_end_time,
            effective_user_id,
            query.min_tokens,
            query.max_tokens,
        )
        .await?;
    let logs = state
        .store
        .list_proxy_logs_filtered(
            Some(auth_user.tenant_id),
            effective_start_time,
            effective_end_time,
            effective_user_id,
            query.min_tokens,
            query.max_tokens,
            offset,
            page_size,
        )
        .await?;

    let mut username_cache: HashMap<Uuid, Option<String>> = HashMap::new();
    let mut model_cache: HashMap<String, (Option<String>, String)> = HashMap::new();
    for log in logs {
        let enriched = enrich_proxy_log(&state, log, &mut username_cache, &mut model_cache, false).await;
        let record = enriched.log;
        items.push(ProxyAuditLogItem {
            id: record.id,
            user_id: record.user_id,
            username: enriched.username,
            conversation_id: record.conversation_id,
            model: record.model,
            model_alias: enriched.model_alias,
            routed_model: record.routed_model,
            original_model_name: enriched.original_model_name,
            provider: record.provider,
            total_tokens: record.total_tokens,
            status: record.status,
            error_message: record.error_message,
            client_ip: record.client_ip,
            created_at: record.created_at,
        });
    }

    Ok(Json(ProxyAuditLogListResponse { total, items }))
}

pub async fn export_proxy_audit_logs(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Query(query): Query<AuditLogListQuery>,
) -> AppResult<Response> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }
    let export_format = query
        .format
        .clone()
        .unwrap_or_else(|| "openai_jsonl".to_string());
    let (_, filtered) = scan_filtered_proxy_logs(&state, &auth_user, &query, None).await?;
    let exportable: Vec<EnrichedProxyLog> = filtered
        .into_iter()
        .filter(|item| item.log.status == "success" && is_exportable_training_log(&item.content))
        .collect();

    let timestamp = Utc::now().format("%Y%m%d-%H%M%S");

    match export_format.as_str() {
        "sharegpt_json" => {
            let dataset: Vec<ExportShareGptRecord> = exportable
                .into_iter()
                .filter_map(|item| {
                    let content = item.content.as_ref()?;
                    Some(ExportShareGptRecord {
                        id: item.log.id.to_string(),
                        conversations: content
                            .messages
                            .iter()
                            .map(|message| ExportShareGptMessage {
                                from: normalize_export_role(&message.role),
                                value: message.content.clone(),
                            })
                            .collect(),
                        metadata: build_export_metadata(&item),
                    })
                })
                .collect();
            let body = serde_json::to_string_pretty(&dataset)
                .map_err(|e| AppError::Internal(format!("Failed to serialize export dataset: {}", e)))?;
            let filename = format!("audit-dataset-sharegpt-{}.json", timestamp);

            Ok(Response::builder()
                .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
                .header(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", filename),
                )
                .body(body.into())
                .map_err(|e| AppError::Internal(format!("Failed to build export response: {}", e)))?)
        }
        "query_response_jsonl" => {
            let lines: Result<Vec<String>, AppError> = exportable
                .into_iter()
                .filter_map(|item| {
                    item.content.as_ref().and_then(|c| {
                        build_query_response_payload(c).map(|(query, response)| {
                            serde_json::to_string(&ExportQueryResponseRecord {
                                query,
                                response,
                                metadata: build_export_metadata(&item),
                            })
                            .map_err(|e| {
                                AppError::Internal(format!(
                                    "Failed to serialize query-response dataset line: {}",
                                    e
                                ))
                            })
                        })
                    })
                })
                .collect();
            let body = lines?.join("\n");
            let filename = format!("audit-dataset-query-response-{}.jsonl", timestamp);

            Ok(Response::builder()
                .header(header::CONTENT_TYPE, "application/x-ndjson; charset=utf-8")
                .header(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", filename),
                )
                .body(body.into())
                .map_err(|e| AppError::Internal(format!("Failed to build export response: {}", e)))?)
        }
        "alpaca_json" => {
            let dataset: Vec<ExportAlpacaRecord> = exportable
                .into_iter()
                .filter_map(|item| {
                    item.content.as_ref().and_then(|c| {
                        build_alpaca_payload(c).map(|(instruction, input, output)| {
                            ExportAlpacaRecord {
                                instruction,
                                input,
                                output,
                                metadata: build_export_metadata(&item),
                            }
                        })
                    })
                })
                .collect();
            let body = serde_json::to_string_pretty(&dataset).map_err(|e| {
                AppError::Internal(format!("Failed to serialize alpaca export dataset: {}", e))
            })?;
            let filename = format!("audit-dataset-alpaca-{}.json", timestamp);

            Ok(Response::builder()
                .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
                .header(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", filename),
                )
                .body(body.into())
                .map_err(|e| AppError::Internal(format!("Failed to build export response: {}", e)))?)
        }
        "openai_jsonl" => {
            let lines: Result<Vec<String>, AppError> = exportable
                .into_iter()
                .filter_map(|item| {
                    let content = item.content.as_ref()?;
                    Some(serde_json::to_string(&ExportOpenAiRecord {
                        messages: content
                            .messages
                            .iter()
                            .map(|message| ExportChatMessage {
                                role: message.role.clone(),
                                content: message.content.clone(),
                            })
                            .collect(),
                        metadata: build_export_metadata(&item),
                    })
                    .map_err(|e| {
                        AppError::Internal(format!("Failed to serialize export dataset line: {}", e))
                    }))
                })
                .collect();
            let body = lines?.join("\n");
            let filename = format!("audit-dataset-openai-{}.jsonl", timestamp);

            Ok(Response::builder()
                .header(header::CONTENT_TYPE, "application/x-ndjson; charset=utf-8")
                .header(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", filename),
                )
                .body(body.into())
                .map_err(|e| AppError::Internal(format!("Failed to build export response: {}", e)))?)
        }
        _ => Err(AppError::BadRequest("Unsupported export format".to_string())),
    }
}

pub async fn get_proxy_audit_log(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    path: axum::extract::Path<Uuid>,
) -> AppResult<Json<ProxyAuditLogDetail>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }
    let id = path.0;
    let record = state
        .store
        .find_proxy_log_by_id(Some(auth_user.tenant_id), id)
        .await?
        .ok_or_else(|| AppError::NotFound("Proxy audit log not found".to_string()))?;

    let (model_alias, original_model_name) = state.store.resolve_model_display(&record.model).await;

    let (request_body, response_body, messages, content_deleted) =
        if let Some(ref log_file) = record.log_file {
            if let Some(ref writer) = state.store.get_proxy_log_writer() {
                match writer.read_content(log_file, id) {
                    Ok(Some(content)) => (content.request_body, content.response_body, content.messages, false),
                    Ok(None) => (None, None, Vec::new(), true),
                    Err(e) => {
                        tracing::warn!("Failed to read proxy log content from file: {}", e);
                        (None, None, Vec::new(), true)
                    }
                }
            } else {
                (None, None, Vec::new(), true)
            }
        } else {
            (None, None, Vec::new(), true)
        };

    Ok(Json(ProxyAuditLogDetail {
        id: record.id,
        user_id: record.user_id,
        username: state
            .store
            .get_user(record.user_id.into())
            .await
            .map(|u| u.username),
        conversation_id: record.conversation_id,
        api_key_id: record.api_key_id,
        model: record.model,
        model_alias,
        routed_model: record.routed_model,
        original_model_name,
        provider: record.provider,
        input_tokens: record.input_tokens,
        output_tokens: record.output_tokens,
        total_tokens: record.total_tokens,
        status: record.status,
        error_message: record.error_message,
        request_body,
        response_body,
        messages,
        content_deleted,
        client_ip: record.client_ip,
        created_at: record.created_at,
    }))
}
