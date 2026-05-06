#![allow(dead_code)]
use crate::db::DbPool;
use crate::models::SqlUuid;
use crate::proxy::UpstreamRateLimiter;
use crate::utils::error::AppError;
use crate::utils::secrets::decrypt_upstream_api_key;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::{QueryBuilder, Sqlite};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;
use uuid::Uuid;

pub mod file_audit_log;
pub use file_audit_log::{AuditLogEntry, FileAuditLogWriter};

pub mod proxy_log_writer;
pub use proxy_log_writer::{ProxyLogContent, ProxyLogWriter};

#[derive(Clone)]
pub struct AdminState {
    pub pool: DbPool,
    pub store: Arc<StoreManager>,
    pub upstream_rate_limiter: Arc<UpstreamRateLimiter>,
}

impl AdminState {
    pub fn new(
        pool: DbPool,
        store: Arc<StoreManager>,
        upstream_rate_limiter: Arc<UpstreamRateLimiter>,
    ) -> Self {
        Self {
            pool,
            store,
            upstream_rate_limiter,
        }
    }
}

#[derive(Clone, Debug)]
struct UserQuotaCount {
    daily_count: i64,
    monthly_count: i64,
    daily_date: String,
    monthly_date: String,
}

#[derive(Clone)]
pub struct StoreManager {
    pub pool: DbPool,
    api_keys: DashMap<String, ApiKeyCache>,
    users: DashMap<Uuid, UserCache>,
    upstreams: DashMap<Uuid, UpstreamCache>,
    tenant_upstreams: DashMap<Uuid, Vec<Uuid>>,
    model_visibility: DashMap<(Uuid, String), ModelVisibilityCache>,
    conditional_alias_routes: DashMap<(Uuid, String), Vec<ConditionalAliasRouteCache>>,
    audit_log_writer: Option<FileAuditLogWriter>,
    proxy_log_writer: Option<ProxyLogWriter>,
    proxy_log_columns_checked: Arc<AtomicBool>,
    last_used_at_buffer: Arc<Mutex<Vec<Uuid>>>,
    user_quota_counts: DashMap<Uuid, UserQuotaCount>,
}


fn normalize_ollama_model_name(model: &str) -> &str {
    model.strip_suffix(":latest").unwrap_or(model)
}

fn model_name_matches(candidate: &str, requested: &str, is_ollama: bool) -> bool {
    if candidate == requested {
        return true;
    }
    if !is_ollama {
        return false;
    }
    normalize_ollama_model_name(candidate) == normalize_ollama_model_name(requested)
}

impl StoreManager {
    fn model_visibility_allows_user(visibility: &ModelVisibilityCache, user_id: Uuid) -> bool {
        if visibility.all_users_visible {
            return true;
        }
        visibility.allowed_users.contains(&user_id)
    }

    pub fn new(pool: DbPool) -> Self {
        Self {
            pool,
            api_keys: DashMap::new(),
            users: DashMap::new(),
            upstreams: DashMap::new(),
            tenant_upstreams: DashMap::new(),
            model_visibility: DashMap::new(),
            conditional_alias_routes: DashMap::new(),
            audit_log_writer: None,
            proxy_log_writer: None,
            proxy_log_columns_checked: Arc::new(AtomicBool::new(false)),
            last_used_at_buffer: Arc::new(Mutex::new(Vec::new())),
            user_quota_counts: DashMap::new(),
        }
    }

    pub fn with_audit_log_writer(mut self, path: &str) -> Result<Self, AppError> {
        self.audit_log_writer = Some(FileAuditLogWriter::new(path)?);
        Ok(self)
    }

    pub fn with_proxy_log_writer(mut self, path: &str) -> Result<Self, AppError> {
        self.proxy_log_writer = Some(ProxyLogWriter::new(path)?);
        Ok(self)
    }

    pub fn get_proxy_log_writer(&self) -> Option<&ProxyLogWriter> {
        self.proxy_log_writer.as_ref()
    }

    pub async fn init_from_sqlite(&self, pool: &DbPool) -> Result<(), AppError> {
        self.reload_api_keys(pool).await?;
        self.reload_users(pool).await?;
        self.reload_upstreams(pool).await?;
        self.reload_model_visibility(pool).await?;
        self.reload_conditional_alias_routes(pool).await?;
        self.ensure_proxy_log_columns_once().await?;
        Ok(())
    }

    pub async fn resolve_model_display(&self, model: &str) -> (Option<String>, String) {
        for entry in self.model_visibility.iter() {
            if entry.key().1 == model {
                if let Some(alias) = entry.value().model_aliases.first() {
                    return (Some(alias.clone()), model.to_string());
                }
                return (None, model.to_string());
            }
        }
        (None, model.to_string())
    }

    pub async fn get_visible_models(&self, user_id: Uuid, tenant_id: Uuid) -> Vec<ModelInfo> {
        let mut models: Vec<ModelInfo> = Vec::new();
        let mut seen_models: std::collections::HashSet<String> = std::collections::HashSet::new();

        for entry in self.model_visibility.iter() {
            let (upstream_id, model_name) = entry.key();
            let vis = entry.value();
            if let Some(upstream) = self.upstreams.get(upstream_id) {
                if upstream.tenant_id != tenant_id {
                    continue;
                }
                if Self::model_visibility_allows_user(vis, user_id) {
                    if seen_models.insert(model_name.clone()) {
                        models.push(ModelInfo {
                            id: vis.model_aliases.first().cloned().unwrap_or_else(|| model_name.clone()),
                            object: "model".to_string(),
                            created: vis.created_at.timestamp(),
                            owned_by: upstream.provider.clone(),
                        });
                    }
                    for alias in &vis.model_aliases {
                        if seen_models.insert(alias.clone()) {
                            models.push(ModelInfo {
                                id: alias.clone(),
                                object: "model".to_string(),
                                created: vis.created_at.timestamp(),
                                owned_by: upstream.provider.clone(),
                            });
                        }
                    }
                }
            }
        }

        for entry in self.upstreams.iter() {
            let upstream = entry.value();
            if upstream.tenant_id != tenant_id {
                continue;
            }
            for model_name in &upstream.models {
                if seen_models.insert(model_name.clone()) {
                    models.push(ModelInfo {
                        id: model_name.clone(),
                        object: "model".to_string(),
                        created: upstream.created_at.timestamp(),
                        owned_by: upstream.provider.clone(),
                    });
                }
            }
        }

        for entry in self.conditional_alias_routes.iter() {
            let (route_tenant_id, alias) = entry.key();
            let routes = entry.value();
            if *route_tenant_id != tenant_id {
                continue;
            }
            if routes.is_empty() {
                continue;
            }
            let first_route = &routes[0];
            if !first_route.is_visible_to(user_id) {
                continue;
            }
            if seen_models.insert(alias.clone()) {
                if let Some(upstream) = self.upstreams.get(&first_route.upstream_id) {
                    models.push(ModelInfo {
                        id: alias.clone(),
                        object: "model".to_string(),
                        created: upstream.created_at.timestamp(),
                        owned_by: upstream.provider.clone(),
                    });
                }
            }
        }

        models
    }

    pub async fn reload_api_keys_cache(&self, pool: &DbPool) -> Result<(), AppError> {
        self.reload_api_keys(pool).await
    }

    pub async fn reload_users_cache(&self, pool: &DbPool) -> Result<(), AppError> {
        self.reload_users(pool).await
    }

    pub async fn reload_upstreams_cache(&self, pool: &DbPool) -> Result<(), AppError> {
        self.reload_upstreams(pool).await
    }

    pub fn update_model_visibility(&self, cache: HashMap<(Uuid, String), ModelVisibilityCache>) {
        self.model_visibility.clear();
        for (key, value) in cache {
            self.model_visibility.insert(key, value);
        }
    }

    pub fn update_model_visibility_entry(&self, key: (Uuid, String), value: ModelVisibilityCache) {
        self.model_visibility.insert(key, value);
    }

