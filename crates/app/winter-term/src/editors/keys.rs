//! Keys: every command, sorted by name, beside the keys bound to it, where
//! that binding comes from, and the id it is known by, under a filter line
//! that narrows the list as it is typed into, or, with `M-k`, to the commands
//! bound to the keys pressed. Over the list, `j` and `k` move between
//! commands, `RET` runs one, `c` captures new keys for it, `a` captures one
//! more, `S-c` captures keys for this platform alone, `d` takes its keys away,
//! and `x` gives it back its built-in ones, every change saved to the user's
//! `keybindings.kdl`.
//!
//! Misty draws the page as a view ([`PageView`]): a table whose keys are key
//! caps, the selected command in a thin accent border with buttons for what
//! can be done to it, which a click works as its keys do. The keys are the
//! page's own, the `keys` block of `keybindings.kdl`; the view takes only
//! the pointer.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

use super::notice::KeysNotice;
use crate::config::schema::WINDOW_BLOCK;
use crate::editors::{COMMAND_BLOCKS, UNBOUND, key_tables};
use misty::layout::Align;
use misty::picker::Query;
use misty::view::{
    Extent, Press, Space, Text, badge, column, divider, page, proportional_keycap, row, text,
};
use misty::{AccessAction, Element, EventCx, Frame as MistyFrame, HostedView, Input, Rect};
use misty::{Theme, ThemeToken};
use winter_field::{Field, FieldNotice};
use winter_model::editor::{
    EditorMenuItem, EditorOutcome, EditorRequest, EditorStatus, Notice, Page, PageView,
    menu_groups, present,
};
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed, TableMatch};
use winter_model::input::{Key, KeyCode, Keymap, MAX_SEQUENCE_LEN, Mods, format_sequence};
use winter_model::modal::{EditMode, EditOutcome};
use winter_model::paint::{Frame, Row};

// ========================================================================
// Constants
// ========================================================================

/// Said where a command cannot be rebound.
pub(super) const BUILT_IN: &str = "Vim's motions are built in, so they cannot be rebound";

/// Joins the keys of one entry, in its keybinding column.
const CHORD_SEPARATOR: &str = " / ";

/// What the filter shows before anything is typed, by where the keys are.
const FILTER_PLACEHOLDER: &str = "type a command's name, id, or keybinding, or record keys";
const FILTER_PLACEHOLDER_NORMAL: &str = "filter by a command's name, id, or keybinding";

/// What the filter says while keys are recorded, and before any are.
const RECORD_LABEL: &str = "Recording keys";
const RECORD_PLACEHOLDER: &str = "press the keys to look up";

/// The capture line's own keys, and what it says of a chord another
/// command has.
const CAPTURE_HELP: &str = "RET saves, Esc cancels, BS removes the last key";
const CAPTURE_TAKEN: &str = "also bound to";

/// What the capture line calls each kind of capture.
const CAPTURE_REPLACE: &str = "New keys for";
const CAPTURE_ADD: &str = "Another key for";
const CAPTURE_PLATFORM: &str = "Keys on this platform for";

/// Said when the filter lets nothing through.
const EMPTY_FILTERED: &str = "No command matches the filter";

/// The page's heading, the table's column headings, and the editor's title.
const HEADING: &str = "Keyboard Shortcuts";
const HEADING_CHORD: &str = "Keybinding";
const HEADING_COMMAND: &str = "Command";
const HEADING_ID: &str = "Id";
const HEADING_SOURCE: &str = "Source";
const TITLE: &str = "Keyboard Shortcuts";

/// What a command bound to nothing shows, and each source's badge.
const NO_CHORD: &str = "-";
const SOURCE_DEFAULT: &str = "default";
const SOURCE_USER: &str = "user";

/// What a row's tooltip says of where the command's binding comes from,
/// and of a key another command of its block has too.
const DETAIL_DEFAULT: &str = "built-in keys";
const DETAIL_UNBOUND: &str = "no keys: runs from the command picker";
const DETAIL_USER: &str = "your keys, in keybindings.kdl";
const DETAIL_SHARED: &str = "shares a key with";

/// Where a command of the window's own is said to run.
const WINDOW_SCOPE: &str = "anywhere in the window";

/// How wide the keybinding, source, and id columns are, in cell columns;
/// the command's name takes the rest.
const CHORD_COLS: f32 = 28.0;
const SOURCE_COLS: f32 = 9.0;
const ID_COLS: f32 = 30.0;

/// What marks the place typing goes in the filter and the capture line.
const CARET: &str = "\u{258f}";

/// About how many of the pane's text rows a command's row takes, for how
/// far a page key moves: misty lays the rows out and scrolls them itself.
const ROWS_PER_COMMAND: usize = 2;

/// The editor's own keys in the list.
const KEYS: KeyTable<Command> = KeyTable {
    block: "keys",
    bindings: &[
        Binding {
            action: Command::Down,
            id: "next_command",
            keys: &["j", "<down>", "C-n"],
            label: "Next Command",
        },
        Binding {
            action: Command::Up,
            id: "previous_command",
            keys: &["k", "<up>", "C-p"],
            label: "Previous Command",
        },
        Binding {
            action: Command::First,
            id: "first_command",
            keys: &["g g", "<home>", "M-<"],
            label: "First Command",
        },
        Binding {
            action: Command::Last,
            id: "last_command",
            keys: &["G", "<end>", "M->"],
            label: "Last Command",
        },
        Binding {
            action: Command::PageDown,
            id: "page_down",
            keys: &["C-d", "<next>"],
            label: "Down Half a Page",
        },
        Binding {
            action: Command::PageUp,
            id: "page_up",
            keys: &["C-u", "<prior>"],
            label: "Up Half a Page",
        },
        Binding {
            action: Command::Run,
            id: "run",
            keys: &["RET"],
            label: "Run the Command",
        },
        Binding {
            action: Command::Filter,
            id: "filter",
            keys: &["i"],
            label: "Type into the Filter",
        },
        Binding {
            action: Command::Change,
            id: "change",
            keys: &["c"],
            label: "Change the Keybinding",
        },
        Binding {
            action: Command::Add,
            id: "add_keys",
            keys: &["a"],
            label: "Add Another Key",
        },
        Binding {
            action: Command::ThisPlatform,
            id: "change_on_this_platform",
            keys: &["S-c"],
            label: "Change the Keybinding on This Platform Alone",
        },
        Binding {
            action: Command::Remove,
            id: "remove_keys",
            keys: &["d"],
            label: "Take the Keybinding Away",
        },
        Binding {
            action: Command::Reset,
            id: "reset",
            keys: &["x"],
            label: "Reset the Keybinding to Its Default",
        },
        Binding {
            action: Command::Record,
            id: "record_keys",
            keys: &["M-k"],
            label: "In the filter: record keys to look up, or stop",
        },
    ],
    configurable: true,
    title: "Keyboard Shortcuts",
};

/// Every key table of this surface, for the Keys editor to list.
pub(crate) const KEY_TABLES: &[&dyn Listed] = &[&KEYS];

// ========================================================================
// Data Structures
// ========================================================================

/// One of the window's own commands, as the Keys editor lists it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowCommand {
    /// The id it is run and bound by.
    pub action: String,
    /// The chord it is bound to, or empty.
    pub chord: String,
    /// What the palette calls it.
    pub label: String,
}

/// What a key of the editor's own does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    /// Capture one more key for the selected command.
    Add,
    /// Capture new keys for the selected command, in place of its own.
    Change,
    Down,
    /// Put the keys in the filter line.
    Filter,
    First,
    Last,
    PageDown,
    PageUp,
    /// In the filter, start or stop recording the keys to look up, as VS
    /// Code's keyboard shortcuts editor has it.
    Record,
    /// Take the selected command's keys away, its built-in ones too.
    Remove,
    /// Take the user's own binding of the selected command away, so its
    /// built-in one is in force again.
    Reset,
    /// Run the selected command.
    Run,
    /// Capture keys for the selected command on this platform alone.
    ThisPlatform,
    Up,
}

