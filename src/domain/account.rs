//! Stable account identity and authorization epoch types.

use std::fmt;

/// Stable GitHub host identifier (e.g. `github.com`).
///
/// The host is part of the isolation identity: two accounts with the same
/// numeric id on different hosts are different accounts. For the first release
/// only `github.com` is produced.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HostId(String);

impl HostId {
    /// Create from a validated host string. Only used by the auth layer after
    /// the `/user` response confirms the host; the constructor is intentionally
    /// explicit so callers cannot accidentally mix user-supplied free text.
    pub fn new(host: impl Into<String>) -> Self {
        let host = host.into();
        debug_assert!(
            !host.is_empty(),
            "HostId must not be empty; constructed at call site only"
        );
        Self(host)
    }

    /// The canonical first-release host.
    pub fn github() -> Self {
        Self::new("github.com")
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HostId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Numeric GitHub account id as returned by `GET /user` (`id` field).
///
/// This is the stable isolation identity. `login` is display-only metadata and
/// may change, so it must never be the sole identity key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountKey {
    pub id: u64,
    pub host: HostId,
}

impl AccountKey {
    pub fn new(id: u64, host: HostId) -> Self {
        Self { id, host }
    }
}

/// Full identity snapshot captured after `GET /user` succeeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountIdentity {
    pub key: AccountKey,
    /// Display login (`login` field). May change; never use as the identity key.
    pub login: String,
    /// Optional display name (`name` field, may be null).
    pub display_name: Option<String>,
    /// Optional avatar URL (`avatar_url` field).
    pub avatar_url: Option<String>,
    /// Optional profile HTML URL (`html_url` field).
    pub html_url: Option<String>,
}

/// Account summary exposed to future UI integration. Holds no credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSummary {
    pub identity: AccountIdentity,
    pub epoch: AuthEpoch,
}

/// Monotonically increasing authorization generation for one account.
///
/// Replacing a PAT or removing an account advances the epoch so that responses
/// produced under an older authorization context are discarded rather than
/// written into the new account's data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct AuthEpoch(pub u64);

impl AuthEpoch {
    pub fn initial() -> Self {
        Self(1)
    }

    /// Advance to the next generation. Used when credentials are replaced or an
    /// account is removed.
    pub fn bump(&mut self) {
        self.0 = self.0.saturating_add(1);
    }

    pub fn value(self) -> u64 {
        self.0
    }
}
