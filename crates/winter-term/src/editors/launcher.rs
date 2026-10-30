//! A launcher: what a split opens in its new pane, holding the place for a
//! terminal, a file, a directory listing, or the git view, opened in the
//! directory the pane split from. Choosing one opens it in the launcher's tab.

use std::path::PathBuf;

use unicode_width::UnicodeWidthStr;

use crate::model::editor::{Editor, EditorOutcome, Launch, OpenTarget};
use crate::model::input::table::{self, Binding, KeyHelp};
use crate::model::input::Key;
use crate::model::paint::{Frame, Row, Span, Style};
use crate::model::path::abbreviate_home;

// ========================================================================
// Constants
// ========================================================================

/// The editor's title.
const TITLE: &str = "New Pane";

/// What each key opens, in the order the page lists them.
const KEYS: &[Binding<Launch>] = &[
    Binding {
        action: Launch::Terminal,
        keys: &["RET", "t"],
        label: "Terminal here",
    },
    Binding {
        action: Launch::Files,
        keys: &["f"],
        label: "Open file...",
    },
    Binding {
        action: Launch::Dir,
        keys: &["d"],
        label: "Directory",
    },
    Binding {
        action: Launch::Git,
        keys: &["g"],
        label: "Git status",
    },
];

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
}

// ========================================================================
// LauncherEditor
// ========================================================================

impl LauncherEditor {
    /// The launcher for `dir`.
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
}

impl Editor for LauncherEditor {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn address(&self) -> Option<String> {
        Some(abbreviate_home(&self.dir))
    }

    fn content(&mut self, rows: usize, cols: usize, _wrap: bool) -> Frame {
        let width = KEYS
            .iter()
            .map(|binding| binding.keys[0].width())
            .max()
            .unwrap_or_default();
        let choices: Vec<String> = KEYS
            .iter()
            .map(|binding| {
                format!("{:<width$}{:KEY_GAP$}{}", binding.keys[0], "", binding.label)
            })
            .collect();
        let block = choices.iter().map(|line| line.width()).max().unwrap_or_default();
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

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        match table::find(KEYS, key) {
            Some(launch) => EditorOutcome::Launch(launch),
            None => EditorOutcome::Ignored,
        }
    }

    fn keys(&self) -> Vec<KeyHelp> {
        table::help(KEYS)
    }

    /// A launcher only holds an empty pane's place: there is nothing to
    /// come back to.
    fn is_kept_when_closed(&self) -> bool {
        false
    }

    fn cwd(&self) -> Option<PathBuf> {
        Some(self.dir.clone())
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
        assert!(table::invalid_specs(KEYS).is_empty());
    }
}
