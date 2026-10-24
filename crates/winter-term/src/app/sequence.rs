//! Multi-key sequences: the chords typed so far, resolved against the window's
//! sequences and the focused editor's ahead of the editor and the mode keymaps, so
//! `M-g M-h` reads the same in a terminal as in an editor tab. Also where the
//! keymap is built from config, with what is wrong in that config.

use std::time::{Duration, Instant};

use crate::editors;
use crate::model::input::{
    format_sequence, Blocks, Catalog, Key, KeyCode, Keymap, Mods, PendingPrefix, SequenceStep,
};
use crate::model::layout::TabId;
use crate::model::palette;

use super::App;

// ========================================================================
// Constants
// ========================================================================

/// The commands a binding may name beyond the keymap's own: the palette's,
/// and each editor's.
pub(crate) const CATALOG: Catalog = Catalog {
    is_command: |name| palette::command_label(name).is_some(),
    editor_commands: editors::block_commands,
};

/// How long a pending key prefix must sit unresolved before the which-key
/// popup explains it. Long enough that a fluent multi-key sequence never
/// raises it, short enough that a genuine pause gets an answer.
pub(super) const WHICH_KEY_DELAY: Duration = Duration::from_millis(1000);

// ========================================================================
// Data Structures
// ========================================================================

/// A sequence part-way typed, and the tab it was typed in: a sequence opened
/// in one tab never completes in another, where it would mean something else.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingSequence {
    keys: Vec<Key>,
    tab: TabId,
}

// ========================================================================
// Keymap from config
// ========================================================================

/// The keymap `blocks` configure, and a notice saying what is wrong with
/// them, if anything is. Every problem is also logged.
pub(crate) fn keymap_from_blocks(blocks: &Blocks) -> (Keymap, Option<String>) {
    let (keymap, problems) = Keymap::load(blocks, CATALOG);
    for problem in &problems {
        eprintln!("winter: keybindings.kdl: {problem}");
    }
    let notice = problems.first().map(|first| match problems.len() {
        1 => format!("keybindings.kdl: {first}"),
        n => format!("keybindings.kdl: {first} (and {} more)", n - 1),
    });
    (keymap, notice)
}

/// What the hint calls a command: its palette or editor label, else its name
/// in words.
fn command_label(editor: Option<&str>, name: &str) -> String {
    let label = match editor {
        Some(block) => editors::command_label(block, name),
        None => palette::command_label(name),
    };
    label.map_or_else(|| name.replace('_', " "), str::to_string)
}

// ========================================================================
// App: key sequences
// ========================================================================

impl App {
    /// Offer `key` to the sequence being typed, or to start one. Returns
    /// `None` when the key is no sequence's, else whether the app should quit.
    pub(super) fn route_sequence_key(&mut self, focused: TabId, key: &Key) -> Option<bool> {
        if !self.feed_sequence_key(focused, key) {
            return None;
        }
        if self.exit_requested {
            self.exit_requested = false;
            return Some(true);
        }
        self.update_window_title();
        self.dirty = true;
        Some(false)
    }

    /// Whether a sequence is part-way typed.
    pub(super) fn is_sequence_pending(&self) -> bool {
        self.sequence.is_some()
    }

    /// Whether anything part-way typed has a which-key hint to show: a
    /// sequence, or one of the terminal's own Vim prefixes.
    pub(super) fn has_pending_hint(&self) -> bool {
        self.is_sequence_pending() || self.pending.hint().is_some()
    }

    /// When the which-key hint for what is part-way typed is due to show,
    /// while something is.
    pub(super) fn hint_deadline(&self) -> Option<Instant> {
        let since = self.pending_since?;
        self.has_pending_hint().then_some(since + WHICH_KEY_DELAY)
    }

    /// Drop the sequence being typed.
    pub(super) fn clear_sequence(&mut self) {
        self.sequence = None;
        self.pending_since = None;
    }

    /// The which-key card for the sequence being typed in `focused`: its
    /// keys so far as the title, then what can follow them.
    pub(super) fn sequence_hint(&self, focused: TabId) -> Option<(String, Vec<(String, String)>)> {
        let sequence = self.sequence.as_ref().filter(|s| s.tab == focused)?;
        let editor = self.focused_editor_block(focused);
        Some((
            format_sequence(&sequence.keys),
            self.keymap
                .sequence_hint(&sequence.keys, editor, command_label),
        ))
    }