    pub async fn get_api_key(&self, key_hash: &str) -> Result<ApiKeyCache, AppError> {
        let api_key = self
            .api_keys
            .get(key_hash)
            .map(|r| r.value().clone())
            .ok_or(AppError::Unauthorized("Invalid API key".to_string()))?;

        if let Some(expires_at) = api_key.expires_at {
            if expires_at < Utc::now() {
                return Err(AppError::Unauthorized("API key has expired".to_string()));
            }
        }
        {
            let mut buffer = self.last_used_at_buffer.lock().await;
            buffer.push(api_key.id);
            if buffer.len() >= 50 {
                let ids: Vec<Uuid> = buffer.drain(..).collect();
                let pool = self.pool.clone();
                tokio::spawn(async move {
                    for id in ids {
                        let _ = sqlx::query("UPDATE api_keys SET last_used_at = datetime('now') WHERE id = ?")
                            .bind(id.to_string())
                            .execute(&pool)
                            .await;
                    }
                });
            }
        }
        Ok(api_key)
    }

    pub fn add_api_key(&self, api_key: ApiKeyCache) {
        self.api_keys.insert(api_key.key_hash.clone(), api_key);
    }

    pub fn remove_api_key(&self, key_hash: &str) {
        self.api_keys.remove(key_hash);
    }

    pub async fn flush_last_used_at(&self) {
        let ids: Vec<Uuid> = {
            let mut buffer = self.last_used_at_buffer.lock().await;
            buffer.drain(..).collect()
        };
        if ids.is_empty() {
            return;
        }
        let pool = self.pool.clone();
        for id in ids {
            let _ = sqlx::query("UPDATE api_keys SET last_used_at = datetime('now') WHERE id = ?")
                .bind(id.to_string())
                .execute(&pool)
                .await;
        }
    }

    pub async fn get_user(&self, user_id: Uuid) -> Option<UserCache> {
        self.users.get(&user_id).map(|r| r.value().clone())
    }

    pub fn add_user(&self, user: UserCache) {
        self.users.insert(user.id, user);
    }

    pub fn remove_user(&self, user_id: Uuid) {
        self.users.remove(&user_id);
        self.user_quota_counts.remove(&user_id);
    }

    pub async fn check_user_request_quota(&self, user: &UserCache) -> Result<(), AppError> {
        if user.daily_request_limit <= 0 && user.monthly_request_limit <= 0 {
            return Ok(());
        }

        let now = Utc::now();
        let today = now.format("%Y-%m-%d").to_string();
        let this_month = now.format("%Y-%m").to_string();

        let mut entry = self.user_quota_counts.entry(user.id).or_insert_with(|| {
            UserQuotaCount {
                daily_count: 0,
                monthly_count: 0,
                daily_date: today.clone(),
                monthly_date: this_month.clone(),
            }
        });

        if entry.daily_date != today {
            entry.daily_count = 0;
            entry.daily_date = today.clone();
        }
        if entry.monthly_date != this_month {
            entry.monthly_count = 0;
            entry.monthly_date = this_month.clone();
        }

        if user.daily_request_limit > 0 && entry.daily_count >= user.daily_request_limit {
            return Err(AppError::RateLimitExceeded(format!(
                "User daily request limit exceeded: {}",
                user.daily_request_limit
            )));
        }
        if user.monthly_request_limit > 0 && entry.monthly_count >= user.monthly_request_limit {
            return Err(AppError::RateLimitExceeded(format!(
                "User monthly request limit exceeded: {}",
                user.monthly_request_limit
            )));
        }

        entry.daily_count += 1;
        entry.monthly_count += 1;

        Ok(())
    }

    pub fn get_user_quota_usage(&self, user_id: Uuid) -> (i64, i64) {
        let now = Utc::now();
        let today = now.format("%Y-%m-%d").to_string();
        let this_month = now.format("%Y-%m").to_string();

        if let Some(entry) = self.user_quota_counts.get(&user_id) {
            let daily = if entry.daily_date == today {
                entry.daily_count
            } else {
                0
            };
            let monthly = if entry.monthly_date == this_month {
                entry.monthly_count
            } else {
                0
            };
            (daily, monthly)
        } else {
            (0, 0)
        }
    }

    pub async fn get_upstream(&self, upstream_id: Uuid) -> Option<UpstreamCache> {
        self.upstreams.get(&upstream_id).map(|r| r.value().clone())
    }

    pub fn add_upstream(&self, upstream: UpstreamCache) {
        let upstream_id = upstream.id;
        let tenant_id = upstream.tenant_id;
        self.upstreams.insert(upstream_id, upstream);
        self.tenant_upstreams.entry(tenant_id).or_default().push(upstream_id);
    }

    pub fn remove_upstream(&self, upstream_id: Uuid) {
        if let Some((_, upstream)) = self.upstreams.remove(&upstream_id) {
            if let Some(mut entry) = self.tenant_upstreams.get_mut(&upstream.tenant_id) {
                entry.retain(|id| *id != upstream_id);
            }
        }
    }

    pub async fn get_tenant_upstreams(&self, tenant_id: Uuid) -> Vec<UpstreamCache> {
        let upstream_ids = self.tenant_upstreams.get(&tenant_id).map(|r| r.value().clone()).unwrap_or_default();
        upstream_ids
            .into_iter()
            .filter_map(|id| self.upstreams.get(&id).map(|r| r.value().clone()))
            .collect()
    }

    pub async fn reload_api_keys(&self, pool: &DbPool) -> Result<(), AppError> {
        let rows = sqlx::query_as::<_, ApiKeyRow>(
            r#"
            SELECT ak.id, ak.tenant_id, ak.user_id, ak.key_hash, ak.name, ak.expires_at, ak.last_used_at, ak.created_at, u.role, ak.rpm_limit, ak.tpm_limit, ak.daily_limit
            FROM api_keys ak
            JOIN users u ON ak.user_id = u.id
            WHERE ak.status = 'active'
            "#,
        )
        .fetch_all(pool)
        .await?;

        let mut new_cache = HashMap::new();
        for row in rows {
            new_cache.insert(
                row.key_hash.clone(),
                ApiKeyCache {
                    id: row.id.into(),
                    tenant_id: row.tenant_id.into(),
                    user_id: row.user_id.into(),
                    key_hash: row.key_hash,
                    name: row.name,
                    expires_at: row.expires_at,
                    last_used_at: row.last_used_at,
                    created_at: row.created_at,
                    user_role: row.role,
                    rpm_limit: row.rpm_limit.unwrap_or(0),
                    tpm_limit: row.tpm_limit.unwrap_or(0),
                    daily_limit: row.daily_limit.unwrap_or(0),
                },
            );
        }
        self.api_keys.clear();
        for (key, value) in new_cache {
            self.api_keys.insert(key, value);
        }
        Ok(())
    }

    pub async fn reload_users(&self, pool: &DbPool) -> Result<(), AppError> {
        let rows = sqlx::query_as::<_, UserRow>(
            "SELECT id, tenant_id, username, email, role, daily_request_limit, monthly_request_limit FROM users WHERE status = 'active'",
        )
        .fetch_all(pool)
        .await?;

        let mut new_cache = HashMap::new();
        for row in rows {
            new_cache.insert(
                row.id.into(),
                UserCache {
                    id: row.id.into(),
                    tenant_id: row.tenant_id.into(),
                    username: row.username,
                    email: row.email,
                    role: row.role,
                    daily_request_limit: row.daily_request_limit,
                    monthly_request_limit: row.monthly_request_limit,
                },
            );
        }
        self.users.clear();
        for (key, value) in new_cache {
            self.users.insert(key, value);
        }
        let active_user_ids: std::collections::HashSet<Uuid> = self.users.iter().map(|r| *r.key()).collect();
        self.user_quota_counts.retain(|id, _| active_user_ids.contains(id));
        Ok(())
    }

