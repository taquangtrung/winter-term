//! Markup's markers hidden, as the `editor-markup-markers` setting asks:
//! Markdown's `**` and `_`, its backticks, a heading's `#`s, and a link's
//! `[` and `](url)`, found by `cherry::markup` and concealed by Cherry. The
//! hidden characters stay in the text: the cursor walks over them, and
//! they take no cell on screen. Each heading shows its number (`1.2.`)
//! before its text, where its `#`s were, in its own color.

use cherry::{ConcealMode, DocumentId, Inlay};
use winter_model::paint::MarkupMarkers;

use super::session::Session;

// ========================================================================
// Constants
// ========================================================================

/// The owner of the concealed markers.
const MARKERS: &str = "markup";

/// The owner of the headings' numbers.
const NUMBERS: &str = "heading-numbers";

/// The extensions of the files whose markers hide.
const MARKDOWN: [&str; 3] = ["md", "markdown", "mdx"];

// ========================================================================
// Session: markup
// ========================================================================

impl Session {
    /// Hides the markers of `document`, where it is Markdown, as its text
    /// now reads; worked out again only once it changed.
    pub(crate) fn conceal_markup(&mut self, document: DocumentId) {
        let version = self.editor.document_version(document);
        let Some(file) = self.files.get_mut(&document) else {
            return;
        };
        let markdown = file
            .path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| MARKDOWN.contains(&extension.to_ascii_lowercase().as_str()));
        if !markdown || file.concealed == version {
            return;
        }
        file.concealed = version;
        let lines = self.lines(document);
        let text = lines.join("\n");
        let hidden = cherry::markup::markdown_conceals(&text);
        self.editor.set_conceals(document, MARKERS, hidden);
        self.editor
            .set_inlays(document, NUMBERS, heading_numbers(&lines, &text));
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The number of each heading of Markdown `text`, its `lines`, as an inlay
/// before the heading's text, past its `#`s and blank, colored as the text.
fn heading_numbers(lines: &[String], text: &str) -> Vec<Inlay> {
    let mut starts = Vec::with_capacity(lines.len());
    let mut at = 0;
    for line in lines {
        starts.push(at);
        at += line.chars().count() + 1;
    }
    cherry::markup::heading_numbers(text)
        .into_iter()
        .filter_map(|heading| {
            let offset = starts.get(heading.line)? + heading.level + 1;
            let level = format!("heading.{}.markdown", heading.level);
            let style = crate::syntax::scoped(&[
                "text.html.markdown",
                "markup.heading.markdown",
                &level,
                "entity.name.section.markdown",
            ]);
            let inlay = Inlay::new(offset, format!("{} ", heading.number));
            Some(inlay.with_highlight(crate::syntax::highlight_id(style)))
        })
        .collect()
}

/// How Cherry conceals for the setting's `markers`.
pub(crate) fn conceal_mode(markers: MarkupMarkers) -> ConcealMode {
    match markers {
        MarkupMarkers::AlwaysHidden => ConcealMode::Hide,
        MarkupMarkers::ShowOnCursor => ConcealMode::RevealCursorLine,
        MarkupMarkers::Visible => ConcealMode::Show,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use crate::session;

    #[test]
    fn test_markdown_s_markers_are_concealed_and_other_files_keep_theirs() {
        let dir = std::env::temp_dir().join(format!("winter-markup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a folder");
        let (notes, code) = (dir.join("notes.md"), dir.join("main.rs"));
        std::fs::write(&notes, "a **b**\n").expect("written");
        std::fs::write(&code, "a **b**\n").expect("written");
        session::with(|session| {
            let (markdown, _) = session.open_view(&notes).expect("opens");
            let (rust, _) = session.open_view(&code).expect("opens");
            session.conceal_markup(markdown);
            session.conceal_markup(rust);
            assert_eq!(
                session.editor.conceals(markdown, super::MARKERS),
                [2..4, 5..7]
            );
            assert!(session.editor.conceals(rust, super::MARKERS).is_empty());
        });
    }

    #[test]
    fn test_a_heading_s_number_shows_before_its_text() {
        let dir = std::env::temp_dir().join(format!("winter-numbers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a folder");
        let notes = dir.join("notes.md");
        std::fs::write(&notes, "# Title\n## One\n## Two\n").expect("written");
        session::with(|session| {
            let (document, _) = session.open_view(&notes).expect("opens");
            session.conceal_markup(document);
            let shown: Vec<(usize, String)> = session
                .editor
                .inlays(document, super::NUMBERS)
                .iter()
                .map(|inlay| (inlay.offset, inlay.text.clone()))
                .collect();
            assert_eq!(shown, [(11, "1. ".to_owned()), (18, "2. ".to_owned())]);
        });
    }
}
