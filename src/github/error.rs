//! Typed error classification for GitHub responses.

use crate::domain::error::{GitHubError, RateLimitKind, RetryAfter};
use crate::network::transport::{HttpResponse, RateLimitHeaders, TransportError};

/// Convert a transport-layer error into a typed GitHub error.
pub fn classify_transport(err: TransportError) -> GitHubError {
    match err {
        TransportError::Timeout { seconds } => GitHubError::Timeout { seconds },
        TransportError::Cancelled => GitHubError::Cancelled,
        TransportError::Network { message } => GitHubError::Network { message },
        TransportError::ResponseTooLarge { limit } => GitHubError::ResponseTooLarge { limit },
        TransportError::BoundedReadUnsupported => GitHubError::Network {
            message: "transport does not support bounded reads".into(),
        },
    }
}

/// Classify a non-2xx HTTP response into a typed error using status code and
/// rate-limit/SSO headers. A 2xx response returns `None`.
pub fn classify_status(response: &HttpResponse) -> Option<GitHubError> {
    let status = response.status;
    if (200..300).contains(&status) {
        return None;
    }

    let rl = response.rate_limit();
    let sso = response.header("x-github-sso").map(|s| s.to_string());

    match status {
        401 => Some(GitHubError::Unauthorized),
        403 => {
            if let Some(retry) = rate_limit_from(&rl) {
                Some(GitHubError::RateLimited { retry })
            } else if let Some(sso_value) = sso.as_deref() {
                if let Some(url) = crate::github::headers::parse_sso_authorization_url(sso_value) {
                    Some(GitHubError::SsoRequired {
                        authorization_url: Some(url),
                    })
                } else {
                    Some(GitHubError::Forbidden)
                }
            } else {
                Some(GitHubError::Forbidden)
            }
        }
        404 => Some(GitHubError::NotFound),
        429 => Some(GitHubError::RateLimited {
            retry: RetryAfter {
                kind: RateLimitKind::Secondary,
                seconds: rl.retry_after,
            },
        }),
        _ => Some(GitHubError::UnexpectedStatus { status }),
    }
}

/// Distinguish a primary limit (`X-RateLimit-Remaining: 0`) from a secondary
/// limit signalled only by `Retry-After`. Primary wins when both are present
/// because the bucket is genuinely exhausted.
fn rate_limit_from(headers: &RateLimitHeaders) -> Option<RetryAfter> {
    if headers.remaining == Some(0) {
        return Some(RetryAfter {
            kind: RateLimitKind::Primary,
            seconds: headers.retry_after,
        });
    }
    headers.retry_after.map(|seconds| RetryAfter {
        kind: RateLimitKind::Secondary,
        seconds: Some(seconds),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, headers: &[(&str, &str)]) -> HttpResponse {
        HttpResponse {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: "{}".to_string(),
        }
    }

    #[test]
    fn status_401_is_unauthorized() {
        assert_eq!(
            classify_status(&response(401, &[])),
            Some(GitHubError::Unauthorized)
        );
    }

    #[test]
    fn status_403_with_zero_remaining_is_rate_limited() {
        let err = classify_status(&response(
            403,
            &[
                ("x-ratelimit-remaining", "0"),
                ("x-ratelimit-reset", "1700000000"),
            ],
        ));
        match err {
            Some(GitHubError::RateLimited { retry }) => {
                assert_eq!(retry.kind, RateLimitKind::Primary);
                assert_eq!(retry.seconds, None);
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[test]
    fn status_403_with_retry_after_is_secondary_rate_limit() {
        let err = classify_status(&response(403, &[("retry-after", "45")]));
        match err {
            Some(GitHubError::RateLimited { retry }) => {
                assert_eq!(retry.kind, RateLimitKind::Secondary);
                assert_eq!(retry.seconds, Some(45));
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[test]
    fn status_403_with_sso_is_sso_required_not_forbidden() {
        let err = classify_status(&response(
            403,
            &[(
                "x-github-sso",
                "required; url=https://github.com/orgs/acme/sso?authorization_request=x",
            )],
        ));
        match err {
            Some(GitHubError::SsoRequired {
                authorization_url: Some(url),
            }) => assert!(url.contains("/orgs/acme/sso")),
            other => panic!("expected SsoRequired, got {other:?}"),
        }
    }

    #[test]
    fn plain_403_is_forbidden_and_does_not_imply_unauthorized() {
        assert_eq!(
            classify_status(&response(403, &[])),
            Some(GitHubError::Forbidden)
        );
    }

    #[test]
    fn status_404_is_not_found() {
        assert_eq!(
            classify_status(&response(404, &[])),
            Some(GitHubError::NotFound)
        );
    }

    #[test]
    fn status_429_is_secondary_rate_limit() {
        let err = classify_status(&response(429, &[("retry-after", "60")]));
        assert!(matches!(
            err,
            Some(GitHubError::RateLimited {
                retry: RetryAfter {
                    kind: RateLimitKind::Secondary,
                    seconds: Some(60)
                }
            })
        ));
    }

    #[test]
    fn status_200_is_ok() {
        assert_eq!(classify_status(&response(200, &[])), None);
    }

    #[test]
    fn status_500_is_unexpected_status() {
        assert_eq!(
            classify_status(&response(500, &[])),
            Some(GitHubError::UnexpectedStatus { status: 500 })
        );
    }

    #[test]
    fn transport_errors_map_to_typed_errors() {
        assert_eq!(
            classify_transport(TransportError::Timeout { seconds: 30 }),
            GitHubError::Timeout { seconds: 30 }
        );
        assert_eq!(
            classify_transport(TransportError::Cancelled),
            GitHubError::Cancelled
        );
        assert!(matches!(
            classify_transport(TransportError::Network {
                message: "offline".to_string()
            }),
            GitHubError::Network { .. }
        ));
    }
}