    pub async fn reload_upstreams(&self, pool: &DbPool) -> Result<(), AppError> {
        let rows = sqlx::query_as::<_, UpstreamRow>(
            r#"
            SELECT id, tenant_id, name, provider, api_type, base_url, api_key_encrypted as api_key, models, custom_headers, status, priority, weight, rate_limit, created_at, updated_at, COALESCE(daily_request_limit, 0) as daily_request_limit, COALESCE(monthly_request_limit, 0) as monthly_request_limit
            FROM upstream_configs
            WHERE status = 'active'
            ORDER BY priority ASC, weight DESC
            "#,
        )
        .fetch_all(pool)
        .await?;

        let mut new_cache: HashMap<Uuid, UpstreamCache> = HashMap::new();
        let mut tenant_upstreams: HashMap<Uuid, Vec<Uuid>> = HashMap::new();

        for row in rows {
            let api_key: Option<String> = match row.api_key {
                Some(encrypted) => {
                    match decrypt_upstream_api_key(&encrypted) {
                        Ok(key) => Some(key),
                        Err(e) => {
                            tracing::error!(
                                "Failed to decrypt upstream API key for '{}' ({}): {}. Clearing key.",
                                row.name, row.id, e
                            );
                            None
                        }
                    }
                }
                None => None,
            };

            let models: Vec<String> = row
                .models
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            let upstream = UpstreamCache {
                id: row.id.into(),
                tenant_id: row.tenant_id.into(),
                name: row.name,
                provider: row.provider,
                api_type: row.api_type,
                base_url: row.base_url,
                api_key,
                models,
                custom_headers: row.custom_headers,
                status: row.status,
                priority: row.priority,
                weight: row.weight,
                rate_limit: row.rate_limit,
                created_at: row.created_at,
                updated_at: row.updated_at,
                daily_request_limit: row.daily_request_limit,
                monthly_request_limit: row.monthly_request_limit,
            };

            new_cache.insert(row.id.into(), upstream.clone());
            tenant_upstreams.entry(row.tenant_id.into()).or_default().push(row.id.into());
        }

        self.upstreams.clear();
        for (key, value) in new_cache {
            self.upstreams.insert(key, value);
        }
        self.tenant_upstreams.clear();
        for (key, value) in tenant_upstreams {
            self.tenant_upstreams.insert(key, value);
        }
        Ok(())
    }

