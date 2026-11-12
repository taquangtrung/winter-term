//! Markdown to styled text spans, for drawing Markdown and HTML blocks
//! natively: laid out by `cosmic-text` and rasterized to a texture. Headings,
//! emphasis, code, lists numbered and not, quotes, rules, and tables set in
//! aligned columns; an image shows its alt text. Line and block breaks are
//! newlines in the span text.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use unicode_width::UnicodeWidthStr;

// ========================================================================
// Constants
// ========================================================================

/// What a bullet item starts with.
const BULLET: &str = "• ";

/// How far each level of a nested list is indented.
const INDENT: &str = "  ";

/// What a quoted line starts with.
const QUOTE_BAR: &str = "│ ";

/// The gap between a table's columns.
const COLUMN_GAP: &str = "  ";

/// What a rule, and the line under a table's header, are drawn with.
const RULE_CHAR: char = '─';

/// How long a horizontal rule is drawn.
const RULE_LENGTH: usize = 40;

// ========================================================================
// Data Structures
// ========================================================================

/// One run of text with a uniform style.
pub(crate) struct MdSpan {
    pub bold: bool,
    /// Code, set in the code color.
    pub code: bool,
    pub italic: bool,
    /// Set in the monospace font: code, and a table's cells, so they align.
    pub mono: bool,
    pub text: String,
}

/// The spans so far, and where the parser stands.
#[derive(Default)]
struct Writer {
    bold: u32,
    code_block: bool,
    heading: bool,
    italic: u32,
    /// Each open list, innermost last: the next number of a numbered one.
    lists: Vec<Option<u64>>,
    /// Whether the next text starts a line, for a quote's bar.
    line_start: bool,
    quote: u32,
    spans: Vec<MdSpan>,
    /// A table being read, set out once it has ended.
    table: Option<Table>,
}

/// A table's cells as read, the header first.
#[derive(Default)]
struct Table {
    cell: String,
    /// How many rows are the header.
    head: usize,
    row: Vec<String>,
    rows: Vec<Vec<String>>,
}

// ========================================================================
// Writer
// ========================================================================

impl Writer {
    /// Add `text` in the current style.
    fn text(&mut self, text: &str, code: bool) {
        if let Some(table) = self.table.as_mut() {
            table.cell.push_str(text);
            return;
        }
        let mono = code || self.code_block;
        self.push(
            text,
            self.bold > 0 || self.heading,
            self.italic > 0,
            mono,
            mono,
        );
    }

    /// Add a break, a prefix, or anything else unstyled.
    fn plain(&mut self, text: &str) {
        self.push(text, false, false, false, false);
    }

    fn push(&mut self, text: &str, bold: bool, italic: bool, mono: bool, code: bool) {
        if text.is_empty() {
            return;
        }
        if self.quote > 0 && self.line_start && text != "\n" {
            self.spans.push(MdSpan {
                bold: false,
                code: false,
                italic: false,
                mono: false,
                text: QUOTE_BAR.repeat(self.quote as usize),
            });
        }
        self.line_start = text.ends_with('\n');
        self.spans.push(MdSpan {
            bold,
            code,
            italic,
            mono,
            text: text.to_string(),
        });
    }

    /// The start of a list item: indented by its depth, then its number or
    /// bullet.
    fn item(&mut self) {
        // A nested list starts under its parent item's text, not after it.
        if !self.line_start {
            self.plain("\n");
        }
        let depth = self.lists.len().saturating_sub(1);
        let marker = match self.lists.last_mut() {
            Some(Some(number)) => {
                let marker = format!("{number}. ");
                *number += 1;
                marker
            }
            Some(None) | None => BULLET.to_string(),
        };
        self.plain(&format!("{}{marker}", INDENT.repeat(depth + 1)));
    }

