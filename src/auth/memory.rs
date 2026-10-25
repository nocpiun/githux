//! In-memory credential store used by tests and as a fallback contract mirror.
//! Tests use a throwaway instance; this store never persists and never reads or
//! modifies any real platform credential.

use crate::auth::credential::{CredentialError, CredentialRef, CredentialStore};
use crate::auth::token::Pat;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct InMemoryCredentialStore {
    entries: Mutex<HashMap<String, String>>,
    /// When true, `save` always fails. Used to test the "credential save
    /// failure does not claim login persisted" acceptance path.
    fail_saves: bool,
}

impl InMemoryCredentialStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Configurable failure injector: `save` will return `Platform` on every
    /// call, while `load`/`delete` behave normally.
    pub fn failing_saves() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            fail_saves: true,
        }
    }
}

impl CredentialStore for InMemoryCredentialStore {
    fn save(&self, reference: &CredentialRef, token: &Pat) -> Result<(), CredentialError> {
        if self.fail_saves {
            return Err(CredentialError::Platform {
                message: "injected failure".to_string(),
            });
        }
        let mut entries = self
            .entries
            .lock()
            .expect("in-memory credential lock poisoned");
        entries.insert(reference.target_name(), token.expose().to_string());
        Ok(())
    }

    fn load(&self, reference: &CredentialRef) -> Result<Option<Pat>, CredentialError> {
        let entries = self
            .entries
            .lock()
            .expect("in-memory credential lock poisoned");
        Ok(entries
            .get(&reference.target_name())
            .map(|s| Pat::from_str(s)))
    }

    fn delete(&self, reference: &CredentialRef) -> Result<(), CredentialError> {
        let mut entries = self
            .entries
            .lock()
            .expect("in-memory credential lock poisoned");
        entries.remove(&reference.target_name());
        Ok(())
    }

    fn describe(&self) -> &'static str {
        "in-memory (test)"
    }
}
