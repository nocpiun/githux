//! Deterministic local fixtures for layout verification.
//!
//! Nothing here is fetched: the shell, the bounded repository list and Home's two columns are
//! checked against stable local data so layout and navigation can be reviewed without accounts,
//! network or a data layer. Real GitHub data replaces these fixtures in later roadmap steps; no
//! network, credential or cache path reads this module.

/// A repository as the sidebar and Home rows show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoFixture {
    pub namespace: String,
    pub name: String,
    pub description: &'static str,
    pub language: &'static str,
}

impl RepoFixture {
    fn new(
        namespace: impl Into<String>,
        name: impl Into<String>,
        description: &'static str,
        language: &'static str,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
            description,
            language,
        }
    }

    /// `namespace/repo`, the full identifier every surface shows.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }
}

/// The single local account this round draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccountFixture {
    pub name: &'static str,
    pub login: &'static str,
    pub bio: &'static str,
}

pub const ACCOUNT: AccountFixture = AccountFixture {
    name: "Lin",
    login: "lin-dev",
    bio: "A calm desktop workspace for GitHub.",
};

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

/// Curated repositories; the generated tail follows them so the bounded list has enough entries
/// to scroll while staying byte-for-byte stable across runs.
const CURATED: &[(&str, &str, &str, &str)] = &[
    (
        "githux-dev",
        "githux",
        "A calm desktop workspace for GitHub.",
        "Rust",
    ),
    (
        "northstar",
        "atlas",
        "Navigation primitives for desktop apps.",
        "Rust",
    ),
    (
        "northstar",
        "notes",
        "Local-first notes with a shared index.",
        "TypeScript",
    ),
    (
        "studio",
        "palette",
        "A focused toolkit for expressive desktop interfaces.",
        "Rust",
    ),
    (
        "demo",
        "http-client",
        "A minimal, easy-to-use HTTP client for Rust.",
        "Rust",
    ),
    (
        "demo",
        "tiny-db",
        "An embedded key-value store for small tools.",
        "Go",
    ),
    (
        "zed-industries",
        "zed",
        "A high-performance code editor.",
        "Rust",
    ),
    (
        "rust-lang",
        "rust",
        "Empowering everyone to build reliable software.",
        "Rust",
    ),
    (
        "开源工具箱",
        "中文排版与输入法验证",
        "用于检查中文字形、行高与输入法候选位置的样例仓库。",
        "Rust",
    ),
    (
        "studio",
        "extremely-long-repository-name-used-to-check-sidebar-truncation-behaviour",
        "A deliberately long name that must stay inside the sidebar column.",
        "TypeScript",
    ),
    (
        "very-long-namespace-organisation-name-for-width-tests",
        "component-gallery",
        "Layout checks for narrow panels and long identifiers.",
        "CSS",
    ),
    (
        "githux-dev",
        "跨平台外壳与窗口行为验证仓库",
        "标题栏、贴靠与多显示器尺寸的回归样例。",
        "Rust",
    ),
];

const NAMESPACES: &[&str] = &[
    "githux-dev",
    "northstar",
    "studio",
    "demo",
    "开源工具箱",
    "research-lab",
    "infra-team",
    "docs-guild",
    "community-zh",
];

const WORDS: &[&str] = &[
    "shell",
    "island",
    "sidebar",
    "palette",
    "router",
    "fixture",
    "typography",
    "cache",
    "session",
    "scroll",
    "focus",
    "window",
    "renderer",
    "protocol",
];

const TOPICS: &[&str] = &[
    "experiments",
    "playground",
    "samples",
    "bindings",
    "cli",
    "docs",
    "bench",
    "notes",
    "archive",
    "support",
    "模板",
    "示例",
];

const LANGUAGES: &[&str] = &["Rust", "TypeScript", "Go", "Python", "Shell"];

const DESCRIPTIONS: &[&str] = &[
    "Fixture repository used to verify layout and scrolling.",
    "Small tool kept for interface experiments.",
    "Shared components and layout notes.",
    "用于验证长列表、滚动约束和焦点归属的样例仓库。",
    "Archive of earlier interface attempts.",
];

/// Generated entries: unique names, including CJK and deliberately long ones.
const GENERATED: usize = 112;

fn generated(index: usize) -> RepoFixture {
    let namespace = NAMESPACES[index % NAMESPACES.len()];
    let description = DESCRIPTIONS[index % DESCRIPTIONS.len()];
    let language = LANGUAGES[index % LANGUAGES.len()];
    let word = WORDS[index % WORDS.len()];
    let topic = TOPICS[(index / WORDS.len()) % TOPICS.len()];
    let name = match index % 23 {
        16 => format!("中文仓库名称验证样例-{index:03}"),
        22 => format!("a-very-long-generated-repository-name-for-truncation-checks-{index:03}"),
        _ => format!("{word}-{topic}-{:03}", index + 1),
    };
    RepoFixture::new(namespace, name, description, language)
}

/// Every repository the sidebar lists, in stable order. The sidebar shows all of them and only
/// its own group scrolls; pagination and sorting are not decided yet, so this list is a fixture,
/// not a product rule.
pub fn repositories() -> Vec<RepoFixture> {
    let mut all: Vec<RepoFixture> = CURATED
        .iter()
        .map(|(namespace, name, description, language)| {
            RepoFixture::new(*namespace, *name, description, language)
        })
        .collect();
    all.extend((0..GENERATED).map(generated));
    all
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
    fn the_repository_fixture_is_stable_and_covers_text_cases() {
        let first = repositories();
        let second = repositories();
        assert_eq!(first, second, "fixtures must not vary between calls");
        assert!(
            first.len() >= 100,
            "the bounded list needs at least 100 entries, got {}",
            first.len()
        );
        let mut names = std::collections::BTreeSet::new();
        for repo in &first {
            assert!(
                names.insert(repo.full_name()),
                "{} is duplicated",
                repo.full_name()
            );
            assert!(!repo.name.is_empty());
            assert!(!repo.namespace.is_empty());
        }
        let has_cjk = first.iter().any(|repo| !repo.name.is_ascii());
        assert!(has_cjk, "a Chinese repository name is required");
        let longest = first
            .iter()
            .map(|repo| repo.namespace.chars().count() + repo.name.chars().count())
            .max()
            .unwrap_or(0);
        assert!(longest >= 40, "a long repository name is required");
    }

    #[test]
    fn home_fixtures_stay_small_and_deterministic() {
        assert!((6..=12).contains(&INVOLVED.len()));
        assert!((4..=8).contains(&FEED.len()));
        assert!(INVOLVED.iter().any(|row| row.title.contains('中')));
        assert!(FEED.iter().any(|row| row.target.contains('中')));
    }
}
