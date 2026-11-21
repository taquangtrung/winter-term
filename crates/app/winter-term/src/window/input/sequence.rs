//! Multi-key sequences: the chords typed so far, resolved against the window's
//! sequences and the focused editor's ahead of the editor and the mode keymaps, so
//! `M-g M-h` reads the same in a terminal as in an editor tab. Also where the
//! keymap is built from config, with what is wrong in that config.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::editors;
use crate::window::commands;
use winter_model::input::{
    Blocks, Catalog, Key, KeyCode, Keymap, Mods, PendingPrefix, SequenceStep, WhenMap,
    default_bindings_maps, default_when_bindings, format_sequence, table,
};
use winter_model::layout::TabId;

use crate::window::App;
use crate::window::InputState;

// ========================================================================
// Constants
// ========================================================================

/// The commands a binding may name beyond the keymap's own: the palette's,
/// and each editor's.
pub(in crate::window) const CATALOG: Catalog = Catalog {
    is_command: |name| commands::command_label(name).is_some(),
    editor_commands: editors::block_commands,
};

/// How long a pending key prefix must sit unresolved before the which-key
/// popup explains it. Long enough that a fluent multi-key sequence never
/// raises it, short enough that a genuine pause gets an answer.
pub(in crate::window) const WHICH_KEY_DELAY: Duration = Duration::from_millis(1200);

// ========================================================================
// Data Structures
// ========================================================================

/// Keyboard state that spans more than one key: a pending prefix and since
/// when, a multi-key sequence in progress, and which tab each forwarded press
/// went to so its release follows it.
pub(in crate::window) struct PendingKeys {
    pub(in crate::window) prefix: PendingPrefix,
    /// When the current `pending` prefix or key sequence was opened, for the
    /// which-key hint delay.
    pub(in crate::window) prefix_since: Option<Instant>,
    /// When the focused editor's own prefix hint (not a menu) began, for
    /// the same delay; see [`App::sync_editor_hint`].
    pub(in crate::window) editor_hint_since: Option<Instant>,
    /// The multi-key sequence typed so far (`M-g` of `M-g M-h`), if one is
    /// open. See `window::input::sequence`.
    pub(in crate::window) sequence: Option<PendingSequence>,
    /// The keys whose presses went to a program, and the tab each went to:
    /// the only ones whose releases follow them, and only there.
    pub(in crate::window) forwarded: HashMap<KeyCode, TabId>,
}

/// A sequence part-way typed, and the tab it was typed in: a sequence opened
/// in one tab never completes in another, where it would mean something else.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct PendingSequence {
    keys: Vec<Key>,
    tab: TabId,
}

// ========================================================================
// PendingKeys Implementations
// ========================================================================

impl Default for PendingKeys {
    fn default() -> Self {
        Self {
            prefix: PendingPrefix::None,
            prefix_since: None,
            editor_hint_since: None,
            sequence: None,
            forwarded: HashMap::new(),
        }
    }
}

// ========================================================================
// Keymap from config
// ========================================================================

/// The keymap `blocks` configure, and a notice saying what is wrong with
/// them, if anything is. Every problem is also logged.
pub(in crate::window) fn keymap_from_blocks(blocks: &Blocks) -> (Keymap, Option<String>) {
    // What rebinds an editor's own keys is put in force for every key table,
    // what rebinds Cherry's commands in this thread's text editor, and the
    // rest goes to the keymap; all are reported alike.
    let split = editors::split_key_overrides(blocks);
    table::set_overrides(split.tables);
    let (keymap, keymap_problems) = Keymap::load(&split.keymap, CATALOG);
    let mut problems = split.problems;
    // The keymap lays the defaults under the user's itself; Cherry's keys
    // get the defaults' `in="editor"` lines here, a command the user binds
    // in a block taking the place of its default there.
    let mut cherry = editors::split_key_overrides(&default_bindings_maps()).cherry;
    for (block, commands) in split.cherry {
        cherry.entry(block).or_default().extend(commands);
    }
    let mut cherry_problems = winter_text::set_user_keys(&cherry);
    cherry_problems.sort();
    problems.extend(cherry_problems);
    problems.extend(keymap_problems);
    for problem in &problems {
        eprintln!("winter: keybindings.kdl: {problem}");
    }
    let notice = problems.first().map(|first| match problems.len() {
        1 => format!("keybindings.kdl: {first}"),
        n => format!("keybindings.kdl: {first} (and {} more)", n - 1),
    });
    (keymap, notice)
}

/// The bindings with a condition in force: the built-in ones, then those
/// `config` gives.
pub(in crate::window) fn when_keys_of(config: &crate::config::Config) -> WhenMap {
    WhenMap::new(default_when_bindings(), config.keybindings_when.clone())
}

