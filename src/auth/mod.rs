//! Accounts, active identity, authorization epochs and platform credential
//! storage for Classic PAT authentication.
//!
//! Design boundaries enforced here:
//! - The token ([`token::Pat`]) never implements `Debug`, `Display` or
//!   `serde::Serialize`; its only accessor is `expose()` which is `pub(crate)`.
//! - The in-memory registry holds [`AccountSummary`] and a [`CredentialRef`]
//!   pointing at the platform store — never the token itself.
//! - [`windows::WindowsCredentialStore`] is the only supported backing store in
//!   the first release; other platforms use [`store::UnsupportedStore`] and
//!   compile without a plaintext fallback.
//! - Replacing a PAT or removing an account advances [`AuthEpoch`] so stale
//!   responses are discarded downstream.
//!
//! Status: first reusable R2 slice. Not yet wired into UI.

// The GPUI shell does not yet consume this service layer (that is the separate
// UI-integration task). This module-scoped allowance is limited to these
// not-yet-called modules so real warnings stay visible elsewhere; the crate
// root deliberately has no `#![allow]` blanket.
#![allow(dead_code)]

pub mod capability;
pub mod credential;
pub mod memory;
pub mod token;
#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "windows")]
#[allow(unused_imports)]
pub use self::windows::WindowsCredentialStore;
#[allow(unused_imports)]
pub use capability::{
    Capability, CapabilityReport, CapabilityStatus, ScopeRequirement, minimal_scope_guidance,
    report_after_identity_check,
};
#[allow(unused_imports)]
pub use credential::{CredentialError, CredentialRef, CredentialStore};
#[allow(unused_imports)]
pub use memory::InMemoryCredentialStore;

use crate::domain::{AccountIdentity, AccountKey, AccountSummary, AuthEpoch};

/// In-memory multi-account registry. One account is *active* at a time.
///
/// The registry owns display metadata and credential references, never raw
/// tokens. It is the authority for account identity and authorization epoch;
/// the data layer snapshots an [`AuthContext`] before issuing a request and
/// validates it again when results arrive.
#[derive(Debug, Default)]
pub struct AccountRegistry {
    accounts: Vec<AccountRecord>,
    active: Option<usize>,
    /// Registry-wide monotonic epoch allocator. Epochs come from here rather
    /// than per-account counters so that removing and re-adding the same
    /// account cannot reuse an epoch (ABA): a response captured for an old
    /// `{account, epoch}` pair can never match a newly issued epoch again.
    next_epoch: u64,
    /// Monotonic generation bumped on every actual change of the active
    /// account. Part of [`AuthContext`] so that A → B → A still invalidates
    /// in-flight responses started before the first switch.
    selection_epoch: u64,
}

#[derive(Debug, Clone)]
struct AccountRecord {
    identity: AccountIdentity,
    credential: CredentialRef,
    epoch: AuthEpoch,
}

/// Snapshot captured before a request so late results can be rejected.
///
/// `selection` is a registry-wide generation that advances on **every** actual
/// change of the active account. Without it, switching A → B → A would restore
/// the same `{account, epoch}` pair and an in-flight response from before the
/// switch would be accepted, contradicting the guarantee that a switch makes
/// in-flight results stale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthContext {
    pub account: AccountKey,
    pub epoch: AuthEpoch,
    pub selection: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    NotFound,
    AccountIdentityMismatch {
        expected: AccountKey,
        actual: AccountKey,
    },
    AlreadyExists,
}

impl AccountRegistry {
    /// Allocate the next registry-wide authorization epoch. Monotonic for the
    /// lifetime of the registry; never reuses a value after removal.
    fn allocate_epoch(&mut self) -> AuthEpoch {
        self.next_epoch = self.next_epoch.saturating_add(1);
        AuthEpoch(self.next_epoch)
    }

    /// Advance the selection generation. Called whenever the active account
    /// actually changes (switch, add-that-activates, remove-that-reactivates).
    fn bump_selection(&mut self) {
        self.selection_epoch = self.selection_epoch.saturating_add(1);
    }

