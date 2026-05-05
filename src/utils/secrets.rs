use aes_gcm::{
    aead::{rand_core::RngCore, Aead, OsRng},
    Aes256Gcm, KeyInit, Nonce,
};
use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use sha2::{Digest, Sha256};

use crate::{config::Config, utils::error::AppError};

const UPSTREAM_KEY_PREFIX: &str = "enc:v1";
const INSECURE_DEFAULT_JWT_SECRET: &str = "change-me-in-production";

pub fn encrypt_upstream_api_key(plaintext: &str) -> Result<String, AppError> {
    if plaintext.is_empty() {
        return Ok(String::new());
    }

    let cipher = Aes256Gcm::new_from_slice(&load_upstream_encryption_key()?).map_err(|e| {
        AppError::Internal(format!("Failed to initialize upstream key cipher: {}", e))
    })?;
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let encrypted = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| AppError::Internal(format!("Failed to encrypt upstream API key: {}", e)))?;

    Ok(format!(
        "{}:{}:{}",
        UPSTREAM_KEY_PREFIX,
        STANDARD_NO_PAD.encode(nonce_bytes),
        STANDARD_NO_PAD.encode(encrypted)
    ))
}

pub fn encrypt_upstream_api_key_with_secret(
    plaintext: &str,
    secret: &str,
) -> Result<String, AppError> {
    if plaintext.is_empty() {
        return Ok(String::new());
    }

    let cipher = Aes256Gcm::new_from_slice(&derive_key(secret)).map_err(|e| {
        AppError::Internal(format!("Failed to initialize upstream key cipher: {}", e))
    })?;
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let encrypted = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| AppError::Internal(format!("Failed to encrypt upstream API key: {}", e)))?;

    Ok(format!(
        "{}:{}:{}",
        UPSTREAM_KEY_PREFIX,
        STANDARD_NO_PAD.encode(nonce_bytes),
        STANDARD_NO_PAD.encode(encrypted)
    ))
}

pub fn decrypt_upstream_api_key(stored_value: &str) -> Result<String, AppError> {
    if stored_value.is_empty() {
        return Ok(String::new());
    }

    let mut parts = stored_value.splitn(3, ':');
    let Some(prefix) = parts.next() else {
        return Ok(stored_value.to_string());
    };
    let Some(version) = parts.next() else {
        return Ok(stored_value.to_string());
    };
    let Some(payload) = parts.next() else {
        return Ok(stored_value.to_string());
    };

    if format!("{}:{}", prefix, version) != UPSTREAM_KEY_PREFIX {
        return Ok(stored_value.to_string());
    }

    let mut payload_parts = payload.splitn(2, ':');
    let nonce_b64 = payload_parts.next().ok_or_else(|| {
        AppError::Internal("Invalid encrypted upstream API key format".to_string())
    })?;
    let ciphertext_b64 = payload_parts.next().ok_or_else(|| {
        AppError::Internal("Invalid encrypted upstream API key payload".to_string())
    })?;

    let nonce_bytes = STANDARD_NO_PAD.decode(nonce_b64).map_err(|e| {
        AppError::Internal(format!("Failed to decode upstream API key nonce: {}", e))
    })?;
    if nonce_bytes.len() != 12 {
        return Err(AppError::Internal(
            "Invalid encrypted upstream API key nonce length".to_string(),
        ));
    }
    let ciphertext = STANDARD_NO_PAD.decode(ciphertext_b64).map_err(|e| {
        AppError::Internal(format!("Failed to decode upstream API key payload: {}", e))
    })?;

    let cipher = Aes256Gcm::new_from_slice(&load_upstream_encryption_key()?).map_err(|e| {
        AppError::Internal(format!("Failed to initialize upstream key cipher: {}", e))
    })?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&nonce_bytes), ciphertext.as_ref())
        .map_err(|e| AppError::Internal(format!("Failed to decrypt upstream API key: {}", e)))?;

    String::from_utf8(plaintext)
        .map_err(|e| AppError::Internal(format!("Invalid decrypted upstream API key: {}", e)))
}

