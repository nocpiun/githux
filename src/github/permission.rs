//! Read-only permission boundary reserved for future write actions.
//!
//! **No write action is implemented in this slice.** This module only encodes
//! the *shape* of a permission decision so that future work cannot accidentally
//! treat "a read succeeded" or "the token has `repo`" as proof that a specific
//! write is allowed. It deliberately has no network calls and no H7 wiring.
//!
//! The rule that matters for correctness today: a Classic PAT's `repo` scope
//! being present is **not** evidence that the account may write to a particular
//! repository. Repository permission, organization policy and server responses
//! decide that, and they are unknown here.

/// A future remote write action. Listed only so the boundary is explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteAction {
    CreateIssue,
    CommentOnIssue,
    StarRepository,
}

/// Three-state answer for a write capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WritePermission {
    /// Confirmed allowed by an authoritative check.
    Allowed,
    /// Confirmed denied; the reason is user-actionable.
    Denied { reason: DenialReason },
    /// Not yet determined. Never treat this as allowed.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenialReason {
    MissingScope,
    OrganizationPolicy,
    NotSignedIn,
}

/// Current answer for any write action: [`WritePermission::Unknown`].
///
/// This is the honest state until the H7 task defines concrete actions and adds
/// authoritative checks. Reads succeeding do not change it.
pub fn write_permission(_action: WriteAction) -> WritePermission {
    WritePermission::Unknown
}

/// Convenience predicate for callers that must gate an action on a *confirmed*
/// allowance. Returns `false` for everything except `Allowed`.
pub fn is_write_confirmed_allowed(permission: &WritePermission) -> bool {
    matches!(permission, WritePermission::Allowed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_write_action_is_confirmed_allowed_in_this_slice() {
        for action in [
            WriteAction::CreateIssue,
            WriteAction::CommentOnIssue,
            WriteAction::StarRepository,
        ] {
            let permission = write_permission(action);
            assert_eq!(permission, WritePermission::Unknown);
            assert!(!is_write_confirmed_allowed(&permission));
        }
    }

    #[test]
    fn unknown_is_not_treated_as_allowed() {
        assert!(!is_write_confirmed_allowed(&WritePermission::Unknown));
        assert!(!is_write_confirmed_allowed(&WritePermission::Denied {
            reason: DenialReason::MissingScope
        }));
        assert!(is_write_confirmed_allowed(&WritePermission::Allowed));
    }
}
