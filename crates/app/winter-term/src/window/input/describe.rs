//! Describing a key, as Emacs's `describe-key` does: the next key pressed,
//! or the keys of the sequence it opens, are not run but said, with what
//! they would do where the keyboard is. Each is looked up as the routing
//! looks it up, in the same order, through the same tables, so what is said
//! is what the key does.

use crate::window::commands::{command_label, BUILTIN};
use crate::window::input::keys::forwarded_to_pty;
use crate::window::App;
use winter_model::input::table::{self, TableMatch};
use winter_model::input::{
    self, command_action, format_sequence, Action, Key, PendingPrefix, SequenceStep,
};
use winter_model::layout::TabId;

// ========================================================================
// Constants
// ========================================================================

/// What is said as the describing begins.
const ASK: &str = "Describe key: press a key";

/// What is said while the keys pressed so far wait for the next.
const ASK_NEXT: &str = "press the next key";

/// What is said of a key nothing here takes.
const UNBOUND: &str = "is not bound here";

/// What is said of a key a terminal hands to the program in it.
const SENT: &str = "is sent to the program";

// ========================================================================
// Data Structures
// ========================================================================

/// What the keys pressed so far come to.
#[derive(Debug, Eq, PartialEq)]
enum Described {
    /// They start something longer: wait for the next key.
    Pending,
    /// What they do, worded to follow their names.
    Said(String),
}

// ========================================================================
// App: describing a key
// ========================================================================

impl App {
    /// Begin describing the next key pressed.
    pub(in crate::window) fn start_describing_key(&mut self) {
        self.input.describing = Some(Vec::new());
        self.set_notice(ASK);
    }

    /// Take `key` as the next of the keys being described, and say what they
    /// do once they come to something.
    pub(in crate::window) fn describe_next_key(&mut self, key: Key) {
        let mut typed = self.input.describing.take().unwrap_or_default();
        typed.push(key);
        let keys = format_sequence(&typed);
        match self.describe(&typed) {
            Described::Pending => {
                self.set_notice(format!("Describe key: {keys}, {ASK_NEXT}"));
                self.input.describing = Some(typed);
            }
            Described::Said(what) => self.set_notice(format!("{keys} {what}")),
        }
    }

    /// What `typed` does in the focused tab: a window chord, then a key
    /// sequence, then the focused editor's own keys, then the terminal's.
    fn describe(&self, typed: &[Key]) -> Described {
        let focused = self.layout().focused();
        if let [key] = typed {
            if let Some(action) = self.keymap.global_action(key) {
                return Described::Said(runs(&action));
            }
        }
        let editor = self.open_tabs.page(&focused);
        let block = editor.as_ref().map(|editor| editor.kind.name());
        match self.keymap.resolve_sequence(typed, block) {
            SequenceStep::Pending => return Described::Pending,
            SequenceStep::Run(action) => return Described::Said(runs(&action)),
            SequenceStep::Editor(name) => {
                let label = block.and_then(|block| crate::editors::command_label(block, &name));
                return Described::Said(format!("runs {}", label.unwrap_or(&name)));
            }
            SequenceStep::Miss => {}
        }
        match editor {
            Some(editor) => {
                let help = editor.page.keys();
                self.describe_in_editor(editor.kind.title(), &help, typed)
            }
            None => self.describe_in_terminal(focused, typed),
        }
    }

    /// What `typed` does in an editor called `title` answering the keys in
    /// `help`, or, for a key it leaves alone, in the window around it.
    fn describe_in_editor(&self, title: &str, help: &[table::KeyHelp], typed: &[Key]) -> Described {
        match table::lookup_help(help, typed) {
            TableMatch::Exact(label) => return Described::Said(format!("runs {title}: {label}")),
            TableMatch::Prefix => return Described::Pending,
            TableMatch::Miss => {}
        }
        let [key] = typed else {
            return Described::Said(UNBOUND.to_string());
        };
        match input::resolve_editor(key, &self.keymap) {
            Action::Ignore => Described::Said(UNBOUND.to_string()),
            action => Described::Said(runs(&action)),
        }
    }

