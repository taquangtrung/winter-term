//! The page a preview shows: a Markdown document written out as HTML, every
//! block wrapped in an element naming the source line it starts on, and a
//! stylesheet in the window's theme.

use mermaid_rs_renderer::RenderOptions;
use pulldown_cmark::{html, CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd};
use winter_render::Theme;

use crate::editors::highlight::{self, LineState};
use crate::model::paint::{Row, Span, Style};

// ========================================================================
// Constants
// ========================================================================

/// The attribute a block's wrapper names its source line in, counting from
/// zero.
pub(super) const LINE_ATTRIBUTE: &str = "data-line";

/// The scope a Markdown document's text sits in, for a code block whose
/// language is not known.
const DOCUMENT: &str = "text.html.markdown";

/// How much of the foreground tints a code background, a rule or border,
/// and text set back from the rest, over the theme's background.
const CODE_TINT: u8 = 20;
const BORDER_TINT: u8 = 56;
const MUTED_TINT: u8 = 150;

/// The palette slot links take their color from: blue.
const ANSI_BLUE: usize = 4;

/// What a link to somewhere in the page is told apart by.
const FRAGMENT: char = '#';

/// The language a fenced block names to be drawn as a diagram.
const MERMAID: &str = "mermaid";

// ========================================================================
// Functions
// ========================================================================

/// The stylesheet a page reads in `theme`'s colors, set out the way a
/// rendered README reads.
pub(super) fn stylesheet(theme: &Theme) -> String {
    let tint = |alpha: u8| theme.foreground.with_alpha(alpha).over(theme.background).to_hex();
    let background = theme.background.to_hex();
    let foreground = theme.foreground.to_hex();
    let link = theme.ansi[ANSI_BLUE].to_hex();
    let code = tint(CODE_TINT);
    let border = tint(BORDER_TINT);
    let muted = tint(MUTED_TINT);
    format!(
        "html {{ background: {background}; }}
body {{ margin: 0; padding: 16px 28px 40vh; color: {foreground}; background: {background};
  font-family: sans-serif; font-size: 16px; line-height: 1.5; }}
h1, h2, h3, h4, h5, h6 {{ margin: 24px 0 16px; font-weight: 600; line-height: 1.25; }}
h1 {{ font-size: 2em; padding-bottom: 0.3em; border-bottom: 1px solid {border}; }}
h2 {{ font-size: 1.5em; padding-bottom: 0.3em; border-bottom: 1px solid {border}; }}
h3 {{ font-size: 1.25em; }}
h6 {{ color: {muted}; }}
p, blockquote, ul, ol, table, .code {{ margin: 0 0 16px; }}
a {{ color: {link}; text-decoration: none; }}
ul, ol {{ padding-left: 2em; }}
li + li {{ margin-top: 0.25em; }}
code, .math {{ font-family: monospace; font-size: 0.9em; padding: 0.1em 0.35em;
  border-radius: 4px; background: {code}; }}
.math {{ color: {muted}; }}
.math-display {{ display: block; margin: 0.5em 0; text-align: center; }}
.diagram {{ margin: 0 0 16px; text-align: center; }}
.diagram svg {{ max-width: 100%; height: auto; }}
.code pre {{ margin: 0; padding: 12px 16px !important; border-radius: 6px; font-size: 0.9em;
  line-height: 1.45; background: {code} !important; white-space: pre-wrap; }}
blockquote {{ padding: 0 1em; color: {muted}; border-left: 0.25em solid {border}; }}
table {{ border-collapse: collapse; }}
th, td {{ padding: 6px 13px; border: 1px solid {border}; }}
th {{ font-weight: 600; }}
tr:nth-child(2n) {{ background: {code}; }}
hr {{ height: 0.25em; margin: 24px 0; border: 0; background: {border}; }}
img {{ max-width: 100%; }}
input[type=checkbox] {{ margin-right: 0.4em; }}"
    )
}