/// What the hint calls a command: its palette or editor label, else its name
/// in words.
fn command_label(editor: Option<&str>, name: &str) -> String {
    let label = match editor {
        Some(block) => editors::command_label(block, name),
        None => commands::command_label(name),
    };
    label.map_or_else(|| name.replace('_', " "), str::to_string)
}

// ========================================================================
// App: key sequences
// ========================================================================

impl App {
    /// Offer `key` to the sequence being typed, or to start one. Returns
    /// `None` when the key is no sequence's, else whether the app should quit.
    pub(in crate::window) fn route_sequence_key(
        &mut self,
        focused: TabId,
        key: &Key,
    ) -> Option<bool> {
        if !self.feed_sequence_key(focused, key) {
            return None;
        }
        if self.runtime.exit.take_requested() {
            return Some(true);
        }
        self.update_window_title();
        self.dirty = true;
        Some(false)
    }

    /// Whether anything part-way typed has a which-key hint to show: a
    /// sequence, or one of the terminal's own Vim prefixes.
    pub(in crate::window) fn has_pending_hint(&self) -> bool {
        self.input.is_sequence_pending() || self.input.keys.prefix.hint().is_some()
    }

    /// When the which-key hint for what is part-way typed is due to show,
    /// while something is: a sequence or a terminal prefix, or the focused
    /// editor's own prefix.
    pub(in crate::window) fn hint_deadline(&self) -> Option<Instant> {
        let window = self
            .input
            .keys
            .prefix_since
            .filter(|_| self.has_pending_hint());
        let since = [window, self.input.keys.editor_hint_since]
            .into_iter()
            .flatten()
            .min()?;
        Some(since + WHICH_KEY_DELAY)
    }

    /// Note when the focused editor began waiting on a prefix whose keys it
    /// hints at, so that hint waits the delay every key hint waits; a menu
    /// is shown at once and is not waited on. Called after every event, as
    /// any of them may start, finish, or move such a prefix.
    pub(in crate::window) fn sync_editor_hint(&mut self) {
        let focused = self.layout().focused();
        let waiting = self
            .open_tabs
            .page(&focused)
            .and_then(|slot| slot.page.hint())
            .is_some_and(|hint| !hint.menu);
        self.input.keys.editor_hint_since = match (waiting, self.input.keys.editor_hint_since) {
            (true, Some(since)) => Some(since),
            (true, None) => Some(Instant::now()),
            (false, _) => None,
        };
    }

    /// The which-key card for the sequence being typed in `focused`: its
    /// keys so far as the title, then what can follow them.
    pub(in crate::window) fn sequence_hint(
        &self,
        focused: TabId,
    ) -> Option<(String, Vec<(String, String)>)> {
        let sequence = self
            .input
            .keys
            .sequence
            .as_ref()
            .filter(|s| s.tab == focused)?;
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
        if self
            .input
            .keys
            .sequence
            .as_ref()
            .is_some_and(|s| s.tab != focused)
        {
            self.input.clear_sequence();
        }
        let starting = !self.input.is_sequence_pending();
        if !starting && key.chord() == (Mods::NONE, KeyCode::Escape) {
            self.input.clear_sequence();
            return true;
        }
        let mut typed = self
            .input
            .keys
            .sequence
            .take()
            .map(|sequence| sequence.keys)
            .unwrap_or_default();
        typed.push(*key);
        let editor = self.focused_editor_block(focused);
        match self.keymap.resolve_sequence(&typed, editor) {
            SequenceStep::Miss if starting => false,
            SequenceStep::Miss => {
                self.input.clear_sequence();
                self.refuse(format!("{} is undefined", format_sequence(&typed)));
                true
            }
            SequenceStep::Pending => {
                if starting {
                    // A sequence drops a half-typed Vim prefix, as a chord does.
                    self.input.keys.prefix = PendingPrefix::None;
                    self.input.keys.prefix_since = Some(Instant::now());
                }
                self.input.keys.sequence = Some(PendingSequence {
                    keys: typed,
                    tab: focused,
                });
                true
            }
            SequenceStep::Run(action) => {
                self.input.clear_sequence();
                self.handle_action(action, focused);
                true
            }
            SequenceStep::Editor(name) => {
                self.input.clear_sequence();
                self.run_editor_command(focused, &name);
                true
            }
        }
    }

    /// Run the focused editor's command `name`, saying so when it has none.
    pub(in crate::window) fn run_editor_command(&mut self, focused: TabId, name: &str) {
        let Some(slot) = self.open_tabs.page_mut(&focused) else {
            return;
        };
        let title = slot.kind().title();
        match slot.page.run_command(name) {
            Some(outcome) => {
                self.act_on_editor_outcome(focused, outcome);
            }
            None => self.refuse(format!("{title} has no command {name}")),
        }
    }

