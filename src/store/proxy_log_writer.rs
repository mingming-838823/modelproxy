use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write as IoWrite};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::store::MessageRecord;
use crate::utils::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyLogContent {
    pub id: Uuid,
    pub request_body: Option<String>,
    pub response_body: Option<String>,
    pub messages: Vec<MessageRecord>,
}

pub struct ProxyLogWriter {
    base_path: PathBuf,
    current_file: Arc<Mutex<Option<(File, String)>>>,
}

impl ProxyLogWriter {
    pub fn new(base_path: &str) -> Result<Self, AppError> {
        let path = PathBuf::from(base_path);
        if !path.exists() {
            std::fs::create_dir_all(&path)
                .map_err(|e| AppError::Internal(format!("Failed to create proxy log directory: {}", e)))?;
        }

        Ok(Self {
            base_path: path,
            current_file: Arc::new(Mutex::new(None)),
        })
    }

    fn get_log_filename(&self) -> String {
        let now = chrono::Utc::now();
        format!("{}.jsonl", now.format("%Y-%m-%d"))
    }

    pub async fn write_content(&self, content: &ProxyLogContent) -> Result<String, AppError> {
        let filename = self.get_log_filename();
        let log_path = self.base_path.join(&filename);

        let mut current = self.current_file.lock().await;

        let needs_new_file = match &*current {
            Some((_, current_filename)) => current_filename != &filename,
            None => true,
        };

        if needs_new_file {
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .map_err(|e| AppError::Internal(format!("Failed to open proxy log file: {}", e)))?;
            *current = Some((file, filename.clone()));
        }

        if let Some((file, _)) = &mut *current {
            let json_line = serde_json::to_string(content)
                .map_err(|e| AppError::Internal(format!("Failed to serialize proxy log content: {}", e)))?;

            file.write_all(json_line.as_bytes())
                .map_err(|e| AppError::Internal(format!("Failed to write proxy log: {}", e)))?;
            file.write_all(b"\n")
                .map_err(|e| AppError::Internal(format!("Failed to write newline: {}", e)))?;
            let _ = file.flush();
        }

        Ok(filename)
    }

    pub fn read_content(&self, log_file: &str, id: Uuid) -> Result<Option<ProxyLogContent>, AppError> {
        let log_path = self.base_path.join(log_file);

        if !log_path.exists() {
            return Ok(None);
        }

        let file = File::open(&log_path)
            .map_err(|e| AppError::Internal(format!("Failed to open proxy log file: {}", e)))?;
        let reader = BufReader::new(file);

        for line in reader.lines() {
            let line = line.map_err(|e| AppError::Internal(format!("Failed to read proxy log line: {}", e)))?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(content) = serde_json::from_str::<ProxyLogContent>(&line) {
                if content.id == id {
                    return Ok(Some(content));
                }
            }
        }

        Ok(None)
    }
}

impl Clone for ProxyLogWriter {
    fn clone(&self) -> Self {
        Self {
            base_path: self.base_path.clone(),
            current_file: Arc::clone(&self.current_file),
        }
    }
}
