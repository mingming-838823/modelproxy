#![allow(dead_code)]
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write as IoWrite;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::utils::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub user_id: Uuid,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub details: serde_json::Value,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct FileAuditLogWriter {
    base_path: PathBuf,
    current_file: Arc<Mutex<Option<(File, String)>>>,
}

impl FileAuditLogWriter {
    pub fn new(base_path: &str) -> Result<Self, AppError> {
        let path = PathBuf::from(base_path);
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| AppError::Internal(format!("Failed to create directory: {}", e)))?;
            }
        }
        if !path.exists() {
            std::fs::create_dir_all(&path)
                .map_err(|e| AppError::Internal(format!("Failed to create directory: {}", e)))?;
        }

        Ok(Self {
            base_path: path,
            current_file: Arc::new(Mutex::new(None)),
        })
    }

    fn get_log_filename(&self) -> String {
        let now = Utc::now();
        format!("{}.log", now.format("%Y-%m-%d"))
    }

    fn get_log_path(&self, filename: &str) -> PathBuf {
        self.base_path.join(filename)
    }

    pub async fn write_log(&self, entry: AuditLogEntry) -> Result<(), AppError> {
        let filename = self.get_log_filename();
        let log_path = self.get_log_path(&filename);
        
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
                .map_err(|e| AppError::Internal(format!("Failed to open audit log file: {}", e)))?;
            *current = Some((file, filename.clone()));
        }

        if let Some((file, _)) = &mut *current {
            let json_line = serde_json::to_string(&entry)
                .map_err(|e| AppError::Internal(format!("Failed to serialize audit log: {}", e)))?;
            
            file.write_all(json_line.as_bytes())
                .map_err(|e| AppError::Internal(format!("Failed to write audit log: {}", e)))?;
            file.write_all(b"\n")
                .map_err(|e| AppError::Internal(format!("Failed to write newline: {}", e)))?;
            let _ = file.flush();
        }

        Ok(())
    }

    pub async fn list_logs(
        &self,
        tenant_id: Option<Uuid>,
        start_time: Option<DateTime<Utc>>,
        end_time: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<Vec<AuditLogEntry>, AppError> {
        let mut entries = Vec::new();
        let mut files: Vec<(String, PathBuf)> = Vec::new();

        let read_dir = std::fs::read_dir(&self.base_path)
            .map_err(|e| AppError::Internal(format!("Failed to read directory: {}", e)))?;

        for entry in read_dir {
            if let Ok(entry) = entry {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    if ext == "log" {
                        if let Some(filename) = path.file_name() {
                            files.push((filename.to_string_lossy().to_string(), path));
                        }
                    }
                }
            }
        }

        files.sort_by(|a, b| b.0.cmp(&a.0));

        for (_, path) in files {
            if entries.len() >= limit {
                break;
            }

            let content = std::fs::read_to_string(&path)
                .map_err(|e| AppError::Internal(format!("Failed to read log file: {}", e)))?;

            for line in content.lines().rev() {
                if entries.len() >= limit {
                    break;
                }

                if line.trim().is_empty() {
                    continue;
                }

                if let Ok(entry) = serde_json::from_str::<AuditLogEntry>(line) {
                    if let Some(tid) = tenant_id {
                        if entry.tenant_id != Some(tid) {
                            continue;
                        }
                    }

                    if let Some(start) = start_time {
                        if entry.created_at < start {
                            continue;
                        }
                    }

                    if let Some(end) = end_time {
                        if entry.created_at > end {
                            continue;
                        }
                    }

                    entries.push(entry);
                }
            }
        }

        Ok(entries)
    }

    pub async fn count_logs(
        &self,
        tenant_id: Option<Uuid>,
    ) -> Result<i64, AppError> {
        let mut count: i64 = 0;
        let read_dir = std::fs::read_dir(&self.base_path)
            .map_err(|e| AppError::Internal(format!("Failed to read directory: {}", e)))?;

        for entry in read_dir {
            if let Ok(entry) = entry {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    if ext != "log" {
                        continue;
                    }
                } else {
                    continue;
                }

                let content = std::fs::read_to_string(&path)
                    .map_err(|e| AppError::Internal(format!("Failed to read log file: {}", e)))?;

                for line in content.lines() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    if let Ok(entry) = serde_json::from_str::<AuditLogEntry>(line) {
                        if let Some(tid) = tenant_id {
                            if entry.tenant_id != Some(tid) {
                                continue;
                            }
                        }
                        count += 1;
                    }
                }
            }
        }

        Ok(count)
    }

    pub async fn find_by_id(&self, id: Uuid) -> Result<Option<AuditLogEntry>, AppError> {
        let read_dir = std::fs::read_dir(&self.base_path)
            .map_err(|e| AppError::Internal(format!("Failed to read directory: {}", e)))?;

        for entry in read_dir {
            if let Ok(entry) = entry {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    if ext != "log" {
                        continue;
                    }
                }

                let content = std::fs::read_to_string(&path)
                    .map_err(|e| AppError::Internal(format!("Failed to read log file: {}", e)))?;

                for line in content.lines() {
                    if line.trim().is_empty() {
                        continue;
                    }

                    if let Ok(log_entry) = serde_json::from_str::<AuditLogEntry>(line) {
                        if log_entry.id == id {
                            return Ok(Some(log_entry));
                        }
                    }
                }
            }
        }

        Ok(None)
    }
}

impl Clone for FileAuditLogWriter {
    fn clone(&self) -> Self {
        Self {
            base_path: self.base_path.clone(),
            current_file: Arc::clone(&self.current_file),
        }
    }
}
