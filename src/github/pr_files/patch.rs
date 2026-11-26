//! Bounded unified patch segments from the pull request files endpoint.

const MAX_PATCH_BYTES: usize = 256 * 1024;
const MAX_PATCH_ROWS: usize = 8_000;
const MAX_LINE_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Header,
    Context,
    Addition,
    Deletion,
    Note,
}

impl RowKind {
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Addition => "+",
            Self::Deletion => "-",
            Self::Context => " ",
            Self::Header | Self::Note => "",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Row {
    pub kind: RowKind,
    pub old: Option<u64>,
    pub new: Option<u64>,
    pub text: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Patch {
    Unavailable,
    Text { rows: Vec<Row>, partial: bool },
}

#[derive(Debug)]
struct Range {
    start: u64,
    end: u64,
}

impl Range {
    fn parse(value: &str) -> Option<Self> {
        let (start, count) = value.split_once(',').unwrap_or((value, "1"));
        let start: u64 = start.parse().ok()?;
        let count: u64 = count.parse().ok()?;
        if start == 0 && count != 0 {
            return None;
        }
        Some(Self {
            start,
            end: start.checked_add(count)?,
        })
    }

    fn advance(&mut self) -> u64 {
        let line = self.start;
        self.start += 1;
        line
    }

    fn has_lines(&self) -> bool {
        self.start < self.end
    }
}

#[derive(Debug)]
struct Hunk {
    old: Range,
    new: Range,
}

impl Hunk {
    fn parse(line: &str) -> Option<Self> {
        let mut fields = line.split_ascii_whitespace();
        if fields.next()? != "@@" {
            return None;
        }
        let old = Range::parse(fields.next()?.strip_prefix('-')?)?;
        let new = Range::parse(fields.next()?.strip_prefix('+')?)?;
        (fields.next()? == "@@").then_some(Self { old, new })
    }

    fn complete(&self) -> bool {
        !self.old.has_lines() && !self.new.has_lines()
    }
}

impl Patch {
    pub(super) fn from_api(patch: Option<&str>, additions: u64, deletions: u64) -> Self {
        let Some(patch) = patch.filter(|patch| !patch.is_empty()) else {
            return Self::Unavailable;
        };
        let mut rows = Vec::new();
        let mut hunk: Option<Hunk> = None;
        let mut seen_additions = 0;
        let mut seen_deletions = 0;
        let mut bytes = 0;
        let mut partial = false;
        let mut can_have_note = false;
        for line in patch.split_terminator('\n') {
            bytes += line.len() + 1;
            if bytes > MAX_PATCH_BYTES
                || rows.len() == MAX_PATCH_ROWS
                || line.len() > MAX_LINE_BYTES
            {
                partial = true;
                break;
            }
            let (kind, old, new, text) = if line.starts_with("@@") {
                let Some(next) = Hunk::parse(line) else {
                    partial = true;
                    break;
                };
                partial |= hunk.as_ref().is_some_and(|previous| !previous.complete());
                hunk = Some(next);
                (RowKind::Header, None, None, line)
            } else if let Some(hunk) = hunk.as_mut() {
                match line.as_bytes().first() {
                    Some(b' ') if hunk.old.has_lines() && hunk.new.has_lines() => (
                        RowKind::Context,
                        Some(hunk.old.advance()),
                        Some(hunk.new.advance()),
                        &line[1..],
                    ),
                    Some(b'+') if hunk.new.has_lines() => {
                        seen_additions += 1;
                        (
                            RowKind::Addition,
                            None,
                            Some(hunk.new.advance()),
                            &line[1..],
                        )
                    }
                    Some(b'-') if hunk.old.has_lines() => {
                        seen_deletions += 1;
                        (
                            RowKind::Deletion,
                            Some(hunk.old.advance()),
                            None,
                            &line[1..],
                        )
                    }
                    Some(b'\\') if can_have_note && line == "\\ No newline at end of file" => {
                        (RowKind::Note, None, None, line)
                    }
                    _ => {
                        partial = true;
                        break;
                    }
                }
            } else {
                partial = true;
                break;
            };
            can_have_note = matches!(
                kind,
                RowKind::Context | RowKind::Addition | RowKind::Deletion
            );
            rows.push(Row {
                kind,
                old,
                new,
                text: text.to_owned(),
            });
        }
        partial |= hunk.is_none_or(|hunk| !hunk.complete())
            || seen_additions != additions
            || seen_deletions != deletions;
        Self::Text { rows, partial }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_changed_preserves_complete_rows_before_a_local_patch_limit() {
        let patch = format!(
            "@@ -0,0 +1,2 @@\n+visible\n+{}",
            "🧪".repeat(MAX_LINE_BYTES)
        );
        let Patch::Text {
            rows,
            partial: true,
        } = Patch::from_api(Some(&patch), 2, 0)
        else {
            panic!("expected partial patch");
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].text, "visible");
        assert_eq!(rows[1].new, Some(1));
    }
}
