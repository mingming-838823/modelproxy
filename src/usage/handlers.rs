use crate::store::AdminState;
use axum::{
    extract::{Query, State},
    Extension, Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    db,
    models::usage::{DailyUsage, ModelUsage, UsageAnalysis, UsageQuery, UserUsage},
    utils::error::{AppError, AppResult},
};

#[derive(Debug, Deserialize)]
pub struct DateRangeQuery {
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
    pub model_keyword: Option<String>,
    pub top_n: Option<i32>,
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct UsageSummaryResponse {
    pub total_requests: i64,
    pub total_tokens: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub today_requests: i64,
    pub today_tokens: i64,
    pub daily_usage: Vec<DailyUsage>,
    pub model_usage: Vec<ModelUsage>,
    pub upstream_model_usage: Vec<ModelUsage>,
    pub analysis: UsageAnalysis,
    pub user_usage: Option<Vec<UserUsage>>,
}

#[derive(Debug, Serialize)]
pub struct UserQuotaResponse {
    pub daily_request_limit: i64,
    pub monthly_request_limit: i64,
    pub daily_request_used: i64,
    pub monthly_request_used: i64,
}

pub async fn get_my_usage(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Query(query): Query<DateRangeQuery>,
) -> AppResult<Json<UsageSummaryResponse>> {
    let usage_query = UsageQuery {
        start_time: query.start_time,
        end_time: query.end_time,
        user_id: Some(auth_user.user_id),
        api_key_id: None,
        model: None,
        model_keyword: query.model_keyword.clone(),
        group_by: None,
        top_n: query.top_n,
        page: 1,
        page_size: 20,
    };

    let summary = db::usage::get_user_summary(
        &state.pool,
        auth_user.tenant_id,
        auth_user.user_id,
        usage_query.clone(),
    )
    .await?;
    let today =
        db::usage::get_today_summary(&state.pool, auth_user.tenant_id, Some(auth_user.user_id))
            .await?;

    let daily =
        db::usage::get_daily_usage(&state.pool, auth_user.tenant_id, usage_query.clone()).await?;
    let models =
        db::usage::get_model_usage(&state.pool, auth_user.tenant_id, usage_query.clone()).await?;
    let upstream_models = if auth_user.is_admin() {
        db::usage::get_upstream_model_usage(&state.pool, auth_user.tenant_id, usage_query).await?
    } else {
        Vec::new()
    };
    let mut analysis = db::usage::get_usage_analysis(
        &state.pool,
        auth_user.tenant_id,
        Some(auth_user.user_id),
        query.start_time,
        query.end_time,
    )
    .await?;
    if !auth_user.is_admin() {
        analysis.distinct_upstream_models = 0;
        analysis.fallback_to_requested_model_requests = 0;
    }

    Ok(Json(UsageSummaryResponse {
        total_requests: summary.total_requests,
        total_tokens: summary.total_tokens,
        total_input_tokens: summary.total_input_tokens,
        total_output_tokens: summary.total_output_tokens,
        today_requests: today.requests,
        today_tokens: today.tokens,
        daily_usage: daily,
        model_usage: models,
        upstream_model_usage: upstream_models,
        analysis,
        user_usage: None,
    }))
}

pub async fn get_my_quota(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
) -> AppResult<Json<UserQuotaResponse>> {
    let user = state
        .store
        .get_user(auth_user.user_id)
        .await
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    let (daily_request_used, monthly_request_used) =
        state.store.get_user_quota_usage(auth_user.user_id);

    Ok(Json(UserQuotaResponse {
        daily_request_limit: user.daily_request_limit,
        monthly_request_limit: user.monthly_request_limit,
        daily_request_used,
        monthly_request_used,
    }))
}

pub async fn get_all_usage(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Query(query): Query<DateRangeQuery>,
) -> AppResult<Json<UsageSummaryResponse>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let usage_query = UsageQuery {
        start_time: query.start_time,
        end_time: query.end_time,
        user_id: None,
        api_key_id: None,
        model: None,
        model_keyword: query.model_keyword.clone(),
        group_by: None,
        top_n: query.top_n,
        page: 1,
        page_size: 20,
    };

    let summary =
        db::usage::get_summary(&state.pool, auth_user.tenant_id, usage_query.clone()).await?;
    let today = db::usage::get_today_summary(&state.pool, auth_user.tenant_id, None).await?;

    let daily =
        db::usage::get_daily_usage(&state.pool, auth_user.tenant_id, usage_query.clone()).await?;
    let models =
        db::usage::get_model_usage(&state.pool, auth_user.tenant_id, usage_query.clone()).await?;
    let upstream_models = db::usage::get_upstream_model_usage(
        &state.pool,
        auth_user.tenant_id,
        usage_query.clone(),
    )
    .await?;
    let analysis = db::usage::get_usage_analysis(
        &state.pool,
        auth_user.tenant_id,
        None,
        query.start_time,
        query.end_time,
    )
    .await?;
    let users = db::usage::get_user_usage_stats(&state.pool, auth_user.tenant_id, usage_query).await?;

    Ok(Json(UsageSummaryResponse {
        total_requests: summary.total_requests,
        total_tokens: summary.total_tokens,
        total_input_tokens: summary.total_input_tokens,
        total_output_tokens: summary.total_output_tokens,
        today_requests: today.requests,
        today_tokens: today.tokens,
        daily_usage: daily,
        model_usage: models,
        upstream_model_usage: upstream_models,
        analysis,
        user_usage: Some(users),
    }))
}

pub async fn get_user_usage(
    State(state): State<AdminState>,
    Extension(auth_user): Extension<AuthUser>,
    Query(query): Query<DateRangeQuery>,
) -> AppResult<Json<UsageSummaryResponse>> {
    if !auth_user.is_admin() {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }

    let target_user_id = query.user_id.unwrap_or(auth_user.user_id);

    let usage_query = UsageQuery {
        start_time: query.start_time,
        end_time: query.end_time,
        user_id: Some(target_user_id),
        api_key_id: None,
        model: None,
        model_keyword: query.model_keyword.clone(),
        group_by: None,
        top_n: query.top_n,
        page: 1,
        page_size: 20,
    };

    let summary = db::usage::get_user_summary(
        &state.pool,
        auth_user.tenant_id,
        target_user_id,
        usage_query.clone(),
    )
    .await?;
    let today =
        db::usage::get_today_summary(&state.pool, auth_user.tenant_id, Some(target_user_id))
            .await?;

    let daily =
        db::usage::get_daily_usage(&state.pool, auth_user.tenant_id, usage_query.clone()).await?;
    let models =
        db::usage::get_model_usage(&state.pool, auth_user.tenant_id, usage_query.clone()).await?;
    let upstream_models =
        db::usage::get_upstream_model_usage(&state.pool, auth_user.tenant_id, usage_query).await?;
    let analysis = db::usage::get_usage_analysis(
        &state.pool,
        auth_user.tenant_id,
        Some(target_user_id),
        query.start_time,
        query.end_time,
    )
    .await?;

    Ok(Json(UsageSummaryResponse {
        total_requests: summary.total_requests,
        total_tokens: summary.total_tokens,
        total_input_tokens: summary.total_input_tokens,
        total_output_tokens: summary.total_output_tokens,
        today_requests: today.requests,
        today_tokens: today.tokens,
        daily_usage: daily,
        model_usage: models,
        upstream_model_usage: upstream_models,
        analysis,
        user_usage: None,
    }))
}