    /// Register a freshly verified account, replacing any prior record with the
    /// same identity and advancing its epoch. Returns the resulting summary.
    pub fn upsert_verified(
        &mut self,
        identity: AccountIdentity,
        credential: CredentialRef,
    ) -> AccountSummary {
        let epoch = self.allocate_epoch();
        if let Some(slot) = self
            .accounts
            .iter_mut()
            .find(|r| r.identity.key == identity.key)
        {
            slot.identity = identity;
            slot.credential = credential;
            slot.epoch = epoch;
            return AccountSummary {
                identity: slot.identity.clone(),
                epoch: slot.epoch,
            };
        }
        let record = AccountRecord {
            identity,
            credential,
            epoch,
        };
        self.accounts.push(record);
        let new_active = self.accounts.len() - 1;
        if self.active != Some(new_active) {
            self.active = Some(new_active);
            self.bump_selection();
        }
        let slot = self.accounts.last().expect("just pushed");
        AccountSummary {
            identity: slot.identity.clone(),
            epoch: slot.epoch,
        }
    }

    /// Replace the PAT for an existing account, validating that the newly
    /// verified identity matches the stored identity key.
    pub fn replace(
        &mut self,
        expected: AccountKey,
        identity: AccountIdentity,
        credential: CredentialRef,
    ) -> Result<AccountSummary, RegistryError> {
        if identity.key != expected {
            return Err(RegistryError::AccountIdentityMismatch {
                expected,
                actual: identity.key,
            });
        }
        // Allocate before taking the mutable borrow of the account slot.
        let epoch = self.allocate_epoch();
        let slot = self
            .accounts
            .iter_mut()
            .find(|r| r.identity.key == expected)
            .ok_or(RegistryError::NotFound)?;
        slot.identity = identity;
        slot.credential = credential;
        slot.epoch = epoch;
        Ok(AccountSummary {
            identity: slot.identity.clone(),
            epoch: slot.epoch,
        })
    }

    /// Remove an account. Any in-flight response for it is rejected because
    /// [`AccountRegistry::active_context`] no longer matches the captured
    /// context (the account is gone, or a different account became active).
    ///
    /// The active pointer is kept valid: removing a non-active account shifts
    /// indices above it down by one; removing the active account activates the
    /// previous one (or the first remaining account at index 0).
    pub fn remove(&mut self, account: AccountKey) -> Result<(), RegistryError> {
        let idx = self
            .accounts
            .iter()
            .position(|r| r.identity.key == account)
            .ok_or(RegistryError::NotFound)?;

        self.accounts.remove(idx);
        let len = self.accounts.len();
        let previous_active = self.active;
        self.active = match self.active {
            None => None,
            Some(_) if len == 0 => None,
            Some(active) if active == idx => Some(if idx == 0 { 0 } else { idx - 1 }),
            Some(active) if active > idx => Some(active - 1),
            Some(active) => Some(active),
        };
        // Any removal can change which account is active (removing the active
        // one selects another; removing one below shifts the index). Treat any
        // removal as a selection change so in-flight results for the old
        // selection are rejected.
        if self.active != previous_active || self.active.is_some() {
            self.bump_selection();
        }
        Ok(())
    }

    pub fn set_active(&mut self, account: AccountKey) -> Result<(), RegistryError> {
        let idx = self
            .accounts
            .iter()
            .position(|r| r.identity.key == account)
            .ok_or(RegistryError::NotFound)?;
        if self.active != Some(idx) {
            self.active = Some(idx);
            self.bump_selection();
        }
        Ok(())
    }

    pub fn active_summary(&self) -> Option<AccountSummary> {
        let idx = self.active?;
        let record = &self.accounts[idx];
        Some(AccountSummary {
            identity: record.identity.clone(),
            epoch: record.epoch,
        })
    }

    pub fn active_context(&self) -> Option<AuthContext> {
        let idx = self.active?;
        Some(AuthContext {
            account: self.accounts[idx].identity.key.clone(),
            epoch: self.accounts[idx].epoch,
            selection: self.selection_epoch,
        })
    }

