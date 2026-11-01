//! Storage provenance. Consumers decide when an observation needs network validation.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheCompleteness {
    Complete,
    Partial { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheMetadata {
    /// Version of the consumer's explicit DTO, separate from the database schema.
    pub representation_version: u32,
    pub fetched_at_ms: i64,
    pub verified_at_ms: i64,
    pub completeness: CacheCompleteness,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub cache_control: Option<String>,
}

impl CacheMetadata {
    pub fn permits_storage(&self) -> bool {
        self.cache_control.as_deref().is_none_or(|value| {
            !value.split(',').any(|directive| {
                directive
                    .split('=')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .eq_ignore_ascii_case("no-store")
            })
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheSnapshot {
    /// A historical observation, never proof of current permission or freshness.
    pub metadata: CacheMetadata,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMiss {
    NotCached,
    Corrupt,
    RepresentationVersion,
}
