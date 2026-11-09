//! Real avatar loading and memory caching for account and repository surfaces.
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
pub use crate::ui::avatar::AvatarKey;

pub const MAX_AVATAR_BYTES: usize = 2 * 1024 * 1024;
const CACHE_CAPACITY: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AvatarState {
    Unknown,
    Loading,
    Ready(String), // URL or loaded identifier
    Failed,
}

#[derive(Default)]
pub struct AvatarCache {
    entries: Mutex<HashMap<AvatarKey, AvatarState>>,
    order: Mutex<Vec<AvatarKey>>,
    requested: Mutex<HashSet<AvatarKey>>,
}

impl AvatarCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state(&self, key: &AvatarKey) -> AvatarState {
        self.entries
            .lock()
            .expect("avatar cache")
            .get(key)
            .cloned()
            .unwrap_or(AvatarState::Unknown)
    }

    pub fn begin(&self, key: AvatarKey) -> bool {
        let mut requested = self.requested.lock().expect("avatar requested");
        if requested.contains(&key) {
            return false;
        }
        requested.insert(key.clone());
        drop(requested);
        self.entries
            .lock()
            .expect("avatar cache")
            .insert(key, AvatarState::Loading);
        true
    }

    pub fn finish_ready(&self, key: AvatarKey, url: String) {
        let mut entries = self.entries.lock().expect("avatar cache");
        entries.insert(key.clone(), AvatarState::Ready(url));
        drop(entries);
        let mut order = self.order.lock().expect("avatar cache order");
        if !order.contains(&key) {
            order.push(key);
            if order.len() > CACHE_CAPACITY {
                let evicted = order.remove(0);
                self.entries.lock().expect("avatar cache").remove(&evicted);
            }
        }
    }

    pub fn finish_failed(&self, key: AvatarKey) {
        self.entries
            .lock()
            .expect("avatar cache")
            .insert(key, AvatarState::Failed);
    }
}
