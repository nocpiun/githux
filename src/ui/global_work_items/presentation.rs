use crate::domain::{
    error::GitHubError,
    work_item::{PartialReason, WorkItemError},
};
pub(super) fn error_message(error: &WorkItemError) -> String {
    let message = match error {
        WorkItemError::GitHub(error) => match error {
            GitHubError::Unauthorized => {
                "Authentication expired. Replace the account token, then retry."
            }
            GitHubError::Forbidden => {
                "Access denied. Check token permissions and organization policy, then retry."
            }
            GitHubError::SsoRequired { .. } => {
                "Organization authorization is required. Authorize your token, then retry."
            }
            GitHubError::RateLimited { retry } => {
                return retry.seconds.map_or_else(
                    || "Rate limited. Wait before retrying.".into(),
                    |seconds| format!("Rate limited. Retry after {seconds} seconds."),
                );
            }
            GitHubError::NotFound => {
                "Results are not visible to this account. Check access, then retry."
            }
            GitHubError::Network { .. } => "Could not connect. Check your connection, then retry.",
            GitHubError::Timeout { .. } => "The request timed out. Retry.",
            GitHubError::Cancelled => "Request cancelled. Retry.",
            GitHubError::StaleContext => "Account changed. Reopen this list.",
            GitHubError::CredentialStore { .. } => {
                "Account credentials are unavailable. Check the account, then retry."
            }
            _ => "Could not load results. Retry.",
        },
        _ => "Could not read results. Retry.",
    };
    message.into()
}
pub(super) fn partial_message(reasons: &[PartialReason]) -> String {
    reasons
        .iter()
        .map(|reason| match reason {
            PartialReason::ResultLimit => {
                "Only the first 1,000 matches are available. Narrow the filters."
            }
            PartialReason::IncompleteResults => "Some matches were omitted. Refresh to try again.",
            PartialReason::SsoPartial { .. } => {
                "Some organizations are missing. Check organization authorization, then refresh."
            }
            _ => "Some results could not be loaded. Refresh to try again.",
        })
        .collect::<Vec<_>>()
        .join(" ")
}
