//! The Markdown a language server answers a hover with, as a card shows it:
//! the marks read rather than shown, each char styled as the editor styles
//! the same Markdown (a heading, a bold run, inline code), and each fenced
//! block's code in its language's colors.

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use winter_model::paint::Style;
use winter_syntax::{Grammar, LineState};

// ========================================================================
// Constants
// ========================================================================

/// The scope a Markdown document's text sits in.
const DOCUMENT: &str = "text.html.markdown";

/// The scopes of each kind of run, inside [`DOCUMENT`].
const BOLD: &str = "markup.bold.markdown";
const HEADING: &str = "markup.heading.markdown";
const INLINE_CODE: &str = "markup.raw.inline.markdown";
const ITALIC: &str = "markup.italic.markdown";
const LINK: &str = "markup.underline.link.markdown";
const STRIKE: &str = "markup.strikethrough.markdown";

/// What a bullet item starts with, and how far each list level indents.
const BULLET: &str = "• ";
const INDENT: &str = "  ";

/// What a quoted line starts with.
const QUOTE_BAR: &str = "│ ";

/// What a rule is drawn with, and how long.
const RULE_CHAR: char = '─';
const RULE_LENGTH: usize = 40;

/// What sets a table's cells apart.
const CELL_GAP: &str = "  ";

// ========================================================================
// Data Structures
// ========================================================================

/// The lines written so far, and where the reading stands.
struct Writer {
    bold: u32,
    /// A fenced or indented block being read: its language, and its text.
    code: Option<(String, String)>,
    /// The language a block that names none is in: the file's.
    fallback: Option<Grammar>,
    /// Whether the last line is open with nothing past its prefix, so a
    /// block starting goes on it: an item's first paragraph, after the
    /// bullet.
    fresh: bool,
    /// Whether a block ended, so the next starts after a blank line.
    gap: bool,
    heading: bool,
    italic: u32,
    lines: Vec<String>,
    link: u32,
    /// Each open list, innermost last: the next number of a numbered one.
    lists: Vec<Option<u64>>,
    /// Whether the last line is open for more text.
    open: bool,
    quote: usize,
    strike: u32,
    styles: Vec<Vec<Style>>,
}

// ========================================================================
// Free functions
// ========================================================================

/// `markdown` as lines and the style of each of their chars: a block of
/// code that names no language is read as `fallback`, the file's.
pub(super) fn styled(markdown: &str, fallback: Option<Grammar>) -> (Vec<String>, Vec<Vec<Style>>) {
    let mut writer = Writer::new(fallback);
    for event in Parser::new_ext(
        markdown,
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES,
    ) {
        writer.event(event);
    }
    while writer
        .lines
        .last()
        .is_some_and(|line| line.trim().is_empty())
    {
        writer.lines.pop();
        writer.styles.pop();
    }
    (writer.lines, writer.styles)
}

// ========================================================================
// Writer
// ========================================================================