/// Where the keys go: into the filter line, or over the list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Focus {
    Filter,
    List,
}

/// Where a command's binding comes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Source {
    /// Built into Winter.
    Default,
    /// None: the command runs only from the palette.
    Unbound,
    /// The user's own `keybindings.kdl`.
    User,
}

/// What the keys captured are for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CaptureKind {
    /// One more key, beside the command's own.
    Add,
    /// The command's keys on this platform alone.
    Platform,
    /// The command's keys, in place of its own.
    Replace,
}

/// A page listing what Winter can do and how to ask for it.
pub struct KeysEditor {
    /// The page's view and what it shows.
    view: PageView<Keys>,
}

/// What the Keys page shows and where its keys are: every command, the
/// filter, and a capture under way, the state its view is built from.
pub struct Keys {
    /// The new keys being pressed for a command's binding, while they are.
    capture: Option<Capture>,
    /// Every command, sorted by name.
    commands: Vec<CommandRow>,
    /// The commands whose chord another command of the same block is bound
    /// to as well, as indices into `commands`, marked as a warning: only
    /// one of them runs.
    contested: HashSet<usize>,
    /// The filter line.
    filter: Field,
    focus: Focus,
    /// A report of the editor's own.
    notice: Option<KeysNotice>,
    /// What the editor asks of the window: a binding saved to the user's
    /// keybindings.
    requests: Vec<EditorRequest>,
    /// The keys pressed so far while recording them into the filter, which
    /// then lets through only the commands bound to them; `None` while
    /// the filter is typed into.
    recording: Option<Vec<Key>>,
    /// The pane's text rows, which a page key moves a half of.
    rows: usize,
    /// The selected command, by its place among those shown.
    selected: usize,
    /// The commands the filter lets through, in order, each with where it
    /// matched.
    shown: Vec<ShownRow>,
    /// The keys pressed so far of a sequence of the editor's own (`g g`).
    typed: Vec<Key>,
    /// What a button asked of the window, until the window takes it.
    outcome: Option<EditorOutcome>,
}

/// The keys pressed so far for command `index`'s new binding, and what
/// they are for.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Capture {
    index: usize,
    keys: Vec<Key>,
    kind: CaptureKind,
}

/// One listed command: what it does, the chord that runs it, where that
/// binding comes from, and the name the host knows it by.
#[derive(Clone, Debug, Eq, PartialEq)]
struct CommandRow {
    action: String,
    /// The block of `keybindings.kdl` it is bound in.
    block: &'static str,
    chord: String,
    label: String,
    /// Whether the user may rebind it: `false` for Vim's motions, listed for
    /// reading only.
    rebindable: bool,
    /// The editor whose tab runs the command, for an editor's own command, which
    /// cannot run from here.
    runs_in: Option<&'static str>,
    source: Source,
}

/// Where the filter matched a command, as character indices into each
/// column's text.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Matched {
    action: Vec<usize>,
    chord: Vec<usize>,
    label: Vec<usize>,
}

/// A command the filter lets through: its index into the sorted commands,
/// and where it matched.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ShownRow {
    index: usize,
    matched: Matched,
}

// ========================================================================
// KeysEditor
// ========================================================================

impl Default for KeysEditor {
    fn default() -> Self {
        Self::of(Keys::default())
    }
}

impl Default for Keys {
    /// The page listing no command.
    fn default() -> Self {
        Self::from_rows(Vec::new())
    }
}

impl KeysEditor {
    /// The page over the bindings currently in force, as [`Keys::new`]
    /// lists them.
    pub fn new(
        keymap: &Keymap,
        commands: Vec<WindowCommand>,
        user: &HashMap<String, HashMap<String, String>>,
    ) -> Self {
        Self::of(Keys::new(keymap, commands, user))
    }

    /// The page showing `keys`.
    fn of(keys: Keys) -> Self {
        Self {
            view: PageView::new(keys),
        }
    }

    /// List the bindings now in force, as [`Keys::refresh`] does.
    pub fn refresh(
        &mut self,
        keymap: &Keymap,
        commands: Vec<WindowCommand>,
        user: &HashMap<String, HashMap<String, String>>,
    ) {
        self.view.state_mut().refresh(keymap, commands, user);
    }

    /// The index, among every command sorted by name, of the selected one.
    pub fn selected(&self) -> usize {
        self.view.state().selected()
    }
}

impl Keys {
    /// Build the editor against the bindings currently in force, listing
    /// `commands`, the window's own, with the editors', every one sorted by
    /// name. `user` is the user's own bindings, by block and then chord,
    /// which say which bindings are theirs. It opens in Normal mode, over the
    /// list.
    pub fn new(
        keymap: &Keymap,
        commands: Vec<WindowCommand>,
        user: &HashMap<String, HashMap<String, String>>,
    ) -> Self {
        let bound_by_user = |block: &str, action: &str| {
            user.get(block)
                .is_some_and(|chords| chords.values().any(|bound| bound == action))
        };
        let source = |block: &str, action: &str, chord: &str| match (
            bound_by_user(block, action),
            chord.is_empty(),
        ) {
            (true, _) => Source::User,
            (false, true) => Source::Unbound,
            (false, false) => Source::Default,
        };
        let builtin = commands.into_iter().map(|entry| CommandRow {
            source: source(WINDOW_BLOCK, &entry.action, &entry.chord),
            action: entry.action,
            block: WINDOW_BLOCK,
            rebindable: true,
            chord: entry.chord,
            label: entry.label,
            runs_in: None,
        });
        let editors = COMMAND_BLOCKS.iter().flat_map(|block| {
            block.commands.iter().map(|command| {
                let chord = keymap.editor_chord_hint(block.block, command.name);
                CommandRow {
                    action: command.name.to_string(),
                    block: block.block,
                    label: format!("{}: {}", block.title, command.label),
                    rebindable: true,
                    runs_in: Some(block.title),
                    source: source(block.block, command.name, &chord),
                    chord,
                }
            })
        });
        // Each editor's own keys, and Vim's motions, the keys in force with
        // where they come from.
        let tables = key_tables().flat_map(|table| {
            table.listed().into_iter().map(move |entry| CommandRow {
                action: entry.id.to_string(),
                block: table.block(),
                chord: entry.keys.join(CHORD_SEPARATOR),
                label: format!("{}: {}", table.title(), entry.label),
                rebindable: table.configurable(),
                runs_in: Some(table.title()),
                source: match (entry.keys == entry.defaults, entry.keys.is_empty()) {
                    (false, _) => Source::User,
                    (true, true) => Source::Unbound,
                    (true, false) => Source::Default,
                },
            })
        });
        Self::from_rows(builtin.chain(editors).chain(tables).collect())
    }

    /// List the bindings now in force, as [`KeysEditor::new`] reads them,
    /// keeping what the filter says and the selection on the command it was
    /// on.
    pub fn refresh(
        &mut self,
        keymap: &Keymap,
        commands: Vec<WindowCommand>,
        user: &HashMap<String, HashMap<String, String>>,
    ) {
        let was_on = self
            .selected_command()
            .map(|command| command.action.clone());
        let fresh = Self::new(keymap, commands, user);
        self.commands = fresh.commands;
        self.contested = fresh.contested;
        self.apply_filter();
        let row = self
            .shown
            .iter()
            .position(|shown| Some(&self.commands[shown.index].action) == was_on.as_ref());
        if let Some(row) = row {
            self.selected = row;
        }
    }