    /// The `keybindings.kdl` block of the editor in `focused`, if it is an editor.
    fn focused_editor_block(&self, focused: TabId) -> Option<&'static str> {
        self.open_tabs.page(&focused).map(|slot| slot.kind().name())
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Whether a hint due at `deadline` still needs a frame to show it: it is
/// due by `now`, and no frame has been drawn since it fell due. Once one has,
/// the hint is on screen and asking for another frame each tick would redraw
/// the window flat out for as long as the hint stays up.
pub(in crate::window) fn is_hint_redraw_due(
    deadline: Instant,
    now: Instant,
    last_frame: Option<Instant>,
) -> bool {
    now >= deadline && last_frame.is_none_or(|frame| frame < deadline)
}

// ========================================================================
// InputState
// ========================================================================

impl InputState {
    /// Whether a sequence is part-way typed.
    pub(in crate::window) fn is_sequence_pending(&self) -> bool {
        self.keys.sequence.is_some()
    }

    /// Drop the sequence being typed.
    pub(in crate::window) fn clear_sequence(&mut self) {
        self.keys.sequence = None;
        self.keys.prefix_since = None;
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use winter_model::editor::{Editor, EditorOutcome, Page};
    use winter_model::input;
    use winter_model::input::test_support::sequence_keymap;
    use winter_model::paint::{Frame, Span};

    use super::*;
    use crate::window::editor::EditorKind;

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

    impl Page for RecordingEditor {
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

    impl Editor for RecordingEditor {}

    /// An app on the built-in keymap with sequences added, its focus on a
    /// recording editor that reads the `kind` block.
    fn app_with_editor(kind: EditorKind) -> (App, TabId, Log) {
        let mut app = App::new();
        app.keymap = sequence_keymap();
        let log = Log::default();
        let tab = app.show_editor(kind, Box::new(RecordingEditor(log.clone())));
        (app, tab, log)
    }

    fn alt(c: char) -> Key {
        Key::with_alt(KeyCode::Char(c))
    }

    /// A recording editor in Insert mode whose completion menu is open
    /// while `open` says so.
    struct MenuEditor {
        log: Log,
        open: Rc<std::cell::Cell<bool>>,
    }

    impl Page for MenuEditor {
        fn title(&self) -> String {
            "Menu".to_string()
        }

        fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
            Frame::new(vec![vec![Span::plain("menu")]])
        }

        fn on_key(&mut self, key: &Key) -> EditorOutcome {
            self.log
                .borrow_mut()
                .push(format!("key {}", input::format_key(key)));
            EditorOutcome::Consumed
        }

        fn run_command(&mut self, name: &str) -> Option<EditorOutcome> {
            self.log.borrow_mut().push(format!("command {name}"));
            Some(EditorOutcome::Consumed)
        }

        fn status(&self) -> winter_model::modal::EditorStatus {
            winter_model::modal::EditorStatus {
                mode: Some(winter_model::modal::EditMode::Insert),
                segments: Vec::new(),
            }
        }

        fn key_context(&self, context: &mut input::KeyContext) {
            context.completion = self.open.get();
        }
    }

    impl Editor for MenuEditor {}

    #[test]
    fn test_a_when_binding_takes_its_key_only_while_its_condition_holds() {
        let mut app = App::new();
        let log = Log::default();
        let open = Rc::new(std::cell::Cell::new(true));
        let editor = MenuEditor {
            log: log.clone(),
            open: open.clone(),
        };
        app.show_editor(EditorKind::Text, Box::new(editor));
        app.route_key(true, Key::plain(KeyCode::Tab));
        app.route_key(true, Key::plain(KeyCode::Enter));
        open.set(false);
        app.route_key(true, Key::plain(KeyCode::Tab));
        assert_eq!(
            *log.borrow(),
            [
                "command accept_completion",
                "command accept_completion",
                "key TAB"
            ],
            "the built-in completion binding, then the editor's own key"
        );
    }

    #[test]
    fn test_an_editor_sequence_under_a_window_leader_runs_the_editor_command() {
        let (mut app, tab, log) = app_with_editor(EditorKind::Git);
        assert_eq!(app.route_sequence_key(tab, &alt('g')), Some(false));
        assert!(app.input.is_sequence_pending());
        assert_eq!(
            app.route_sequence_key(tab, &Key::plain(KeyCode::Char('r'))),
            Some(false)
        );
        assert!(!app.input.is_sequence_pending());
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
        assert!(!app.input.is_sequence_pending());
        assert!(log.borrow().is_empty());
        let notice = app.chrome.notice_text();
        assert_eq!(notice, Some("M-g x is undefined"));
    }

    #[test]
    fn test_another_editor_never_sees_the_git_editor_sequence() {
        let (mut app, tab, _) = app_with_editor(EditorKind::Dir);
        app.route_sequence_key(tab, &alt('g'));
        app.route_sequence_key(tab, &Key::plain(KeyCode::Char('r')));
        let notice = app.chrome.notice_text();
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
        assert!(!app.input.is_sequence_pending());
        assert!(app.chrome.notice_text().is_none());
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
        assert!(items.contains(&("M-h".to_string(), "Tab: Move to Left Pane".to_string())));
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
        assert!(!app.input.is_sequence_pending());
        assert!(log.borrow().is_empty());
    }

    #[test]
    fn test_the_text_editors_named_commands_bind_in_its_block() {
        // `problem_next "M-g M-n" in="editor"` was handed to Cherry, which
        // knows none of Winter's commands, then refused by the keymap, whose
        // list of the editor's commands was empty.
        let mut editor = HashMap::new();
        editor.insert("M-g M-n".to_string(), "problem_next".to_string());
        editor.insert("M-g M-c".to_string(), "editor_goto_offset".to_string());
        let mut blocks = HashMap::new();
        blocks.insert("editor".to_string(), editor);
        let (keymap, notice) = keymap_from_blocks(&blocks);
        assert_eq!(notice, None);
        assert_eq!(
            keymap.editor_chord_hint("editor", "problem_next"),
            "M-g M-n"
        );
    }

    #[test]
    fn test_an_editors_own_key_is_rebound_from_keybindings_kdl() {
        let mut dir = HashMap::new();
        dir.insert("M-z".to_string(), "toggle_archive".to_string());
        let mut blocks = HashMap::new();
        blocks.insert("dir".to_string(), dir);
        let (_, notice) = keymap_from_blocks(&blocks);
        assert_eq!(
            notice, None,
            "no editor's block reported, no unknown command"
        );
        let listed = editors::key_table("dir").expect("Dir's keys").listed();
        let archive = listed
            .iter()
            .find(|entry| entry.id == "toggle_archive")
            .expect("listed");
        assert_eq!(
            (archive.keys.clone(), archive.defaults.clone()),
            (vec!["M-z".to_string()], vec!["Z".to_string()])
        );
    }

    #[test]
    fn test_a_users_key_for_a_cherry_command_takes_effect_in_the_text_editor() {
        let path =
            std::env::temp_dir().join(format!("winter-user-keys-{}.txt", std::process::id()));
        std::fs::write(&path, "abcd").expect("written");
        let mut editor = winter_text::TextEditor::new(path.clone(), None).expect("opens");
        let lines = |editor: &winter_text::TextEditor| {
            editor
                .shared_text(&path, None)
                .and_then(|text| text.lines)
                .expect("the file's text")
        };
        let mut normal = HashMap::new();
        normal.insert("F5".to_string(), "delete-char".to_string());
        let blocks = HashMap::from([("editor".to_string(), normal)]);
        let (_, notice) = keymap_from_blocks(&blocks);
        assert_eq!(notice, None, "Cherry knows the command");
        editor.on_key(&Key::plain(KeyCode::F(5)));
        assert_eq!(lines(&editor), ["bcd"], "F5 deletes, in Cherry's <f5>");
        editor.on_key(&Key::plain(KeyCode::Char('x')));
        assert_eq!(lines(&editor), ["bcd"], "its own key replaced");
        // Bound no longer, the command gets its own key back.
        keymap_from_blocks(&HashMap::new());
        editor.on_key(&Key::plain(KeyCode::F(5)));
        editor.on_key(&Key::plain(KeyCode::Char('x')));
        assert_eq!(lines(&editor), ["cd"]);
        let mut unknown = HashMap::new();
        unknown.insert("F5".to_string(), "no-such-command".to_string());
        let (_, notice) = keymap_from_blocks(&HashMap::from([("editor".to_string(), unknown)]));
        assert_eq!(
            notice.as_deref(),
            Some(r#"keybindings.kdl: `editor` has no command named "no-such-command""#)
        );
        drop(editor);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_the_defaults_keys_for_cherry_commands_take_effect_with_no_user_file() {
        // The built-in `save "C-S-s" in="editor"` reached Cherry only when a
        // test handed the defaults in as a user's file: the app hands in the
        // user's alone.
        let path =
            std::env::temp_dir().join(format!("winter-default-keys-{}.txt", std::process::id()));
        std::fs::write(&path, "abcd").expect("written");
        let mut editor = winter_text::TextEditor::new(path.clone(), None).expect("opens");
        let (_, notice) = keymap_from_blocks(&HashMap::new());
        assert_eq!(notice, None);
        editor.on_key(&Key::plain(KeyCode::Char('x')));
        editor.on_key(&Key {
            ctrl: true,
            shift: true,
            ..Key::plain(KeyCode::Char('s'))
        });
        let saved = std::fs::read_to_string(&path).expect("read");
        drop(editor);
        let _ = std::fs::remove_file(&path);
        assert_eq!(saved.trim_end(), "bcd", "C-S-s saved the edit");
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
