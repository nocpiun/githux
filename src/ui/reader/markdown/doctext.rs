//! The read-order text model for document-wide selection.
//!
//! Selection is a pair of offsets into this model, not a pair of paragraphs.
//! Units are registered while the blocks are walked in exactly the order the
//! renderer walks them, and each unit records the top-level block it belongs
//! to. A virtualized block therefore maps its own bytes to document offsets
//! without being built, and Ctrl+A/C work over the whole model.

use std::ops::Range;

use super::ast::{Block, Inline};

/// One selectable unit: paragraph, heading, table cell, code block or maths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    /// Its range in the document text.
    pub at: Range<usize>,
    /// Its own text, in bytes.
    pub len: usize,
    /// Text inserted after this unit before the next one, such as a blank
    /// line between blocks or a tab between table cells.
    pub separator: &'static str,
    /// Link ranges inside this unit, in its own bytes.
    pub links: Vec<(Range<usize>, String)>,
}

/// The document's selectable text and its units.
#[derive(Clone, Debug, Default)]
pub struct DocText {
    text: String,
    units: Vec<Unit>,
    /// Unit indices of each top-level block, in render order.
    by_block: Vec<Vec<usize>>,
}

impl DocText {
    /// Build the model from the blocks the renderer will walk.
    pub fn build(blocks: &[Block]) -> Self {
        let mut model = Self::default();
        for block in blocks {
            model.by_block.push(Vec::new());
            model.block(block);
        }
        model
    }

    /// The whole text, as Ctrl+C would copy it.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// All the document text's range.
    pub fn range(&self) -> Range<usize> {
        0..self.text.len()
    }

    /// The text inside `range`, on char boundaries.
    pub fn slice(&self, range: Range<usize>) -> &str {
        let start = floor_char_boundary(&self.text, range.start.min(self.text.len()));
        let end = floor_char_boundary(&self.text, range.end.min(self.text.len())).max(start);
        &self.text[start..end]
    }

    /// The global unit index of block `block`'s unit `at`.
    pub fn unit_index(&self, block: usize, at: usize) -> Option<usize> {
        self.by_block.get(block)?.get(at).copied()
    }

    /// The units of one top-level block, in render order.
    pub fn units_of(&self, block: usize) -> impl Iterator<Item = &Unit> + '_ {
        self.by_block
            .get(block)
            .into_iter()
            .flatten()
            .filter_map(|ix| self.units.get(*ix))
    }

    /// The unit whose range holds `at`, half-open so a unit's last byte and
    /// the next unit's first byte are never confused.
    pub fn unit_containing(&self, at: usize) -> Option<&Unit> {
        self.units
            .iter()
            .find(|unit| unit.at.start <= at && at < unit.at.end)
            .or_else(|| {
                // The document's very last byte belongs to its last unit.
                self.units
                    .last()
                    .filter(|unit| unit.at.end == at && !unit.at.is_empty())
            })
    }

    /// The URL of a link covering document offset `at`.
    pub fn link_at(&self, at: usize) -> Option<String> {
        // Half-open: a unit ending exactly where the next starts must not
        // claim the next unit's first glyph (that glyph can begin a link).
        let unit = self
            .units
            .iter()
            .find(|unit| unit.at.start <= at && at < unit.at.end)
            .or_else(|| {
                self.units
                    .iter()
                    .find(|unit| unit.at.start <= at && at <= unit.at.end)
            })?;
        let local = at.checked_sub(unit.at.start)?;
        unit.links
            .iter()
            .find(|(range, _)| range.contains(&local))
            .map(|(_, url)| url.clone())
    }

    fn push(&mut self, text: &str, links: Vec<(Range<usize>, String)>) -> Range<usize> {
        self.push_with_separator(
            text, links, "

",
        )
    }

    /// Appends a unit followed by `separator`. The separator is part of the
    /// document text but not of the unit, so unit offsets stay exact while a
    /// copy keeps block and cell boundaries.
    fn push_with_separator(
        &mut self,
        text: &str,
        links: Vec<(Range<usize>, String)>,
        separator: &'static str,
    ) -> Range<usize> {
        let start = self.text.len();
        self.text.push_str(text);
        let at = start..self.text.len();
        let index = self.units.len();
        self.units.push(Unit {
            at: at.clone(),
            len: text.len(),
            separator,
            links,
        });
        if let Some(block) = self.by_block.last_mut() {
            block.push(index);
        }
        self.text.push_str(separator);
        at
    }

    fn block(&mut self, block: &Block) {
        match block {
            Block::Paragraph(inline) | Block::Heading(_, inline) => self.inline(inline),
            Block::Quote { body, .. } | Block::Footnote { body, .. } => self.blocks(body),
            Block::Code { text, .. } => {
                self.push(text, Vec::new());
            }
            Block::List { items, .. } => {
                for (_, blocks) in items {
                    self.blocks(blocks);
                }
            }
            Block::Table { head, rows, .. } => {
                for cell in head.iter().chain(rows.iter().flatten()) {
                    if cell.text.is_empty() {
                        continue;
                    }
                    self.push_with_separator(&cell.text, cell.links.clone(), "	");
                }
            }
            Block::Math { tex, .. } => {
                self.push(tex, Vec::new());
            }
            Block::Image { alt, .. } => {
                if !alt.is_empty() {
                    self.push(alt, Vec::new());
                }
            }
            Block::Html(text) => {
                if !text.is_empty() {
                    self.push(text, Vec::new());
                }
            }
            Block::Rule => {}
        }
    }

    fn blocks(&mut self, blocks: &[Block]) {
        for block in blocks {
            self.block(block);
        }
    }

    fn inline(&mut self, inline: &Inline) {
        if inline.text.is_empty() {
            return;
        }
        self.push(&inline.text, inline.links.clone());
    }
}

