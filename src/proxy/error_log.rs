#![allow(dead_code)]
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write as IoWrite;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::utils::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamErrorEntry {
    pub timestamp: String,
    pub upstream_url: String,
    pub model_name: String,
    pub error_message: String,
    pub status_code: String,
}

impl UpstreamErrorEntry {
    pub fn to_log_line(&self) -> String {
        format!(
            "[{}] upstream_url={} model={} status_code={} error={}\n",
            self.timestamp, self.upstream_url, self.model_name, self.status_code, self.error_message
        )
    }

    pub fn to_gui_line(&self) -> String {
        format!(
            "[{}] upstream={} model={} code={} error={}",
            self.timestamp, self.upstream_url, self.model_name, self.status_code, self.error_message
        )
    }
}

#[derive(Clone)]
pub struct UpstreamErrorLogger {
    file: Arc<Mutex<File>>,
    tx: tokio::sync::broadcast::Sender<UpstreamErrorEntry>,
}

impl UpstreamErrorLogger {
    pub fn new(log_dir: &std::path::Path) -> Result<Self, AppError> {
        let log_path = log_dir.join("error.log");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|e| {
                AppError::Internal(format!("Failed to open error.log at {:?}: {}", log_path, e))
            })?;

        let (tx, _) = tokio::sync::broadcast::channel(256);

        Ok(Self {
            file: Arc::new(Mutex::new(file)),
            tx,
        })
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<UpstreamErrorEntry> {
        self.tx.subscribe()
    }

    pub async fn log_error(&self, entry: UpstreamErrorEntry) {
        let line = entry.to_log_line();

        let mut file = self.file.lock().await;
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();

        let _ = self.tx.send(entry);
    }

    pub async fn log(
        &self,
        upstream_url: &str,
        model_name: &str,
        error_message: &str,
        status_code: Option<u16>,
    ) {
        let entry = UpstreamErrorEntry {
            timestamp: Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            upstream_url: upstream_url.to_string(),
            model_name: model_name.to_string(),
            error_message: error_message.to_string(),
            status_code: status_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "N/A".to_string()),
        };
        self.log_error(entry).await;
    }
}