    pub async fn reload_model_visibility(&self, pool: &DbPool) -> Result<(), AppError> {
        let rows = sqlx::query(
            r#"
            SELECT id, upstream_id, model_name, model_alias, model_headers, all_users_visible,
                   retry_count, retry_interval_seconds, retry_backoff_strategy, retry_max_interval_seconds,
                   retry_failure_strategy, retry_fallback_upstream_id, retry_fallback_model_name,
                   created_at, updated_at
            FROM model_visibility
            "#,
        )
        .fetch_all(pool)
        .await?;

        let mut new_cache = HashMap::new();
        for row in rows {
            let id_str: String = row.try_get("id")?;
            let upstream_id_str: String = row.try_get("upstream_id")?;
            let id = Uuid::parse_str(&id_str).map_err(|e| AppError::Internal(format!("Invalid UUID: {}", e)))?;
            let upstream_id = Uuid::parse_str(&upstream_id_str).map_err(|e| AppError::Internal(format!("Invalid UUID: {}", e)))?;
            let model_name: String = row.try_get("model_name")?;
            let model_alias: Option<String> = row.try_get("model_alias")?;
            let model_aliases: Vec<String> = model_alias
                .as_ref()
                .map(|s| {
                    s.split(',')
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();
            let model_headers: serde_json::Value = row.try_get("model_headers")?;
            let all_users_visible: bool = row.try_get("all_users_visible")?;
            let retry_count: i64 = row.try_get("retry_count").unwrap_or(0);
            let retry_interval_seconds: i64 = row.try_get("retry_interval_seconds").unwrap_or(0);
            let retry_backoff_strategy: String = row.try_get("retry_backoff_strategy").unwrap_or_else(|_| "fixed".to_string());
            let retry_max_interval_seconds: i64 = row.try_get("retry_max_interval_seconds").unwrap_or(0);
            let retry_failure_strategy: String = row.try_get("retry_failure_strategy").unwrap_or_else(|_| "error".to_string());
            let retry_fallback_upstream_id_str: Option<String> = row.try_get("retry_fallback_upstream_id").unwrap_or(None);
            let retry_fallback_upstream_id = retry_fallback_upstream_id_str.and_then(|s| Uuid::parse_str(&s).ok());
            let retry_fallback_model_name: Option<String> = row.try_get("retry_fallback_model_name").unwrap_or(None);
            let created_at: DateTime<Utc> = row.try_get("created_at")?;
            let updated_at: DateTime<Utc> = row.try_get("updated_at")?;
            let allowed_users = sqlx::query_scalar::<_, String>(
                "SELECT user_id FROM model_visibility_users WHERE visibility_id = ?",
            )
            .bind(id_str)
            .fetch_all(pool)
            .await?
            .into_iter()
            .filter_map(|s| Uuid::parse_str(&s).ok())
            .collect();
            new_cache.insert(
                (upstream_id, model_name.clone()),
                ModelVisibilityCache {
                    id,
                    upstream_id,
                    model_name,
                    model_aliases,
                    model_headers,
                    all_users_visible,
                    allowed_users,
                    retry_count,
                    retry_interval_seconds,
                    retry_backoff_strategy,
                    retry_max_interval_seconds,
                    retry_failure_strategy,
                    retry_fallback_upstream_id,
                    retry_fallback_model_name,
                    created_at,
                    updated_at,
                },
            );
        }
        self.model_visibility.clear();
        for (key, value) in new_cache {
            self.model_visibility.insert(key, value);
        }
        Ok(())
    }

    pub async fn reload_conditional_alias_routes(&self, pool: &DbPool) -> Result<(), AppError> {
        let table_exists: bool = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS (SELECT name FROM sqlite_master WHERE type='table' AND name='conditional_alias_routes')",
        )
        .fetch_one(pool)
        .await? == 1;

        if !table_exists {
            self.conditional_alias_routes.clear();
            return Ok(());
        }

        let rows = sqlx::query_as::<_, ConditionalAliasRouteRow>(
            r#"
            SELECT tenant_id, alias, priority, upstream_id, model_name, min_input_tokens, max_input_tokens, keywords, has_image, start_time, end_time, is_fallback, status, all_users_visible, user_ids
            FROM conditional_alias_routes
            WHERE status = 'active'
            ORDER BY tenant_id ASC, alias ASC, priority ASC, created_at ASC
            "#,
        )
        .fetch_all(pool)
        .await?;

        let mut grouped: HashMap<(Uuid, String), Vec<ConditionalAliasRouteCache>> = HashMap::new();
        for row in rows {
            grouped
                .entry((row.tenant_id.into(), row.alias.clone()))
                .or_default()
                .push(ConditionalAliasRouteCache {
                    tenant_id: row.tenant_id.into(),
                    alias: row.alias,
                    priority: row.priority,
                    upstream_id: row.upstream_id.into(),
                    model_name: row.model_name,
                    min_input_tokens: row.min_input_tokens,
                    max_input_tokens: row.max_input_tokens,
                    keywords: row.keywords,
                    has_image: row.has_image,
                    start_time: row.start_time,
                    end_time: row.end_time,
                    is_fallback: row.is_fallback,
                    status: row.status,
                    all_users_visible: row.all_users_visible,
                    user_ids: row.user_ids,
                });
        }

        self.conditional_alias_routes.clear();
        for (key, value) in grouped {
            self.conditional_alias_routes.insert(key, value);
        }
        Ok(())
    }

    pub fn create_conversation(
        &self,
        conversation: ConversationRecord,
    ) -> Result<(), AppError> {
        let pool = self.pool.clone();
        tokio::spawn(async move {
            let result = sqlx::query("INSERT INTO conversations (id, tenant_id, user_id, api_key_id, conversation_id, model, provider, total_input_tokens, total_output_tokens, total_tokens, client_ip, started_at, ended_at, created_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
                .bind(conversation.id.to_string())
                .bind(conversation.tenant_id.to_string())
                .bind(conversation.user_id.to_string())
                .bind(conversation.api_key_id.to_string())
                .bind(conversation.conversation_id)
                .bind(conversation.model)
                .bind(conversation.provider)
                .bind(conversation.input_tokens)
                .bind(conversation.output_tokens)
                .bind(conversation.total_tokens)
                .bind(conversation.client_ip)
                .bind(conversation.started_at.to_rfc3339())
                .bind(conversation.ended_at.map(|t| t.to_rfc3339()))
                .bind(Utc::now().to_rfc3339())
                .execute(&pool)
                .await;
            if let Err(e) = result {
                tracing::error!("Failed to create conversation {}: {}", conversation.id, e);
            }
        });
        Ok(())
    }

    pub fn update_conversation(
        &self,
        conversation_id: Uuid,
        update: ConversationUpdate,
    ) -> Result<(), AppError> {
        let pool = self.pool.clone();
        tokio::spawn(async move {
            let input_tokens = update.input_tokens.unwrap_or(0);
            let output_tokens = update.output_tokens.unwrap_or(0);
            let total_tokens = update.total_tokens.unwrap_or(input_tokens + output_tokens);
            let result = if let Some(ref provider) = update.provider {
                sqlx::query("UPDATE conversations SET provider = ?, total_input_tokens = total_input_tokens + ?, total_output_tokens = total_output_tokens + ?, total_tokens = total_tokens + ?, ended_at = COALESCE(?, ended_at) WHERE id = ?")
                    .bind(provider)
                    .bind(input_tokens)
                    .bind(output_tokens)
                    .bind(total_tokens)
                    .bind(update.ended_at.map(|t| t.to_rfc3339()))
                    .bind(conversation_id.to_string())
                    .execute(&pool)
                    .await
            } else {
                sqlx::query("UPDATE conversations SET total_input_tokens = total_input_tokens + ?, total_output_tokens = total_output_tokens + ?, total_tokens = total_tokens + ?, ended_at = COALESCE(?, ended_at) WHERE id = ?")
                    .bind(input_tokens)
                    .bind(output_tokens)
                    .bind(total_tokens)
                    .bind(update.ended_at.map(|t| t.to_rfc3339()))
                    .bind(conversation_id.to_string())
                    .execute(&pool)
                    .await
            };
            if let Err(e) = result {
                tracing::error!("Failed to update conversation {}: {}", conversation_id, e);
            }
        });
        Ok(())
    }

    pub fn end_conversation(&self, conversation_id: Uuid) -> Result<(), AppError> {
        let pool = self.pool.clone();
        tokio::spawn(async move {
            let result = sqlx::query("UPDATE conversations SET ended_at = datetime('now') WHERE id = ?")
                .bind(conversation_id.to_string())
                .execute(&pool)
                .await;
            if let Err(e) = result {
                tracing::error!("Failed to end conversation {}: {}", conversation_id, e);
            }
        });
        Ok(())
    }

    pub async fn get_conversation(&self, conversation_id: Uuid) -> Option<ConversationRecord> {
        let row = sqlx::query("SELECT id, conversation_id, tenant_id, user_id, api_key_id, model, provider, total_input_tokens, total_output_tokens, total_tokens, client_ip, started_at, ended_at FROM conversations WHERE id = ?")
            .bind(conversation_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .ok()??;

        let id: String = row.try_get("id").ok()?;
        let conversation_id: String = row.try_get("conversation_id").ok()?;
        let tenant_id: String = row.try_get("tenant_id").ok()?;
        let user_id: String = row.try_get("user_id").ok()?;
        let api_key_id: String = row.try_get("api_key_id").ok()?;
        let model: String = row.try_get("model").ok()?;
        let provider: String = row.try_get("provider").ok()?;
        let input_tokens: i64 = row.try_get("total_input_tokens").unwrap_or(0);
        let output_tokens: i64 = row.try_get("total_output_tokens").unwrap_or(0);
        let total_tokens: i64 = row.try_get("total_tokens").unwrap_or(0);
        let status: Option<DateTime<Utc>> = if let Ok(Some(s)) = row.try_get::<Option<String>, _>("ended_at") {
            s.parse().ok()
        } else {
            None
        };
        let client_ip: String = row.try_get("client_ip").unwrap_or_default();
        let started_at: DateTime<Utc> = row.try_get("started_at").ok()?;
        let ended_at: Option<DateTime<Utc>> = row.try_get("ended_at").ok()?;
        Some(ConversationRecord {
            id: Uuid::parse_str(&id).ok()?,
            conversation_id,
            tenant_id: Uuid::parse_str(&tenant_id).unwrap(),
            user_id: Uuid::parse_str(&user_id).unwrap(),
            api_key_id: Uuid::parse_str(&api_key_id).unwrap(),
            model,
            provider,
            input_tokens,
            output_tokens,
            total_tokens,
            status,
            client_ip,
            started_at,
            ended_at,
        })
    }

    pub fn write_proxy_log(&self, log: ProxyLogRecord, content: ProxyLogContent) -> Result<(), AppError> {
        let pool = self.pool.clone();
        let writer = self.proxy_log_writer.clone();
        tokio::spawn(async move {
            let log_id = log.id;
            let log_file = if let Some(ref w) = writer {
                match w.write_content(&content).await {
                    Ok(filename) => Some(filename),
                    Err(e) => {
                        tracing::error!("Failed to write proxy log content to file {}: {}", log_id, e);
                        None
                    }
                }
            } else {
                None
            };

            let result = sqlx::query("INSERT INTO proxy_logs (id, tenant_id, user_id, api_key_id, conversation_id, model, routed_model, provider, input_tokens, output_tokens, total_tokens, status, error_message, log_file, client_ip, created_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
                .bind(log.id.to_string())
                .bind(log.tenant_id.to_string())
                .bind(log.user_id.to_string())
                .bind(log.api_key_id.to_string())
                .bind(log.conversation_id)
                .bind(log.model)
                .bind(log.routed_model)
                .bind(log.provider)
                .bind(log.input_tokens)
                .bind(log.output_tokens)
                .bind(log.total_tokens)
                .bind(log.status)
                .bind(log.error_message)
                .bind(&log_file)
                .bind(log.client_ip)
                .bind(log.created_at.to_rfc3339())
                .execute(&pool)
                .await;
            if let Err(e) = result {
                tracing::error!("Failed to write proxy log {}: {}", log_id, e);
            }
        });
        Ok(())
    }

    pub async fn list_proxy_logs(
        &self,
        tenant_id: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<ProxyLogRecord>, AppError> {
        let rows = if let Some(tenant_id) = tenant_id {
            sqlx::query("SELECT id, tenant_id, user_id, api_key_id, conversation_id, model, routed_model, provider, input_tokens, output_tokens, total_tokens, status, error_message, log_file, client_ip, created_at FROM proxy_logs WHERE tenant_id = ? ORDER BY created_at DESC LIMIT ?")
                .bind(tenant_id.to_string())
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await?
        } else {
            sqlx::query("SELECT id, tenant_id, user_id, api_key_id, conversation_id, model, routed_model, provider, input_tokens, output_tokens, total_tokens, status, error_message, log_file, client_ip, created_at FROM proxy_logs ORDER BY created_at DESC LIMIT ?")
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await?
        };
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(ProxyLogRecord {
                id: row.try_get("id")?,
                tenant_id: row.try_get("tenant_id")?,
                user_id: row.try_get("user_id")?,
                api_key_id: row.try_get("api_key_id")?,
                conversation_id: row.try_get("conversation_id")?,
                model: row.try_get("model")?,
                routed_model: row.try_get("routed_model")?,
                provider: row.try_get("provider")?,
                input_tokens: row.try_get("input_tokens")?,
                output_tokens: row.try_get("output_tokens")?,
                total_tokens: row.try_get("total_tokens")?,
                status: row.try_get("status")?,
                error_message: row.try_get("error_message")?,
                log_file: row.try_get("log_file")?,
                client_ip: row.try_get("client_ip")?,
                created_at: row.try_get("created_at")?,
            });
        }
        Ok(out)
    }

    pub fn create_pending_proxy_log(&self, log: ProxyLogRecord) -> Result<Uuid, AppError> {
        let pool = self.pool.clone();
        let log_id: Uuid = log.id.into();
        tokio::spawn(async move {
            let result = sqlx::query("INSERT INTO proxy_logs (id, tenant_id, user_id, api_key_id, conversation_id, model, routed_model, provider, input_tokens, output_tokens, total_tokens, status, error_message, log_file, client_ip, created_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
                .bind(log.id.to_string())
                .bind(log.tenant_id.to_string())
                .bind(log.user_id.to_string())
                .bind(log.api_key_id.to_string())
                .bind(log.conversation_id)
                .bind(log.model)
                .bind(log.routed_model)
                .bind(log.provider)
                .bind(log.input_tokens)
                .bind(log.output_tokens)
                .bind(log.total_tokens)
                .bind(log.status)
                .bind(log.error_message)
                .bind(&log.log_file)
                .bind(log.client_ip)
                .bind(log.created_at.to_rfc3339())
                .execute(&pool)
                .await;
            if let Err(e) = result {
                tracing::error!("Failed to create pending proxy log {}: {}", log_id, e);
            }
        });
        Ok(log_id)
    }

    pub fn update_proxy_log_status(&self, log_id: Uuid, status: String, error_message: Option<String>) {
        let pool = self.pool.clone();
        tokio::spawn(async move {
            let result = if let Some(ref msg) = error_message {
                sqlx::query("UPDATE proxy_logs SET status = ?, error_message = ? WHERE id = ?")
                    .bind(&status)
                    .bind(msg)
                    .bind(log_id.to_string())
                    .execute(&pool)
                    .await
            } else {
                sqlx::query("UPDATE proxy_logs SET status = ?, error_message = NULL WHERE id = ?")
                    .bind(&status)
                    .bind(log_id.to_string())
                    .execute(&pool)
                    .await
            };
            if let Err(e) = result {
                tracing::error!("Failed to update proxy log status {}: {}", log_id, e);
            }
        });
    }

    pub fn update_proxy_log_with_content(&self, log_id: Uuid, status: String, error_message: Option<String>, input_tokens: i64, output_tokens: i64, total_tokens: i64, routed_model: Option<String>, provider: String, content: ProxyLogContent) {
        let pool = self.pool.clone();
        let writer = self.proxy_log_writer.clone();
        tokio::spawn(async move {
            let log_file = if let Some(ref w) = writer {
                match w.write_content(&content).await {
                    Ok(filename) => Some(filename),
                    Err(e) => {
                        tracing::error!("Failed to write proxy log content to file {}: {}", log_id, e);
                        None
                    }
                }
            } else {
                None
            };

            let result = if let Some(ref msg) = error_message {
                sqlx::query("UPDATE proxy_logs SET status = ?, error_message = ?, input_tokens = ?, output_tokens = ?, total_tokens = ?, routed_model = ?, provider = ?, log_file = ? WHERE id = ?")
                    .bind(&status)
                    .bind(msg)
                    .bind(input_tokens)
                    .bind(output_tokens)
                    .bind(total_tokens)
                    .bind(&routed_model)
                    .bind(&provider)
                    .bind(&log_file)
                    .bind(log_id.to_string())
                    .execute(&pool)
                    .await
            } else {
                sqlx::query("UPDATE proxy_logs SET status = ?, error_message = NULL, input_tokens = ?, output_tokens = ?, total_tokens = ?, routed_model = ?, provider = ?, log_file = ? WHERE id = ?")
                    .bind(&status)
                    .bind(input_tokens)
                    .bind(output_tokens)
                    .bind(total_tokens)
                    .bind(&routed_model)
                    .bind(&provider)
                    .bind(&log_file)
                    .bind(log_id.to_string())
                    .execute(&pool)
                    .await
            };
            if let Err(e) = result {
                tracing::error!("Failed to update proxy log with content {}: {}", log_id, e);
            }
        });
    }

    pub async fn list_proxy_logs_filtered(
        &self,
        tenant_id: Option<Uuid>,
        start_time: Option<DateTime<Utc>>,
        end_time: Option<DateTime<Utc>>,
        user_id: Option<Uuid>,
        min_tokens: Option<i64>,
        max_tokens: Option<i64>,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<ProxyLogRecord>, AppError> {
        let mut builder = QueryBuilder::<Sqlite>::new(
            "SELECT id, tenant_id, user_id, api_key_id, conversation_id, model, routed_model, provider, input_tokens, output_tokens, total_tokens, status, error_message, log_file, client_ip, created_at FROM proxy_logs WHERE 1=1",
        );

        if let Some(tid) = tenant_id {
            builder.push(" AND tenant_id = ").push_bind(tid.to_string());
        }
        if let Some(start) = start_time {
            builder.push(" AND created_at >= ").push_bind(start.to_rfc3339());
        }
        if let Some(end) = end_time {
            builder.push(" AND created_at <= ").push_bind(end.to_rfc3339());
        }
        if let Some(uid) = user_id {
            builder.push(" AND user_id = ").push_bind(uid.to_string());
        }
        if let Some(min) = min_tokens {
            builder.push(" AND total_tokens >= ").push_bind(min);
        }
        if let Some(max) = max_tokens {
            builder.push(" AND total_tokens <= ").push_bind(max);
        }

        builder
            .push(" ORDER BY created_at DESC LIMIT ")
            .push_bind(limit as i64)
            .push(" OFFSET ")
            .push_bind(offset as i64);

        let rows = builder.build().fetch_all(&self.pool).await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(ProxyLogRecord {
                id: row.try_get("id")?,
                tenant_id: row.try_get("tenant_id")?,
                user_id: row.try_get("user_id")?,
                api_key_id: row.try_get("api_key_id")?,
                conversation_id: row.try_get("conversation_id")?,
                model: row.try_get("model")?,
                routed_model: row.try_get("routed_model")?,
                provider: row.try_get("provider")?,
                input_tokens: row.try_get("input_tokens")?,
                output_tokens: row.try_get("output_tokens")?,
                total_tokens: row.try_get("total_tokens")?,
                status: row.try_get("status")?,
                error_message: row.try_get("error_message")?,
                log_file: row.try_get("log_file")?,
                client_ip: row.try_get("client_ip")?,
                created_at: row.try_get("created_at")?,
            });
        }
        Ok(out)
    }

    pub async fn count_proxy_logs_filtered(
        &self,
        tenant_id: Option<Uuid>,
        start_time: Option<DateTime<Utc>>,
        end_time: Option<DateTime<Utc>>,
        user_id: Option<Uuid>,
        min_tokens: Option<i64>,
        max_tokens: Option<i64>,
    ) -> Result<i64, AppError> {
        let mut builder =
            QueryBuilder::<Sqlite>::new("SELECT COUNT(*) AS total FROM proxy_logs WHERE 1=1");

        if let Some(tid) = tenant_id {
            builder.push(" AND tenant_id = ").push_bind(tid.to_string());
        }
        if let Some(start) = start_time {
            builder.push(" AND created_at >= ").push_bind(start.to_rfc3339());
        }
        if let Some(end) = end_time {
            builder.push(" AND created_at <= ").push_bind(end.to_rfc3339());
        }
        if let Some(uid) = user_id {
            builder.push(" AND user_id = ").push_bind(uid.to_string());
        }
        if let Some(min) = min_tokens {
            builder.push(" AND total_tokens >= ").push_bind(min);
        }
        if let Some(max) = max_tokens {
            builder.push(" AND total_tokens <= ").push_bind(max);
        }

        let row = builder.build().fetch_one(&self.pool).await?;
        let total: i64 = row.try_get("total")?;
        Ok(total)
    }

    pub async fn find_proxy_log_by_id(
        &self,
        tenant_id: Option<Uuid>,
        id: Uuid,
    ) -> Result<Option<ProxyLogRecord>, AppError> {
        let row = if let Some(tid) = tenant_id {
            sqlx::query("SELECT id, tenant_id, user_id, api_key_id, conversation_id, model, routed_model, provider, input_tokens, output_tokens, total_tokens, status, error_message, log_file, client_ip, created_at FROM proxy_logs WHERE id = ? AND tenant_id = ? LIMIT 1")
                .bind(id.to_string())
                .bind(tid.to_string())
                .fetch_optional(&self.pool)
                .await?
        } else {
            sqlx::query("SELECT id, tenant_id, user_id, api_key_id, conversation_id, model, routed_model, provider, input_tokens, output_tokens, total_tokens, status, error_message, log_file, client_ip, created_at FROM proxy_logs WHERE id = ? LIMIT 1")
                .bind(id.to_string())
                .fetch_optional(&self.pool)
                .await?
        };

        let Some(row) = row else {
            return Ok(None);
        };

        Ok(Some(ProxyLogRecord {
            id: row.try_get("id")?,
            tenant_id: row.try_get("tenant_id")?,
            user_id: row.try_get("user_id")?,
            api_key_id: row.try_get("api_key_id")?,
            conversation_id: row.try_get("conversation_id")?,
            model: row.try_get("model")?,
            routed_model: row.try_get("routed_model")?,
            provider: row.try_get("provider")?,
            input_tokens: row.try_get("input_tokens")?,
            output_tokens: row.try_get("output_tokens")?,
            total_tokens: row.try_get("total_tokens")?,
            status: row.try_get("status")?,
            error_message: row.try_get("error_message")?,
            log_file: row.try_get("log_file")?,
            client_ip: row.try_get("client_ip")?,
            created_at: row.try_get("created_at")?,
        }))
    }

    async fn ensure_proxy_log_columns_once(&self) -> Result<(), AppError> {
        if self.proxy_log_columns_checked.load(Ordering::Relaxed) {
            return Ok(());
        }
        let columns_to_add = [
            ("routed_model", "TEXT"),
            ("log_file", "TEXT"),
        ];

        for (column, definition) in columns_to_add {
            let column_exists: bool = sqlx::query_scalar::<_, i64>(
                &format!(
                    "SELECT COUNT(*) FROM pragma_table_info('proxy_logs') WHERE name = '{}'",
                    column
                ),
            )
            .fetch_one(&self.pool)
            .await? > 0;

            if !column_exists {
                sqlx::query(&format!(
                    "ALTER TABLE proxy_logs ADD COLUMN {} {}",
                    column, definition
                ))
                .execute(&self.pool)
                .await?;
            }
        }
        self.proxy_log_columns_checked.store(true, Ordering::Relaxed);
        Ok(())
    }

    pub fn write_audit_log(&self, log: AuditLogRecord) -> Result<(), AppError> {
        if let Some(ref writer) = self.audit_log_writer {
            let entry = AuditLogEntry {
                id: log.id.into(),
                tenant_id: Some(log.tenant_id.into()),
                user_id: log.user_id.into(),
                action: log.action,
                resource_type: log.resource_type,
                resource_id: log.resource_id,
                details: log.details,
                ip_address: log.ip_address,
                user_agent: log.user_agent,
                created_at: log.created_at,
            };
            let writer = writer.clone();
            tokio::spawn(async move {
                let _ = writer.write_log(entry).await;
            });
        } else {
            let pool = self.pool.clone();
            tokio::spawn(async move {
                let _ = sqlx::query("INSERT INTO audit_logs (id, tenant_id, user_id, action, resource_type, resource_id, details, ip_address, user_agent, created_at) VALUES (?,?,?,?,?,?,?,?,?,?)")
                    .bind(log.id.to_string())
                    .bind(log.tenant_id.to_string())
                    .bind(log.user_id.to_string())
                    .bind(log.action)
                    .bind(log.resource_type)
                    .bind(log.resource_id)
                    .bind(log.details.to_string())
                    .bind(log.ip_address)
                    .bind(log.user_agent)
                    .bind(log.created_at.to_rfc3339())
                    .execute(&pool)
                    .await;
            });
        }
        Ok(())
    }

    pub async fn list_audit_logs(
        &self,
        tenant_id: Option<Uuid>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<AuditLogRecord>, AppError> {
        self.list_audit_logs_filtered(tenant_id, None, None, None, None, None, limit, offset)
            .await
    }

    pub async fn list_audit_logs_filtered(
        &self,
        tenant_id: Option<Uuid>,
        start_time: Option<DateTime<Utc>>,
        end_time: Option<DateTime<Utc>>,
        user_id: Option<Uuid>,
        action: Option<&str>,
        resource_type: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<AuditLogRecord>, AppError> {
        if let Some(ref writer) = self.audit_log_writer {
            let entries = writer
                .list_logs(tenant_id, start_time, end_time, limit + offset)
                .await?;
            let filtered: Vec<AuditLogRecord> = entries
                .into_iter()
                .filter(|e| {
                    if let Some(uid) = user_id {
                        if e.user_id != uid {
                            return false;
                        }
                    }
                    if let Some(a) = action {
                        if e.action != a {
                            return false;
                        }
                    }
                    if let Some(rt) = resource_type {
                        if e.resource_type != rt {
                            return false;
                        }
                    }
                    true
                })
                .skip(offset)
                .map(|e| AuditLogRecord {
                    id: SqlUuid::from(e.id),
                    tenant_id: SqlUuid::from(e.tenant_id.unwrap_or_default()),
                    user_id: SqlUuid::from(e.user_id),
                    action: e.action,
                    resource_type: e.resource_type,
                    resource_id: e.resource_id,
                    details: e.details,
                    ip_address: e.ip_address,
                    user_agent: e.user_agent,
                    created_at: e.created_at,
                })
                .collect();
            return Ok(filtered);
        }

        let mut sql = String::from(
            "SELECT id, tenant_id, user_id, action, resource_type, resource_id, details, ip_address, user_agent, created_at FROM audit_logs WHERE 1=1",
        );

        if tenant_id.is_some() {
            sql.push_str(" AND tenant_id = ?");
        }
        if start_time.is_some() {
            sql.push_str(" AND created_at >= ?");
        }
        if end_time.is_some() {
            sql.push_str(" AND created_at <= ?");
        }
        if user_id.is_some() {
            sql.push_str(" AND user_id = ?");
        }
        if action.is_some() {
            sql.push_str(" AND action = ?");
        }
        if resource_type.is_some() {
            sql.push_str(" AND resource_type = ?");
        }
        sql.push_str(" ORDER BY created_at DESC LIMIT ? OFFSET ?");

        let mut query = sqlx::query(&sql);

        if let Some(tid) = tenant_id {
            query = query.bind(tid.to_string());
        }
        if let Some(st) = start_time {
            query = query.bind(st.to_rfc3339());
        }
        if let Some(et) = end_time {
            query = query.bind(et.to_rfc3339());
        }
        if let Some(uid) = user_id {
            query = query.bind(uid.to_string());
        }
        if let Some(a) = action {
            query = query.bind(a);
        }
        if let Some(rt) = resource_type {
            query = query.bind(rt);
        }
        query = query.bind(limit as i64).bind(offset as i64);

        let rows = query.fetch_all(&self.pool).await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(AuditLogRecord {
                id: row.try_get("id")?,
                tenant_id: row.try_get("tenant_id")?,
                user_id: row.try_get("user_id")?,
                action: row.try_get("action")?,
                resource_type: row.try_get("resource_type")?,
                resource_id: row.try_get("resource_id")?,
                details: serde_json::from_str(&row.try_get::<String, _>("details")?)
                    .unwrap_or(serde_json::Value::Null),
                ip_address: row.try_get("ip_address")?,
                user_agent: row.try_get("user_agent")?,
                created_at: row.try_get("created_at")?,
            });
        }
        Ok(out)
    }

    pub async fn count_audit_logs(
        &self,
        tenant_id: Option<Uuid>,
    ) -> Result<i64, AppError> {
        self.count_audit_logs_filtered(tenant_id, None, None, None, None, None)
            .await
    }

    pub async fn count_audit_logs_filtered(
        &self,
        tenant_id: Option<Uuid>,
        start_time: Option<DateTime<Utc>>,
        end_time: Option<DateTime<Utc>>,
        user_id: Option<Uuid>,
        action: Option<&str>,
        resource_type: Option<&str>,
    ) -> Result<i64, AppError> {
        if let Some(ref writer) = self.audit_log_writer {
            let entries = writer.list_logs(tenant_id, start_time, end_time, i32::MAX as usize).await?;
            let count = entries
                .into_iter()
                .filter(|e| {
                    if let Some(uid) = user_id {
                        if e.user_id != uid {
                            return false;
                        }
                    }
                    if let Some(a) = action {
                        if e.action != a {
                            return false;
                        }
                    }
                    if let Some(rt) = resource_type {
                        if e.resource_type != rt {
                            return false;
                        }
                    }
                    true
                })
                .count() as i64;
            return Ok(count);
        }

        let mut sql = String::from("SELECT COUNT(*) FROM audit_logs WHERE 1=1");

        if tenant_id.is_some() {
            sql.push_str(" AND tenant_id = ?");
        }
        if start_time.is_some() {
            sql.push_str(" AND created_at >= ?");
        }
        if end_time.is_some() {
            sql.push_str(" AND created_at <= ?");
        }
        if user_id.is_some() {
            sql.push_str(" AND user_id = ?");
        }
        if action.is_some() {
            sql.push_str(" AND action = ?");
        }
        if resource_type.is_some() {
            sql.push_str(" AND resource_type = ?");
        }

        let mut query = sqlx::query_scalar::<_, i64>(&sql);

        if let Some(tid) = tenant_id {
            query = query.bind(tid.to_string());
        }
        if let Some(st) = start_time {
            query = query.bind(st.to_rfc3339());
        }
        if let Some(et) = end_time {
            query = query.bind(et.to_rfc3339());
        }
        if let Some(uid) = user_id {
            query = query.bind(uid.to_string());
        }
        if let Some(a) = action {
            query = query.bind(a);
        }
        if let Some(rt) = resource_type {
            query = query.bind(rt);
        }

        let count = query.fetch_one(&self.pool).await?;
        Ok(count)
    }

    pub async fn find_audit_log_by_id(
        &self,
        id: Uuid,
    ) -> Result<Option<AuditLogRecord>, AppError> {
        if let Some(ref writer) = self.audit_log_writer {
            let entry = writer.find_by_id(id).await?;
            return Ok(entry.map(|e| AuditLogRecord {
                id: SqlUuid::from(e.id),
                tenant_id: SqlUuid::from(e.tenant_id.unwrap_or_default()),
                user_id: SqlUuid::from(e.user_id),
                action: e.action,
                resource_type: e.resource_type,
                resource_id: e.resource_id,
                details: e.details,
                ip_address: e.ip_address,
                user_agent: e.user_agent,
                created_at: e.created_at,
            }));
        }

        let row = sqlx::query(
            "SELECT id, tenant_id, user_id, action, resource_type, resource_id, details, ip_address, user_agent, created_at FROM audit_logs WHERE id = ? LIMIT 1",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        Ok(Some(AuditLogRecord {
            id: row.try_get("id")?,
            tenant_id: row.try_get("tenant_id")?,
            user_id: row.try_get("user_id")?,
            action: row.try_get("action")?,
            resource_type: row.try_get("resource_type")?,
            resource_id: row.try_get("resource_id")?,
            details: serde_json::from_str(&row.try_get::<String, _>("details")?).unwrap_or(serde_json::Value::Null),
            ip_address: row.try_get("ip_address")?,
            user_agent: row.try_get("user_agent")?,
            created_at: row.try_get("created_at")?,
        }))
    }

    pub async fn list_conversations(
        &self,
        tenant_id: Option<Uuid>,
        user_id: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<ConversationRecord>, AppError> {
        let rows = if let (Some(tenant_id), Some(user_id)) = (tenant_id, user_id) {
            sqlx::query("SELECT id, conversation_id, tenant_id, user_id, api_key_id, model, provider, total_input_tokens, total_output_tokens, total_tokens, client_ip, started_at, ended_at FROM conversations WHERE tenant_id = ? AND user_id = ? ORDER BY started_at DESC LIMIT ?")
                .bind(tenant_id.to_string())
                .bind(user_id.to_string())
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await?
        } else if let Some(tenant_id) = tenant_id {
            sqlx::query("SELECT id, conversation_id, tenant_id, user_id, api_key_id, model, provider, total_input_tokens, total_output_tokens, total_tokens, client_ip, started_at, ended_at FROM conversations WHERE tenant_id = ? ORDER BY started_at DESC LIMIT ?")
                .bind(tenant_id.to_string())
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await?
        } else if let Some(user_id) = user_id {
            sqlx::query("SELECT id, conversation_id, tenant_id, user_id, api_key_id, model, provider, total_input_tokens, total_output_tokens, total_tokens, client_ip, started_at, ended_at FROM conversations WHERE user_id = ? ORDER BY started_at DESC LIMIT ?")
                .bind(user_id.to_string())
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await?
        } else {
            sqlx::query("SELECT id, conversation_id, tenant_id, user_id, api_key_id, model, provider, total_input_tokens, total_output_tokens, total_tokens, client_ip, started_at, ended_at FROM conversations ORDER BY started_at DESC LIMIT ?")
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await?
        };
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let id: String = row.try_get("id")?;
            let tenant_id: String = row.try_get("tenant_id")?;
            let user_id: String = row.try_get("user_id")?;
            let api_key_id: String = row.try_get("api_key_id")?;
            out.push(ConversationRecord {
                id: Uuid::parse_str(&id)?,
                conversation_id: row.try_get("conversation_id")?,
                tenant_id: Uuid::parse_str(&tenant_id)?,
                user_id: Uuid::parse_str(&user_id)?,
                api_key_id: Uuid::parse_str(&api_key_id)?,
                model: row.try_get("model")?,
                provider: row.try_get("provider")?,
                input_tokens: row.try_get("total_input_tokens")?,
                output_tokens: row.try_get("total_output_tokens")?,
                total_tokens: row.try_get("total_tokens")?,
                status: None,
                client_ip: row.try_get("client_ip")?,
                started_at: row.try_get("started_at")?,
                ended_at: row.try_get("ended_at")?,
            });
        }
        Ok(out)
    }

    pub async fn verify_api_key(&self, key: &str) -> Result<ApiKeyCache, AppError> {
        let key = key.strip_prefix("sk-").unwrap_or(key);
        let key_hash = crate::db::api_keys::hash_key(key);
        self.get_api_key(&key_hash).await
    }

    pub async fn get_model_visibility(
        &self,
        upstream_id: Uuid,
        model_name: &str,
    ) -> Option<ModelVisibilityCache> {
        self.model_visibility
            .get(&(upstream_id, model_name.to_string()))
            .map(|r| r.value().clone())
    }

    pub async fn check_model_access(&self, user_id: Uuid, upstream_id: Uuid, model_name: &str) -> bool {
        if let Some(vis) = self.model_visibility.get(&(upstream_id, model_name.to_string())) {
            if vis.all_users_visible {
                return true;
            }
            return vis.allowed_users.contains(&user_id);
        }
        true
    }

    pub async fn resolve_conditional_alias_routes(
        &self,
        user_id: Uuid,
        tenant_id: Uuid,
        requested_model: &str,
        request_text: &str,
        estimated_input_tokens: i64,
        request_has_image: bool,
    ) -> Vec<(Uuid, String)> {
        let routes = self
            .conditional_alias_routes
            .get(&(tenant_id, requested_model.to_string()))
            .map(|r| r.value().clone())
            .unwrap_or_default();
        if routes.is_empty() {
            return Vec::new();
        }

        let mut fallback: Option<(Uuid, String)> = None;
        for route in &routes {
            if !route.is_fallback {
                continue;
            }
            if !route.is_visible_to(user_id) {
                continue;
            }
            let has_access = self
                .check_model_access(user_id, route.upstream_id, &route.model_name)
                .await;
            if has_access {
                fallback = Some((route.upstream_id, route.model_name.clone()));
            }
        }

        for route in &routes {
            if route.is_fallback {
                continue;
            }
            if !route.is_visible_to(user_id) {
                continue;
            }
            let has_access = self
                .check_model_access(user_id, route.upstream_id, &route.model_name)
                .await;
            if !has_access {
                continue;
            }

            if route.matches(request_text, estimated_input_tokens, request_has_image) {
                let mut out = vec![(route.upstream_id, route.model_name.clone())];
                if let Some(ref fallback_route) = fallback {
                    if fallback_route.0 != route.upstream_id || fallback_route.1 != route.model_name
                    {
                        out.push(fallback_route.clone());
                    }
                }
                return out;
            }
        }

        fallback.into_iter().collect()
    }


    pub async fn get_upstreams_by_tenant(&self, tenant_id: Uuid) -> Vec<UpstreamCache> {
        self.get_tenant_upstreams(tenant_id).await
    }

    pub async fn resolve_requested_model(
        &self,
        user_id: Uuid,
        upstream_id: Uuid,
        requested_model: &str,
    ) -> Option<String> {
        let (is_ollama, upstream_models) = self
            .upstreams
            .get(&upstream_id)
            .map(|u| {
                (
                    u.api_type.eq_ignore_ascii_case("ollama") || u.provider.eq_ignore_ascii_case("ollama"),
                    u.models.clone(),
                )
            })
            .unwrap_or((false, Vec::new()));

        if !upstream_models.is_empty() {
            let model_supported = upstream_models.iter().any(|m| {
                model_name_matches(m, requested_model, is_ollama)
            });
            if !model_supported {
                return None;
            }
        }

        let mut direct_match: Option<(String, ModelVisibilityCache)> = None;
        let mut alias_matched_but_forbidden = false;
        let mut has_visibility_rule = false;

        for entry in self.model_visibility.iter() {
            let (uid, original_model) = entry.key();
            if *uid != upstream_id {
                continue;
            }
            let visibility = entry.value();
            has_visibility_rule = true;
            if visibility
                .model_aliases
                .iter()
                .any(|alias| model_name_matches(alias, requested_model, is_ollama))
            {
                if Self::model_visibility_allows_user(visibility, user_id) {
                    return Some(original_model.clone());
                }
                alias_matched_but_forbidden = true;
                continue;
            }
            if model_name_matches(original_model, requested_model, is_ollama) {
                direct_match = Some((original_model.clone(), visibility.clone()));
            }
        }

        if alias_matched_but_forbidden {
            return None;
        }

        if let Some((original_model, visibility)) = direct_match {
            if !Self::model_visibility_allows_user(&visibility, user_id) {
                return None;
            }
            return Some(original_model);
        }

        if has_visibility_rule {
            return None;
        }

        Some(requested_model.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyCache {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub key_hash: String,
    pub name: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub user_role: String,
    pub rpm_limit: i32,
    pub tpm_limit: i32,
    pub daily_limit: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserCache {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub username: String,
    pub email: String,
    pub role: String,
    pub daily_request_limit: i64,
    pub monthly_request_limit: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamCache {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub provider: String,
    pub api_type: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub models: Vec<String>,
    pub custom_headers: serde_json::Value,
    pub status: String,
    pub priority: i32,
    pub weight: i32,
    pub rate_limit: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub daily_request_limit: i64,
    pub monthly_request_limit: i64,
}

impl UpstreamCache {
    pub fn to_config(&self) -> crate::models::upstream::UpstreamConfig {
        crate::models::upstream::UpstreamConfig {
            id: crate::models::SqlUuid::from(self.id),
            tenant_id: crate::models::SqlUuid::from(self.tenant_id),
            name: self.name.clone(),
            provider: self.provider.clone(),
            api_type: self.api_type.clone(),
            base_url: self.base_url.clone(),
            api_key_encrypted: self.api_key.clone().unwrap_or_default(),
            models: self.models.join(","),
            custom_headers: self.custom_headers.clone(),
            priority: self.priority,
            weight: self.weight,
            rate_limit: self.rate_limit,
            daily_request_limit: self.daily_request_limit,
            monthly_request_limit: self.monthly_request_limit,
            daily_request_used: 0,
            monthly_request_used: 0,
            status: self.status.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelVisibilityCache {
    pub id: Uuid,
    pub upstream_id: Uuid,
    pub model_name: String,
    pub model_aliases: Vec<String>,
    pub model_headers: serde_json::Value,
    pub all_users_visible: bool,
    pub allowed_users: Vec<Uuid>,
    pub retry_count: i64,
    pub retry_interval_seconds: i64,
    pub retry_backoff_strategy: String,
    pub retry_max_interval_seconds: i64,
    pub retry_failure_strategy: String,
    pub retry_fallback_upstream_id: Option<Uuid>,
    pub retry_fallback_model_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionalAliasRouteCache {
    pub tenant_id: Uuid,
    pub alias: String,
    pub priority: i32,
    pub upstream_id: Uuid,
    pub model_name: String,
    pub min_input_tokens: Option<i64>,
    pub max_input_tokens: Option<i64>,
    pub keywords: Option<String>,
    pub has_image: bool,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub is_fallback: bool,
    pub status: String,
    pub all_users_visible: bool,
    pub user_ids: Option<String>,
}
impl ConditionalAliasRouteCache {
    fn is_visible_to(&self, user_id: Uuid) -> bool {
        if self.all_users_visible {
            return true;
        }
        if let Some(ref user_ids) = self.user_ids {
            if let Ok(id) = Uuid::parse_str(user_ids) {
                return id == user_id;
            }
            // Try comma-separated list
            for id_str in user_ids.split(',') {
                if let Ok(id) = Uuid::parse_str(id_str.trim()) {
                    if id == user_id {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn matches(&self, request_text: &str, estimated_input_tokens: i64, request_has_image: bool) -> bool {
        if self.has_image && !request_has_image {
            return false;
        }
        if let Some(min_tokens) = self.min_input_tokens {
            if estimated_input_tokens < min_tokens {
                return false;
            }
        }
        if let Some(max_tokens) = self.max_input_tokens {
            if estimated_input_tokens > max_tokens {
                return false;
            }
        }
        if let Some(ref keywords) = self.keywords {
            if !keywords.is_empty() {
                let normalized = request_text.to_lowercase();
                let found = keywords
                    .split(',')
                    .any(|keyword| normalized.contains(&keyword.trim().to_lowercase()));
                if !found {
                    return false;
                }
            }
        }
        if !self.matches_time_range() {
            return false;
        }
        true
    }

    fn matches_time_range(&self) -> bool {
        match (&self.start_time, &self.end_time) {
            (Some(start), Some(end)) => {
                let now = chrono::Local::now();
                let now_time = now.format("%H:%M").to_string();
                now_time >= *start && now_time <= *end
            }
            (Some(start), None) => {
                let now = chrono::Local::now();
                let now_time = now.format("%H:%M").to_string();
                now_time >= *start
            }
            (None, Some(end)) => {
                let now = chrono::Local::now();
                let now_time = now.format("%H:%M").to_string();
                now_time <= *end
            }
            (None, None) => true,
        }
    }
}


#[derive(Debug, Clone, sqlx::FromRow)]
struct ApiKeyRow {
    id: SqlUuid,
    tenant_id: SqlUuid,
    user_id: SqlUuid,
    key_hash: String,
    name: String,
    expires_at: Option<DateTime<Utc>>,
    last_used_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    role: String,
    rpm_limit: Option<i32>,
    tpm_limit: Option<i32>,
    daily_limit: Option<i32>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct UserRow {
    id: SqlUuid,
    tenant_id: SqlUuid,
    username: String,
    email: String,
    role: String,
    daily_request_limit: i64,
    monthly_request_limit: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct UpstreamRow {
    id: SqlUuid,
    tenant_id: SqlUuid,
    name: String,
    provider: String,
    api_type: String,
    base_url: String,
    api_key: Option<String>,
    models: String,
    custom_headers: serde_json::Value,
    status: String,
    priority: i32,
    weight: i32,
    rate_limit: Option<i32>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    daily_request_limit: i64,
    monthly_request_limit: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ConditionalAliasRouteRow {
    tenant_id: SqlUuid,
    alias: String,
    priority: i32,
    upstream_id: SqlUuid,
    model_name: String,
    min_input_tokens: Option<i64>,
    max_input_tokens: Option<i64>,
    keywords: Option<String>,
    has_image: bool,
    start_time: Option<String>,
    end_time: Option<String>,
    is_fallback: bool,
    status: String,
    all_users_visible: bool,
    user_ids: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyLogRecord {
    pub id: SqlUuid,
    pub tenant_id: SqlUuid,
    pub user_id: SqlUuid,
    pub api_key_id: SqlUuid,
    pub conversation_id: Option<String>,
    pub model: String,
    pub routed_model: Option<String>,
    pub provider: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub status: String,
    pub error_message: Option<String>,
    pub log_file: Option<String>,
    pub client_ip: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationRecord {
    pub id: Uuid,
    pub conversation_id: String,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub api_key_id: Uuid,
    pub model: String,
    pub provider: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub status: Option<DateTime<Utc>>,
    pub client_ip: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationUpdate {
    pub provider: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub owned_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsResponse {
    pub object: String,
    pub data: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogRecord {
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