    /// Add `key` to the sequence and act on what the keys now resolve to.
    /// Returns whether the key was spent on a sequence.
    fn feed_sequence_key(&mut self, focused: TabId, key: &Key) -> bool {
        // Focus moved since the sequence opened (a click on another tab):
        // it means nothing here, so the key starts afresh.
        if self.sequence.as_ref().is_some_and(|s| s.tab != focused) {
            self.clear_sequence();
        }
        let starting = !self.is_sequence_pending();
        if !starting && key.glyph() == KeyCode::Escape && key.mods() == Mods::NONE {
            self.clear_sequence();
            return true;
        }
        let mut typed = self
            .sequence
            .take()
            .map(|sequence| sequence.keys)
            .unwrap_or_default();
        typed.push(*key);
        let editor = self.focused_editor_block(focused);
        match self.keymap.resolve_sequence(&typed, editor) {
            SequenceStep::Miss if starting => false,
            SequenceStep::Miss => {
                self.clear_sequence();
                self.set_error(format!("{} is undefined", format_sequence(&typed)));
                true
            }
            SequenceStep::Pending => {
                if starting {
                    // A sequence drops a half-typed Vim prefix, as a chord does.
                    self.pending = PendingPrefix::None;
                    self.pending_since = Some(Instant::now());
                }
                self.sequence = Some(PendingSequence {
                    keys: typed,
                    tab: focused,
                });
                true
            }
            SequenceStep::Run(action) => {
                self.clear_sequence();
                self.handle_action(action, focused);
                true
            }
            SequenceStep::Editor(name) => {
                self.clear_sequence();
                self.run_editor_command(focused, &name);
                true
            }
        }
    }

    /// Run the focused editor's command `name`, saying so when it has none.
    pub(super) fn run_editor_command(&mut self, focused: TabId, name: &str) {
        let Some(slot) = self.open_tabs.editor_mut(&focused) else {
            return;
        };
        let title = slot.kind().title();
        match slot.editor.run_command(name) {
            Some(outcome) => {
                self.act_on_editor_outcome(focused, outcome);
            }
            None => self.set_error(format!("{title} has no command {name}")),
        }
    }

    /// The `keybindings.kdl` block of the editor in `focused`, if it is an editor.
    fn focused_editor_block(&self, focused: TabId) -> Option<&'static str> {
        self.open_tabs
            .editor(&focused)
            .map(|slot| slot.kind().name())
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Whether a hint due at `deadline` still needs a frame to show it: it is
/// due by `now`, and no frame has been drawn since it fell due. Once one has,
/// the hint is on screen and asking for another frame each tick would redraw
/// the window flat out for as long as the hint stays up.
pub(super) fn is_hint_redraw_due(
    deadline: Instant,
    now: Instant,
    last_frame: Option<Instant>,
) -> bool {
    now >= deadline && last_frame.is_none_or(|frame| frame < deadline)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use crate::model::editor::{Editor, EditorOutcome};
    use crate::model::input::{self, Keymap};
    use crate::model::paint::{Frame, Span};

    use super::super::editor::EditorKind;
    use super::*;

    #[test]
    fn test_a_due_hint_asks_for_one_frame_and_no_more() {
        // Regression: once the hint was due every tick asked for a frame,
        // redrawing flat out for as long as the hint stayed up.
        let deadline = Instant::now();
        let now = deadline + WHICH_KEY_DELAY;
        assert!(
            !is_hint_redraw_due(deadline, deadline - WHICH_KEY_DELAY, None),
            "not yet"
        );
        assert!(is_hint_redraw_due(deadline, now, None));
        assert!(
            is_hint_redraw_due(deadline, now, Some(deadline - WHICH_KEY_DELAY)),
            "the last frame predates the hint"
        );
        assert!(
            !is_hint_redraw_due(deadline, now, Some(deadline)),
            "a frame drawn since already shows it"
        );
    }

    /// What a [`RecordingEditor`] was handed: keys, then commands, as text.
    type Log = Rc<RefCell<Vec<String>>>;

    /// An editor that writes down every key and command it is given.
    struct RecordingEditor(Log);

    impl Editor for RecordingEditor {
        fn title(&self) -> String {
            "Recording".to_string()
        }

        fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
            Frame::new(vec![vec![Span::plain("recording")]])
        }

        fn on_key(&mut self, key: &Key) -> EditorOutcome {
            self.0
                .borrow_mut()
                .push(format!("key {}", input::format_key(key)));
            EditorOutcome::Consumed
        }

        fn run_command(&mut self, name: &str) -> Option<EditorOutcome> {
            self.0.borrow_mut().push(format!("command {name}"));
            Some(EditorOutcome::Consumed)
        }
    }

    /// An app on the built-in keymap, its focus on a recording editor that
    /// reads the `kind` block.
    fn app_with_editor(kind: EditorKind) -> (App, TabId, Log) {
        let mut app = App::new();
        app.keymap = Keymap::default();
        let log = Log::default();
        let tab = app.show_editor(kind, Box::new(RecordingEditor(log.clone())));
        (app, tab, log)
    }