    pub fn summaries(&self) -> Vec<AccountSummary> {
        self.accounts
            .iter()
            .map(|r| AccountSummary {
                identity: r.identity.clone(),
                epoch: r.epoch,
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }

    /// Look up the credential reference for an account, if present.
    pub fn credential_ref(&self, account: AccountKey) -> Option<CredentialRef> {
        self.accounts
            .iter()
            .find(|r| r.identity.key == account)
            .map(|r| r.credential.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(id: u64, login: &str) -> AccountIdentity {
        AccountIdentity {
            key: AccountKey::new(id, crate::domain::HostId::github()),
            login: login.to_string(),
            display_name: None,
            avatar_url: None,
            html_url: None,
        }
    }

    fn cred(id: u64) -> CredentialRef {
        CredentialRef::new(crate::domain::HostId::github(), id)
    }

    #[test]
    fn login_changes_are_display_only_not_identity() {
        let mut reg = AccountRegistry::default();
        let first = reg.upsert_verified(identity(1, "old-login"), cred(1));
        // Same numeric id with a renamed login must update the same account.
        let second = reg.upsert_verified(identity(1, "new-login"), cred(1));
        assert_eq!(reg.summaries().len(), 1);
        assert_eq!(second.identity.login, "new-login");
        assert_eq!(second.identity.key.id, 1);
        // And it advances the epoch because the credential was re-saved.
        assert!(second.epoch.value() > first.epoch.value());
    }

    #[test]
    fn two_accounts_can_coexist_with_one_active() {
        let mut reg = AccountRegistry::default();
        reg.upsert_verified(identity(1, "a"), cred(1));
        reg.upsert_verified(identity(2, "b"), cred(2));
        assert_eq!(reg.summaries().len(), 2);
        assert_eq!(reg.active_context().expect("active").account.id, 2);

        reg.set_active(AccountKey::new(1, crate::domain::HostId::github()))
            .expect("switch");
        assert_eq!(reg.active_context().expect("active").account.id, 1);
    }

    #[test]
    fn replace_rejects_different_account_identity() {
        let mut reg = AccountRegistry::default();
        reg.upsert_verified(identity(1, "a"), cred(1));
        let result = reg.replace(
            AccountKey::new(1, crate::domain::HostId::github()),
            identity(2, "b"),
            cred(2),
        );
        assert!(matches!(
            result,
            Err(RegistryError::AccountIdentityMismatch { .. })
        ));
        // The original identity is untouched.
        assert_eq!(reg.summaries().len(), 1);
        assert_eq!(reg.summaries()[0].identity.key.id, 1);
    }

    #[test]
    fn replace_advances_epoch_for_the_same_account() {
        let mut reg = AccountRegistry::default();
        let first = reg.upsert_verified(identity(5, "a"), cred(5));
        let second = reg
            .replace(
                AccountKey::new(5, crate::domain::HostId::github()),
                identity(5, "a"),
                cred(5),
            )
            .expect("replace");
        assert_eq!(second.epoch.value(), first.epoch.value() + 1);
    }

    #[test]
    fn remove_active_account_activates_previous() {
        let mut reg = AccountRegistry::default();
        reg.upsert_verified(identity(1, "a"), cred(1));
        reg.upsert_verified(identity(2, "b"), cred(2));
        reg.remove(AccountKey::new(2, crate::domain::HostId::github()))
            .expect("remove");
        assert_eq!(reg.summaries().len(), 1);
        assert_eq!(reg.active_context().expect("still active").account.id, 1);
    }

    #[test]
    fn remove_before_active_keeps_the_same_active_account() {
        let mut reg = AccountRegistry::default();
        reg.upsert_verified(identity(1, "a"), cred(1));
        reg.upsert_verified(identity(2, "b"), cred(2));
        reg.upsert_verified(identity(3, "c"), cred(3));
        // Active is account 3 (the last upsert). Removing account 1 sits *before*
        // the active index; the active account must remain account 3 rather than
        // shifting to a stale index that points at account 2.
        reg.remove(AccountKey::new(1, crate::domain::HostId::github()))
            .expect("remove");
        assert_eq!(reg.active_context().expect("active").account.id, 3);
        assert_eq!(reg.summaries().len(), 2);
    }

    #[test]
    fn removing_every_account_leaves_no_active_context() {
        let mut reg = AccountRegistry::default();
        reg.upsert_verified(identity(1, "a"), cred(1));
        reg.remove(AccountKey::new(1, crate::domain::HostId::github()))
            .expect("remove");
        assert!(reg.is_empty());
        assert!(reg.active_context().is_none());
    }

    #[test]
    fn removing_unknown_account_is_not_found() {
        let mut reg = AccountRegistry::default();
        assert!(matches!(
            reg.remove(AccountKey::new(99, crate::domain::HostId::github())),
            Err(RegistryError::NotFound)
        ));
    }

    /// Regression (ABA): removing an account and re-adding it must not reuse an
    /// epoch, otherwise an in-flight response captured before the removal would
    /// match the re-added account's context and be wrongly accepted.
    #[test]
    fn epochs_are_registry_wide_monotonic_across_removal_and_readd() {
        let mut reg = AccountRegistry::default();
        let key = AccountKey::new(7, crate::domain::HostId::github());

        let first = reg.upsert_verified(identity(7, "dev"), cred(7));
        reg.remove(key.clone()).expect("remove");
        let readded = reg.upsert_verified(identity(7, "dev"), cred(7));

        assert!(
            readded.epoch.value() > first.epoch.value(),
            "re-added account reused epoch {} (first {})",
            readded.epoch.value(),
            first.epoch.value()
        );

        // The stale captured context can never equal the new active context.
        let stale = AuthContext {
            account: key.clone(),
            epoch: first.epoch,
            selection: 0,
        };
        assert_ne!(reg.active_context().as_ref(), Some(&stale));

        // A second account does not collide either.
        let other = reg.upsert_verified(identity(8, "other"), cred(8));
        assert!(other.epoch.value() > readded.epoch.value());
    }

    /// Repeated re-verification keeps increasing the epoch even within one
    /// account, so an older in-flight read can never be revalidated.
    #[test]
    fn repeated_verification_never_reuses_an_epoch() {
        let mut reg = AccountRegistry::default();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..5 {
            let summary = reg.upsert_verified(identity(3, "dev"), cred(3));
            assert!(
                seen.insert(summary.epoch.value()),
                "epoch {} was reused",
                summary.epoch.value()
            );
        }
    }

    /// Regression: switching A → B → A must still invalidate responses started
    /// before the first switch. Without a selection generation, the restored
    /// `{account, epoch}` pair would compare equal and the stale result would be
    /// accepted.
    #[test]
    fn switching_away_and_back_invalidates_in_flight_context() {
        let mut reg = AccountRegistry::default();
        let a = AccountKey::new(1, crate::domain::HostId::github());
        let b = AccountKey::new(2, crate::domain::HostId::github());
        reg.upsert_verified(identity(1, "a"), cred(1));
        reg.set_active(a.clone()).expect("activate a");
        reg.upsert_verified(identity(2, "b"), cred(2));

        // Capture the context for account A, then switch A -> B -> A.
        reg.set_active(a.clone()).expect("activate a");
        let captured = reg.active_context().expect("ctx");

        reg.set_active(b).expect("activate b");
        reg.set_active(a).expect("activate a again");

        let now = reg.active_context().expect("ctx");
        assert_eq!(now.account, captured.account, "same account");
        assert_eq!(now.epoch, captured.epoch, "same epoch (no PAT change)");
        assert_ne!(
            now, captured,
            "switching away and back must still invalidate the captured context"
        );
    }

    /// Re-selecting the account that is already active is not a switch and must
    /// not churn the selection generation.
    #[test]
    fn selecting_the_already_active_account_does_not_bump_selection() {
        let mut reg = AccountRegistry::default();
        let a = AccountKey::new(1, crate::domain::HostId::github());
        reg.upsert_verified(identity(1, "a"), cred(1));
        let before = reg.active_context().expect("ctx");
        reg.set_active(a).expect("set active");
        assert_eq!(reg.active_context().expect("ctx"), before);
    }

    #[test]
    fn hosts_are_part_of_identity() {
        let mut reg = AccountRegistry::default();
        reg.upsert_verified(identity(1, "a"), cred(1));
        let other_host = AccountIdentity {
            key: AccountKey::new(1, crate::domain::HostId::new("ghe.example.com")),
            login: "a".to_string(),
            display_name: None,
            avatar_url: None,
            html_url: None,
        };
        reg.upsert_verified(other_host, cred(1));
        assert_eq!(reg.summaries().len(), 2);
    }
}
