//! A launcher: what a split opens in its new pane, holding the place for a
//! terminal, a file, a directory listing, or the git view, opened in the
//! directory the pane split from. Choosing one opens it in the launcher's tab.

use std::path::PathBuf;

use unicode_width::UnicodeWidthStr;

use winter_model::editor::{Editor, EditorOutcome, Launch, OpenTarget, Page};
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed};
use winter_model::input::Key;
use winter_model::paint::{Frame, Point, Row, Span, Style};
use winter_model::path::abbreviate_home;

// ========================================================================
// Constants
// ========================================================================

/// The editor's title.
const TITLE: &str = "New Pane";

/// What each key opens, in the order the page lists them.
const KEYS: KeyTable<Launch> = KeyTable {
    block: "launcher",
    bindings: &[
        Binding {
            action: Launch::Terminal,
            id: "terminal",
            keys: &["RET", "t"],
            label: "Terminal here",
        },
        Binding {
            action: Launch::Files,
            id: "files",
            keys: &["f"],
            label: "Open file…",
        },
        Binding {
            action: Launch::Dir,
            id: "dir",
            keys: &["d"],
            label: "Directory",
        },
        Binding {
            action: Launch::Git,
            id: "git",
            keys: &["g"],
            label: "Git status",
        },
    ],
    configurable: true,
    title: "New Pane",
};

/// Every key table of this surface, for the Keys editor to list.
pub(crate) const KEY_TABLES: &[&dyn Listed] = &[&KEYS];

/// The columns between a key and what it opens.
const KEY_GAP: usize = 3;

/// The page starts this far down the pane, as a share of its rows: one in
/// this many.
const TOP_SHARE: usize = 3;

// ========================================================================
// Data Structures
// ========================================================================

/// A pane's placeholder until what it holds is chosen.
pub struct LauncherEditor {
    /// Where what is chosen opens.
    dir: PathBuf,
    /// Height of the viewport when last painted.
    rows: usize,
}

// ========================================================================
// LauncherEditor
// ========================================================================

impl LauncherEditor {
    /// The launcher for `dir`.
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, rows: 0 }
    }
}

impl Page for LauncherEditor {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn content(&mut self, rows: usize, cols: usize, _wrap: bool) -> Frame {
        self.rows = rows;
        let width = KEYS
            .iter()
            .map(|binding| binding.keys[0].width())
            .max()
            .unwrap_or_default();
        let choices: Vec<String> = KEYS
            .iter()
            .map(|binding| {
                format!(
                    "{:<width$}{:KEY_GAP$}{}",
                    binding.keys[0], "", binding.label
                )
            })
            .collect();
        let block = choices
            .iter()
            .map(|line| line.width())
            .max()
            .unwrap_or_default();
        let pad = " ".repeat(cols.saturating_sub(block) / 2);
        let mut painted = vec![Row::new(); rows / TOP_SHARE];
        painted.push(vec![
            Span::plain(pad.clone()),
            Span::new(Style::Header, abbreviate_home(&self.dir)),
        ]);
        painted.push(Row::new());
        for (binding, line) in KEYS.iter().zip(&choices) {
            let (keys, label) = line.split_at(binding.keys[0].len());
            painted.push(vec![
                Span::plain(pad.clone()),
                Span::new(Style::Accent, keys.to_string()),
                Span::new(Style::Dim, label.to_string()),
            ]);
        }
        Frame::new(painted)
    }

    fn on_mouse(&mut self, at: Point) -> EditorOutcome {
        if at.drag {
            return EditorOutcome::Ignored;
        }
        let top = self.rows / TOP_SHARE + 2;
        if at.row >= top && at.row < top + KEYS.len() {
            let index = at.row - top;
            EditorOutcome::Launch(KEYS[index].action)
        } else {
            EditorOutcome::Ignored
        }
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        match table::find(&KEYS, key) {
            Some(launch) => EditorOutcome::Launch(launch),
            None => EditorOutcome::Ignored,
        }
    }

    fn keys(&self) -> Vec<KeyHelp> {
        table::help(&KEYS)
    }

    fn cwd(&self) -> Option<PathBuf> {
        Some(self.dir.clone())
    }
}

impl Editor for LauncherEditor {
    fn address(&self) -> Option<String> {
        Some(abbreviate_home(&self.dir))
    }

    /// A launcher only holds an empty pane's place: there is nothing to
    /// come back to.
    fn is_kept_when_closed(&self) -> bool {
        false
    }

    fn snapshot(&self) -> Option<OpenTarget> {
        Some(OpenTarget::file(self.dir.clone()))
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_every_key_spec_parses() {
        assert!(table::invalid_specs(&KEYS).is_empty());
    }

    #[test]
    fn test_mouse_click_launches_action() {
        let mut editor = LauncherEditor::new(PathBuf::from("/test"));
        // Render 24 rows, cols 80.
        // top = 24 / 3 + 2 = 10.
        // row 10 -> Launch::Terminal
        // row 11 -> Launch::Files
        // row 12 -> Launch::Dir
        // row 13 -> Launch::Git
        let _ = editor.content(24, 80, false);

        assert_eq!(
            editor.on_mouse(Point {
                col: 20,
                drag: false,
                row: 10,
            }),
            EditorOutcome::Launch(Launch::Terminal)
        );

        assert_eq!(
            editor.on_mouse(Point {
                col: 20,
                drag: false,
                row: 11,
            }),
            EditorOutcome::Launch(Launch::Files)
        );

        assert_eq!(
            editor.on_mouse(Point {
                col: 20,
                drag: false,
                row: 12,
            }),
            EditorOutcome::Launch(Launch::Dir)
        );

        assert_eq!(
            editor.on_mouse(Point {
                col: 20,
                drag: false,
                row: 13,
            }),
            EditorOutcome::Launch(Launch::Git)
        );

        // Outside choice rows
        assert_eq!(
            editor.on_mouse(Point {
                col: 20,
                drag: false,
                row: 5,
            }),
            EditorOutcome::Ignored
        );

        // Drag is ignored
        assert_eq!(
            editor.on_mouse(Point {
                col: 20,
                drag: true,
                row: 10,
            }),
            EditorOutcome::Ignored
        );
    }
}
