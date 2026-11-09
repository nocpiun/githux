//! Framework-free summary helpers shared by the overview strip and its tests.

use crate::domain::repository_extras::{Contributor, ContributorKind, LanguagesOutcome};

/// A contributor's display name: the profile name when it has one, else the login.
pub(crate) fn contributor_display(contributor: &Contributor) -> String {
    contributor
        .name
        .as_deref()
        .filter(|name| !name.is_empty())
        .or(contributor.login.as_deref())
        .unwrap_or("Anonymous contributor")
        .to_string()
}

#[derive(Debug)]
pub(crate) struct Segment {
    pub name: String,
    pub share: f64,
    pub color: usize,
}

/// Checked byte arithmetic; never trust independently rounded/malformed supplied shares.
pub(crate) fn language_segments(outcome: &LanguagesOutcome) -> Option<Vec<Segment>> {
    let total = outcome
        .languages
        .iter()
        .try_fold(0u64, |sum, item| sum.checked_add(item.bytes))?;
    if total != outcome.total_bytes {
        return None;
    }
    if total == 0 {
        return Some(Vec::new());
    }
    let mut languages: Vec<_> = outcome
        .languages
        .iter()
        .filter(|item| item.bytes > 0)
        .collect();
    languages.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.name.cmp(&b.name)));
    let result: Vec<_> = languages
        .iter()
        .map(|item| Segment {
            name: item.name.clone(),
            share: item.bytes as f64 / total as f64,
            color: language_color(&item.name),
        })
        .collect();
    Some(result)
}

fn language_color(name: &str) -> usize {
    name.bytes().fold(0usize, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte as usize)
    }) % 8
}

pub(crate) fn format_share(share: f64) -> String {
    if share > 0.0 && share < 0.005 {
        "<1%".into()
    } else {
        format!("{:.0}%", share * 100.0)
    }
}

/// Whether a contributor can be navigated to (a GitHub user with a canonical URL).
pub(crate) fn contributor_is_linked(contributor: &Contributor) -> bool {
    contributor.kind() == ContributorKind::GitHubUser && contributor.html_url.is_some()
}