    /// The editor listing `commands`, sorted by name, the keys over the
    /// list.
    fn from_rows(mut commands: Vec<CommandRow>) -> Self {
        commands.sort_by_cached_key(|command| command.label.to_lowercase());
        let shown = (0..commands.len())
            .map(|index| ShownRow {
                index,
                matched: Matched::default(),
            })
            .collect();
        Self {
            capture: None,
            contested: contested(&commands),
            commands,
            filter: Field::line(""),
            focus: Focus::List,
            notice: None,
            recording: None,
            requests: Vec::new(),
            outcome: None,
            rows: 0,
            selected: 0,
            shown,
            typed: Vec::new(),
        }
    }

    /// The index, among every command sorted by name, of the selected one.
    pub fn selected(&self) -> usize {
        self.shown.get(self.selected).map_or(0, |shown| shown.index)
    }

    /// The selected command, while the filter lets any through.
    fn selected_command(&self) -> Option<&CommandRow> {
        let shown = self.shown.get(self.selected)?;
        self.commands.get(shown.index)
    }

    /// Let through only the commands the filter's text matches by name, id,
    /// or chord, keeping the selection on the command it was on where that
    /// is still listed.
    fn apply_filter(&mut self) {
        let was_on = self.selected();
        let query = self.filter.text().trim().to_string();
        self.shown = self
            .commands
            .iter()
            .enumerate()
            .filter_map(|(index, command)| {
                let matched = match self.recording.is_some() {
                    true => match_chord(command, &query)?,
                    false => match_command(command, &query)?,
                };
                Some(ShownRow { index, matched })
            })
            .collect();
        self.selected = self
            .shown
            .iter()
            .position(|shown| shown.index == was_on)
            .unwrap_or(0);
    }

    /// Move the selection `by` commands, kept on the list.
    fn move_by(&mut self, by: isize) {
        let last = self.shown.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(by).min(last);
    }

    /// How many commands a page key moves by.
    fn half_page(&self) -> isize {
        (self.rows / ROWS_PER_COMMAND / 2).max(1) as isize
    }

    /// Start capturing keys of `kind` for the selected command.
    fn start_capture(&mut self, kind: CaptureKind) {
        let Some(command) = self.selected_command() else {
            return;
        };
        if !command.rebindable {
            self.notice = Some(KeysNotice::BuiltIn);
            return;
        }
        self.capture = Some(Capture {
            index: self.selected(),
            keys: Vec::new(),
            kind,
        });
    }

    /// A key while new keys are being captured: `RET` saves them, `Esc`
    /// gives up, `BS` takes the last back, and any other is the next of
    /// them, starting over once a sequence as long as a binding may be is
    /// pressed.
    fn capture_key(&mut self, key: &Key) -> EditorOutcome {
        let Some(capture) = self.capture.as_mut() else {
            return EditorOutcome::Ignored;
        };
        match key.chord() {
            // key-table: the capture line's own keys, fixed so a capture can always end.
            (Mods::NONE, KeyCode::Enter) => {
                let (index, kind) = (capture.index, capture.kind);
                let keys = std::mem::take(&mut capture.keys);
                self.capture = None;
                match keys.is_empty() {
                    true => self.notice = Some(KeysNotice::NoKeysPressed),
                    false => self.save_captured(index, kind, format_sequence(&keys)),
                }
            }
            (Mods::NONE, KeyCode::Escape) => self.capture = None, // key-table: the capture's own
            // key-table: the capture's own, as `Enter` and `Escape` are.
            (Mods::NONE, KeyCode::Backspace) => {
                capture.keys.pop();
            }
            _ => {
                if capture.keys.len() >= MAX_SEQUENCE_LEN {
                    capture.keys.clear();
                }
                capture.keys.push(*key);
            }
        }
        EditorOutcome::Consumed
    }

    /// Save `chord`, captured for command `index`, as `kind` asks: in place
    /// of its keys, beside them, or on this platform alone.
    fn save_captured(&mut self, index: usize, kind: CaptureKind, chord: String) {
        let Some(command) = self.commands.get(index) else {
            return;
        };
        match kind {
            CaptureKind::Add => {
                let mut chords: Vec<String> = command.chords().map(str::to_string).collect();
                if !chords.contains(&chord) {
                    chords.push(chord);
                }
                self.save_binding(index, chords, None);
            }
            CaptureKind::Platform => {
                self.save_binding(index, vec![chord], Some(std::env::consts::OS.to_string()));
            }
            CaptureKind::Replace => self.save_binding(index, vec![chord], None),
        }
    }

    /// Take the selected command's keys away, its built-in ones too.
    fn remove_binding(&mut self) {
        let Some(command) = self.selected_command() else {
            return;
        };
        if !command.rebindable {
            self.notice = Some(KeysNotice::BuiltIn);
            return;
        }
        self.save_binding(self.selected(), vec![UNBOUND.to_string()], None);
    }

    /// Take the user's own binding of the selected command away, so its
    /// built-in one is in force again; say so where there is none.
    fn reset_binding(&mut self) {
        let Some(command) = self.selected_command() else {
            return;
        };
        if !command.rebindable {
            self.notice = Some(KeysNotice::BuiltIn);
            return;
        }
        match command.source {
            Source::User => self.save_binding(self.selected(), Vec::new(), None),
            Source::Default | Source::Unbound => {
                self.notice = Some(KeysNotice::AlreadyBuiltIn);
            }
        }
    }

    /// Ask the window to bind command `index` to `chords` in the user's
    /// keybindings, on `platform` alone where one is named, or with none to
    /// take their binding of it away, and show the new binding at once
    /// rather than once the file is read again.
    fn save_binding(&mut self, index: usize, chords: Vec<String>, platform: Option<String>) {
        let Some(command) = self.commands.get_mut(index) else {
            return;
        };
        self.requests.push(EditorRequest::Rebind {
            block: command.block.to_string(),
            chords: chords.clone(),
            command: command.action.clone(),
            platform,
        });
        if !chords.is_empty() {
            let shown: Vec<&str> = chords
                .iter()
                .map(String::as_str)
                .filter(|chord| *chord != UNBOUND)
                .collect();
            command.chord = shown.join(CHORD_SEPARATOR);
            command.source = Source::User;
        }
        self.contested = contested(&self.commands);
    }

    /// Start recording keys into the filter, or stop and clear it.
    fn toggle_recording(&mut self) {
        self.recording = match self.recording {
            Some(_) => None,
            None => Some(Vec::new()),
        };
        self.filter = Field::line("");
        self.apply_filter();
    }

    /// Take `key` as the next of the keys being recorded, starting over
    /// once a sequence as long as a binding may be is recorded, and filter
    /// to the commands bound to them.
    fn record_key(&mut self, key: &Key) {
        let Some(keys) = self.recording.as_mut() else {
            return;
        };
        if keys.len() >= MAX_SEQUENCE_LEN {
            keys.clear();
        }
        keys.push(*key);
        self.filter = Field::line(&format_sequence(keys));
        self.apply_filter();
    }

    /// Run the selected command, or say where an editor's own one runs
    /// instead.
    fn run_row(&mut self) -> EditorOutcome {
        match self.selected_command() {
            Some(CommandRow {
                runs_in: Some(editor),
                ..
            }) => {
                self.notice = Some(KeysNotice::RunsIn(editor.to_string()));
                EditorOutcome::Consumed
            }
            Some(command) => EditorOutcome::RunAction(command.action.clone()),
            None => EditorOutcome::Consumed,
        }
    }

    /// Do what the key `command` names, over the list.
    fn run(&mut self, command: Command) -> EditorOutcome {
        match command {
            Command::Add => self.start_capture(CaptureKind::Add),
            Command::Change => self.start_capture(CaptureKind::Replace),
            Command::Down => self.move_by(1),
            Command::Filter => self.focus = Focus::Filter,
            Command::First => self.selected = 0,
            Command::Last => self.selected = self.shown.len().saturating_sub(1),
            Command::PageDown => self.move_by(self.half_page()),
            Command::PageUp => self.move_by(-self.half_page()),
            Command::Record => {
                self.focus = Focus::Filter;
                self.toggle_recording();
            }
            Command::Remove => self.remove_binding(),
            Command::Reset => self.reset_binding(),
            Command::Run => return self.run_row(),
            Command::ThisPlatform => self.start_capture(CaptureKind::Platform),
            Command::Up => self.move_by(-1),
        }
        EditorOutcome::Consumed
    }

