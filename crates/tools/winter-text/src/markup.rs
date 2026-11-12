//! Markup's markers hidden, as the `editor-markup-markers` setting asks:
//! Markdown's `**` and `_`, its backticks, a heading's `#`s, and a link's
//! `[` and `](url)`, found by `viento-markup` and concealed by Viento. The
//! hidden characters stay in the text: the cursor walks over them, and
//! they take no cell on screen.

use viento::{ConcealMode, DocumentId};
use winter_model::paint::MarkupMarkers;

use super::session::Session;

// ========================================================================
// Constants
// ========================================================================

/// The owner of the concealed markers.
const MARKERS: &str = "markup";

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
        let text = self.lines(document).join("\n");
        let hidden = viento_markup::markdown_conceals(&text);
        self.editor.set_conceals(document, MARKERS, hidden);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// How Viento conceals for the setting's `markers`.
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
}
