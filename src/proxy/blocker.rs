#![allow(dead_code)]
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use uuid::Uuid;

const DEFAULT_BLOCK_DURATION_SECS: u64 = 600;

#[derive(Debug, Clone)]
pub struct BlockedUpstream {
    pub upstream_id: Uuid,
    pub blocked_at: Instant,
    pub unblock_at: Instant,
    pub reason: String,
}

#[derive(Clone)]
pub struct UpstreamBlocker {
    blocked: Arc<DashMap<Uuid, BlockedUpstream>>,
    block_duration_secs: u64,
}

impl UpstreamBlocker {
    pub fn new(block_duration_secs: Option<u64>) -> Self {
        let blocked = Arc::new(DashMap::new());
        let block_duration_secs = block_duration_secs.unwrap_or(DEFAULT_BLOCK_DURATION_SECS);

        let blocked_clone = blocked.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;

                let now = Instant::now();
                blocked_clone.retain(|_, blocked: &mut BlockedUpstream| now < blocked.unblock_at);
            }
        });

        Self {
            blocked,
            block_duration_secs,
        }
    }

    pub fn is_blocked(&self, upstream_id: Uuid) -> bool {
        if let Some(blocked) = self.blocked.get(&upstream_id) {
            Instant::now() < blocked.unblock_at
        } else {
            false
        }
    }

    pub fn block_upstream(&self, upstream_id: Uuid, reason: String) {
        let now = Instant::now();
        let blocked = BlockedUpstream {
            upstream_id,
            blocked_at: now,
            unblock_at: now + Duration::from_secs(self.block_duration_secs),
            reason,
        };
        self.blocked.insert(upstream_id, blocked);
    }

    pub fn unblock_upstream(&self, upstream_id: Uuid) {
        self.blocked.remove(&upstream_id);
    }

    pub fn get_blocked_reason(&self, upstream_id: Uuid) -> Option<String> {
        self.blocked.get(&upstream_id).map(|b| b.reason.clone())
    }

    pub fn get_all_blocked(&self) -> Vec<BlockedUpstream> {
        let now = Instant::now();
        self.blocked
            .iter()
            .filter(|b| now < b.unblock_at)
            .map(|b| b.clone())
            .collect()
    }
}