    /// A key in Insert mode, typed into the filter, the list narrowed to
    /// match: `RET` runs the selected command, and `Esc` goes back to Normal
    /// mode over the list, keeping what is typed. Nothing else leaves Insert
    /// mode.
    fn filter_key(&mut self, key: &Key) -> EditorOutcome {
        if table::find(&KEYS, key) == Some(Command::Record) {
            self.toggle_recording();
            return EditorOutcome::Consumed;
        }
        if self.recording.is_some() {
            self.record_key(key);
            return EditorOutcome::Consumed;
        }
        let before = self.filter.text().to_string();
        match self.filter.on_key(&key.typed()) {
            EditOutcome::Submit => return self.run_row(),
            // `Esc` leaves Insert mode, as in Vim, keeping what is typed.
            EditOutcome::Cancel => {
                self.filter = Field::line(&before);
                self.focus = Focus::List;
            }
            _ => {}
        }
        if self.filter.text() != before {
            self.apply_filter();
        }
        EditorOutcome::Consumed
    }

    /// A key in Normal mode over the list, a sequence of the editor's own
    /// (`g g`) waited out.
    fn list_key(&mut self, key: &Key) -> EditorOutcome {
        self.typed.push(*key);
        match table::lookup(&KEYS, &self.typed) {
            TableMatch::Exact(command) => {
                self.typed.clear();
                self.run(command)
            }
            TableMatch::Prefix => EditorOutcome::Consumed,
            TableMatch::Miss => {
                let ended = self.typed.len() > 1;
                self.typed.clear();
                match ended {
                    true => EditorOutcome::Consumed,
                    false => EditorOutcome::Ignored,
                }
            }
        }
    }

    /// Do what a click on shown command `row` asks: select it, and do
    /// `command` on it where a button names one. A capture ends only by its
    /// keys.
    fn click(&mut self, row: usize, command: Option<Command>) {
        if self.capture.is_some() || row >= self.shown.len() {
            return;
        }
        self.focus = Focus::List;
        self.selected = row;
        if let Some(command) = command {
            self.outcome = Some(self.run(command));
        }
    }

    /// Note that the pane shows `rows` text rows, which says how far a page
    /// key moves.
    fn set_rows(&mut self, rows: usize) {
        self.rows = rows;
    }

    /// A key: to the capture while one is under way, else to the filter
    /// or over the list, as the focus is.
    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        match (self.capture.is_some(), self.focus) {
            (true, _) => self.capture_key(key),
            (false, Focus::Filter) => self.filter_key(key),
            (false, Focus::List) => self.list_key(key),
        }
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice
            .take()
            .map(KeysNotice::into_notice)
            .or_else(|| self.filter.take_notice().map(FieldNotice::into_notice))
    }

    fn take_requests(&mut self) -> Vec<EditorRequest> {
        std::mem::take(&mut self.requests)
    }

    /// How many commands are listed, of how many.
    fn total(&self) -> String {
        match self.shown.len() == self.commands.len() {
            true => format!("{} commands", self.commands.len()),
            false => format!("{} of {} commands", self.shown.len(), self.commands.len()),
        }
    }

    /// Typing while the keys are in the filter or captured, else Normal;
    /// and how many commands are listed.
    fn status(&self) -> EditorStatus {
        let typing = self.capture.is_some() || self.focus == Focus::Filter;
        EditorStatus {
            mode: Some(match typing {
                true => EditMode::Insert,
                false => EditMode::Normal,
            }),
            segments: vec![self.total()],
        }
    }

    /// The labels of the other commands of the captured command's block
    /// bound to the keys captured so far.
    fn capture_conflicts(&self) -> Vec<&str> {
        let Some(capture) = self
            .capture
            .as_ref()
            .filter(|capture| !capture.keys.is_empty())
        else {
            return Vec::new();
        };
        let Some(captured) = self.commands.get(capture.index) else {
            return Vec::new();
        };
        let chord = format_sequence(&capture.keys);
        self.commands
            .iter()
            .enumerate()
            .filter(|(index, command)| {
                *index != capture.index
                    && command.block == captured.block
                    && command.chords().any(|bound| bound == chord)
            })
            .map(|(_, command)| command.label.as_str())
            .collect()
    }

    /// The filter line: what is typed with the caret while the keys are in
    /// it, or what to type, led by a label only while keys are recorded;
    /// lined with the accent while typed into, and a click puts the keys
    /// in it.
    fn filter_view(&self) -> Element<'_, Self> {
        let typing = self.focus == Focus::Filter && self.capture.is_none();
        let label =
            self.recording
                .as_ref()
                .map(|_| match table::menu_typed(&KEYS, Command::Record) {
                    Some((keys, _)) => {
                        format!("{RECORD_LABEL} ({} to stop)", format_sequence(&keys))
                    }
                    None => RECORD_LABEL.to_string(),
                });
        let typed = self.filter.text();
        let shown: Text<'_> = match (typed.is_empty(), typing) {
            (true, _) => {
                let placeholder = match (self.recording.is_some(), typing) {
                    (true, _) => RECORD_PLACEHOLDER,
                    (false, true) => FILTER_PLACEHOLDER,
                    (false, false) => FILTER_PLACEHOLDER_NORMAL,
                };
                let caret = if typing { CARET } else { "" };
                text(caret)
                    .color(ThemeToken::Accent)
                    .then(placeholder)
                    .muted()
            }
            (false, false) => text(typed.to_string()),
            (false, true) => {
                let at = self.filter.col();
                let before: String = typed.chars().take(at).collect();
                let after: String = typed.chars().skip(at).collect();
                text(before)
                    .then(CARET)
                    .color(ThemeToken::Accent)
                    .then(after)
            }
        };

        let mut parts: Vec<Element<'_, Self>> = Vec::new();
        if let Some(label) = label {
            parts.push(text(label).muted().into());
        }
        parts.push(shown.into());
        row(parts)
            .padding(1.0, 0.25)
            .corners(misty::view::Corners::Control)
            .border(match typing {
                true => ThemeToken::Accent,
                false => ThemeToken::Border,
            })
            // A field: it answers the pointer with its caret, not a light.
            .unlit()
            .on_pointer_press(|keys: &mut Self, _| {
                if keys.capture.is_none() {
                    keys.focus = Focus::Filter;
                }
            })
    }

    /// The capture line, while keys are captured: what for, the keys so
    /// far, what else they run, and its own keys.
    fn capture_view(&self) -> Option<Element<'_, Self>> {
        let capture = self.capture.as_ref()?;
        let label = self
            .commands
            .get(capture.index)
            .map_or("", |command| command.label.as_str());
        let what = match capture.kind {
            CaptureKind::Add => CAPTURE_ADD,
            CaptureKind::Platform => CAPTURE_PLATFORM,
            CaptureKind::Replace => CAPTURE_REPLACE,
        };
        let mut parts = vec![text(format!("{what} {label}:")).into()];
        if !capture.keys.is_empty() {
            parts.push(proportional_keycap(format_sequence(&capture.keys)));
        }
        parts.push(text(CARET).color(ThemeToken::Accent).into());
        let conflicts = self.capture_conflicts();
        if !conflicts.is_empty() {
            let taken = format!("{CAPTURE_TAKEN} {}", conflicts.join(", "));
            parts.push(text(taken).color(ThemeToken::Warning).into());
        }
        parts.push(column(Vec::new()).grow(1.0));
        parts.push(text(CAPTURE_HELP).muted().into());
        Some(
            row(parts)
                .padding(1.0, 0.25)
                .corners(misty::view::Corners::Control)
                .border(ThemeToken::Accent),
        )
    }

    /// The table's heading: what each column holds.
    fn heading_view(&self) -> Element<'_, Self> {
        table_row([
            text(HEADING_COMMAND).muted().into(),
            text(HEADING_CHORD).muted().into(),
            text(HEADING_SOURCE).muted().into(),
            text(HEADING_ID).muted().into(),
        ])
    }

    /// The table row of shown command `at`: its name, its keys as key caps
    /// (in the warning color where another command of its block has them
    /// too), its source, and its id, what the filter matched lit, on one
    /// line; the selected one marked, its details in the page's footer. A
    /// click selects it, a double click runs it.
    fn row_view(&self, at: usize, shown: &ShownRow) -> Element<'_, Self> {
        let command = &self.commands[shown.index];
        let selected = at == self.selected;
        let contested = self.contested.contains(&shown.index);
        let name = lit(&command.label, &shown.matched.label);
        let chords: Vec<Element<'_, Self>> = match command.chord.is_empty() {
            true => vec![text(NO_CHORD).muted().into()],
            false => command
                .chords()
                .map(|chord| match contested {
                    true => badge(chord, ThemeToken::Warning),
                    false => proportional_keycap(chord),
                })
                .collect(),
        };
        let source: Element<'_, Self> = match command.source {
            Source::Default => text(SOURCE_DEFAULT).muted().into(),
            Source::Unbound => text("").monospace().into(),
            Source::User => text(SOURCE_USER).color(ThemeToken::Accent).into(),
        };
        let line = table_row([
            name.into(),
            row(chords).gap(Space::Cells(0.5)),
            source,
            lit(&command.action, &shown.matched.action).muted().into(),
        ])
        .on_pointer_press(move |keys: &mut Self, press: Press| {
            keys.click(at, (press.clicks >= 2).then_some(Command::Run));
        });
        // The footer says the selected row's details.
        match selected {
            true => line
                .keep_in_view()
                .background(ThemeToken::CursorLine)
                .border(ThemeToken::Accent)
                .corners(misty::view::Corners::Control),
            false => line
                .hover_background(ThemeToken::CursorLine)
                .tooltip(self.details(shown.index)),
        }
    }

    /// What the tooltip of command `index` says: where it runs, where its
    /// keys come from, and the commands it shares a key with.
    fn details(&self, index: usize) -> String {
        let command = &self.commands[index];
        let runs = match command.runs_in {
            Some(editor) => format!("in a {editor} tab"),
            None => WINDOW_SCOPE.to_string(),
        };
        let source = match command.source {
            Source::Default => DETAIL_DEFAULT,
            Source::Unbound => DETAIL_UNBOUND,
            Source::User => DETAIL_USER,
        };
        let mut said = format!("Runs {runs}, {source}");
        if !command.rebindable {
            said = format!("{said}; {BUILT_IN}");
        }
        let shared: Vec<&str> = self
            .commands
            .iter()
            .enumerate()
            .filter(|(other, other_command)| {
                *other != index
                    && other_command.block == command.block
                    && other_command
                        .chords()
                        .any(|chord| command.chords().any(|own| own == chord))
            })
            .map(|(_, other_command)| other_command.label.as_str())
            .collect();
        if !shared.is_empty() {
            said.push_str(&format!("; {DETAIL_SHARED} {}", shared.join(", ")));
        }
        said
    }
}