    fn alt(c: char) -> Key {
        Key::with_alt(KeyCode::Char(c))
    }

    #[test]
    fn test_an_editor_sequence_under_a_window_leader_runs_the_editor_command() {
        let (mut app, tab, log) = app_with_editor(EditorKind::Git);
        assert_eq!(app.route_sequence_key(tab, &alt('g')), Some(false));
        assert!(app.is_sequence_pending());
        assert_eq!(
            app.route_sequence_key(tab, &Key::plain(KeyCode::Char('r'))),
            Some(false)
        );
        assert!(!app.is_sequence_pending());
        // The leader never reached the editor as a key of its own.
        assert_eq!(*log.borrow(), vec!["command open_remote".to_string()]);
    }

    #[test]
    fn test_a_key_continuing_nothing_drops_the_sequence_and_says_so() {
        let (mut app, tab, log) = app_with_editor(EditorKind::Git);
        app.route_sequence_key(tab, &alt('g'));
        assert_eq!(
            app.route_sequence_key(tab, &Key::plain(KeyCode::Char('x'))),
            Some(false),
            "the key is spent, not typed into the editor"
        );
        assert!(!app.is_sequence_pending());
        assert!(log.borrow().is_empty());
        let notice = app.notice_text();
        assert_eq!(notice, Some("M-g x is undefined"));
    }

    #[test]
    fn test_another_editor_never_sees_the_git_editor_sequence() {
        let (mut app, tab, _) = app_with_editor(EditorKind::Dir);
        app.route_sequence_key(tab, &alt('g'));
        app.route_sequence_key(tab, &Key::plain(KeyCode::Char('r')));
        let notice = app.notice_text();
        assert_eq!(notice, Some("M-g r is undefined"));
    }

    #[test]
    fn test_escape_drops_a_sequence_without_a_notice() {
        let (mut app, tab, _) = app_with_editor(EditorKind::Git);
        app.route_sequence_key(tab, &alt('g'));
        assert_eq!(
            app.route_sequence_key(tab, &Key::plain(KeyCode::Escape)),
            Some(false)
        );
        assert!(!app.is_sequence_pending());
        assert!(app.notice_text().is_none());
    }

    #[test]
    fn test_a_key_opening_no_sequence_is_left_to_the_editor() {
        let (mut app, tab, _) = app_with_editor(EditorKind::Git);
        assert_eq!(
            app.route_sequence_key(tab, &Key::plain(KeyCode::Char('s'))),
            None
        );
    }

    #[test]
    fn test_the_hint_lists_the_editor_and_window_continuations() {
        let (mut app, tab, _) = app_with_editor(EditorKind::Git);
        app.route_sequence_key(tab, &alt('g'));
        let (title, items) = app.sequence_hint(tab).expect("a hint while pending");
        assert_eq!(title, "M-g");
        // Labelled the way the Keys editor and the palette label them.
        assert!(items.contains(&("r".to_string(), "Open Remote in Browser".to_string())));
        assert!(items.contains(&("M-h".to_string(), "Move Tab to Left Pane".to_string())));
    }

    #[test]
    fn test_every_editor_command_block_is_an_editor_kind() {
        // The keymap finds an editor's block by its kind's name.
        for block in editors::COMMAND_BLOCKS {
            assert!(
                EditorKind::from_name(block.block).is_some(),
                "{}",
                block.block
            );
        }
    }

    #[test]
    fn test_a_sequence_opened_in_one_tab_does_not_complete_in_another() {
        let (mut app, git, _) = app_with_editor(EditorKind::Git);
        app.route_sequence_key(git, &alt('g'));
        let (_, dir, log) = {
            let log = Log::default();
            let tab = app.show_editor(EditorKind::Dir, Box::new(RecordingEditor(log.clone())));
            ((), tab, log)
        };
        // `r` on its own opens nothing, so in the new tab it is the editor's.
        assert_eq!(
            app.route_sequence_key(dir, &Key::plain(KeyCode::Char('r'))),
            None
        );
        assert!(!app.is_sequence_pending());
        assert!(log.borrow().is_empty());
    }

    #[test]
    fn test_a_keymap_problem_becomes_one_notice() {
        let mut window = HashMap::new();
        window.insert("M-z".to_string(), "nope".to_string());
        window.insert("M-y".to_string(), "nope_either".to_string());
        let mut blocks = HashMap::new();
        blocks.insert("window".to_string(), window);
        let (_, notice) = keymap_from_blocks(&blocks);
        assert_eq!(
            notice.as_deref(),
            Some(
                r#"keybindings.kdl: window: "M-y" runs unknown command "nope_either" (and 1 more)"#
            )
        );
    }
}
