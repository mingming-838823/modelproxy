#![allow(dead_code)]
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use dashmap::DashMap;
use parking_lot::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    pub rpm_limit: i32,
    pub tpm_limit: i32,
    pub daily_limit: i64,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            rpm_limit: 60,
            tpm_limit: 100000,
            daily_limit: 1000000,
        }
    }
}

#[derive(Debug)]
struct SlidingWindow {
    requests: VecDeque<Instant>,
    tokens: VecDeque<(Instant, i64)>,
    daily_tokens: i64,
    daily_date: String,
}

impl SlidingWindow {
    fn new() -> Self {
        let now = Utc::now();
        Self {
            requests: VecDeque::new(),
            tokens: VecDeque::new(),
            daily_tokens: 0,
            daily_date: now.format("%Y-%m-%d").to_string(),
        }
    }

    fn cleanup(&mut self, window_secs: u64) {
        let cutoff = Instant::now() - Duration::from_secs(window_secs);

        while let Some(&front) = self.requests.front() {
            if front < cutoff {
                self.requests.pop_front();
            } else {
                break;
            }
        }

        while let Some(&(front, _)) = self.tokens.front() {
            if front < cutoff {
                self.tokens.pop_front();
            } else {
                break;
            }
        }

        let today = Utc::now().format("%Y-%m-%d").to_string();
        if self.daily_date != today {
            self.daily_tokens = 0;
            self.daily_date = today;
        }
    }
}

#[derive(Clone)]
pub struct RateLimiter {
    windows: Arc<DashMap<Uuid, RwLock<SlidingWindow>>>,
    window_secs: u64,
}

impl RateLimiter {
    pub fn new(window_secs: u64, _cleanup_interval_secs: u64) -> Self {
        let windows = Arc::new(DashMap::new());

        let windows_clone = windows.clone();
        let window_secs_clone = window_secs;

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;

                windows_clone.retain(|_key: &Uuid, window: &mut RwLock<SlidingWindow>| {
                    let mut w = window.write();
                    w.cleanup(window_secs_clone);
                    true
                });
            }
        });

        Self {
            windows,
            window_secs,
        }
    }

    pub fn check_rate_limit(&self, key_id: Uuid, config: &RateLimitConfig) -> Result<(), String> {
        let window = self
            .windows
            .entry(key_id)
            .or_insert_with(|| RwLock::new(SlidingWindow::new()));
        let mut w = window.write();
        w.cleanup(self.window_secs);

        let rpm = w.requests.len() as i32;
        if config.rpm_limit > 0 && rpm >= config.rpm_limit {
            return Err(format!(
                "Rate limit exceeded: {} requests per minute",
                config.rpm_limit
            ));
        }

        let tpm: i64 = w.tokens.iter().map(|(_, t)| *t).sum();
        if config.tpm_limit > 0 && tpm >= config.tpm_limit as i64 {
            return Err(format!(
                "Rate limit exceeded: {} tokens per minute",
                config.tpm_limit
            ));
        }

        if config.daily_limit > 0 && w.daily_tokens >= config.daily_limit {
            return Err(format!(
                "Daily limit exceeded: {} tokens",
                config.daily_limit
            ));
        }

        Ok(())
    }

    pub fn record_usage(&self, key_id: Uuid, tokens: i64) {
        let window = self
            .windows
            .entry(key_id)
            .or_insert_with(|| RwLock::new(SlidingWindow::new()));
        let mut w = window.write();

        w.requests.push_back(Instant::now());
        w.tokens.push_back((Instant::now(), tokens));
        w.daily_tokens += tokens;
    }

    pub fn get_current_usage(&self, key_id: Uuid) -> (i32, i64, i64) {
        if let Some(window) = self.windows.get(&key_id) {
            let mut w = window.write();
            w.cleanup(self.window_secs);
            (
                w.requests.len() as i32,
                w.tokens.iter().map(|(_, t)| *t).sum(),
                w.daily_tokens,
            )
        } else {
            (0, 0, 0)
        }
    }
}

#[derive(Debug, Clone)]
pub struct UpstreamRateLimitConfig {
    pub daily_request_limit: i64,
    pub monthly_request_limit: i64,
}

#[derive(Debug)]
struct UpstreamWindow {
    requests: VecDeque<Instant>,
    daily_requests: i64,
    monthly_requests: i64,
    daily_date: String,
    monthly_date: String,
}

impl UpstreamWindow {
    fn new() -> Self {
        let now = Utc::now();
        Self {
            requests: VecDeque::new(),
            daily_requests: 0,
            monthly_requests: 0,
            daily_date: now.format("%Y-%m-%d").to_string(),
            monthly_date: now.format("%Y-%m").to_string(),
        }
    }

    fn cleanup(&mut self) {
        let cutoff = Instant::now() - Duration::from_secs(60);
        while let Some(&front) = self.requests.front() {
            if front < cutoff {
                self.requests.pop_front();
            } else {
                break;
            }
        }
        let now = Utc::now();
        let today = now.format("%Y-%m-%d").to_string();
        let this_month = now.format("%Y-%m").to_string();
        if self.daily_date != today {
            self.daily_requests = 0;
            self.daily_date = today;
        }
        if self.monthly_date != this_month {
            self.monthly_requests = 0;
            self.monthly_date = this_month;
        }
    }
}

#[derive(Clone)]
pub struct UpstreamRateLimiter {
    windows: Arc<DashMap<Uuid, RwLock<UpstreamWindow>>>,
}

impl UpstreamRateLimiter {
    pub fn new() -> Self {
        Self {
            windows: Arc::new(DashMap::new()),
        }
    }

    pub fn check_limit(
        &self,
        upstream_id: Uuid,
        config: &UpstreamRateLimitConfig,
    ) -> Result<(), String> {
        let window = self
            .windows
            .entry(upstream_id)
            .or_insert_with(|| RwLock::new(UpstreamWindow::new()));
        let mut w = window.write();
        w.cleanup();

        if config.daily_request_limit > 0 && w.daily_requests >= config.daily_request_limit {
            return Err(format!(
                "Upstream daily request limit exceeded: {}",
                config.daily_request_limit
            ));
        }
        if config.monthly_request_limit > 0 && w.monthly_requests >= config.monthly_request_limit {
            return Err(format!(
                "Upstream monthly request limit exceeded: {}",
                config.monthly_request_limit
            ));
        }

        Ok(())
    }

    pub fn record_usage(&self, upstream_id: Uuid, _tokens: i64) {
        let window = self
            .windows
            .entry(upstream_id)
            .or_insert_with(|| RwLock::new(UpstreamWindow::new()));
        let mut w = window.write();
        w.cleanup();
        w.requests.push_back(Instant::now());
        w.daily_requests += 1;
        w.monthly_requests += 1;
    }

    pub fn reset_upstream_window(&self, upstream_id: Uuid) {
        self.windows.remove(&upstream_id);
    }
}