/// The char boundary at or before `at`.
pub fn floor_char_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::reader::markdown::ast::parse;

    #[test]
    fn the_model_follows_read_order() {
        let doc = parse("# Title\n\none *two*\n\n- a\n- b\n\n```rust\nfn x() {}\n```");
        let model = DocText::build(&doc.blocks);
        let text = model.text();
        let title = text.find("Title").unwrap();
        let one = text.find("one two").unwrap();
        let code = text.find("fn x()").unwrap();
        assert!(title < one && one < code, "{text:?}");
    }

    #[test]
    fn every_block_maps_to_its_units() {
        let doc = parse("# T\n\np\n\n| a | b |\n|---|---|\n| c | d |");
        let model = DocText::build(&doc.blocks);
        for ix in 0..doc.blocks.len() {
            let units: Vec<_> = model.units_of(ix).collect();
            if matches!(doc.blocks[ix], Block::Rule) {
                continue;
            }
            assert!(!units.is_empty(), "block {ix} has a unit");
        }
    }

    #[test]
    fn link_lookup_uses_document_offsets() {
        let doc = parse("see [Guide](guide.md) now");
        let model = DocText::build(&doc.blocks);
        let at = model.text().find("Guide").unwrap();
        assert_eq!(model.link_at(at).as_deref(), Some("guide.md"));
    }

    #[test]
    fn copying_keeps_block_and_cell_boundaries() {
        let doc = parse(
            "intro

next

- a
- b

| x | y |
|---|---|
| p | q |",
        );
        let model = DocText::build(&doc.blocks);
        let text = model.text();
        assert!(
            text.contains(
                "intro

next"
            ),
            "{text:?}"
        );
        assert!(text.contains('a') && text.contains('b'), "{text:?}");
        assert!(!text.contains("intronext"), "{text:?}");
        assert!(
            !text.contains(
                "ab

|x|y|"
            ),
            "{text:?}"
        );
    }

    #[test]
    fn a_link_at_the_start_of_a_unit_is_found() {
        let doc = parse(
            "intro

[x](guide.md)",
        );
        let model = DocText::build(&doc.blocks);
        let at = model.text().find('x').unwrap();
        assert_eq!(model.link_at(at).as_deref(), Some("guide.md"));
    }

    #[test]
    fn slices_never_split_a_multibyte_character() {
        let model = DocText::build(&parse("中文 emoji 🚀").blocks);
        let text = model.text().to_string();
        let inside = text.find('🚀').unwrap() + 1;
        let sliced = model.slice(0..inside).to_string();
        assert!(text.starts_with(&sliced), "{sliced:?}");
    }
}
