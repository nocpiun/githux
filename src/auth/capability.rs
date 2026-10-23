//! Capability and minimal-scope guidance returned by the auth layer.
//!
//! The service reports what a token can and cannot do rather than requiring a
//! single all-or-nothing scope set. Nothing here performs a scope check against
//! the server; it encodes the documented first-release guidance and the
//! observation that `/user` succeeded.
//!
//! Important: `repo` (needed for private repositories) also grants **write**
//! access to private repository contents. This module therefore never describes
//! a Classic PAT as "read-only". It also does not demand administration scopes
//! for the read paths implemented here.

/// A concrete piece of functionality the product knows about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Public repository read (`/user/repos` without private visibility).
    PublicRepoRead,
    /// Private repository read. Requires the coarse `repo` scope.
    PrivateRepoRead,
    /// Current-user identity and private profile fields (`read:user`).
    PrivateProfileRead,
    /// Notifications (H1). Requires `notifications` or `repo`.
    Notifications,
    /// Organization membership and private org data (`read:org`).
    OrganizationRead,
}

/// What a capability needs from a Classic PAT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeRequirement {
    /// Works with no scope at all (authenticated request only).
    None,
    /// Needs one of the listed scopes.
    AnyOf(&'static [&'static str]),
}

/// The three-state answer for whether a capability is currently available.
///
/// `Unknown` is distinct from `Unavailable`: scope lists are not authoritative
/// for organization policy or SSO, and no probe has been made for this
/// capability. Never collapse `Unknown` into `Unavailable` in UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityStatus {
    Available,
    Unavailable { needs: ScopeRequirement },
    Unknown { needs: ScopeRequirement },
}

/// Report for one capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityReport {
    pub capability: Capability,
    pub status: CapabilityStatus,
}

impl Capability {
    /// Documented minimum scope requirement for the first release.
    pub fn requirement(self) -> ScopeRequirement {
        match self {
            Capability::PublicRepoRead => ScopeRequirement::None,
            // `repo` covers private read *and write*; we cannot get read-only
            // private access from a Classic PAT.
            Capability::PrivateRepoRead => ScopeRequirement::AnyOf(&["repo"]),
            Capability::PrivateProfileRead => ScopeRequirement::AnyOf(&["read:user", "user"]),
            Capability::Notifications => ScopeRequirement::AnyOf(&["notifications", "repo"]),
            Capability::OrganizationRead => ScopeRequirement::AnyOf(&["read:org", "user"]),
        }
    }

    /// One-line explanation suitable for onboarding copy.
    pub fn explanation(self) -> &'static str {
        match self {
            Capability::PublicRepoRead => {
                "Reading public repositories needs no extra scope beyond being signed in."
            }
            Capability::PrivateRepoRead => {
                "Private repositories need the `repo` scope. It also grants write access to \
                 private repositories, so Githux does not treat this token as read-only."
            }
            Capability::PrivateProfileRead => {
                "Private profile fields need `read:user`; public profile data works without it."
            }
            Capability::Notifications => {
                "Notifications need `notifications` (or `repo`). Fine-grained tokens are not \
                 supported for this endpoint."
            }
            Capability::OrganizationRead => {
                "Organization membership detail needs `read:org`; public organization data \
                 works without it."
            }
        }
    }
}

/// Minimal scope guidance list for the onboarding UI (not a hard login gate).
///
/// The UI task decides what to ask for; the service only reports the honest
/// minimum. Organization SSO and admin policies can still reduce what a token
/// can see, so capability results remain advisory.
pub fn minimal_scope_guidance() -> Vec<(Capability, ScopeRequirement, &'static str)> {
    [
        Capability::PublicRepoRead,
        Capability::PrivateRepoRead,
        Capability::PrivateProfileRead,
        Capability::Notifications,
        Capability::OrganizationRead,
    ]
    .into_iter()
    .map(|c| (c, c.requirement(), c.explanation()))
    .collect()
}

/// Report capabilities from a single successful `GET /user` observation.
///
/// A successful `/user` proves the token is valid. It does **not** tell us which
/// scopes are present (`/user` succeeds for any valid token), so every scope
/// dependent capability is `Unknown` until a targeted probe runs. This is
/// deliberate: no silent "you have `repo`" or "you cannot read private repos".
pub fn report_after_identity_check() -> Vec<CapabilityReport> {
    [
        Capability::PublicRepoRead,
        Capability::PrivateRepoRead,
        Capability::PrivateProfileRead,
        Capability::Notifications,
        Capability::OrganizationRead,
    ]
    .into_iter()
    .map(|capability| {
        let status = match capability {
            Capability::PublicRepoRead => CapabilityStatus::Available,
            other => CapabilityStatus::Unknown {
                needs: other.requirement(),
            },
        };
        CapabilityReport { capability, status }
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_read_requires_no_scope() {
        assert_eq!(
            Capability::PublicRepoRead.requirement(),
            ScopeRequirement::None
        );
    }

    #[test]
    fn private_read_documents_coarse_repo_scope() {
        assert_eq!(
            Capability::PrivateRepoRead.requirement(),
            ScopeRequirement::AnyOf(&["repo"])
        );
        assert!(
            Capability::PrivateRepoRead
                .explanation()
                .contains("also grants write")
        );
    }

    #[test]
    fn identity_check_does_not_claim_unknown_scopes_available() {
        let report = report_after_identity_check();
        let private = report
            .iter()
            .find(|r| r.capability == Capability::PrivateRepoRead)
            .expect("entry");
        assert!(matches!(private.status, CapabilityStatus::Unknown { .. }));
        let public = report
            .iter()
            .find(|r| r.capability == Capability::PublicRepoRead)
            .expect("entry");
        assert_eq!(public.status, CapabilityStatus::Available);
    }

    #[test]
    fn notifications_documented_as_classic_only() {
        assert_eq!(
            Capability::Notifications.requirement(),
            ScopeRequirement::AnyOf(&["notifications", "repo"])
        );
    }

    #[test]
    fn guidance_does_not_request_admin_scopes() {
        for (_, requirement, _) in minimal_scope_guidance() {
            if let ScopeRequirement::AnyOf(scopes) = requirement {
                for scope in scopes {
                    assert!(
                        !scope.starts_with("admin:") && *scope != "delete_repo",
                        "guidance must not request administrative scope {scope}"
                    );
                }
            }
        }
    }
}
