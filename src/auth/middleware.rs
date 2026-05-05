use std::sync::Arc;

use axum::{
    extract::Request,
    middleware::Next,
    response::Response,
};
use axum_extra::{
    headers::{authorization::Bearer, Authorization},
    TypedHeader,
};

use crate::auth::jwt::{AuthUser, JwtService};
use crate::store::StoreManager;
use crate::utils::error::AppError;

pub async fn auth_middleware(
    axum::extract::State((jwt_service, store)): axum::extract::State<(Arc<JwtService>, Arc<StoreManager>)>,
    TypedHeader(auth): TypedHeader<Authorization<Bearer>>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let token = auth.token();
    let claims = jwt_service.validate_token(token)?;

    let auth_user = AuthUser::from(claims);

    if store.get_user(auth_user.user_id).await.is_none() {
        return Err(AppError::Unauthorized("User account is disabled or deleted".to_string()));
    }

    request.extensions_mut().insert(auth_user);

    Ok(next.run(request).await)
}
