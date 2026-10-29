//! Deterministic local fixtures for Home's two demo columns only.
//!
//! The sidebar account row, the repository list and the palette now use real account and
//! /user/repos data, so the old account and repository fixtures are gone rather than being kept
//! around as a second source of truth. Home's columns remain demo content, which the product scope
//! explicitly leaves untouched for a later Home task; no network, credential or cache path reads
//! this module.

/// Whether a Home row is an issue or a pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityKind {
    Issue,
    PullRequest,
}

impl ActivityKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Issue => "Issue",
            Self::PullRequest => "Pull request",
        }
    }
}

/// One entry under Home's left column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvolvedFixture {
    pub kind: ActivityKind,
    pub title: &'static str,
    pub repo: &'static str,
    pub number: u32,
    pub updated: &'static str,
}

/// One event under Home's right "Feed" column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeedFixture {
    pub actor: &'static str,
    pub action: &'static str,
    pub target: &'static str,
    pub detail: &'static str,
    pub when: &'static str,
}

/// Home's left column: relevant recent issues and pull requests, newest update first.
pub const INVOLVED: &[InvolvedFixture] = &[
    InvolvedFixture {
        kind: ActivityKind::Issue,
        title: "Improve keyboard navigation in repository tree",
        repo: "githux-dev/githux",
        number: 42,
        updated: "8m",
    },
    InvolvedFixture {
        kind: ActivityKind::PullRequest,
        title: "Fix sidebar scroll position",
        repo: "githux-dev/githux",
        number: 128,
        updated: "21m",
    },
    InvolvedFixture {
        kind: ActivityKind::Issue,
        title: "Add Markdown table rendering",
        repo: "northstar/atlas",
        number: 86,
        updated: "43m",
    },
    InvolvedFixture {
        kind: ActivityKind::PullRequest,
        title: "Refine Windows title bar hit targets",
        repo: "githux-dev/githux",
        number: 131,
        updated: "1h",
    },
    InvolvedFixture {
        kind: ActivityKind::Issue,
        title: "Cache repository avatars",
        repo: "northstar/atlas",
        number: 92,
        updated: "2h",
    },
    InvolvedFixture {
        kind: ActivityKind::PullRequest,
        title: "Preserve focus when switching tabs",
        repo: "githux-dev/githux",
        number: 124,
        updated: "3h",
    },
    InvolvedFixture {
        kind: ActivityKind::Issue,
        title: "中文输入法候选框位置随窗口缩放偏移",
        repo: "开源工具箱/中文排版与输入法验证",
        number: 17,
        updated: "4h",
    },
    InvolvedFixture {
        kind: ActivityKind::PullRequest,
        title: "Handle repositories without a README",
        repo: "githux-dev/githux",
        number: 39,
        updated: "6h",
    },
    InvolvedFixture {
        kind: ActivityKind::Issue,
        title: "Reduce background refresh requests",
        repo: "northstar/atlas",
        number: 81,
        updated: "yesterday",
    },
];

/// Home's right column: following activity, newest first.
pub const FEED: &[FeedFixture] = &[
    FeedFixture {
        actor: "Maya",
        action: "starred",
        target: "studio/palette",
        detail: "A focused toolkit for expressive desktop interfaces.",
        when: "12m",
    },
    FeedFixture {
        actor: "northstar",
        action: "released",
        target: "atlas v0.8.0",
        detail: "Faster repository navigation and improved Markdown previews.",
        when: "38m",
    },
    FeedFixture {
        actor: "Theo",
        action: "opened a pull request",
        target: "githux-dev/githux #132",
        detail: "Keep page state when switching tabs.",
        when: "1h",
    },
    FeedFixture {
        actor: "Ava",
        action: "forked",
        target: "demo/http-client",
        detail: "A minimal, easy-to-use HTTP client for Rust.",
        when: "2h",
    },
    FeedFixture {
        actor: "Lin",
        action: "starred",
        target: "zed-industries/zed",
        detail: "A high-performance code editor.",
        when: "3h",
    },
    FeedFixture {
        actor: "Chen",
        action: "opened an issue",
        target: "开源工具箱/中文排版与输入法验证 #18",
        detail: "长仓库名称在侧栏与标题栏中的截断方式需要统一。",
        when: "5h",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_fixtures_stay_small_and_deterministic() {
        assert!((6..=12).contains(&INVOLVED.len()));
        assert!((4..=8).contains(&FEED.len()));
        assert!(INVOLVED.iter().any(|row| row.title.contains('中')));
        assert!(FEED.iter().any(|row| row.target.contains('中')));
    }
}
