#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::SocketAddr;
use std::path::Path;

fn resolve_relative_path(path: &str) -> String {
    let p = Path::new(path);
    if p.is_absolute() {
        return path.to_string();
    }
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            return exe_dir.join(path).to_string_lossy().to_string();
        }
    }
    path.to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub server: ServerConfig,
    pub admin: AdminServerConfig,
    pub database: DatabaseConfig,
    pub jwt: JwtConfig,
    pub proxy: ProxyConfig,
    pub rate_limit: RateLimitConfig,
    #[serde(default)]
    pub audit_log: AuditLogConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub workers: usize,
}

impl ServerConfig {
    pub fn addr(&self) -> SocketAddr {
        format!("{}:{}", self.host, self.port)
            .parse()
            .expect("Invalid server address")
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AdminServerConfig {
    pub host: String,
    pub port: u16,
    pub base_url: Option<String>,
    #[serde(default)]
    pub allow_public_registration: bool,
}

impl AdminServerConfig {
    pub fn addr(&self) -> SocketAddr {
        format!("{}:{}", self.host, self.port)
            .parse()
            .expect("Invalid admin server address")
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct DatabaseConfig {
    pub path: String,
    pub max_connections: u32,
}

impl DatabaseConfig {
    pub fn get_db_path(&self) -> String {
        resolve_relative_path(&self.path)
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct JwtConfig {
    #[serde(default = "default_jwt_secret")]
    pub secret: String,
    pub expiration_hours: u64,
    #[serde(default)]
    pub upstream_key_secret: Option<String>,
}

fn default_jwt_secret() -> String {
    "Rb9t2CSk+LsbCBqlKOtsp2VHQF25swfQ6TFvx9QiwQw".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProxyConfig {
    pub request_timeout_secs: u64,
    pub connect_timeout_secs: u64,
    pub max_idle_connections: usize,
    #[serde(default = "default_proxy_max_request_body_hard_bytes")]
    pub max_request_body_bytes: usize,
    #[serde(default = "default_proxy_max_text_request_body_bytes")]
    pub max_text_request_body_bytes: usize,
    #[serde(default = "default_proxy_max_multimodal_request_body_bytes")]
    pub max_multimodal_request_body_bytes: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RateLimitConfig {
    pub cleanup_interval_secs: u64,
    pub window_size_secs: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuditLogConfig {
    #[serde(default = "default_audit_log_path")]
    pub path: String,
    #[serde(default = "default_audit_log_enabled")]
    pub enabled: bool,
}

fn default_audit_log_path() -> String {
    "data/audit_logs".to_string()
}

fn default_audit_log_enabled() -> bool {
    true
}

impl Default for AuditLogConfig {
    fn default() -> Self {
        Self {
            path: default_audit_log_path(),
            enabled: default_audit_log_enabled(),
        }
    }
}

impl AuditLogConfig {
    pub fn get_resolved_path(&self) -> String {
        resolve_relative_path(&self.path)
    }
}

impl Config {
    pub fn load() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let exe_config = exe_dir.join("config.json");
                if exe_config.exists() {
                    let content = std::fs::read_to_string(exe_config)?;
                    let config = serde_json::from_str::<Config>(&content)?;
                    return Ok(config);
                }
            }
        }

        let cwd_config = Path::new("config.json");
        if cwd_config.exists() {
            let content = std::fs::read_to_string(cwd_config)?;
            let config = serde_json::from_str::<Config>(&content)?;
            return Ok(config);
        }

        let config = Self::from_env()?;

        let save_path = Self::get_save_path();
        if let Some(parent) = save_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = config.save_to_file(save_path.to_string_lossy().as_ref());

        Ok(config)
    }

    pub fn get_save_path() -> std::path::PathBuf {
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                return exe_dir.join("config.json");
            }
        }

        std::path::PathBuf::from("config.json")
    }

    pub fn save_to_file(
        &self,
        file_path: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(file_path, content)?;
        Ok(())
    }

    pub fn from_env() -> Result<Self, config::ConfigError> {
        config::Config::builder()
            .set_default("server.host", "0.0.0.0")?
            .set_default("server.port", 3000)?
            .set_default("server.workers", 8)?
            .set_default("admin.host", "127.0.0.1")?
            .set_default("admin.port", 3001)?
            .set_default("admin.allow_public_registration", false)?
            .set_default("database.path", "data/modelproxy.db")?
            .set_default("database.max_connections", 10)?
            .set_default("jwt.expiration_hours", 0)?
            .set_default("jwt.secret", default_jwt_secret())?
            .set_default("proxy.request_timeout_secs", 300)?
            .set_default("proxy.connect_timeout_secs", 30)?
            .set_default("proxy.max_idle_connections", 200)?
            .set_default(
                "proxy.max_request_body_bytes",
                default_proxy_max_request_body_hard_bytes() as u64,
            )?
            .set_default(
                "proxy.max_text_request_body_bytes",
                default_proxy_max_text_request_body_bytes() as u64,
            )?
            .set_default(
                "proxy.max_multimodal_request_body_bytes",
                default_proxy_max_multimodal_request_body_bytes() as u64,
            )?
            .set_default("rate_limit.cleanup_interval_secs", 60)?
            .set_default("rate_limit.window_size_secs", 60)?
            .add_source(config::Environment::default().separator("__"))
            .build()?
            .try_deserialize()
    }
}

impl fmt::Debug for DatabaseConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DatabaseConfig")
            .field("path", &self.path)
            .field("max_connections", &self.max_connections)
            .finish()
    }
}

impl fmt::Debug for JwtConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JwtConfig")
            .field("secret", &redact_secret(&self.secret))
            .field(
                "upstream_key_secret",
                &self
                    .upstream_key_secret
                    .as_deref()
                    .map(redact_secret)
                    .unwrap_or_else(|| "<empty>".to_string()),
            )
            .field("expiration_hours", &self.expiration_hours)
            .finish()
    }
}

fn default_proxy_max_request_body_hard_bytes() -> usize {
    32 * 1024 * 1024
}

fn default_proxy_max_text_request_body_bytes() -> usize {
    2 * 1024 * 1024
}

fn default_proxy_max_multimodal_request_body_bytes() -> usize {
    20 * 1024 * 1024
}

fn redact_secret(secret: &str) -> String {
    if secret.is_empty() {
        return "<empty>".to_string();
    }
    format!("<redacted:{} chars>", secret.chars().count())
}

#[cfg(test)]
mod tests {
    use super::redact_secret;

    #[test]
    fn redacts_secret_value() {
        assert_eq!(redact_secret("abcdef"), "<redacted:6 chars>");
    }
}