    /// Set out a finished table: each column as wide as its widest cell,
    /// the header bold over a rule.
    fn table(&mut self, table: Table) {
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        let widths: Vec<usize> = (0..columns)
            .map(|column| {
                table
                    .rows
                    .iter()
                    .filter_map(|row| row.get(column))
                    .map(|cell| UnicodeWidthStr::width(cell.as_str()))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        for (index, row) in table.rows.iter().enumerate() {
            let line: Vec<String> = widths
                .iter()
                .enumerate()
                .map(|(column, width)| {
                    let cell = row.get(column).map_or("", String::as_str);
                    let pad = width.saturating_sub(UnicodeWidthStr::width(cell));
                    format!("{cell}{}", " ".repeat(pad))
                })
                .collect();
            let header = index < table.head;
            self.push(&line.join(COLUMN_GAP), header, false, true, false);
            self.plain("\n");
            if index + 1 == table.head {
                let total = widths.iter().sum::<usize>()
                    + COLUMN_GAP.len() * widths.len().saturating_sub(1);
                self.push(
                    &RULE_CHAR.to_string().repeat(total),
                    false,
                    false,
                    true,
                    false,
                );
                self.plain("\n");
            }
        }
        self.plain("\n");
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Parse `markdown` into styled spans. Newlines in `text` mark line and block
/// breaks; the caller lays the spans out as rich text.
pub fn parse(markdown: &str) -> Vec<MdSpan> {
    let mut out = Writer {
        line_start: true,
        ..Writer::default()
    };
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Start(Tag::Strong) => out.bold += 1,
            Event::End(TagEnd::Strong) => out.bold = out.bold.saturating_sub(1),
            Event::Start(Tag::Emphasis) => out.italic += 1,
            Event::End(TagEnd::Emphasis) => out.italic = out.italic.saturating_sub(1),
            Event::Start(Tag::Heading { .. }) => out.heading = true,
            Event::End(TagEnd::Heading(_)) => {
                out.heading = false;
                out.plain("\n\n");
            }
            Event::Start(Tag::List(first)) => out.lists.push(first),
            Event::End(TagEnd::List(_)) => {
                out.lists.pop();
                if out.lists.is_empty() {
                    out.plain("\n");
                }
            }
            Event::Start(Tag::Item) => out.item(),
            Event::End(TagEnd::Item) => {
                if !out.line_start {
                    out.plain("\n");
                }
            }
            Event::Start(Tag::BlockQuote(_)) => out.quote += 1,
            Event::End(TagEnd::BlockQuote(_)) => out.quote = out.quote.saturating_sub(1),
            Event::Start(Tag::CodeBlock(_)) => out.code_block = true,
            Event::End(TagEnd::CodeBlock) => {
                out.code_block = false;
                out.plain("\n");
            }
            Event::End(TagEnd::Paragraph) => {
                // A list item's paragraph ends with the item.
                if out.lists.is_empty() {
                    out.plain("\n\n");
                }
            }
            Event::Start(Tag::Image { .. }) => out.push("[", false, true, false, false),
            Event::End(TagEnd::Image) => out.push("]", false, true, false, false),
            Event::Rule => {
                out.plain(&RULE_CHAR.to_string().repeat(RULE_LENGTH));
                out.plain("\n\n");
            }
            Event::Start(Tag::Table(_)) => out.table = Some(Table::default()),
            Event::End(TagEnd::TableHead) => {
                if let Some(table) = out.table.as_mut() {
                    table.rows.push(std::mem::take(&mut table.row));
                    table.head = table.rows.len();
                }
            }
            Event::End(TagEnd::TableRow) => {
                if let Some(table) = out.table.as_mut() {
                    table.rows.push(std::mem::take(&mut table.row));
                }
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(table) = out.table.as_mut() {
                    let cell = std::mem::take(&mut table.cell);
                    table.row.push(cell.trim().to_string());
                }
            }
            Event::End(TagEnd::Table) => {
                if let Some(table) = out.table.take() {
                    out.table(table);
                }
            }
            Event::Text(text) => out.text(&text, false),
            Event::Code(text) => out.text(&text, true),
            Event::SoftBreak => out.plain(" "),
            Event::HardBreak => out.plain("\n"),
            _ => {}
        }
    }
    out.spans
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[MdSpan]) -> String {
        spans.iter().map(|span| span.text.as_str()).collect()
    }

    #[test]
    fn bold_and_code_spans_carry_style() {
        let spans = parse("a **b** `c`");
        let bold = spans.iter().find(|s| s.text == "b").expect("bold span");
        assert!(bold.bold);
        let code = spans.iter().find(|s| s.text == "c").expect("code span");
        assert!(code.mono && code.code);
    }

    #[test]
    fn test_a_table_is_set_in_aligned_columns_under_a_bold_header() {
        let spans = parse("| Name | Score |\n|---|---|\n| Alice | 95 |\n| Bo | 7 |\n");
        assert_eq!(
            text(&spans),
            "Name   Score\n────────────\nAlice  95   \nBo     7    \n\n"
        );
        let header = spans
            .iter()
            .find(|s| s.text.starts_with("Name"))
            .expect("header");
        assert!(
            header.bold && header.mono && !header.code,
            "aligned, not tinted"
        );
    }

    #[test]
    fn test_numbered_and_nested_lists_keep_their_markers() {
        let spans = parse("3. three\n4. four\n   - inner\n");
        assert_eq!(text(&spans), "  3. three\n  4. four\n    • inner\n\n");
    }

    #[test]
    fn test_a_quote_is_barred_on_each_line() {
        let spans = parse("> one\n>\n> two\n");
        assert_eq!(text(&spans), "│ one\n\n│ two\n\n");
    }

    #[test]
    fn test_an_image_shows_its_alt_text() {
        assert_eq!(text(&parse("![a chart](x.png)\n")), "[a chart]\n\n");
    }
}
