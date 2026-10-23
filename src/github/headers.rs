//! Header parsing helpers: `Link` pagination and `X-GitHub-SSO`.

/// Parse a `Link` header value and return the `rel="next"` URL if present.
///
/// Example: `<https://api.github.com/user/repos?page=2>; rel="next"; ...`
pub fn parse_link_next(link_header: Option<&str>) -> Option<String> {
    let header = link_header?;
    for segment in header.split(',') {
        let trimmed = segment.trim();
        let url = trimmed
            .strip_prefix('<')
            .and_then(|s| s.split('>').next())?;
        if trimmed.contains(r#"rel="next""#) || trimmed.contains("rel=next") {
            return Some(url.to_string());
        }
    }
    None
}

/// Parse `X-GitHub-SSO` header. Returns the list of organizations that require
/// SSO authorization when `partial-results` is present.
pub fn parse_sso_header(header: Option<&str>) -> Option<Vec<String>> {
    let header = header?;
    if !header.starts_with("partial-results") {
        return None;
    }
    // Format: `partial-results; organizations=org1,org2`
    for part in header.split(';').map(str::trim) {
        if let Some(rest) = part.strip_prefix("organizations=") {
            let orgs: Vec<String> = rest.split(',').map(|s| s.trim().to_string()).collect();
            return Some(orgs);
        }
    }
    Some(Vec::new())
}

/// Parse an SSO authorization URL from a `403` response's `X-GitHub-SSO` header.
/// Format: `required; url=https://github.com/orgs/.../sso?...`
pub fn parse_sso_authorization_url(header: &str) -> Option<String> {
    for part in header.split(';').map(str::trim) {
        if let Some(rest) = part.strip_prefix("url=") {
            return Some(rest.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_next_link() {
        let header = r#"<https://api.github.com/user/repos?page=2>; rel="next", <https://api.github.com/user/repos?page=10>; rel="last""#;
        assert_eq!(
            parse_link_next(Some(header)),
            Some("https://api.github.com/user/repos?page=2".to_string())
        );
    }

    #[test]
    fn no_next_link() {
        let header = r#"<https://api.github.com/user/repos?page=1>; rel="prev""#;
        assert_eq!(parse_link_next(Some(header)), None);
    }

    #[test]
    fn parses_partial_sso() {
        let header = "partial-results; organizations=octocat,github";
        assert_eq!(
            parse_sso_header(Some(header)),
            Some(vec!["octocat".to_string(), "github".to_string()])
        );
    }

    #[test]
    fn parses_sso_url() {
        let header = "required; url=https://github.com/orgs/foo/sso?authorization_request=abc";
        assert_eq!(
            parse_sso_authorization_url(header),
            Some("https://github.com/orgs/foo/sso?authorization_request=abc".to_string())
        );
    }
}