impl Writer {
    fn new(fallback: Option<Grammar>) -> Self {
        Self {
            bold: 0,
            code: None,
            fallback,
            fresh: false,
            gap: false,
            heading: false,
            italic: 0,
            lines: Vec::new(),
            link: 0,
            lists: Vec::new(),
            open: false,
            quote: 0,
            strike: 0,
            styles: Vec::new(),
        }
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => match &mut self.code {
                Some((_, code)) => code.push_str(&text),
                None => self.push(&text, self.style()),
            },
            Event::Code(text) => {
                let style = self.scoped(Some(INLINE_CODE));
                self.push(&text, style);
            }
            Event::SoftBreak => self.push(" ", Style::Normal),
            Event::HardBreak => self.open = false,
            Event::Rule => {
                self.start_block();
                let rule: String = std::iter::repeat_n(RULE_CHAR, RULE_LENGTH).collect();
                self.push(&rule, Style::Dim);
                self.end_block();
            }
            Event::TaskListMarker(done) => {
                let mark = match done {
                    true => "[x] ",
                    false => "[ ] ",
                };
                self.push(mark, Style::Dim);
            }
            Event::Html(_)
            | Event::InlineHtml(_)
            | Event::FootnoteReference(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_) => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => self.start_block(),
            Tag::Heading { .. } => {
                self.start_block();
                self.heading = true;
            }
            Tag::BlockQuote(_) => {
                self.start_block();
                self.quote += 1;
                self.open = false;
            }
            Tag::CodeBlock(kind) => {
                self.start_block();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split(|c: char| c == ',' || c.is_whitespace())
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((language, String::new()));
            }
            Tag::List(first) => {
                if self.lists.is_empty() {
                    self.start_block();
                }
                self.lists.push(first);
            }
            Tag::Item => {
                self.open = false;
                self.gap = false;
                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        *number += 1;
                        format!("{}. ", *number - 1)
                    }
                    Some(None) | None => BULLET.to_string(),
                };
                let depth = self.lists.len().saturating_sub(1);
                self.start_line(depth);
                self.push(&marker, Style::Dim);
                self.fresh = true;
            }
            Tag::Table(_) => self.start_block(),
            Tag::TableHead | Tag::TableRow => {
                self.open = false;
                self.start_line(self.lists.len());
            }
            Tag::TableCell => {
                if !self.fresh {
                    self.push(CELL_GAP, Style::Normal);
                }
            }
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { .. } => self.link += 1,
            Tag::Image { .. }
            | Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_) => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Table => self.end_block(),
            TagEnd::Heading(_) => {
                self.heading = false;
                self.end_block();
            }
            TagEnd::BlockQuote(_) => {
                self.quote = self.quote.saturating_sub(1);
                self.end_block();
            }
            TagEnd::CodeBlock => {
                if let Some((language, text)) = self.code.take() {
                    self.write_code(&language, &text);
                }
                self.end_block();
            }
            TagEnd::List(_) => {
                self.lists.pop();
                if self.lists.is_empty() {
                    self.end_block();
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => self.link = self.link.saturating_sub(1),
            TagEnd::Item
            | TagEnd::TableHead
            | TagEnd::TableRow
            | TagEnd::TableCell
            | TagEnd::Image
            | TagEnd::HtmlBlock
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::MetadataBlock(_) => {}
        }
    }

    /// Start a block: on the open line where it has only its prefix, else
    /// on a line of its own, after a blank one where a block came before.
    fn start_block(&mut self) {
        if self.open && self.fresh {
            return;
        }
        if self.gap && !self.lines.is_empty() {
            self.start_line(self.lists.len());
        }
        self.gap = false;
        self.open = false;
    }

    fn end_block(&mut self) {
        self.open = false;
        self.gap = true;
    }

    /// Open a line under a quote's bars, indented `depth` list levels.
    fn start_line(&mut self, depth: usize) {
        self.lines.push(String::new());
        self.styles.push(Vec::new());
        self.open = true;
        for _ in 0..self.quote {
            self.push(QUOTE_BAR, Style::Dim);
        }
        self.push(&INDENT.repeat(depth), Style::Normal);
        self.fresh = true;
    }

    /// Write `text` in `style` on the open line, a new one where none is.
    fn push(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        if !self.open {
            self.start_line(self.lists.len());
        }
        let (Some(line), Some(styles)) = (self.lines.last_mut(), self.styles.last_mut()) else {
            return;
        };
        line.push_str(text);
        styles.extend(std::iter::repeat_n(style, text.chars().count()));
        self.fresh = false;
    }

    /// Write a block of code, each line in the colors of `language`, or of
    /// the file's language where it names none.
    fn write_code(&mut self, language: &str, text: &str) {
        let grammar = match language.is_empty() {
            true => self.fallback,
            false => winter_syntax::of_token(language),
        };
        let mut state = grammar.map(LineState::start);
        for line in text.trim_end_matches('\n').split('\n') {
            let line = line.replace('\t', INDENT);
            self.open = false;
            self.start_line(self.lists.len());
            let styles = match state.as_mut() {
                Some(now) => {
                    let (styles, next) = winter_syntax::highlight(&line, now);
                    *now = next;
                    styles
                }
                None => Vec::new(),
            };
            let styles = styles.into_iter().chain(std::iter::repeat(Style::Normal));
            for (c, style) in line.chars().zip(styles) {
                self.push(c.encode_utf8(&mut [0; 4]), style);
            }
        }
    }

    /// The style text takes inside the runs now open.
    fn style(&self) -> Style {
        let link = (self.link > 0).then_some(LINK);
        self.scoped(link)
    }

    /// The style of text inside the runs now open, and `inner` within them.
    fn scoped(&self, inner: Option<&str>) -> Style {
        let open = [
            (self.heading, HEADING),
            (self.bold > 0, BOLD),
            (self.italic > 0, ITALIC),
            (self.strike > 0, STRIKE),
        ];
        let names: Vec<&str> = open
            .into_iter()
            .filter_map(|(on, name)| on.then_some(name))
            .chain(inner)
            .collect();
        match names.is_empty() {
            true => Style::Normal,
            false => winter_syntax::scoped(&[&[DOCUMENT][..], &names].concat()),
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn rust() -> Option<Grammar> {
        winter_syntax::named("Rust")
    }

    #[test]
    fn test_marks_are_read_not_shown_and_blocks_are_set_apart() {
        let markdown = "```rust\nfn f() -> u8\n```\n\n---\n\n# Title\n\nDoes **a** `thing`, see [here](https://x.y).";
        let (lines, styles) = styled(markdown, None);
        let rule: String = std::iter::repeat_n(RULE_CHAR, RULE_LENGTH).collect();
        assert_eq!(
            lines,
            vec![
                "fn f() -> u8".to_string(),
                String::new(),
                rule,
                String::new(),
                "Title".to_string(),
                String::new(),
                "Does a thing, see here.".to_string(),
            ]
        );
        for (line, styles) in lines.iter().zip(&styles) {
            assert_eq!(line.chars().count(), styles.len(), "a style a char: {line}");
        }
        assert!(
            styles[0].iter().any(|style| *style != Style::Normal),
            "the code is colored"
        );
        let prose = &styles[6];
        assert_eq!(prose[0], Style::Normal);
        assert_ne!(prose[5], Style::Normal, "the bold run");
        assert_ne!(prose[7], prose[5], "the inline code, apart from the bold");
    }

    #[test]
    fn test_a_block_naming_no_language_is_read_as_the_file_s() {
        let markdown = "```\nlet x = 1;\n```";
        let (_, plain) = styled(markdown, None);
        let (_, colored) = styled(markdown, rust());
        assert!(plain[0].iter().all(|style| *style == Style::Normal));
        assert!(colored[0].iter().any(|style| *style != Style::Normal));
        let (lines, _) = styled("```plaintext\na *b*\n```", rust());
        assert_eq!(
            lines,
            vec!["a *b*".to_string()],
            "plain text kept as written"
        );
    }

    #[test]
    fn test_lists_take_their_markers_and_a_loose_item_keeps_its_first_line() {
        let (lines, _) = styled("- one\n- two\n  1. deep\n\nafter", None);
        assert_eq!(
            lines,
            vec![
                "• one".to_string(),
                "• two".to_string(),
                "  1. deep".to_string(),
                String::new(),
                "after".to_string(),
            ]
        );
        let (lines, _) = styled("- one\n\n- two", None);
        assert_eq!(lines[0], "• one");
        assert!(lines.contains(&"• two".to_string()), "{lines:?}");
    }
}
