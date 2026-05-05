#![allow(dead_code)]
use crate::db::DbPool;
use sqlx::Row;
use uuid::Uuid;

use crate::models::upstream::{
    ConditionalAliasConfig, ConditionalAliasFallbackInput, ConditionalAliasRule, ModelVisibility,
    ModelVisibilityUser, ModelWithVisibility, UpdateModelVisibilityRequest,
    UpsertConditionalAliasRequest,
};
use crate::utils::error::AppError;

fn parse_aliases(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or("")
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn normalize_aliases(input: &UpdateModelVisibilityRequest) -> Vec<String> {
    let source = if let Some(aliases) = &input.model_aliases {
        aliases.join(",")
    } else {
        input.model_alias.clone().unwrap_or_default()
    };
    let mut out: Vec<String> = Vec::new();
    for alias in source
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        if !out.iter().any(|a| a == alias) {
            out.push(alias.to_string());
        }
    }
    out
}

fn normalize_keywords(raw: Option<Vec<String>>) -> Vec<String> {
    let mut out = Vec::new();
    for item in raw.unwrap_or_default() {
        let keyword = item.trim();
        if keyword.is_empty() {
            continue;
        }
        if !out.iter().any(|v| v == keyword) {
            out.push(keyword.to_string());
        }
    }
    out
}

fn normalize_user_ids(mut ids: Vec<Uuid>) -> Vec<Uuid> {
    ids.sort();
    ids.dedup();
    ids
}

fn parse_uuid_list(raw: Option<&str>) -> Vec<Uuid> {
    parse_aliases(raw)
        .into_iter()
        .filter_map(|s| Uuid::parse_str(&s).ok())
        .collect()
}

fn normalize_model_headers(
    input: &UpdateModelVisibilityRequest,
) -> Result<serde_json::Value, AppError> {
    let headers = input
        .model_headers
        .clone()
        .unwrap_or_else(|| serde_json::json!({}));
    let obj = headers
        .as_object()
        .ok_or_else(|| AppError::BadRequest("模型请求头必须是 JSON 对象".to_string()))?;
    let mut normalized = serde_json::Map::new();
    for (k, v) in obj {
        let name = k.trim();
        if name.is_empty() {
            continue;
        }
        let value = if let Some(s) = v.as_str() {
            s.to_string()
        } else {
            v.to_string()
        };
        normalized.insert(name.to_string(), serde_json::Value::String(value));
    }
    Ok(serde_json::Value::Object(normalized))
}

pub async fn get_models_with_visibility(
    pool: &DbPool,
    tenant_id: Uuid,
) -> Result<Vec<ModelWithVisibility>, AppError> {
    let upstreams = sqlx::query_as::<_, crate::models::upstream::UpstreamConfig>(
        "SELECT * FROM upstream_configs WHERE tenant_id = ? AND status = 'active'",
    )
    .bind(tenant_id.to_string())
    .fetch_all(pool)
    .await?;

    let mut result = Vec::new();

    for upstream in upstreams {
        let visibilities = sqlx::query_as::<_, ModelVisibility>(
            "SELECT * FROM model_visibility WHERE upstream_id = ?",
        )
        .bind(upstream.id.to_string())
        .fetch_all(pool)
        .await?;

        for visibility in visibilities {
            let aliases = parse_aliases(visibility.model_alias.as_deref());
            let users = sqlx::query_as::<_, ModelVisibilityUser>(
                "SELECT * FROM model_visibility_users WHERE visibility_id = ?",
            )
            .bind(visibility.id.to_string())
            .fetch_all(pool)
            .await?;

            result.push(ModelWithVisibility {
                upstream_id: upstream.id.into(),
                upstream_name: upstream.name.clone(),
                model_name: visibility.model_name.clone(),
                original_model_name: visibility.model_name,
                model_alias: aliases.first().cloned(),
                model_aliases: aliases,
                model_headers: visibility.model_headers,
                provider: upstream.provider.clone(),
                all_users_visible: visibility.all_users_visible,
                allowed_users: users.into_iter().map(|u| u.user_id.into()).collect(),
                retry_count: visibility.retry_count,
                retry_interval_seconds: visibility.retry_interval_seconds,
                retry_backoff_strategy: visibility.retry_backoff_strategy,
                retry_max_interval_seconds: visibility.retry_max_interval_seconds,
                retry_failure_strategy: visibility.retry_failure_strategy,
                retry_fallback_upstream_id: visibility.retry_fallback_upstream_id.and_then(|s| Uuid::parse_str(&s).ok()),
                retry_fallback_model_name: visibility.retry_fallback_model_name,
            });
        }
    }

    Ok(result)
}

