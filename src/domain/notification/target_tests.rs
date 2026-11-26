use super::*;
use crate::domain::repository::RepositoryLocator;

fn repository(id: u64, owner: &str, name: &str) -> RepositoryIdentity {
    RepositoryIdentity {
        id,
        locator: RepositoryLocator::new(owner, name).expect("test locator"),
    }
}

fn subject(kind: SubjectKind, url: Option<&str>) -> NotificationSubject {
    NotificationSubject {
        title: "Synthetic subject".into(),
        kind,
        url: url.map(str::to_owned),
        latest_comment_url: None,
    }
}

fn target(kind: SubjectKind, url: Option<&str>) -> Option<NotificationTarget> {
    NotificationTarget::from_subject(&repository(10, "octo", "repo"), &subject(kind, url))
}

#[test]
fn official_issue_and_pull_request_urls_match_the_exact_repository() {
    let issue = target(
        SubjectKind::Issue,
        Some("https://api.github.com/repos/octo/repo/issues/123"),
    )
    .expect("issue target");
    assert_eq!(issue.noun(), "issue");
    assert_eq!(issue.number(), 123);
    assert_eq!(issue.repository(), &repository(10, "octo", "repo"));

    let pull = target(
        SubjectKind::PullRequest,
        Some("https://api.github.com/repos/octo/repo/pulls/77"),
    )
    .expect("pull request target");
    assert_eq!(pull.noun(), "pull request");
    assert_eq!(pull.number(), 77);
}

/// Every mismatch keeps the previous behavior: the title remains a plain,
/// non-navigating label. This table is the contract for unreliable sources.
#[test]
fn missing_or_unreliable_subjects_never_produce_a_target() {
    let cases = [
        // Missing, malformed or non-REST URLs.
        ("issue no url", SubjectKind::Issue, None),
        ("issue empty url", SubjectKind::Issue, Some("")),
        (
            "issue relative",
            SubjectKind::Issue,
            Some("/repos/octo/repo/issues/1"),
        ),
        (
            "issue html",
            SubjectKind::Issue,
            Some("https://github.com/octo/repo/issues/1"),
        ),
        (
            "issue http",
            SubjectKind::Issue,
            Some("http://api.github.com/repos/octo/repo/issues/1"),
        ),
        // Wrong host or origin fields.
        (
            "issue evil host",
            SubjectKind::Issue,
            Some("https://evil.example/repos/octo/repo/issues/1"),
        ),
        (
            "issue lookalike host",
            SubjectKind::Issue,
            Some("https://api.github.com.evil.example/repos/octo/repo/issues/1"),
        ),
        (
            "issue userinfo",
            SubjectKind::Issue,
            Some("https://api.github.com@evil.example/repos/octo/repo/issues/1"),
        ),
        (
            "issue port",
            SubjectKind::Issue,
            Some("https://api.github.com:8443/repos/octo/repo/issues/1"),
        ),
        (
            "issue query",
            SubjectKind::Issue,
            Some("https://api.github.com/repos/octo/repo/issues/1?x=1"),
        ),
        (
            "issue fragment",
            SubjectKind::Issue,
            Some("https://api.github.com/repos/octo/repo/issues/1#x"),
        ),
        // Wrong repository or path shape.
        (
            "issue wrong owner",
            SubjectKind::Issue,
            Some("https://api.github.com/repos/other/repo/issues/1"),
        ),
        (
            "issue wrong repo",
            SubjectKind::Issue,
            Some("https://api.github.com/repos/octo/other/issues/1"),
        ),
        (
            "issue nested repo",
            SubjectKind::Issue,
            Some("https://api.github.com/repos/octo/repo/extra/issues/1"),
        ),
        (
            "issue trailing slash",
            SubjectKind::Issue,
            Some("https://api.github.com/repos/octo/repo/issues/1/"),
        ),
        (
            "issue other endpoint",
            SubjectKind::Issue,
            Some("https://api.github.com/repos/octo/repo"),
        ),
        // Type and path must agree; one endpoint cannot impersonate another.
        (
            "pr on issues path",
            SubjectKind::PullRequest,
            Some("https://api.github.com/repos/octo/repo/issues/1"),
        ),
        (
            "issue on pulls path",
            SubjectKind::Issue,
            Some("https://api.github.com/repos/octo/repo/pulls/1"),
        ),
        (
            "release",
            SubjectKind::Release,
            Some("https://api.github.com/repos/octo/repo/releases/1"),
        ),
        (
            "commit",
            SubjectKind::Commit,
            Some("https://api.github.com/repos/octo/repo/commits/1"),
        ),
        (
            "discussion",
            SubjectKind::Discussion,
            Some("https://api.github.com/repos/octo/repo/discussions/1"),
        ),
        (
            "unknown",
            SubjectKind::Unknown("Future".into()),
            Some("https://api.github.com/repos/octo/repo/issues/1"),
        ),
        // A number must be a positive decimal integer.
        (
            "issue zero",
            SubjectKind::Issue,
            Some("https://api.github.com/repos/octo/repo/issues/0"),
        ),
        (
            "issue leading zero",
            SubjectKind::Issue,
            Some("https://api.github.com/repos/octo/repo/issues/007"),
        ),
        (
            "issue signed",
            SubjectKind::Issue,
            Some("https://api.github.com/repos/octo/repo/issues/+1"),
        ),
        (
            "issue negative",
            SubjectKind::Issue,
            Some("https://api.github.com/repos/octo/repo/issues/-1"),
        ),
        (
            "issue space",
            SubjectKind::Issue,
            Some("https://api.github.com/repos/octo/repo/issues/1%20"),
        ),
        (
            "issue overflow",
            SubjectKind::Issue,
            Some("https://api.github.com/repos/octo/repo/issues/18446744073709551616"),
        ),
    ];
    for (name, kind, url) in cases {
        assert_eq!(target(kind, url), None, "{name}");
    }
}

#[test]
fn the_thread_helper_uses_its_own_repository_and_subject() {
    let thread = crate::domain::notification::NotificationThread {
        id: "9000".into(),
        repository: repository(10, "octo", "repo"),
        subject: subject(
            SubjectKind::PullRequest,
            Some("https://api.github.com/repos/octo/repo/pulls/9"),
        ),
        reason: "mention".into(),
        unread: true,
        updated_at: "2026-11-24T00:00:00Z".into(),
        last_read_at: None,
    };
    assert_eq!(
        thread.target(),
        Some(NotificationTarget::PullRequest {
            repository: repository(10, "octo", "repo"),
            number: 9,
        })
    );
}
