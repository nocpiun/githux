//! Header parsing helpers: `Link` pagination and `X-GitHub-SSO`.

/// Parse a `Link` header value and return the `rel="next"` URL if present.
///
/// Implements the RFC 8288 shape GitHub uses: comma-separated link entries, each
/// `<uri-reference>; param=value; ...`. Commas may legally appear inside the
/// angle-bracketed target (GitHub's own pagination links carry
/// `affiliation=owner,collaborator,organization_member`) and inside quoted
/// parameter values, so splitting the header on every comma is wrong: it would
/// break such a URL in half and silently stop pagination, reporting an
/// incomplete list as complete.
///
/// Example: `<https://api.github.com/user/repos?page=2>; rel="next"; ...`
pub fn parse_link_next(link_header: Option<&str>) -> Option<String> {
    let header = link_header?;
    for entry in split_link_entries(header) {
        let Some(target) = link_target(entry) else {
            continue;
        };
        if link_params(entry).any(is_rel_next) {
            return Some(target.to_string());
        }
    }
    None
}

/// Split a `Link` header into entries at commas that are **outside** `<...>`
/// targets and outside quoted parameter values.
fn split_link_entries(header: &str) -> Vec<&str> {
    let mut entries = Vec::new();
    let mut start = 0usize;
    let mut in_target = false;
    let mut in_quotes = false;
    let mut escaped = false;

    for (idx, ch) in header.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quotes => escaped = true,
            '"' if !in_target => in_quotes = !in_quotes,
            '<' if !in_quotes => in_target = true,
            '>' if !in_quotes => in_target = false,
            ',' if !in_target && !in_quotes => {
                entries.push(&header[start..idx]);
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }
    entries.push(&header[start..]);
    entries
}

/// The URI reference inside the first `<...>` of a link entry.
fn link_target(entry: &str) -> Option<&str> {
    let open = entry.find('<')?;
    let rest = &entry[open + 1..];
    let close = rest.find('>')?;
    Some(&rest[..close])
}

/// Whether one `key=value` link parameter declares `rel="next"`.
///
/// Accepts both the quoted (`rel="next"`) and unquoted (`rel=next`) forms; the
/// `rel` value may itself be a space-separated list, in which case any `next`
/// token counts.
fn is_rel_next(param: &str) -> bool {
    let Some((key, value)) = param.split_once('=') else {
        return false;
    };
    if !key.trim().eq_ignore_ascii_case("rel") {
        return false;
    }
    let value = value.trim().trim_matches('"');
    value
        .split_ascii_whitespace()
        .any(|token| token.eq_ignore_ascii_case("next"))
}

/// Parameter tokens after the `>` of a link entry, split on `;`.
fn link_params(entry: &str) -> impl Iterator<Item = &str> {
    let after_target = link_target(entry)
        .and_then(|target| entry.find(target))
        .map(|start| &entry[start + 1..])
        .and_then(|rest| rest.find('>').map(|close| &rest[close + 1..]))
        .unwrap_or("");
    after_target
        .split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty())
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

    /// Regression: GitHub pagination links legitimately contain commas inside
    /// the `<...>` target (e.g. the `affiliation=owner,collaborator,...` filter
    /// this client itself sends). Splitting the header on every comma cut such a
    /// URL in half, returned `None`, and made paging stop while reporting the
    /// list as `Complete`.
    #[test]
    fn next_link_with_commas_inside_the_target() {
        let header = r#"<https://api.github.com/user/repos?affiliation=owner,collaborator,organization_member&page=2>; rel="next", <https://api.github.com/user/repos?affiliation=owner,collaborator,organization_member&page=99>; rel="last""#;
        assert_eq!(
            parse_link_next(Some(header)),
            Some(
                "https://api.github.com/user/repos?affiliation=owner,collaborator,organization_member&page=2"
                    .to_string()
            )
        );
    }

    /// A comma inside a quoted parameter value must not split the entry either.
    #[test]
    fn comma_inside_quoted_parameter_does_not_split() {
        let header = r#"<https://api.github.com/x?page=2>; rel="next"; title="a,b", <https://api.github.com/x?page=3>; rel="last""#;
        assert_eq!(
            parse_link_next(Some(header)),
            Some("https://api.github.com/x?page=2".to_string())
        );
    }

    /// An entry that is not the first one and not `rel="next"` must not abort
    /// the whole parse (the earlier implementation returned early on the first
    /// non-`<`-prefixed segment).
    #[test]
    fn non_next_entry_does_not_abort_the_scan() {
        let header = r#"<https://api.github.com/a?page=1>; rel="first", <https://api.github.com/a?page=2>; rel="next""#;
        assert_eq!(
            parse_link_next(Some(header)),
            Some("https://api.github.com/a?page=2".to_string())
        );
    }

    /// Unquoted `rel=next` is accepted as well.
    #[test]
    fn unquoted_rel_next_is_recognised() {
        assert_eq!(
            parse_link_next(Some("<https://api.github.com/a?page=2>; rel=next")),
            Some("https://api.github.com/a?page=2".to_string())
        );
    }

    /// `last`/`prev`/`first` alone yield no next link, and a malformed entry is
    /// skipped rather than panicking.
    #[test]
    fn only_next_produces_a_next_link() {
        let header = r#"<https://api.github.com/a?page=1>; rel="first", <https://api.github.com/a?page=3>; rel="last""#;
        assert_eq!(parse_link_next(Some(header)), None);
        assert_eq!(parse_link_next(Some("")), None);
        assert_eq!(parse_link_next(Some("garbage")), None);
        assert_eq!(parse_link_next(None), None);
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