/// How diagrams are drawn in `theme`: its background and text, boxes in the
/// tint code sits on, edged and joined in the tint of the page's rules.
pub(super) fn diagram_options(theme: &Theme) -> RenderOptions {
    let tint = |alpha: u8| theme.foreground.with_alpha(alpha).over(theme.background).to_hex();
    let background = theme.background.to_hex();
    let foreground = theme.foreground.to_hex();
    let mut diagrams = match theme.background.luminance() < 0.5 {
        true => mermaid_rs_renderer::Theme::dark(),
        false => mermaid_rs_renderer::Theme::modern(),
    };
    diagrams.background = background.clone();
    diagrams.edge_label_background = background;
    diagrams.primary_color = tint(CODE_TINT);
    diagrams.cluster_background = tint(CODE_TINT);
    diagrams.sequence_actor_fill = tint(CODE_TINT);
    diagrams.primary_border_color = tint(BORDER_TINT);
    diagrams.cluster_border = tint(BORDER_TINT);
    diagrams.sequence_actor_border = tint(BORDER_TINT);
    diagrams.sequence_actor_line = tint(BORDER_TINT);
    diagrams.line_color = tint(MUTED_TINT);
    diagrams.primary_text_color = foreground.clone();
    diagrams.text_color = foreground;
    RenderOptions {
        theme: diagrams,
        ..RenderOptions::default()
    }
}

/// `markdown` as a whole HTML page in `stylesheet`: every block wrapped in
/// a `div` naming the line it starts on, code blocks colored by their
/// language as `styled_html` writes highlighted rows, and Mermaid blocks
/// drawn as `diagrams` says.
pub(super) fn html(
    markdown: &str,
    stylesheet: &str,
    diagrams: &RenderOptions,
    styled_html: &dyn Fn(&[Row]) -> String,
) -> String {
    let starts: Vec<usize> = std::iter::once(0)
        .chain(markdown.match_indices('\n').map(|(at, _)| at + 1))
        .collect();
    let line_of = |offset: usize| starts.partition_point(|start| *start <= offset).saturating_sub(1);
    let marker = |line: usize| Event::Html(CowStr::from(format!("<div {LINE_ATTRIBUTE}=\"{line}\">")));
    let close = || Event::Html(CowStr::Borrowed("</div>"));

    let mut events = Vec::new();
    let mut code: Option<(usize, String, String)> = None;
    for (event, range) in Parser::new_ext(markdown, options()).into_offset_iter() {
        if let Some((line, language, text)) = code.as_mut() {
            match event {
                Event::Text(chunk) => text.push_str(&chunk),
                Event::End(TagEnd::CodeBlock) => {
                    let block = match diagram(language, text, diagrams) {
                        Some(svg) => format!("<div class=\"diagram\" {LINE_ATTRIBUTE}=\"{line}\">{svg}</div>"),
                        None => {
                            let block = code_block(language, text, styled_html);
                            format!("<div class=\"code\" {LINE_ATTRIBUTE}=\"{line}\">{block}</div>")
                        }
                    };
                    events.push(Event::Html(CowStr::from(block)));
                    code = None;
                }
                _ => {}
            }
            continue;
        }
        let line = line_of(range.start);
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or_default().to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((line, language, String::new()));
            }
            // A list item's wrapper goes inside it, where a list allows one.
            Event::Start(Tag::Item) => events.extend([Event::Start(Tag::Item), marker(line)]),
            Event::End(TagEnd::Item) => events.extend([close(), Event::End(TagEnd::Item)]),
            Event::Start(tag) if wraps(&tag) => events.extend([marker(line), Event::Start(tag)]),
            Event::End(tag) if wraps_end(tag) => events.extend([Event::End(tag), close()]),
            Event::Rule => events.extend([marker(line), Event::Rule, close()]),
            Event::InlineMath(tex) => events.push(math(tex, false)),
            Event::DisplayMath(tex) => events.push(math(tex, true)),
            other => events.push(other),
        }
    }
    name_headings(&mut events);
    let mut body = String::new();
    html::push_html(&mut body, events.into_iter());
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>{stylesheet}</style></head>\
         <body>{body}</body></html>"
    )
}

