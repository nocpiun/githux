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
}

#[derive(Debug, Clone)]
struct AccountRecord {
    identity: AccountIdentity,
    credential: CredentialRef,
    epoch: AuthEpoch,
}

/// Snapshot captured before a request so late results can be rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthContext {
    pub account: AccountKey,
    pub epoch: AuthEpoch,
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
    /// Register a freshly verified account, replacing any prior record with the
    /// same identity and advancing its epoch. Returns the resulting summary.
    pub fn upsert_verified(
        &mut self,
        identity: AccountIdentity,
        credential: CredentialRef,
    ) -> AccountSummary {
        if let Some(slot) = self
            .accounts
            .iter_mut()
            .find(|r| r.identity.key == identity.key)
        {
            slot.identity = identity;
            slot.credential = credential;
            slot.epoch.bump();
            return AccountSummary {
                identity: slot.identity.clone(),
                epoch: slot.epoch,
            };
        }
        let epoch = AuthEpoch::initial();
        let record = AccountRecord {
            identity,
            credential,
            epoch,
        };
        self.accounts.push(record);
        self.active = Some(self.accounts.len() - 1);
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
        let slot = self
            .accounts
            .iter_mut()
            .find(|r| r.identity.key == expected)
            .ok_or(RegistryError::NotFound)?;
        slot.identity = identity;
        slot.credential = credential;
        slot.epoch.bump();
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
        self.active = match self.active {
            None => None,
            Some(_) if len == 0 => None,
            Some(active) if active == idx => Some(if idx == 0 { 0 } else { idx - 1 }),
            Some(active) if active > idx => Some(active - 1),
            Some(active) => Some(active),
        };
        Ok(())
    }

    pub fn set_active(&mut self, account: AccountKey) -> Result<(), RegistryError> {
        let idx = self
            .accounts
            .iter()
            .position(|r| r.identity.key == account)
            .ok_or(RegistryError::NotFound)?;
        self.active = Some(idx);
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