impl HostedView for Keys {
    /// The heading and the count, the filter (and the capture line while
    /// keys are captured), the table of commands, and the selected one's
    /// details at the foot, where a keyboard user, who sees no tooltip,
    /// reads them.
    fn view(&self) -> Element<'_, Self> {
        let mut table = vec![self.filter_view()];
        table.extend(self.capture_view());
        table.push(self.heading_view());
        table.push(divider());
        if self.shown.is_empty() && !self.commands.is_empty() {
            table.push(text(EMPTY_FILTERED).muted().into());
        }
        // Every command, the page scrolling them and keeping the selected
        // one in view.
        table.extend(
            self.shown
                .iter()
                .enumerate()
                .map(|(at, shown)| self.row_view(at, shown)),
        );
        let table = column(table).gap(Space::Cells(0.25));
        let shown = page(HEADING)
            .action(text(self.total()).muted())
            .scrolling(table);
        match self.shown.get(self.selected) {
            Some(row) => shown.footer(text(self.details(row.index)).muted()).into(),
            None => shown.into(),
        }
    }
}

impl Page for KeysEditor {
    /// Everything the selected command offers: to run it, to change, add,
    /// or set its keys on this platform alone, and to take them away or
    /// back to the built-in ones.
    fn context_items(&self) -> Vec<EditorMenuItem> {
        menu_groups([
            present([EditorMenuItem::bound(&KEYS, Command::Run)]),
            present([
                EditorMenuItem::bound(&KEYS, Command::Change),
                EditorMenuItem::bound(&KEYS, Command::Add),
                EditorMenuItem::bound(&KEYS, Command::ThisPlatform),
            ]),
            present([
                EditorMenuItem::bound(&KEYS, Command::Remove),
                EditorMenuItem::bound(&KEYS, Command::Reset),
            ]),
        ])
    }

    fn title(&self) -> String {
        TITLE.to_string()
    }

    /// Misty draws the page; the grid under it is empty, its rows kept for
    /// the page keys to move by.
    fn content(&mut self, rows: usize, _cols: usize, _wrap: bool) -> Frame {
        self.view.state_mut().set_rows(rows);
        Frame::new(vec![Row::new()])
    }

    fn is_view(&self) -> bool {
        true
    }

    fn draw_view(&mut self, frame: &mut MistyFrame, area: Rect, theme: &Theme) {
        self.view.draw_view(frame, area, theme);
    }

    fn view_input(&mut self, input: &Input, cx: &mut EventCx<'_>) -> Option<EditorOutcome> {
        let used = self.view.view_input(input, cx);
        let asked = self.view.state_mut().outcome.take();
        asked.or(used.then_some(EditorOutcome::Consumed))
    }

    fn view_access_action(&mut self, action: AccessAction) -> Option<EditorOutcome> {
        let used = self.view.access_action(action);
        let asked = self.view.state_mut().outcome.take();
        asked.or(used.then_some(EditorOutcome::Consumed))
    }

    fn view_about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.view.view_about_to_wait(cx)
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        self.view.state_mut().on_key(key)
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.view.state_mut().take_notice()
    }

    fn take_requests(&mut self) -> Vec<EditorRequest> {
        self.view.state_mut().take_requests()
    }

    fn status(&self) -> EditorStatus {
        self.view.state().status()
    }

    fn keys(&self) -> Vec<KeyHelp> {
        table::help(&KEYS)
    }
}

// ========================================================================
// CommandRow
// ========================================================================

impl CommandRow {
    /// Each of the keys it is bound to, as its keybinding column lists them.
    fn chords(&self) -> impl Iterator<Item = &str> {
        self.chord
            .split(CHORD_SEPARATOR)
            .filter(|chord| !chord.is_empty())
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The indices of the `commands` bound to a chord another command of the
/// same block is bound to as well.
fn contested(commands: &[CommandRow]) -> HashSet<usize> {
    let mut by_chord: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
    for (index, command) in commands.iter().enumerate() {
        for chord in command.chords() {
            by_chord
                .entry((command.block, chord))
                .or_default()
                .push(index);
        }
    }
    by_chord
        .into_values()
        .filter(|indices| indices.len() > 1)
        .flatten()
        .collect()
}

/// A row of the commands' table: the command's name taking the room the
/// keybinding, source, and id columns leave.
fn table_row<'a>(cells: [Element<'a, Keys>; 4]) -> Element<'a, Keys> {
    let [name, chords, source, id] = cells;
    row([
        column([name]).grow(1.0),
        column([chords])
            .width(Extent::Cells(CHORD_COLS))
            .shrink(0.0),
        column([source])
            .width(Extent::Cells(SOURCE_COLS))
            .shrink(0.0),
        column([id]).width(Extent::Cells(ID_COLS)).shrink(0.0),
    ])
    .align(Align::Start)
    .padding(1.0, 0.25)
}

