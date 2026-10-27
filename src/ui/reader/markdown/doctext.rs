//! The read-order text model for document-wide selection.
//!
//! Selection is a pair of offsets into this model, not a pair of paragraphs.
//! Units are registered while the blocks are walked in exactly the order the
//! renderer walks them, and each unit records the top-level block it belongs
//! to. A virtualized block therefore maps its own bytes to document offsets
//! without being built, and Ctrl+A/C work over the whole model.

use std::ops::Range;

use super::ast::{Block, Inline};

/// Separators a copied table keeps: a tab between cells, a newline between
/// rows, and a blank line after the whole table (added by `separate`).
const CELL: &str = "\t";
const ROW: &str = "\n";

/// A placeholder cell for a repeated, presentation-only table header.
static EMPTY: Inline = Inline {
    text: String::new(),
    styles: Vec::new(),
    links: Vec::new(),
};

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
    /// The table being registered is the last chunk of its table: it closes
    /// the table instead of continuing into another chunk.
    table_final: bool,
    /// Top-level block currently being registered.
    block_ix: usize,
    /// The next top-level block index `sequence` will hand out at this depth.
    next_block: usize,
}

impl DocText {
    /// Build the model from the blocks the renderer will walk.
    pub fn build(blocks: &[Block]) -> Self {
        let mut model = Self::default();
        for _ in blocks {
            model.by_block.push(Vec::new());
        }
        model.sequence(blocks);
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
        if let Some(block) = self.by_block.get_mut(self.block_ix) {
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
            Block::Table {
                head,
                rows,
                repeated_head,
                ..
            } => {
                // Every cell is a unit, including an empty one: both table
                // renderers consume one unit per cell, so skipping an empty
                // cell here would shift every later cell's range.
                //
                // A chunk's repeated header is presentation only: its cells
                // get empty units (keeping the unit cursor in step) that add
                // no text and no separator, so a copy joins the chunk rows
                // exactly as if the table had never been split.
                if *repeated_head {
                    for _ in head {
                        self.push_with_separator(&EMPTY.text, Vec::new(), "");
                    }
                } else {
                    let last_cell = head.len().saturating_sub(1);
                    for (index, cell) in head.iter().enumerate() {
                        let separator = if index == last_cell { ROW } else { CELL };
                        self.push_with_separator(&cell.text, cell.links.clone(), separator);
                    }
                }
                // Body rows: tabs between cells, a newline after each row.
                // The last row leaves its ending to `separate`, which adds
                // the block gap exactly once per table, so a virtualized
                // chunk never inserts a blank row of its own.
                let last_row = rows.len().saturating_sub(1);
                let final_chunk = self.table_final;
                for (row, cells) in rows.iter().enumerate() {
                    let last_cell = cells.len().saturating_sub(1);
                    for (index, cell) in cells.iter().enumerate() {
                        let separator = if index != last_cell {
                            CELL
                        } else if row == last_row && final_chunk {
                            ""
                        } else {
                            ROW
                        };
                        self.push_with_separator(&cell.text, cell.links.clone(), separator);
                    }
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
        // Nested blocks belong to their top-level block: keep the index.
        for block in blocks {
            self.walk(block);
        }
    }

    /// Register one block at the current depth, advancing the top-level index
    /// only when `sequence` hands out a new top-level block.
    fn walk(&mut self, block: &Block) {
        self.block(block);
    }

    /// Walk blocks in render order, giving each table its ending: a chunk
    /// that continues into another chunk ends with a row break, the final
    /// chunk ends with the block gap.
    fn sequence(&mut self, blocks: &[Block]) {
        let mut rest = blocks.iter().peekable();
        while let Some(block) = rest.next() {
            let final_chunk = !matches!(
                rest.peek(),
                Some(Block::Table {
                    repeated_head: true,
                    ..
                })
            );
            self.table_final = final_chunk;
            self.block_ix = self.next_block;
            self.next_block += 1;
            self.walk(block);
            if matches!(block, Block::Table { .. }) && final_chunk {
                self.text.push_str("\n\n");
            }
        }
    }

    fn inline(&mut self, inline: &Inline) {
        // Empty inline text still registers: callers consume one unit per
        // block or cell in render order.
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

    fn plain(text: &str) -> Inline {
        Inline {
            text: text.to_string(),
            styles: Vec::new(),
            links: Vec::new(),
        }
    }

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
    fn an_empty_table_cell_keeps_the_following_cells_in_step() {
        let doc = parse(
            "| a | b |
|---|---|
|  | [x](guide.md) |",
        );
        let model = DocText::build(&doc.blocks);
        // Header row (2), then the body row's empty cell and link cell.
        let cells: Vec<_> = model.units.iter().map(|unit| unit.at.clone()).collect();
        assert_eq!(cells.len(), 4, "{cells:?}");
        let at = model.text().find('x').unwrap();
        assert_eq!(model.link_at(at).as_deref(), Some("guide.md"));
        assert!(model.slice(cells[2].clone()).is_empty(), "{:?}", cells[2]);
    }

    #[test]
    fn a_virtualized_table_chunk_adds_no_rows() {
        // Two chunks of the same table: the second repeats the header for
        // display but must not add text or separators.
        let first = Block::Table {
            aligns: Vec::new(),
            head: vec![plain("a"), plain("b")],
            rows: vec![vec![plain("c"), plain("d")]],
            repeated_head: false,
        };
        let second = Block::Table {
            aligns: Vec::new(),
            head: vec![plain("a"), plain("b")],
            rows: vec![vec![plain("e"), plain("f")]],
            repeated_head: true,
        };
        let model = DocText::build(&[first, second]);
        let text = model.text();
        let expected = ["a\tb\nc\td\ne\tf\n\n"][0];
        assert_eq!(text, expected, "{text:?}");
    }
    #[test]
    fn a_table_copies_as_rows_not_one_line() {
        let doc = parse(
            "| a | b |
|---|---|
| c | d |
| e | f |

after",
        );
        let model = DocText::build(&doc.blocks);
        let text = model.text();
        assert!(
            text.contains(
                "a	b
c	d
e	f"
            ),
            "{text:?}"
        );
        assert!(
            text.contains(
                "e	f

after"
            ),
            "{text:?}"
        );
        // Offsets still point at the original bytes.
        let at = text.find('d').unwrap();
        assert!(model.slice(at..at + 1) == "d");
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
