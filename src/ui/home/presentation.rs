use crate::domain::error::GitHubError;
use crate::domain::home::{
    FeedOutcome, FeedPartialReason, FeedReadiness, HomeError, HomePartialReason, HomeReadiness,
    HomeSearchOutcome,
};

pub(super) struct Summary {
    pub notice: Option<String>,
}

pub(super) fn home_summary(outcome: &HomeSearchOutcome) -> Summary {
    let notice = match &outcome.readiness {
        HomeReadiness::Complete if outcome.incomplete_results => {
            "GitHub marked the search results incomplete. involves:current login · updated descending · no time window; this is not an unread or complete-history list.".to_string()
        }
        HomeReadiness::Complete if outcome.total_count > 1000 => {
            "Partial result · GitHub Search exposes at most 1,000 results. This is not a complete history or unread list.".to_string()
        }
        HomeReadiness::Complete => return Summary { notice: None },
        HomeReadiness::Partial { reasons } => {
            let details = reasons.iter().map(home_partial).collect::<Vec<_>>().join(" ");
            format!(
                "Partial result · {details} The visible entries keep the supplied updated-descending order; this is not a complete history or unread list."
            )
        }
    };
    Summary {
        notice: Some(notice),
    }
}

fn home_partial(reason: &HomePartialReason) -> String {
    match reason {
        HomePartialReason::PageFailed { .. } => {
            "A later page failed; earlier entries remain available.".to_string()
        }
        HomePartialReason::PageLimit => "The local page budget stopped the search.".to_string(),
        HomePartialReason::EntryLimit => "The local entry budget truncated the search.".to_string(),
        HomePartialReason::ResultLimit => {
            "GitHub Search exposes at most 1,000 results for this query.".to_string()
        }
        HomePartialReason::IncompleteResults => {
            "GitHub marked the search results incomplete.".to_string()
        }
        HomePartialReason::UntrustedPagination => {
            "An untrusted pagination link was rejected.".to_string()
        }
        HomePartialReason::InvalidPagination => {
            "A pagination link was invalid for this search.".to_string()
        }
        HomePartialReason::PaginationCycle => "A pagination cycle was stopped.".to_string(),
        HomePartialReason::ResponseLimit => {
            "A later response exceeded its size budget.".to_string()
        }
        HomePartialReason::ParseFailed { .. } => {
            "A later page could not be parsed; earlier entries remain available.".to_string()
        }
        HomePartialReason::SsoPartial { organizations } => {
            let organizations = safe_list(organizations);
            format!("Some organizations require SSO authorization: {organizations}.")
        }
    }
}

pub(super) fn feed_summary(outcome: &FeedOutcome) -> Summary {
    let notice = match &outcome.readiness {
        FeedReadiness::Complete => return Summary { notice: None },
        FeedReadiness::Partial { reasons } => {
            let details = reasons
                .iter()
                .map(feed_partial)
                .collect::<Vec<_>>()
                .join(" ");
            format!(
                "Partial result · {details} Received Events is limited to at most 30 days / 300 events and may be delayed."
            )
        }
    };
    Summary {
        notice: Some(notice),
    }
}

fn feed_partial(reason: &FeedPartialReason) -> String {
    match reason {
        FeedPartialReason::PageFailed { .. } => {
            "A later page failed; earlier events remain available.".to_string()
        }
        FeedPartialReason::PageLimit => "The local page budget stopped the read.".to_string(),
        FeedPartialReason::EntryLimit => "The local event budget truncated the read.".to_string(),
        FeedPartialReason::ResponseLimit => {
            "A later response exceeded its size budget.".to_string()
        }
        FeedPartialReason::ParseFailed { .. } => {
            "A later page could not be parsed; earlier events remain available.".to_string()
        }
        FeedPartialReason::UntrustedPagination => {
            "An untrusted pagination link was rejected.".to_string()
        }
        FeedPartialReason::InvalidPagination => {
            "A pagination link was invalid for this event stream.".to_string()
        }
        FeedPartialReason::PaginationCycle => "A pagination cycle was stopped.".to_string(),
        FeedPartialReason::SsoPartial { organizations } => format!(
            "Some organizations require SSO authorization: {}.",
            safe_list(organizations)
        ),
    }
}

fn safe_list(values: &[String]) -> String {
    let values = values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .take(5)
        .collect::<Vec<_>>();
    if values.is_empty() {
        "names unavailable".to_string()
    } else {
        values.join(", ")
    }
}

pub(super) fn error_message(error: &HomeError) -> String {
    match error {
        HomeError::GitHub(GitHubError::Unauthorized) => {
            "Authentication failed. The host should verify or replace this account's authorization."
                .to_string()
        }
        HomeError::GitHub(GitHubError::RateLimited { retry }) => match retry.seconds {
            Some(seconds) => format!(
                "GitHub rate limit reached. Retry after the host's cooldown (server hint: {seconds}s)."
            ),
            None => {
                "GitHub rate limit reached. Refresh when the host makes it available.".to_string()
            }
        },
        HomeError::GitHub(GitHubError::SsoRequired { .. }) => {
            "Organization SSO authorization is required.".to_string()
        }
        HomeError::GitHub(GitHubError::Forbidden) => {
            "The current authorization cannot access this resource.".to_string()
        }
        HomeError::GitHub(GitHubError::NotFound) => {
            "The resource was not found or is not visible to this account.".to_string()
        }
        HomeError::GitHub(GitHubError::Network { .. }) => {
            "The network request failed. This is not an empty activity result.".to_string()
        }
        HomeError::GitHub(GitHubError::Timeout { .. }) => {
            "The request timed out. No new result was verified.".to_string()
        }
        HomeError::GitHub(GitHubError::Cancelled) => "The request was cancelled.".to_string(),
        HomeError::GitHub(GitHubError::StaleContext) => {
            "The account authorization changed; this result was discarded.".to_string()
        }
        HomeError::GitHub(GitHubError::UnexpectedStatus { status }) => {
            format!("GitHub returned unexpected HTTP status {status}.")
        }
        HomeError::GitHub(GitHubError::Parse { .. }) | HomeError::InvalidResponse { .. } => {
            "The response could not be understood.".to_string()
        }
        HomeError::GitHub(GitHubError::ResponseTooLarge { .. })
        | HomeError::LimitExceeded { .. } => {
            "The response exceeded its configured limit.".to_string()
        }
        HomeError::GitHub(GitHubError::CredentialStore { .. }) => {
            "The host could not access the account credential.".to_string()
        }
        HomeError::InvalidInput { .. } => "The host supplied an invalid query.".to_string(),
    }
}