pub async fn set_model_visibility(
    pool: &DbPool,
    upstream_id: Uuid,
    model_name: &str,
    input: UpdateModelVisibilityRequest,
) -> Result<ModelVisibility, AppError> {
    let all_users_visible = input.all_users_visible || input.user_ids.is_empty();

    let tenant_id: String =
        sqlx::query_scalar("SELECT tenant_id FROM upstream_configs WHERE id = ?")
            .bind(upstream_id.to_string())
            .fetch_one(pool)
            .await?;

    let normalized_aliases = normalize_aliases(&input);
    if !normalized_aliases.is_empty() {
        let conditional_aliases = sqlx::query_scalar::<_, String>(
            r#"
            SELECT alias
            FROM conditional_alias_routes
            WHERE tenant_id = ? AND status = 'active'
            GROUP BY alias
            "#,
        )
        .bind(tenant_id.clone())
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        let rows = sqlx::query(
            r#"
            SELECT mv.model_alias
            FROM model_visibility mv
            JOIN upstream_configs uc ON uc.id = mv.upstream_id
            WHERE uc.tenant_id = ?
              AND NOT (mv.upstream_id = ? AND mv.model_name = ?)
            "#,
        )
        .bind(tenant_id.clone())
        .bind(upstream_id.to_string())
        .bind(model_name)
        .fetch_all(pool)
        .await?;

        let mut existing_aliases: Vec<String> = Vec::new();
        for row in rows {
            let raw: Option<String> = row.try_get("model_alias")?;
            for alias in parse_aliases(raw.as_deref()) {
                existing_aliases.push(alias);
            }
        }

        for alias in &normalized_aliases {
            if conditional_aliases.iter().any(|a| a == alias) {
                return Err(AppError::BadRequest(format!(
                    "模型别名 '{}' 已被智能路由占用",
                    alias
                )));
            }
            if existing_aliases.iter().any(|a| a == alias) {
                return Err(AppError::BadRequest(format!("模型别名 '{}' 已存在", alias)));
            }
        }
    }

    let serialized_alias = if normalized_aliases.is_empty() {
        None
    } else {
        Some(normalized_aliases.join(","))
    };

    let existing = sqlx::query_as::<_, ModelVisibility>(
        "SELECT * FROM model_visibility WHERE upstream_id = ? AND model_name = ?",
    )
    .bind(upstream_id.to_string())
    .bind(model_name)
    .fetch_optional(pool)
    .await?;

    let normalized_headers = if input.model_headers.is_some() {
        normalize_model_headers(&input)?
    } else if let Some(v) = &existing {
        v.model_headers.clone()
    } else {
        serde_json::json!({})
    };

    let retry_count = input.retry_count.unwrap_or(0);
    let retry_interval_seconds = input.retry_interval_seconds.unwrap_or(0);
    let retry_backoff_strategy = input.retry_backoff_strategy.unwrap_or_else(|| "fixed".to_string());
    let retry_max_interval_seconds = input.retry_max_interval_seconds.unwrap_or(0);
    let retry_failure_strategy = input.retry_failure_strategy.unwrap_or_else(|| "error".to_string());
    let retry_fallback_upstream_id = input.retry_fallback_upstream_id.map(|id| id.to_string());
    let retry_fallback_model_name = input.retry_fallback_model_name;

    let visibility_id = if let Some(v) = existing {
        sqlx::query("UPDATE model_visibility SET all_users_visible = ?, model_alias = ?, model_headers = ?, retry_count = ?, retry_interval_seconds = ?, retry_backoff_strategy = ?, retry_max_interval_seconds = ?, retry_failure_strategy = ?, retry_fallback_upstream_id = ?, retry_fallback_model_name = ?, updated_at = datetime('now') WHERE id = ?")
            .bind(all_users_visible)
            .bind(serialized_alias)
            .bind(normalized_headers)
            .bind(retry_count)
            .bind(retry_interval_seconds)
            .bind(&retry_backoff_strategy)
            .bind(retry_max_interval_seconds)
            .bind(&retry_failure_strategy)
            .bind(&retry_fallback_upstream_id)
            .bind(&retry_fallback_model_name)
            .bind(v.id.to_string())
            .execute(pool)
            .await?;
        v.id.into()
    } else {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO model_visibility (id, upstream_id, model_name, all_users_visible, model_headers, retry_count, retry_interval_seconds, retry_backoff_strategy, retry_max_interval_seconds, retry_failure_strategy, retry_fallback_upstream_id, retry_fallback_model_name) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(id.to_string())
        .bind(upstream_id.to_string())
        .bind(model_name)
        .bind(all_users_visible)
        .bind(normalized_headers.clone())
        .bind(retry_count)
        .bind(retry_interval_seconds)
        .bind(&retry_backoff_strategy)
        .bind(retry_max_interval_seconds)
        .bind(&retry_failure_strategy)
        .bind(&retry_fallback_upstream_id)
        .bind(&retry_fallback_model_name)
        .execute(pool)
        .await?;
        sqlx::query("UPDATE model_visibility SET model_alias = ? WHERE id = ?")
            .bind(serialized_alias)
            .bind(id.to_string())
            .execute(pool)
            .await?;
        id
    };

    sqlx::query("DELETE FROM model_visibility_users WHERE visibility_id = ?")
        .bind(visibility_id.to_string())
        .execute(pool)
        .await?;

    if !all_users_visible {
        for user_id in input.user_ids {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO model_visibility_users (id, visibility_id, user_id) VALUES (?, ?, ?)"
            )
            .bind(id.to_string())
            .bind(visibility_id.to_string())
            .bind(user_id.to_string())
            .execute(pool)
            .await?;
        }
    }

    let updated =
        sqlx::query_as::<_, ModelVisibility>("SELECT * FROM model_visibility WHERE id = ?")
            .bind(visibility_id.to_string())
            .fetch_one(pool)
            .await
            .map_err(|e| {
                AppError::Internal(format!("Failed to fetch updated visibility: {}", e))
            })?;

    Ok(updated)
}

