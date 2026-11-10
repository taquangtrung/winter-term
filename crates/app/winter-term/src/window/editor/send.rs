//! Code an editor sends to a terminal to run (see `winter_text`'s
//! `send`): which terminal takes it, and typing it there. The editor keeps
//! the keyboard throughout, so a script is walked through without leaving
//! it.

use winter_model::layout::{Direction, TabId};

use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// The key typed after what is sent, so it runs.
const RUN: &str = "\r";

// ========================================================================
// App: sending code to a terminal
// ========================================================================

impl App {
    /// Type `text` into a terminal beside the editor tab `from`, and run
    /// it: the one last sent to while it is still shown, else the first
    /// shown, else the dock's brought back, else a new one split beside
    /// `from`, in its directory.
    pub(in crate::window) fn send_to_terminal(&mut self, from: TabId, text: &str) {
        let Some(target) = self.terminal_to_send_to(from) else {
            return;
        };
        self.term.sent_to = Some(target);
        // Pasted, so a REPL taking pastes takes the lines whole; then the
        // key that runs them, typed, since a pasted one is only text.
        if !self.paste_into_terminal(target, text) {
            return;
        }
        if let Some(terminal) = self.open_tabs.terminal_mut(&target) {
            terminal.write(RUN.as_bytes());
        }
        self.dirty = true;
    }

    /// The terminal the next sending from `from` goes to, opening one where
    /// none is to be had.
    fn terminal_to_send_to(&mut self, from: TabId) -> Option<TabId> {
        let is_terminal = |app: &Self, tab: &TabId| app.open_tabs.terminal(tab).is_some();
        let shown: Vec<TabId> = self
            .layout()
            .shown_tabs()
            .into_iter()
            .filter(|tab| is_terminal(self, tab))
            .collect();
        if let Some(last) = self.term.sent_to.filter(|last| shown.contains(last)) {
            return Some(last);
        }
        if let Some(first) = shown.first() {
            return Some(*first);
        }
        if let Some(docked) = self
            .layout()
            .dock_tab()
            .filter(|tab| is_terminal(self, tab))
        {
            self.layout_mut().show_dock();
            self.layout_mut().focus(from);
            return Some(docked);
        }
        let cwd = self.focused_start_cwd();
        let opened = self.split_pane_at(Direction::Vertical, cwd)?;
        self.layout_mut().focus(from);
        self.after_tab_change();
        Some(opened)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use crate::window::editor::EditorKind;
    use crate::window::test_support::idle_terminal;

    #[test]
    fn test_code_goes_to_a_terminal_beside_and_the_editor_keeps_the_keyboard() {
        let mut app = App::new();
        let main = app.layout().focused();
        app.open_tabs.insert_terminal(main, idle_terminal());
        let path = std::env::temp_dir().join(format!("winter-send-{}.py", std::process::id()));
        std::fs::write(&path, "x = 1\n").expect("file");
        let editor = winter_text::TextEditor::new(path.clone(), None).expect("opens");
        let tab = app.show_editor(EditorKind::Text, Box::new(editor));
        assert!(
            !app.layout().shown_tabs().contains(&main),
            "the terminal is behind the editor"
        );
        app.send_to_terminal(tab, "x = 1");
        let opened = app.term.sent_to.expect("sent somewhere");
        assert_ne!(opened, main, "a terminal beside, not one out of sight");
        assert!(app.layout().shown_tabs().contains(&opened));
        assert_eq!(app.layout().focused(), tab, "the editor keeps the keyboard");
        app.send_to_terminal(tab, "x");
        assert_eq!(app.term.sent_to, Some(opened), "the same one again");
        let _ = std::fs::remove_file(path);
    }
}