    /// What `typed` does in the terminal of tab `focused`, in its mode: its
    /// Vim keys, the window chords, or the bytes the program is sent.
    fn describe_in_terminal(&self, focused: TabId, typed: &[Key]) -> Described {
        let terminal = self.open_tabs.terminal(&focused);
        let mode = self.open_tabs.mode(&focused).unwrap_or_default();
        let kitty_flags = terminal.map(|terminal| terminal.kitty_flags()).unwrap_or(0);
        let modify_other_keys = terminal.and_then(|terminal| terminal.modify_other_keys());
        let is_alt_screen = terminal.is_some_and(|terminal| terminal.grid().is_alt_screen());
        // A prefix of its own, so the terminal's half-typed one is left as it is.
        let mut prefix = PendingPrefix::None;
        let mut action = Action::Ignore;
        for key in typed {
            action = input::resolve_with(
                mode,
                key,
                &mut prefix,
                &self.keymap,
                kitty_flags,
                modify_other_keys,
                is_alt_screen,
            );
        }
        if prefix != PendingPrefix::None {
            return Described::Pending;
        }
        let what = match action {
            Action::Ignore => UNBOUND.to_string(),
            action if forwarded_to_pty(&action) => SENT.to_string(),
            action => runs(&action),
        };
        Described::Said(format!("{what} in {mode:?} mode"))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What running `action` is said to do: the command it is, by the name the
/// palette gives it, else the action itself.
fn runs(action: &Action) -> String {
    let command = match action {
        Action::RunCommand(id) => command_label(id),
        action => BUILTIN
            .iter()
            .find(|builtin| command_action(builtin.id) == *action)
            .map(|builtin| builtin.title),
    };
    match command {
        Some(title) => format!("runs {title}"),
        None => format!("does {action:?}"),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use crate::window::editor::EditorKind;
    use crate::window::test_support::app_pane_and_id;
    use winter_model::input::KeyCode;
    use winter_model::mode::Mode;

    use super::*;

    fn ctrl(c: char) -> Key {
        Key {
            ctrl: true,
            ..Key::plain(KeyCode::Char(c))
        }
    }

    fn alt(c: char) -> Key {
        Key {
            alt: true,
            ..Key::plain(KeyCode::Char(c))
        }
    }

    /// Describe `keys` pressed one after another, and what is said at the end.
    fn described(app: &mut App, keys: &[Key]) -> Option<String> {
        app.start_describing_key();
        for key in keys {
            app.describe_next_key(*key);
        }
        app.chrome.notice_text().map(str::to_string)
    }

    #[test]
    fn test_a_window_sequence_waits_for_its_second_key_then_names_its_command() {
        let mut app = App::new();
        app.start_describing_key();
        app.describe_next_key(alt('g'));
        assert!(app.input.describing.is_some(), "waiting on the next key");
        app.describe_next_key(alt('h'));
        assert!(app.input.describing.is_none(), "done");
        assert_eq!(
            app.chrome.notice_text(),
            Some("M-g M-h runs Move Tab to Left Pane")
        );
    }

    #[test]
    fn test_f1_k_starts_describing_the_next_key() {
        let mut app = App::new();
        let focused = app.layout().focused();
        app.route_sequence_key(focused, &Key::plain(KeyCode::F(1)));
        app.route_sequence_key(focused, &Key::plain(KeyCode::Char('k')));
        assert!(
            app.input.describing.is_some(),
            "waiting for the key to describe"
        );
    }

    #[test]
    fn test_a_window_chord_is_said_and_never_run() {
        // Run, `C-,` would have opened the settings page over the window.
        let mut app = App::new();
        let said = described(&mut app, &[ctrl(',')]).expect("said");
        assert_eq!(said, "C-, runs Settings");
        assert!(app.chrome.overlay.is_none(), "nothing opened");
    }

    #[test]
    fn test_an_editor_s_own_key_is_named_with_the_editor() {
        let mut app = App::new();
        app.show_page(
            EditorKind::Keys,
            Box::new(crate::editors::keys::KeysEditor::default()),
        );
        let said = described(&mut app, &[Key::plain(KeyCode::Enter)]).expect("said");
        assert_eq!(said, "RET runs Keys: Run the Command");
    }

    #[test]
    fn test_a_plain_letter_in_insert_mode_goes_to_the_program() {
        let mut app = App::new();
        let said = described(&mut app, &[Key::plain(KeyCode::Char('a'))]);
        assert_eq!(
            said.as_deref(),
            Some("a is sent to the program in Insert mode")
        );
    }

    #[test]
    fn test_a_vim_prefix_in_normal_mode_waits_for_the_key_that_ends_it() {
        // Fed one key at a time with no prefix kept, `g g` read as two `g`s,
        // each said on its own.
        let (mut app, focused) = app_pane_and_id();
        app.open_tabs.set_mode(&focused, Mode::Normal);
        app.start_describing_key();
        app.describe_next_key(Key::plain(KeyCode::Char('g')));
        assert!(app.input.describing.is_some(), "waiting on the next key");
        app.describe_next_key(Key::plain(KeyCode::Char('g')));
        let said = app.chrome.notice_text().expect("said");
        assert!(said.starts_with("g g does "), "{said}");
        assert!(said.ends_with("in Normal mode"), "{said}");
    }
}