pub async fn check_model_visibility(
    pool: &DbPool,
    upstream_id: Uuid,
    model_name: &str,
    user_id: Uuid,
) -> Result<bool, AppError> {
    let visibility = sqlx::query_as::<_, ModelVisibility>(
        "SELECT * FROM model_visibility WHERE upstream_id = ? AND model_name = ?",
    )
    .bind(upstream_id.to_string())
    .bind(model_name)
    .fetch_optional(pool)
    .await?;

    if let Some(v) = visibility {
        if v.all_users_visible {
            return Ok(true);
        }

        let allowed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM model_visibility_users WHERE visibility_id = ? AND user_id = ?",
        )
        .bind(v.id.to_string())
        .bind(user_id.to_string())
        .fetch_optional(pool)
        .await?
        .unwrap_or(0);

        Ok(allowed > 0)
    } else {
        Ok(false)
    }
}

pub async fn get_all_visibility_settings(
    pool: &DbPool,
    tenant_id: Uuid,
) -> Result<Vec<ModelWithVisibility>, AppError> {
    get_models_with_visibility(pool, tenant_id).await
}

pub async fn get_visible_models(
    pool: &DbPool,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<ModelWithVisibility>, AppError> {
    let all = get_models_with_visibility(pool, tenant_id).await?;
    let filtered = all
        .into_iter()
        .filter(|m| m.all_users_visible || m.allowed_users.contains(&user_id))
        .collect();
    Ok(filtered)
}

pub async fn list_conditional_aliases(
    pool: &DbPool,
    tenant_id: Uuid,
) -> Result<Vec<ConditionalAliasConfig>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT alias, priority, upstream_id, model_name, min_input_tokens, max_input_tokens, keywords, has_image, start_time, end_time, is_fallback, all_users_visible, user_ids
        FROM conditional_alias_routes
        WHERE tenant_id = ? AND status = 'active'
        ORDER BY alias ASC, priority ASC, created_at ASC
        "#,
    )
    .bind(tenant_id.to_string())
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let mut grouped: std::collections::BTreeMap<String, ConditionalAliasConfig> =
        std::collections::BTreeMap::new();

    for row in rows {
        let alias: String = row.try_get("alias")?;
        let priority: i32 = row.try_get("priority")?;
        let upstream_id_str: String = row.try_get("upstream_id")?;
        let upstream_id = Uuid::parse_str(&upstream_id_str).unwrap_or_default();
        let model_name: String = row.try_get("model_name")?;
        let min_input_tokens: Option<i64> = row.try_get("min_input_tokens")?;
        let max_input_tokens: Option<i64> = row.try_get("max_input_tokens")?;
        let keywords_text: Option<String> = row.try_get("keywords")?;
        let has_image: bool = row.try_get("has_image")?;
        let start_time: Option<String> = row.try_get("start_time")?;
        let end_time: Option<String> = row.try_get("end_time")?;
        let is_fallback: bool = row.try_get("is_fallback")?;
        let all_users_visible: bool = row.try_get("all_users_visible")?;
        let user_ids_text: Option<String> = row.try_get("user_ids")?;
        let keywords = parse_aliases(keywords_text.as_deref());
        let user_ids = parse_uuid_list(user_ids_text.as_deref());

        let entry = grouped
            .entry(alias.clone())
            .or_insert_with(|| ConditionalAliasConfig {
                alias: alias.clone(),
                rules: Vec::new(),
                fallback: ConditionalAliasFallbackInput {
                    upstream_id,
                    model_name: model_name.clone(),
                },
                all_users_visible,
                user_ids: user_ids.clone(),
            });

        entry.all_users_visible = all_users_visible;
        entry.user_ids = user_ids.clone();

        if is_fallback {
            entry.fallback = ConditionalAliasFallbackInput {
                upstream_id,
                model_name,
            };
        } else {
            entry.rules.push(ConditionalAliasRule {
                priority,
                upstream_id,
                model_name,
                min_input_tokens,
                max_input_tokens,
                keywords,
                has_image,
                start_time,
                end_time,
            });
        }
    }

    Ok(grouped.into_values().collect())
}