pub fn decrypt_upstream_api_key_with_secret(
    stored_value: &str,
    secret: &str,
) -> Result<String, AppError> {
    if stored_value.is_empty() {
        return Ok(String::new());
    }

    let mut parts = stored_value.splitn(3, ':');
    let Some(prefix) = parts.next() else {
        return Ok(stored_value.to_string());
    };
    let Some(version) = parts.next() else {
        return Ok(stored_value.to_string());
    };
    let Some(payload) = parts.next() else {
        return Ok(stored_value.to_string());
    };

    if format!("{}:{}", prefix, version) != UPSTREAM_KEY_PREFIX {
        return Ok(stored_value.to_string());
    }

    let mut payload_parts = payload.splitn(2, ':');
    let nonce_b64 = payload_parts.next().ok_or_else(|| {
        AppError::Internal("Invalid encrypted upstream API key format".to_string())
    })?;
    let ciphertext_b64 = payload_parts.next().ok_or_else(|| {
        AppError::Internal("Invalid encrypted upstream API key payload".to_string())
    })?;

    let nonce_bytes = STANDARD_NO_PAD.decode(nonce_b64).map_err(|e| {
        AppError::Internal(format!("Failed to decode upstream API key nonce: {}", e))
    })?;
    let ciphertext = STANDARD_NO_PAD.decode(ciphertext_b64).map_err(|e| {
        AppError::Internal(format!("Failed to decode upstream API key payload: {}", e))
    })?;

    let cipher = Aes256Gcm::new_from_slice(&derive_key(secret)).map_err(|e| {
        AppError::Internal(format!("Failed to initialize upstream key cipher: {}", e))
    })?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&nonce_bytes), ciphertext.as_ref())
        .map_err(|e| AppError::Internal(format!("Failed to decrypt upstream API key: {}", e)))?;

    String::from_utf8(plaintext)
        .map_err(|e| AppError::Internal(format!("Invalid decrypted upstream API key: {}", e)))
}

pub fn is_insecure_default_jwt_secret(secret: &str) -> bool {
    secret.trim().is_empty() || secret == INSECURE_DEFAULT_JWT_SECRET
}

pub fn is_encrypted_upstream_api_key(value: &str) -> bool {
    value.starts_with(&format!("{}:", UPSTREAM_KEY_PREFIX))
}

fn load_upstream_encryption_key() -> Result<[u8; 32], AppError> {
    if let Ok(secret) = std::env::var("MODELPROXY_UPSTREAM_KEY_SECRET") {
        if !secret.trim().is_empty() {
            return Ok(derive_key(&secret));
        }
    }

    if let Ok(secret) = std::env::var("JWT__SECRET") {
        if !secret.trim().is_empty() {
            return Ok(derive_key(&secret));
        }
    }

    let config = Config::load().map_err(|e| {
        AppError::Internal(format!(
            "Failed to load config for upstream key encryption: {}",
            e
        ))
    })?;

    if let Some(ref upstream_secret) = config.jwt.upstream_key_secret {
        if !upstream_secret.trim().is_empty() {
            return Ok(derive_key(upstream_secret));
        }
    }

    Ok(derive_key(&config.jwt.secret))
}

pub fn generate_secure_secret() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    STANDARD_NO_PAD.encode(bytes)
}

fn derive_key(secret: &str) -> [u8; 32] {
    let digest = Sha256::digest(secret.as_bytes());
    let mut key = [0u8; 32];
    key.copy_from_slice(&digest);
    key
}

#[cfg(test)]
mod tests {
    use super::{
        decrypt_upstream_api_key, encrypt_upstream_api_key, is_insecure_default_jwt_secret,
    };

    #[test]
    fn treats_plaintext_values_as_legacy_and_keeps_them_readable() {
        let value = "legacy-secret";
        assert_eq!(decrypt_upstream_api_key(value).unwrap(), value);
    }

    #[test]
    fn detects_insecure_jwt_secret() {
        assert!(is_insecure_default_jwt_secret("change-me-in-production"));
        assert!(is_insecure_default_jwt_secret("   "));
        assert!(!is_insecure_default_jwt_secret("a-real-secret"));
    }

    #[test]
    fn encrypts_and_decrypts_round_trip() {
        std::env::set_var("MODELPROXY_UPSTREAM_KEY_SECRET", "test-upstream-secret");
        let ciphertext = encrypt_upstream_api_key("super-secret").unwrap();
        assert_ne!(ciphertext, "super-secret");
        assert_eq!(
            decrypt_upstream_api_key(&ciphertext).unwrap(),
            "super-secret"
        );
        std::env::remove_var("MODELPROXY_UPSTREAM_KEY_SECRET");
    }
}