/// `label` as text, the characters at `matched` lit in the accent.
fn lit<'a>(label: &'a str, matched: &[usize]) -> Text<'a> {
    let mut runs: Vec<(String, bool)> = Vec::new();
    for (at, c) in label.chars().enumerate() {
        let hit = matched.contains(&at);
        match runs.last_mut() {
            Some((run, was)) if *was == hit => run.push(c),
            _ => runs.push((c.to_string(), hit)),
        }
    }
    let mut runs = runs.into_iter();
    let Some((first, hit)) = runs.next() else {
        return text(Cow::Borrowed(label));
    };
    let start = match hit {
        true => text(first).color(ThemeToken::Accent),
        false => text(first),
    };
    runs.fold(start, |shown, (run, hit)| match hit {
        true => shown.then(run).color(ThemeToken::Accent),
        false => shown.then(run),
    })
}

/// Where `query` matches `command`, by its name, its chord, or its id;
/// `None` where it matches none of them. An empty query matches every
/// command, nothing lit.
fn match_command(command: &CommandRow, query: &str) -> Option<Matched> {
    if query.is_empty() {
        return Some(Matched::default());
    }
    let query = Query::parse(query);
    let positions = |text: &str| query.score_text(text).map(|(_, at)| at);
    let (label, chord, action) = (
        positions(&command.label),
        positions(&command.chord),
        positions(&command.action),
    );
    if label.is_none() && chord.is_none() && action.is_none() {
        return None;
    }
    Some(Matched {
        action: action.unwrap_or_default(),
        chord: chord.unwrap_or_default(),
        label: label.unwrap_or_default(),
    })
}