pub async fn upsert_conditional_alias(
    pool: &DbPool,
    tenant_id: Uuid,
    alias: &str,
    input: UpsertConditionalAliasRequest,
) -> Result<ConditionalAliasConfig, AppError> {
    let alias = alias.trim();
    if alias.is_empty() {
        return Err(AppError::BadRequest("智能路由不能为空".to_string()));
    }

    let duplicate = sqlx::query(
        r#"
        SELECT mv.model_alias
        FROM model_visibility mv
        JOIN upstream_configs uc ON uc.id = mv.upstream_id
        WHERE uc.tenant_id = ?
        "#,
    )
    .bind(tenant_id.to_string())
    .fetch_all(pool)
    .await?;

    for row in duplicate {
        let raw: Option<String> = row.try_get("model_alias")?;
        if parse_aliases(raw.as_deref()).iter().any(|a| a == alias) {
            return Err(AppError::BadRequest(format!(
                "智能路由 '{}' 与模型别名冲突",
                alias
            )));
        }
    }

    for rule in &input.rules {
        if rule.model_name.trim().is_empty() {
            return Err(AppError::BadRequest(
                "条件路由的 model_name 不能为空".to_string(),
            ));
        }
        if let (Some(min), Some(max)) = (rule.min_input_tokens, rule.max_input_tokens) {
            if min > max {
                return Err(AppError::BadRequest(
                    "条件路由中 min_input_tokens 不能大于 max_input_tokens".to_string(),
                ));
            }
        }
        let keywords = normalize_keywords(rule.keywords.clone());
        if rule.min_input_tokens.is_none() && rule.max_input_tokens.is_none() && keywords.is_empty()
            && !rule.has_image && rule.start_time.is_none() && rule.end_time.is_none()
        {
            return Err(AppError::BadRequest(
                "每条条件路由至少需要一个匹配条件（token上下限、关键字、包含图片或时间段）".to_string(),
            ));
        }
        if let (Some(ref start), Some(ref end)) = (&rule.start_time, &rule.end_time) {
            if start >= end {
                return Err(AppError::BadRequest(
                    "条件路由中开始时间必须早于结束时间（不支持跨日）".to_string(),
                ));
            }
        }
    }

    if input.fallback.model_name.trim().is_empty() {
        return Err(AppError::BadRequest(
            "兜底路由的 model_name 不能为空".to_string(),
        ));
    }

    let normalized_user_ids = normalize_user_ids(input.user_ids.clone());
    let all_users_visible = input.all_users_visible || normalized_user_ids.is_empty();

    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM conditional_alias_routes WHERE tenant_id = ? AND alias = ?")
        .bind(tenant_id.to_string())
        .bind(alias)
        .execute(&mut *tx)
        .await?;

    for (idx, rule) in input.rules.iter().enumerate() {
        let keywords = normalize_keywords(rule.keywords.clone());
        sqlx::query(
            r#"
            INSERT INTO conditional_alias_routes
                (id, tenant_id, alias, priority, upstream_id, model_name, min_input_tokens, max_input_tokens, keywords, has_image, start_time, end_time, is_fallback, all_users_visible, user_ids, status, created_at, updated_at)
            VALUES
                (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, FALSE, ?, ?, 'active', datetime('now'), datetime('now'))
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(tenant_id.to_string())
        .bind(alias)
        .bind((idx as i32) + 1)
        .bind(rule.upstream_id.to_string())
        .bind(rule.model_name.trim())
        .bind(rule.min_input_tokens)
        .bind(rule.max_input_tokens)
        .bind(if keywords.is_empty() {
            None
        } else {
            Some(keywords.join(","))
        })
        .bind(rule.has_image)
        .bind(rule.start_time.as_deref().filter(|s| !s.is_empty()))
        .bind(rule.end_time.as_deref().filter(|s| !s.is_empty()))
        .bind(all_users_visible)
        .bind(if all_users_visible {
            None
        } else {
            Some(
                normalized_user_ids
                    .iter()
                    .map(Uuid::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            )
        })
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query(
        r#"
        INSERT INTO conditional_alias_routes
            (id, tenant_id, alias, priority, upstream_id, model_name, min_input_tokens, max_input_tokens, keywords, has_image, start_time, end_time, is_fallback, all_users_visible, user_ids, status, created_at, updated_at)
        VALUES
            (?, ?, ?, ?, ?, ?, NULL, NULL, NULL, FALSE, NULL, NULL, TRUE, ?, ?, 'active', datetime('now'), datetime('now'))
        "#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(tenant_id.to_string())
    .bind(alias)
    .bind((input.rules.len() as i32) + 1)
    .bind(input.fallback.upstream_id.to_string())
    .bind(input.fallback.model_name.trim())
    .bind(all_users_visible)
    .bind(if all_users_visible {
        None
    } else {
        Some(
            normalized_user_ids
                .iter()
                .map(Uuid::to_string)
                .collect::<Vec<_>>()
                .join(","),
        )
    })
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    let list = list_conditional_aliases(pool, tenant_id).await?;
    list.into_iter()
        .find(|cfg| cfg.alias == alias)
        .ok_or_else(|| AppError::Internal("智能路由保存成功但读取失败".to_string()))
}

pub async fn update_conditional_alias_visibility(
    pool: &DbPool,
    tenant_id: Uuid,
    alias: &str,
    all_users_visible: bool,
    user_ids: Vec<Uuid>,
) -> Result<ConditionalAliasConfig, AppError> {
    let alias = alias.trim();
    if alias.is_empty() {
        return Err(AppError::BadRequest("智能路由不能为空".to_string()));
    }
    let normalized_user_ids = normalize_user_ids(user_ids);
    if !all_users_visible && normalized_user_ids.is_empty() {
        return Err(AppError::BadRequest(
            "当智能路由不是全部用户可见时，至少需要选择一个用户".to_string(),
        ));
    }

    let updated = sqlx::query(
        r#"
        UPDATE conditional_alias_routes
        SET all_users_visible = ?,
            user_ids = ?,
            updated_at = datetime('now')
        WHERE tenant_id = ? AND alias = ?
        "#,
    )
    .bind(all_users_visible)
    .bind(if all_users_visible {
        None
    } else {
        Some(
            normalized_user_ids
                .iter()
                .map(Uuid::to_string)
                .collect::<Vec<_>>()
                .join(","),
        )
    })
    .bind(tenant_id.to_string())
    .bind(alias)
    .execute(pool)
    .await?
    .rows_affected();

    if updated == 0 {
        return Err(AppError::NotFound("智能路由不存在".to_string()));
    }

    let list = list_conditional_aliases(pool, tenant_id).await?;
    list.into_iter()
        .find(|cfg| cfg.alias == alias)
        .ok_or_else(|| AppError::Internal("智能路由可见性更新成功但读取失败".to_string()))
}

pub async fn delete_conditional_alias(
    pool: &DbPool,
    tenant_id: Uuid,
    alias: &str,
) -> Result<(), AppError> {
    let alias = alias.trim();
    if alias.is_empty() {
        return Err(AppError::BadRequest("智能路由不能为空".to_string()));
    }
    let deleted =
        sqlx::query("DELETE FROM conditional_alias_routes WHERE tenant_id = ? AND alias = ?")
            .bind(tenant_id.to_string())
            .bind(alias)
            .execute(pool)
            .await?
            .rows_affected();

    if deleted == 0 {
        return Err(AppError::NotFound("智能路由不存在".to_string()));
    }

    Ok(())
}
