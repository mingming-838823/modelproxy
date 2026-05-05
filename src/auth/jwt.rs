use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::JwtConfig;
use crate::utils::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: Uuid,
    pub tenant_id: Uuid,
    pub role: String,
    pub exp: i64,
    pub iat: i64,
}

#[derive(Clone)]
pub struct JwtService {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    expiration_hours: u64,
}

impl JwtService {
    pub fn new(config: &JwtConfig) -> Self {
        Self {
            encoding_key: EncodingKey::from_secret(config.secret.as_bytes()),
            decoding_key: DecodingKey::from_secret(config.secret.as_bytes()),
            expiration_hours: config.expiration_hours,
        }
    }

    pub fn generate_token(
        &self,
        user_id: Uuid,
        tenant_id: Uuid,
        role: &str,
    ) -> Result<String, AppError> {
        let now = Utc::now();
        let exp = if self.expiration_hours == 0 {
            now + Duration::days(36500)
        } else {
            now + Duration::hours(self.expiration_hours as i64)
        };

        let claims = Claims {
            sub: user_id,
            tenant_id,
            role: role.to_string(),
            exp: exp.timestamp(),
            iat: now.timestamp(),
        };

        encode(&Header::default(), &claims, &self.encoding_key).map_err(AppError::from)
    }

    pub fn validate_token(&self, token: &str) -> Result<Claims, AppError> {
        let mut validation = Validation::default();
        if self.expiration_hours == 0 {
            validation.validate_exp = false;
        }
        decode::<Claims>(token, &self.decoding_key, &validation)
            .map(|data| data.claims)
            .map_err(AppError::from)
    }
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: Uuid,
    pub tenant_id: Uuid,
    pub role: String,
}

impl From<Claims> for AuthUser {
    fn from(claims: Claims) -> Self {
        Self {
            user_id: claims.sub,
            tenant_id: claims.tenant_id,
            role: claims.role,
        }
    }
}

impl AuthUser {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }
}