/// Whether `href` points somewhere in the page itself, and where: the
/// fragment after its `#`.
pub(super) fn fragment(href: &str) -> Option<&str> {
    href.strip_prefix(FRAGMENT).filter(|name| !name.is_empty())
}

/// The Markdown the preview reads: what GitHub renders, with math typeset.
fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_MATH
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
}

/// Whether a block of this kind is wrapped in a line-naming `div`: every
/// block a reader can point at, but not an HTML block, which may open an
/// element one block and close it another.
fn wraps(tag: &Tag) -> bool {
    matches!(
        tag,
        Tag::Paragraph | Tag::Heading { .. } | Tag::BlockQuote(_) | Tag::List(_) | Tag::Table(_)
    )
}

/// [`wraps`], for the end of a block.
fn wraps_end(tag: TagEnd) -> bool {
    matches!(
        tag,
        TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::BlockQuote(_) | TagEnd::List(_) | TagEnd::Table
    )
}

/// `tex` typeset as an SVG drawn in the text's color, set on its own line
/// when `display`; as written, where it cannot be typeset.
fn math(tex: CowStr<'_>, display: bool) -> Event<'_> {
    let options = mathra::Options {
        display,
        ..mathra::Options::default()
    };
    match (mathra::convert(&tex, &options), display) {
        (Ok(typeset), true) => Event::InlineHtml(CowStr::from(format!("<span class=\"math-display\">{}</span>", typeset.svg))),
        (Ok(typeset), false) => Event::InlineHtml(CowStr::from(typeset.svg)),
        (Err(_), true) => Event::DisplayMath(tex),
        (Err(_), false) => Event::InlineMath(tex),
    }
}

/// Give every heading not named in the source the name GitHub gives it, so
/// a `#name` link written for GitHub lands on it: its text lowercased, words
/// joined by hyphens, punctuation dropped, and a count after a name taken.
fn name_headings(events: &mut [Event]) {
    let mut taken: Vec<String> = Vec::new();
    for at in 0..events.len() {
        if !matches!(&events[at], Event::Start(Tag::Heading { id: None, .. })) {
            continue;
        }
        let text: String = events[at + 1..]
            .iter()
            .take_while(|event| !matches!(event, Event::End(TagEnd::Heading(_))))
            .filter_map(|event| match event {
                Event::Text(text) | Event::Code(text) => Some(text.as_ref()),
                _ => None,
            })
            .collect();
        let slug: String = text
            .trim()
            .to_lowercase()
            .chars()
            .filter_map(|c| match c {
                ' ' => Some('-'),
                c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
                _ => None,
            })
            .collect();
        let repeats = taken.iter().filter(|name| **name == slug).count();
        let name = match repeats {
            0 => slug.clone(),
            n => format!("{slug}-{n}"),
        };
        taken.push(slug);
        if let Event::Start(Tag::Heading { id, .. }) = &mut events[at] {
            *id = Some(CowStr::from(name));
        }
    }
}

/// A fenced block's `text` drawn as an SVG diagram, where its `language`
/// names Mermaid and the text reads as a diagram; `None` otherwise, to be
/// shown as code.
fn diagram(language: &str, text: &str, diagrams: &RenderOptions) -> Option<String> {
    (language == MERMAID)
        .then(|| mermaid_rs_renderer::render_with_options(text, diagrams.clone()).ok())
        .flatten()
}

/// A code block's `text` as `styled_html` writes it, colored by `language`
/// where the highlighter knows it.
fn code_block(language: &str, text: &str, styled_html: &dyn Fn(&[Row]) -> String) -> String {
    let grammar = (!language.is_empty()).then(|| highlight::of_token(language)).flatten();
    let mut state = grammar.map(LineState::start);
    let plain = highlight::scoped(&[DOCUMENT, "markup.raw.block.markdown"]);
    let rows: Vec<Row> = text
        .trim_end_matches('\n')
        .split('\n')
        .map(|line| match state.as_mut() {
            Some(state) => {
                let (styles, next) = highlight::highlight(line, state);
                *state = next;
                styled_runs(line, &styles)
            }
            None => vec![Span::new(plain, line)],
        })
        .collect();
    styled_html(&rows)
}

/// `line` cut into one span for each run of characters `styles` colors
/// alike.
fn styled_runs(line: &str, styles: &[Style]) -> Row {
    let mut row: Row = Vec::new();
    let styles = styles.iter().copied().chain(std::iter::repeat(Style::Normal));
    for (c, style) in line.chars().zip(styles) {
        match row.last_mut() {
            Some(last) if last.style == style => last.text.push(c),
            _ => row.push(Span::new(style, c.to_string())),
        }
    }
    row
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_rows(rows: &[Row]) -> String {
        let text: Vec<String> = rows.iter().map(|row| row.iter().map(|span| span.text.as_str()).collect()).collect();
        format!("<pre>{}</pre>", text.join("\n"))
    }

    #[test]
    fn test_each_block_names_the_source_line_it_starts_on() {
        let page = html("# Title\n\ntext\n\n- one\n- two\n", "", &RenderOptions::default(), &plain_rows).replace('\n', "");
        assert!(page.contains("<div data-line=\"0\"><h1 id=\"title\">Title</h1></div>"), "{page}");
        assert!(page.contains("<div data-line=\"2\"><p>text</p></div>"), "{page}");
        assert!(page.contains("<li><div data-line=\"4\">one</div></li>"), "{page}");
        assert!(page.contains("<li><div data-line=\"5\">two</div></li>"), "{page}");
    }

    #[test]
    fn test_a_code_block_is_written_whole_by_the_highlighter_under_its_line() {
        let page = html("intro\n\n```rust\nlet a = 1;\nlet b = 2;\n```\n", "", &RenderOptions::default(), &plain_rows);
        assert!(
            page.contains("<div class=\"code\" data-line=\"2\"><pre>let a = 1;\nlet b = 2;</pre></div>"),
            "{page}"
        );
    }

    #[test]
    fn test_headings_are_named_as_github_names_them_for_links_into_the_page() {
        let page = html("# Getting Started!\n\n## Usage\n\n## Usage\n", "", &RenderOptions::default(), &plain_rows);
        assert!(page.contains("<h1 id=\"getting-started\">"), "{page}");
        assert!(page.contains("<h2 id=\"usage\">"), "{page}");
        assert!(page.contains("<h2 id=\"usage-1\">"), "a name taken gets a count: {page}");
    }

    #[test]
    fn test_math_is_typeset_as_svg_in_the_text_s_color_and_display_math_set_apart() {
        let page = html("Inline $x^2$ here.\n\n$$\\frac{a}{b}$$\n", "", &RenderOptions::default(), &plain_rows);
        assert!(!page.contains("class=\"math math-"), "nothing left as source: {page}");
        let inline = page.find("Inline <svg").expect("inline math typeset in the line");
        let display = page.find("<span class=\"math-display\"><svg").expect("display math set apart");
        assert!(inline < display);
        assert!(page.contains("currentColor"), "drawn in the text's color: {page}");
    }

    #[test]
    fn test_a_mermaid_block_is_drawn_as_a_diagram_and_one_that_does_not_parse_is_kept_as_code() {
        let page = html("```mermaid\ngraph TD\n  A --> B\n```\n", "", &RenderOptions::default(), &plain_rows);
        assert!(page.contains("<div class=\"diagram\" data-line=\"0\"><svg"), "{page}");
        let broken = html("```mermaid\nnot a diagram {{{\n```\n", "", &RenderOptions::default(), &plain_rows);
        assert!(broken.contains("<div class=\"code\" data-line=\"0\"><pre>not a diagram {{{</pre></div>"), "{broken}");
    }

    #[test]
    fn test_only_a_hash_link_with_a_name_points_into_the_page() {
        assert_eq!(fragment("#usage"), Some("usage"));
        assert_eq!(fragment("#"), None);
        assert_eq!(fragment("docs/usage.md#usage"), None);
    }
}