/// Where recorded keys `keys` match `command`: its chord is those keys, or
/// a sequence starting with them; `None` otherwise. No keys yet matches
/// every command.
fn match_chord(command: &CommandRow, keys: &str) -> Option<Matched> {
    if keys.is_empty() {
        return Some(Matched::default());
    }
    let prefix = format!("{keys} ");
    let bound = command
        .chords()
        .find(|chord| *chord == keys || chord.starts_with(&prefix))?;
    // Where that chord starts in the column, which lists every chord.
    let start = command.chord[..command.chord.find(bound).unwrap_or(0)]
        .chars()
        .count();
    Some(Matched {
        chord: (start..start + keys.chars().count()).collect(),
        ..Matched::default()
    })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use misty::testing::TestWindow;

    use super::*;
    use crate::editors::view_tests::{drawn, text_of};

    /// The page showing `keys`, lent for `run` and handed back after.
    fn as_page<R>(keys: &mut Keys, run: impl FnOnce(&mut KeysEditor) -> R) -> R {
        let mut page = KeysEditor::of(std::mem::take(keys));
        let ran = run(&mut page);
        *keys = std::mem::take(page.view.state_mut());
        ran
    }

    /// What `keys` draws, as the page draws it over a window.
    fn shown_text(keys: &mut Keys) -> String {
        as_page(keys, |page| {
            text_of(&drawn(page, &TestWindow::new(1400, 900)))
        })
    }

    fn press(code: KeyCode) -> Key {
        Key {
            alt: false,
            code,
            ctrl: false,
            shift: false,
            sup: false,
        }
    }

    fn ctrl_shift(c: char) -> Key {
        Key {
            ctrl: true,
            shift: true,
            ..press(KeyCode::Char(c))
        }
    }

    fn row(action: &str, chord: &str, label: &str) -> CommandRow {
        CommandRow {
            action: action.to_string(),
            block: WINDOW_BLOCK,
            chord: chord.to_string(),
            label: label.to_string(),
            rebindable: true,
            runs_in: None,
            source: Source::Default,
        }
    }

    fn type_filter(editor: &mut Keys, text: &str) {
        for c in text.chars() {
            editor.on_key(&press(KeyCode::Char(c)));
        }
    }

    fn listed(editor: &Keys) -> Vec<&str> {
        editor
            .shown
            .iter()
            .map(|shown| editor.commands[shown.index].action.as_str())
            .collect()
    }

    fn filtering() -> Keys {
        let mut editor = Keys::from_rows(vec![
            row("split_vertical", "S-M--", "Pane: Split Vertically"),
            row("new_terminal", "C-S-t", "Tab: New Tab"),
            row("open_settings", "C-,", "Settings: Open"),
        ]);
        editor.on_key(&press(KeyCode::Char('i')));
        editor
    }

    /// Two commands, `new_terminal` first by name.
    fn rebinding() -> Keys {
        Keys::from_rows(vec![
            row("open_settings", "C-,", "Settings: Open"),
            row("new_terminal", "C-S-t", "New Tab"),
        ])
    }

    fn rebind(chords: &[&str], platform: Option<&str>) -> EditorRequest {
        EditorRequest::Rebind {
            block: WINDOW_BLOCK.to_string(),
            chords: chords.iter().map(|chord| chord.to_string()).collect(),
            command: "new_terminal".to_string(),
            platform: platform.map(str::to_string),
        }
    }

    #[test]
    fn test_the_list_keys_move_through_the_commands_and_stop_at_the_ends() {
        let mut editor = filtering();
        editor.on_key(&press(KeyCode::Escape));
        editor.on_key(&press(KeyCode::Char('j')));
        assert_eq!(editor.selected_command().unwrap().action, "open_settings");
        for _ in 0..5 {
            editor.on_key(&press(KeyCode::Char('j')));
        }
        assert_eq!(editor.selected, 2, "stops at the last");
        editor.on_key(&press(KeyCode::Char('g')));
        editor.on_key(&press(KeyCode::Char('g')));
        assert_eq!(editor.selected, 0, "g g goes to the first");
        editor.on_key(&Key {
            shift: true,
            ..press(KeyCode::Char('G'))
        });
        assert_eq!(editor.selected, 2, "G to the last");
        editor.on_key(&press(KeyCode::Char('k')));
        assert_eq!(editor.selected, 1);
    }

    #[test]
    fn test_enter_hands_back_the_selected_command_and_an_editor_s_says_where_it_runs() {
        let mut editor = rebinding();
        assert_eq!(
            editor.on_key(&press(KeyCode::Enter)),
            EditorOutcome::RunAction("new_terminal".to_string())
        );
        let keymap = winter_model::input::test_support::sequence_keymap();
        let mut editor = Keys::new(
            &keymap,
            crate::window::commands::window_commands(&keymap),
            &HashMap::new(),
        );
        let index = editor
            .commands
            .iter()
            .position(|row| row.action == "open_remote")
            .expect("the Git editor's command is listed");
        assert_eq!(editor.commands[index].chord, "M-g r");
        editor.selected = editor
            .shown
            .iter()
            .position(|shown| shown.index == index)
            .unwrap();
        assert_eq!(
            editor.on_key(&press(KeyCode::Enter)),
            EditorOutcome::Consumed
        );
        assert!(
            matches!(editor.notice.take(), Some(KeysNotice::RunsIn(_))),
            "says where it runs"
        );
    }

    #[test]
    fn test_the_commands_are_sorted_by_name_whatever_their_case() {
        let editor = Keys::from_rows(vec![row("b", "", "beta"), row("a", "", "Alpha")]);
        assert_eq!(listed(&editor), ["a", "b"]);
    }

    #[test]
    fn test_the_filter_narrows_the_list_by_name_id_or_chord() {
        let mut editor = filtering();
        type_filter(&mut editor, "split");
        assert_eq!(listed(&editor), ["split_vertical"]);
        editor.filter = Field::line("");
        editor.apply_filter();
        type_filter(&mut editor, "C-,");
        assert_eq!(listed(&editor), ["open_settings"]);
        assert_eq!(
            editor.on_key(&press(KeyCode::Enter)),
            EditorOutcome::RunAction("open_settings".to_string()),
            "RET in the filter runs the first it lets through"
        );
    }

    #[test]
    fn test_escape_leaves_the_filter_keeping_it_and_only_escape_does() {
        let mut editor = filtering();
        type_filter(&mut editor, "tab");
        assert_eq!(editor.focus, Focus::Filter, "j, k and the rest type");
        editor.on_key(&press(KeyCode::Escape));
        assert_eq!(editor.focus, Focus::List);
        assert_eq!(editor.filter.text(), "tab");
        assert_eq!(editor.status().mode, Some(EditMode::Normal));
    }

    #[test]
    fn test_a_filter_matching_nothing_lists_nothing_says_so_and_runs_nothing() {
        let mut editor = filtering();
        type_filter(&mut editor, "zzzz");
        assert!(listed(&editor).is_empty());
        assert!(shown_text(&mut editor).contains(EMPTY_FILTERED));
        assert_eq!(
            editor.on_key(&press(KeyCode::Enter)),
            EditorOutcome::Consumed
        );
    }

    #[test]
    fn test_the_source_says_whose_binding_it_is() {
        let user = HashMap::from([(
            WINDOW_BLOCK.to_string(),
            HashMap::from([("C-S-n".to_string(), "new_terminal".to_string())]),
        )]);
        let command = |action: &str, chord: &str| WindowCommand {
            action: action.to_string(),
            chord: chord.to_string(),
            label: action.to_string(),
        };
        let commands = vec![
            command("new_terminal", "C-S-n"),
            command("close_tab", "C-w"),
            command("reload", ""),
        ];
        let editor = Keys::new(&Keymap::default(), commands, &user);
        let source = |action: &str| {
            editor
                .commands
                .iter()
                .find(|command| command.block == WINDOW_BLOCK && command.action == action)
                .unwrap()
                .source
        };
        assert_eq!(source("new_terminal"), Source::User);
        assert_eq!(source("close_tab"), Source::Default);
        assert_eq!(source("reload"), Source::Unbound);
        assert_eq!(editor.focus, Focus::List, "it opens over the list");
    }

    #[test]
    fn test_recorded_keys_filter_to_the_commands_bound_to_them_whole_chords_only() {
        let mut editor = Keys::from_rows(vec![
            row("down", "j / <down>", "Move down"),
            row("up", "k", "Move up"),
            row("settings", "C-k", "Settings"),
        ]);
        editor.on_key(&press(KeyCode::Char('i')));
        editor.on_key(&Key {
            alt: true,
            ..press(KeyCode::Char('k'))
        });
        editor.on_key(&press(KeyCode::Char('k')));
        assert_eq!(listed(&editor), ["up"], "k is not C-k");
        editor.on_key(&Key {
            alt: true,
            ..press(KeyCode::Char('k'))
        });
        editor.on_key(&Key {
            alt: true,
            ..press(KeyCode::Char('k'))
        });
        editor.on_key(&press(KeyCode::Down));
        assert_eq!(listed(&editor), ["down"]);
        assert_eq!(
            editor.shown[0].matched.chord.first(),
            Some(&"j / ".chars().count()),
            "lit on the key that matched"
        );
    }

    #[test]
    fn test_c_captures_new_keys_and_enter_asks_for_them_to_be_saved() {
        let mut editor = rebinding();
        editor.on_key(&press(KeyCode::Char('c')));
        assert!(editor.capture.is_some());
        editor.on_key(&ctrl_shift('n'));
        editor.on_key(&press(KeyCode::Char('x')));
        editor.on_key(&press(KeyCode::Backspace));
        editor.on_key(&press(KeyCode::Enter));
        assert_eq!(editor.take_requests(), [rebind(&["C-S-n"], None)]);
        let command = &editor.commands[0];
        assert_eq!(
            (command.chord.as_str(), command.source),
            ("C-S-n", Source::User),
            "shown at once"
        );
        assert!(editor.capture.is_none());
    }

    #[test]
    fn test_a_adds_a_key_beside_the_command_s_own() {
        let mut editor = rebinding();
        editor.on_key(&press(KeyCode::Char('a')));
        editor.on_key(&ctrl_shift('n'));
        editor.on_key(&press(KeyCode::Enter));
        assert_eq!(editor.take_requests(), [rebind(&["C-S-t", "C-S-n"], None)]);
        assert_eq!(editor.commands[0].chord, "C-S-t / C-S-n");
    }

    #[test]
    fn test_shift_c_binds_on_this_platform_alone() {
        let mut editor = rebinding();
        editor.on_key(&Key {
            shift: true,
            ..press(KeyCode::Char('C'))
        });
        editor.on_key(&ctrl_shift('n'));
        editor.on_key(&press(KeyCode::Enter));
        assert_eq!(
            editor.take_requests(),
            [rebind(&["C-S-n"], Some(std::env::consts::OS))]
        );
    }

    #[test]
    fn test_d_takes_the_keys_away_built_in_ones_too() {
        let mut editor = rebinding();
        editor.on_key(&press(KeyCode::Char('d')));
        assert_eq!(editor.take_requests(), [rebind(&[UNBOUND], None)]);
        let command = &editor.commands[0];
        assert_eq!((command.chord.as_str(), command.source), ("", Source::User));
    }

    #[test]
    fn test_escape_or_no_keys_while_capturing_changes_nothing() {
        let mut editor = rebinding();
        editor.on_key(&press(KeyCode::Char('c')));
        editor.on_key(&ctrl_shift('n'));
        editor.on_key(&press(KeyCode::Escape));
        assert!(editor.capture.is_none());
        assert!(editor.take_requests().is_empty());
        assert_eq!(editor.commands[0].chord, "C-S-t");
        editor.on_key(&press(KeyCode::Char('c')));
        editor.on_key(&press(KeyCode::Enter));
        assert!(editor.take_requests().is_empty());
        assert!(editor.take_notice().is_some(), "says why");
    }

    #[test]
    fn test_the_capture_line_warns_of_a_chord_another_command_has() {
        let mut editor = rebinding();
        editor.on_key(&press(KeyCode::Char('c')));
        editor.on_key(&Key {
            ctrl: true,
            ..press(KeyCode::Char(','))
        });
        assert_eq!(editor.capture_conflicts(), ["Settings: Open"]);
        assert!(shown_text(&mut editor).contains("also bound to Settings: Open"));
    }

    #[test]
    fn test_x_resets_only_a_binding_of_the_users_own() {
        let mut editor = rebinding();
        editor.on_key(&press(KeyCode::Char('x')));
        assert!(
            editor.take_requests().is_empty(),
            "a built-in binding has nothing to reset"
        );
        assert_eq!(editor.notice.take(), Some(KeysNotice::AlreadyBuiltIn));
        editor.commands[0].source = Source::User;
        editor.on_key(&press(KeyCode::Char('x')));
        assert_eq!(editor.take_requests(), [rebind(&[], None)]);
    }

    #[test]
    fn test_a_click_selects_its_command_and_works_its_button() {
        let mut editor = rebinding();
        editor.click(1, None);
        assert_eq!(editor.selected_command().unwrap().action, "open_settings");
        editor.click(0, Some(Command::Run));
        assert_eq!(
            editor.outcome.take(),
            Some(EditorOutcome::RunAction("new_terminal".to_string()))
        );
        editor.click(0, Some(Command::Change));
        assert!(editor.capture.is_some());
        editor.click(1, None);
        assert_eq!(editor.selected, 0, "a capture ends only by its keys");
        editor.capture = None;
        editor.click(9, Some(Command::Remove));
        assert!(editor.take_requests().is_empty(), "no such row");
    }

    #[test]
    fn test_the_page_draws_the_keys_as_caps_and_only_the_selected_rows_buttons() {
        let mut editor = rebinding();
        let shown = shown_text(&mut editor);
        for words in [
            HEADING,
            "2 commands",
            "New Tab",
            "C-S-t",
            "new_terminal",
            SOURCE_DEFAULT,
        ] {
            assert!(shown.contains(words), "{words:?} in {shown:?}");
        }
        assert!(
            !shown.contains("Add a Key"),
            "no buttons; what a row offers is in its context menu: {shown:?}"
        );
    }

    #[test]
    fn test_a_row_s_tooltip_describes_its_command_and_a_shared_key() {
        let editor = Keys::from_rows(vec![
            row("new_terminal", "C-S-t", "New Tab"),
            row("other_tab", "C-S-t", "Other Tab"),
            row("close_tab", "C-S-w", "Close Tab"),
        ]);
        let at = |label: &str| {
            editor
                .commands
                .iter()
                .position(|command| command.label == label)
                .expect("listed")
        };
        assert_eq!(
            editor.details(at("New Tab")),
            format!("Runs {WINDOW_SCOPE}, {DETAIL_DEFAULT}; {DETAIL_SHARED} Other Tab")
        );
        assert_eq!(
            editor.details(at("Close Tab")),
            format!("Runs {WINDOW_SCOPE}, {DETAIL_DEFAULT}")
        );
    }

    #[test]
    fn test_the_context_menu_offers_everything_a_row_can_do() {
        let mut keys = rebinding();
        let labels: Vec<String> = as_page(&mut keys, |page| {
            page.context_items()
                .iter()
                .map(|item| item.label.clone())
                .collect()
        });
        for wanted in [
            "Run the Command",
            "Change the Keybinding",
            "Add Another Key",
        ] {
            assert!(
                labels.iter().any(|label| label == wanted),
                "{wanted:?} in {labels:?}"
            );
        }
        assert!(
            labels
                .iter()
                .any(|label| label == "Take the Keybinding Away"),
            "{labels:?}"
        );
    }

    #[test]
    fn test_keys_go_to_the_keymap_and_never_to_the_view() {
        let mut editor = rebinding();
        let mut window = TestWindow::new(1400, 900);
        as_page(&mut editor, |page| {
            drawn(page, &window);
            let enter = Input::Key(misty::Key::new(misty::KeyCode::Enter, misty::Mods::NONE));
            assert_eq!(page.view_input(&enter, &mut window.cx()), None);
        });
    }

    #[test]
    fn test_the_page_scrolls_the_selected_command_into_view() {
        let rows: Vec<CommandRow> = (0..200)
            .map(|at| row(&format!("command_{at:03}"), "", &format!("Command {at:03}")))
            .collect();
        let mut editor = Keys::from_rows(rows);
        let mut window = TestWindow::new(1400, 900);
        // The scroll jumps rather than glides, so one frame shows where it
        // went.
        window.set_reduced_motion(true);
        as_page(&mut editor, |page| {
            let shown = text_of(&drawn(page, &window));
            assert!(shown.contains("command_000"));
            assert!(!shown.contains("command_199"), "the last out of sight");
            page.on_key(&Key {
                shift: true,
                ..press(KeyCode::Char('G'))
            });
            drawn(page, &window);
            page.view_about_to_wait(&mut window.cx());
            let shown = text_of(&drawn(page, &window));
            assert!(shown.contains("command_199"), "the last, selected, in view");
            assert!(!shown.contains("command_000"), "the first scrolled away");
        });
    }

    #[test]
    fn test_a_refresh_keeps_the_filter_and_the_selected_command() {
        let entries = |chord: &str| {
            ["a_tab", "b_tab", "other"]
                .iter()
                .map(|action| WindowCommand {
                    action: action.to_string(),
                    chord: if *action == "b_tab" {
                        chord.to_string()
                    } else {
                        String::new()
                    },
                    label: match *action {
                        "a_tab" => "Tab: A".to_string(),
                        "b_tab" => "Tab: B".to_string(),
                        _ => "Other".to_string(),
                    },
                })
                .collect::<Vec<_>>()
        };
        let mut editor = Keys::new(&Keymap::default(), entries(""), &HashMap::new());
        editor.on_key(&press(KeyCode::Char('i')));
        type_filter(&mut editor, "tab:");
        editor.on_key(&press(KeyCode::Escape));
        editor.on_key(&press(KeyCode::Char('j')));
        assert_eq!(editor.selected_command().unwrap().action, "b_tab");
        editor.refresh(&Keymap::default(), entries("C-S-b"), &HashMap::new());
        assert_eq!(editor.filter.text(), "tab:");
        assert_eq!(editor.selected_command().unwrap().action, "b_tab");
        assert_eq!(
            editor.selected_command().unwrap().chord,
            "C-S-b",
            "the new binding listed"
        );
    }

    #[test]
    fn test_a_chord_two_commands_of_one_block_share_is_drawn_as_a_warning() {
        let mut elsewhere = row("git_tab", "C-S-t", "Git: Tab");
        elsewhere.block = "git";
        let editor = Keys::from_rows(vec![
            row("new_terminal", "C-S-t", "Tab: New Tab"),
            row("other", "C-S-t", "Another Tab"),
            elsewhere,
        ]);
        let index = |action: &str| {
            editor
                .commands
                .iter()
                .position(|command| command.action == action)
                .unwrap()
        };
        assert!(
            editor.contested.contains(&index("new_terminal"))
                && editor.contested.contains(&index("other"))
        );
        assert!(
            !editor.contested.contains(&index("git_tab")),
            "another block's chord is its own"
        );
    }

    fn every_command() -> Keys {
        Keys::new(
            &Keymap::default(),
            crate::window::commands::window_commands(&Keymap::default()),
            &HashMap::new(),
        )
    }

    #[test]
    fn test_every_editors_own_keys_are_listed() {
        let editor = every_command();
        let archive = editor
            .commands
            .iter()
            .find(|command| command.block == "dir" && command.action == "toggle_archive")
            .expect("Dir's archive key is listed");
        assert_eq!(
            (archive.chord.as_str(), archive.source),
            ("Z", Source::Default)
        );
        assert!(archive.label.starts_with("Dir: "), "{}", archive.label);
        assert!(archive.rebindable);
    }

    #[test]
    fn test_the_vim_motions_are_listed_but_cannot_be_rebound() {
        let mut editor = every_command();
        let index = editor
            .commands
            .iter()
            .position(|command| command.block == "vim" && command.action == "down")
            .expect("Vim's motions are listed");
        assert!(!editor.commands[index].rebindable);
        editor.selected = editor
            .shown
            .iter()
            .position(|shown| shown.index == index)
            .unwrap();
        for key in ['c', 'a', 'd', 'x'] {
            editor.on_key(&press(KeyCode::Char(key)));
            assert!(
                editor.capture.is_none() && editor.take_requests().is_empty(),
                "{key}"
            );
            assert_eq!(editor.notice.take(), Some(KeysNotice::BuiltIn), "{key}");
        }
    }

    #[test]
    fn test_every_key_of_the_editor_is_spelled_right() {
        assert_eq!(table::invalid_specs(&KEYS), Vec::<&str>::new());
    }

    #[test]
    fn test_the_footer_says_the_selected_command_s_details_for_a_keyboard_user() {
        let mut keys = rebinding();
        let shown = shown_text(&mut keys);
        assert!(shown.contains(&format!("Runs {WINDOW_SCOPE}")), "{shown:?}");
    }

    #[test]
    fn test_every_control_says_what_it_is_and_lights_under_the_pointer() {
        let mut keys = rebinding();
        let unnamed = as_page(&mut keys, |page| {
            misty::testing::unnamed_controls(&drawn(page, &TestWindow::new(1400, 900)))
        });
        assert!(unnamed.is_empty(), "{unnamed:?}");
        let unlit = as_page(&mut keys, |page| {
            crate::editors::view_tests::unlit(page, &mut TestWindow::new(1400, 900))
        });
        assert!(unlit.is_empty(), "unlit under the pointer: {unlit:?}");
    }
}
