//! Keys: every command, sorted by name, beside the keys bound to it, where
//! that binding comes from, and the id it is known by, under a filter line
//! that narrows the list as it is typed into, or, with `M-k`, to the commands
//! bound to the keys pressed. Over the list, `j` and `k` move between
//! commands, `RET` runs one, `c` captures new keys for it, `a` captures one
//! more, `S-c` captures keys for this platform alone, `d` takes its keys away,
//! and `x` gives it back its built-in ones, every change saved to the user's
//! `keybindings.kdl`.
//!
//! The page is drawn as HTML (see [`PageView`]): a table whose keys are key
//! caps, the selected command in a thin accent border with buttons for what
//! can be done to it, which a click works as its keys do.

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use crate::config::schema::WINDOW_BLOCK;
use crate::editors::{key_tables, COMMAND_BLOCKS, UNBOUND};
use misty::picker::Query;
use winter_field::Field;
use winter_model::editor::{
    menu_groups, present, Appearance, EditorMenuItem, EditorOutcome, EditorRequest, EditorStatus,
    Notice, Page, ScrollPosition,
};
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed, TableMatch};
use winter_model::input::{format_sequence, Key, KeyCode, Keymap, Mods, MAX_SEQUENCE_LEN};
use winter_model::paint::{Frame, Point, Row};
use winter_model::picture::Tile;
use winter_model::textedit::modal::{EditMode, EditOutcome};

use super::web_page::{escape, PageView};

// ========================================================================
// Constants
// ========================================================================

/// Joins the keys of one entry, in its keybinding column.
const CHORD_SEPARATOR: &str = " / ";

/// What the filter shows before anything is typed, by where the keys are.
const FILTER_PLACEHOLDER: &str = "type a command's name, id, or keybinding, or record keys";
const FILTER_PLACEHOLDER_NORMAL: &str = "press i to filter by a command's name, id, or keybinding";

/// What stands before the filter's text.
const FILTER_LABEL: &str = "Filter";

/// What the filter says while keys are recorded, and before any are.
const RECORD_LABEL: &str = "Recording keys";
const RECORD_PLACEHOLDER: &str = "press the keys to look up";

/// Said where a command cannot be rebound.
const BUILT_IN: &str = "Vim's motions are built in, so they cannot be rebound";

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
const TITLE: &str = "Keys";

/// What a command bound to nothing shows, and each source's badge.
const NO_CHORD: &str = "-";
const SOURCE_DEFAULT: &str = "default";
const SOURCE_USER: &str = "user";

/// The keys said under the filter, over the list.
const HINT: &str =
    "j/k Move   RET Run   i Filter   c Change   a Add   S-c This Platform   d Remove   x Reset";

/// What a click names, before the row it is on: `select:3`, `change:3`.
const CLICK_FILTER: &str = "filter";
const CLICK_SELECT: &str = "select";
const CLICK_CHANGE: &str = "change";
const CLICK_ADD: &str = "add";
const CLICK_PLATFORM: &str = "platform";
const CLICK_REMOVE: &str = "remove";
const CLICK_RESET: &str = "reset";
const CLICK_RUN: &str = "run";

/// The rows of the pane a page key moves by: a half of those shown, each
/// command's row about this many of the pane's text rows tall.
const ROWS_PER_COMMAND: usize = 2;

/// The page's own look, over the theme's stylesheet.
const STYLE: &str = "
.head { display: flex; align-items: baseline; gap: 16px; margin: 4px 0 8px; }
.title { font-size: 20px; font-weight: 600; }
.count { color: var(--muted); }
.filter { display: flex; gap: 8px; padding: 5px 10px; border: 1px solid var(--border); border-radius: 6px;
  font-family: monospace; }
.filter.typing { border-color: var(--accent); }
.filter .label { color: var(--muted); }
.empty { color: var(--muted); }
.capture { margin-top: 8px; padding: 6px 10px; border: 1px solid var(--accent); border-radius: 6px; }
.capture .taken { color: var(--warning); margin-left: 12px; }
.capture .help { color: var(--muted); margin-left: 12px; }
.hint { color: var(--muted); font-size: 12px; margin: 8px 0 6px; }
.row { display: flex; align-items: center; gap: 12px; padding: 4px 10px; border: 1px solid transparent;
  border-radius: 6px; }
.row.selected { border-color: var(--accent); background: var(--tint); }
.cols { color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: 0.05em; }
.c-cmd { flex: 1; min-width: 0; }
.c-keys { width: 220px; display: flex; flex-wrap: wrap; gap: 4px; }
.c-src { width: 64px; color: var(--muted); font-size: 12px; }
.c-id { width: 240px; color: var(--muted); font-family: monospace; font-size: 12px; }
.kbd { padding: 0 6px; border: 1px solid var(--border); border-bottom-width: 2px; border-radius: 4px;
  font-family: monospace; font-size: 12px; }
.contested .kbd { border-color: var(--warning); color: var(--warning); }
.user { color: var(--accent); }
.lit { color: var(--accent); font-weight: 600; }
.actions { display: flex; gap: 4px; margin-top: 4px; }
.button { padding: 0 8px; border: 1px solid var(--border); border-radius: 4px; font-size: 12px; }
.caret { display: inline-block; width: 0; height: 1.1em; border-left: 1px solid var(--accent);
  vertical-align: text-bottom; }
";

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
    title: "Keys",
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
    notice: Option<Notice>,
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
    /// The page drawn as HTML.
    view: PageView,
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
        Self::from_rows(Vec::new())
    }
}

impl KeysEditor {
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
            rows: 0,
            selected: 0,
            shown,
            typed: Vec::new(),
            view: PageView::default(),
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
            self.notice = Some(Notice::info(BUILT_IN));
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
                    true => {
                        self.notice =
                            Some(Notice::info("no keys pressed, so the binding is unchanged"))
                    }
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
            self.notice = Some(Notice::info(BUILT_IN));
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
            self.notice = Some(Notice::info(BUILT_IN));
            return;
        }
        match command.source {
            Source::User => self.save_binding(self.selected(), Vec::new(), None),
            Source::Default | Source::Unbound => {
                self.notice = Some(Notice::info("already the built-in binding"));
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
                self.notice = Some(Notice::info(format!("runs in a {editor} tab")));
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

    /// Do what a click on the element naming `action` asks: put the keys in
    /// the filter, or select its command and work its button.
    fn clicked(&mut self, action: &str) -> EditorOutcome {
        let mut parts = action.split(':');
        let verb = parts.next().unwrap_or_default();
        if verb == CLICK_FILTER {
            self.focus = Focus::Filter;
            return EditorOutcome::Consumed;
        }
        let Some(row) = parts.next().and_then(|row| row.parse::<usize>().ok()) else {
            return EditorOutcome::Consumed;
        };
        if row >= self.shown.len() {
            return EditorOutcome::Consumed;
        }
        self.focus = Focus::List;
        self.selected = row;
        match verb {
            CLICK_ADD => self.run(Command::Add),
            CLICK_CHANGE => self.run(Command::Change),
            CLICK_PLATFORM => self.run(Command::ThisPlatform),
            CLICK_REMOVE => self.run(Command::Remove),
            CLICK_RESET => self.run(Command::Reset),
            CLICK_RUN => self.run(Command::Run),
            _ => EditorOutcome::Consumed,
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

    /// The page as HTML: the heading, the filter or the capture line, the
    /// keys, then the table of commands.
    fn html(&self) -> String {
        let total = match self.shown.len() == self.commands.len() {
            true => format!("{} commands", self.commands.len()),
            false => format!("{} of {} commands", self.shown.len(), self.commands.len()),
        };
        let mut out = format!(
            "<style>{STYLE}</style><div class=\"head\"><span class=\"title\">{HEADING}</span>\
             <span class=\"count\">{total}</span></div>"
        );
        out.push_str(&self.filter_html());
        out.push_str(&self.capture_html());
        let _ = write!(out, "<div class=\"hint\">{}</div>", escape(HINT));
        let _ = write!(
            out,
            "<div class=\"row cols\"><span class=\"c-cmd\">{HEADING_COMMAND}</span>\
             <span class=\"c-keys\">{HEADING_CHORD}</span><span class=\"c-src\">{HEADING_SOURCE}</span>\
             <span class=\"c-id\">{HEADING_ID}</span></div>"
        );
        if self.shown.is_empty() && !self.commands.is_empty() {
            let _ = write!(out, "<div class=\"empty\">{EMPTY_FILTERED}</div>");
        }
        for (row, shown) in self.shown.iter().enumerate() {
            out.push_str(&self.row_html(row, shown));
        }
        out
    }

    /// The filter line: its label, then what is typed with the caret while
    /// the keys are in it, or what to type.
    fn filter_html(&self) -> String {
        let typing = self.focus == Focus::Filter && self.capture.is_none();
        let label = match self.recording.is_some() {
            false => FILTER_LABEL.to_string(),
            true => match table::menu_typed(&KEYS, Command::Record) {
                Some((keys, _)) => format!("{RECORD_LABEL} ({} to stop)", format_sequence(&keys)),
                None => RECORD_LABEL.to_string(),
            },
        };
        let text = self.filter.text();
        let shown = match (text.is_empty(), typing) {
            (true, _) => {
                let placeholder = match (self.recording.is_some(), typing) {
                    (true, _) => RECORD_PLACEHOLDER,
                    (false, true) => FILTER_PLACEHOLDER,
                    (false, false) => FILTER_PLACEHOLDER_NORMAL,
                };
                let caret = if typing {
                    "<span class=\"caret\"></span>"
                } else {
                    ""
                };
                format!("{caret}<span class=\"empty\">{placeholder}</span>")
            }
            (false, false) => escape(&text),
            (false, true) => {
                let at = self.filter.col();
                let before: String = text.chars().take(at).collect();
                let after: String = text.chars().skip(at).collect();
                format!(
                    "{}<span class=\"caret\"></span>{}",
                    escape(&before),
                    escape(&after)
                )
            }
        };
        let class = if typing { "filter typing" } else { "filter" };
        format!(
            "<div class=\"{class}\" data-action=\"{CLICK_FILTER}\"><span class=\"label\">{}</span>\
             <span>{shown}</span></div>",
            escape(&label)
        )
    }

    /// The capture line, while keys are captured: what for, the keys so
    /// far, what else they run, and its own keys.
    fn capture_html(&self) -> String {
        let Some(capture) = self.capture.as_ref() else {
            return String::new();
        };
        let label = self
            .commands
            .get(capture.index)
            .map_or("", |command| command.label.as_str());
        let what = match capture.kind {
            CaptureKind::Add => CAPTURE_ADD,
            CaptureKind::Platform => CAPTURE_PLATFORM,
            CaptureKind::Replace => CAPTURE_REPLACE,
        };
        let keys = match capture.keys.is_empty() {
            true => String::new(),
            false => kbd(&format_sequence(&capture.keys)),
        };
        let conflicts = self.capture_conflicts();
        let taken = match conflicts.is_empty() {
            true => String::new(),
            false => format!(
                "<span class=\"taken\">{CAPTURE_TAKEN} {}</span>",
                escape(&conflicts.join(", "))
            ),
        };
        format!(
            "<div class=\"capture\">{what} {}: {keys}<span class=\"caret\"></span>{taken}\
             <span class=\"help\">{CAPTURE_HELP}</span></div>",
            escape(label)
        )
    }

    /// The table row of shown command `row`: its name, its keys as key
    /// caps, its source, and its id, what the filter matched lit; the
    /// selected one with its buttons.
    fn row_html(&self, row: usize, shown: &ShownRow) -> String {
        let command = &self.commands[shown.index];
        let selected = row == self.selected;
        let mut class = String::from("row");
        if selected {
            class.push_str(" selected");
        }
        if self.contested.contains(&shown.index) {
            class.push_str(" contested");
        }
        let current = if selected { " data-current" } else { "" };
        let keys = match command.chord.is_empty() {
            true => format!("<span class=\"empty\">{NO_CHORD}</span>"),
            false => command.chords().map(kbd).collect(),
        };
        let source = match command.source {
            Source::Default => SOURCE_DEFAULT.to_string(),
            Source::Unbound => String::new(),
            Source::User => format!("<span class=\"user\">{SOURCE_USER}</span>"),
        };
        let buttons = match (selected, command.rebindable) {
            (true, true) => {
                let button = |verb: &str, label: &str| {
                    format!("<span class=\"button\" data-action=\"{verb}:{row}\">{label}</span>")
                };
                format!(
                    "<div class=\"actions\">{}{}{}{}{}{}</div>",
                    button(CLICK_RUN, "Run"),
                    button(CLICK_CHANGE, "Change"),
                    button(CLICK_ADD, "Add a key"),
                    button(CLICK_PLATFORM, "On this platform"),
                    button(CLICK_REMOVE, "Remove"),
                    button(CLICK_RESET, "Reset"),
                )
            }
            (true, false) => {
                format!("<div class=\"actions\"><span class=\"empty\">{BUILT_IN}</span></div>")
            }
            (false, _) => String::new(),
        };
        format!(
            "<div class=\"{class}\"{current} data-action=\"{CLICK_SELECT}:{row}\">\
             <div class=\"c-cmd\">{}{buttons}</div><span class=\"c-keys\">{keys}</span>\
             <span class=\"c-src\">{source}</span><span class=\"c-id\">{}</span></div>",
            lit(&command.label, &shown.matched.label),
            lit(&command.action, &shown.matched.action),
        )
    }
}

impl Page for KeysEditor {
    /// The selected command to run or rebind.
    fn context_items(&self) -> Vec<EditorMenuItem> {
        menu_groups([present([
            EditorMenuItem::bound(&KEYS, Command::Run),
            EditorMenuItem::bound(&KEYS, Command::Change),
            EditorMenuItem::bound(&KEYS, Command::Reset),
        ])])
    }

    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn content(&mut self, rows: usize, cols: usize, _wrap: bool) -> Frame {
        self.rows = rows;
        self.view.show(self.html(), (rows, cols));
        Frame::new(vec![Row::new()])
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        if self.capture.is_some() {
            return self.capture_key(key);
        }
        match self.focus {
            Focus::Filter => self.filter_key(key),
            Focus::List => self.list_key(key),
        }
    }

    fn on_mouse(&mut self, at: Point) -> EditorOutcome {
        // A press, not the drag after it; a capture ends only by its keys.
        if at.drag || self.capture.is_some() {
            return EditorOutcome::Consumed;
        }
        match self.view.action_at(at) {
            Some(action) => self.clicked(&action),
            None => EditorOutcome::Consumed,
        }
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        self.view.scroll_rows(lines);
        EditorOutcome::Consumed
    }

    fn scrolls_smoothly(&self) -> bool {
        true
    }

    fn on_scroll_pixels(&mut self, pixels: f32) -> EditorOutcome {
        self.view.scroll_by(pixels);
        EditorOutcome::Consumed
    }

    fn scroll_position(&self) -> Option<ScrollPosition> {
        self.view.scroll_position()
    }

    fn scroll_to(&mut self, offset: usize) -> EditorOutcome {
        self.view.scroll_to(offset as f32);
        EditorOutcome::Consumed
    }

    fn tiles(&self) -> Vec<Tile> {
        self.view.tiles()
    }

    fn set_appearance(&mut self, appearance: &Appearance) {
        self.view.set_appearance(appearance.theme, appearance.scale);
    }

    fn set_pane_size(&mut self, pane: (f32, f32)) {
        self.view.set_pane_size(pane);
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take().or_else(|| self.filter.take_notice())
    }

    fn take_requests(&mut self) -> Vec<EditorRequest> {
        std::mem::take(&mut self.requests)
    }

    /// Typing while the keys are in the filter or captured, else Normal;
    /// and how many commands are listed.
    fn status(&self) -> EditorStatus {
        let total = match self.shown.len() == self.commands.len() {
            true => format!("{} commands", self.commands.len()),
            false => format!("{} of {} commands", self.shown.len(), self.commands.len()),
        };
        let typing = self.capture.is_some() || self.focus == Focus::Filter;
        EditorStatus {
            mode: Some(match typing {
                true => EditMode::Insert,
                false => EditMode::Normal,
            }),
            segments: vec![total],
        }
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

/// `chord` as a key cap.
fn kbd(chord: &str) -> String {
    format!("<span class=\"kbd\">{}</span>", escape(chord))
}

/// `text` as HTML, the characters at `matched` lit.
fn lit(text: &str, matched: &[usize]) -> String {
    let mut out = String::new();
    for (at, c) in text.chars().enumerate() {
        let shown = escape(&c.to_string());
        match matched.contains(&at) {
            true => {
                let _ = write!(out, "<span class=\"lit\">{shown}</span>");
            }
            false => out.push_str(&shown),
        }
    }
    out
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
    use super::*;

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

    fn type_filter(editor: &mut KeysEditor, text: &str) {
        for c in text.chars() {
            editor.on_key(&press(KeyCode::Char(c)));
        }
    }

    fn listed(editor: &KeysEditor) -> Vec<&str> {
        editor
            .shown
            .iter()
            .map(|shown| editor.commands[shown.index].action.as_str())
            .collect()
    }

    fn filtering() -> KeysEditor {
        let mut editor = KeysEditor::from_rows(vec![
            row("split_vertical", "S-M--", "Pane: Split Vertically"),
            row("new_tab", "C-S-t", "Tab: New Tab"),
            row("open_settings", "C-,", "Settings: Open"),
        ]);
        editor.on_key(&press(KeyCode::Char('i')));
        editor
    }

    /// Two commands, `new_tab` first by name.
    fn rebinding() -> KeysEditor {
        KeysEditor::from_rows(vec![
            row("open_settings", "C-,", "Settings: Open"),
            row("new_tab", "C-S-t", "New Tab"),
        ])
    }

    fn rebind(chords: &[&str], platform: Option<&str>) -> EditorRequest {
        EditorRequest::Rebind {
            block: WINDOW_BLOCK.to_string(),
            chords: chords.iter().map(|chord| chord.to_string()).collect(),
            command: "new_tab".to_string(),
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
            EditorOutcome::RunAction("new_tab".to_string())
        );
        let keymap = winter_model::input::test_support::sequence_keymap();
        let mut editor = KeysEditor::new(
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
        assert!(editor.take_notice().is_some(), "says where it runs");
    }

    #[test]
    fn test_the_commands_are_sorted_by_name_whatever_their_case() {
        let editor = KeysEditor::from_rows(vec![row("b", "", "beta"), row("a", "", "Alpha")]);
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
        assert!(editor.html().contains(EMPTY_FILTERED));
        assert_eq!(
            editor.on_key(&press(KeyCode::Enter)),
            EditorOutcome::Consumed
        );
    }

    #[test]
    fn test_the_source_says_whose_binding_it_is() {
        let user = HashMap::from([(
            WINDOW_BLOCK.to_string(),
            HashMap::from([("C-S-n".to_string(), "new_tab".to_string())]),
        )]);
        let command = |action: &str, chord: &str| WindowCommand {
            action: action.to_string(),
            chord: chord.to_string(),
            label: action.to_string(),
        };
        let commands = vec![
            command("new_tab", "C-S-n"),
            command("close_tab", "C-w"),
            command("reload", ""),
        ];
        let editor = KeysEditor::new(&Keymap::default(), commands, &user);
        let source = |action: &str| {
            editor
                .commands
                .iter()
                .find(|command| command.block == WINDOW_BLOCK && command.action == action)
                .unwrap()
                .source
        };
        assert_eq!(source("new_tab"), Source::User);
        assert_eq!(source("close_tab"), Source::Default);
        assert_eq!(source("reload"), Source::Unbound);
        assert_eq!(editor.focus, Focus::List, "it opens over the list");
    }

    #[test]
    fn test_recorded_keys_filter_to_the_commands_bound_to_them_whole_chords_only() {
        let mut editor = KeysEditor::from_rows(vec![
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
        assert!(editor.html().contains("also bound to Settings: Open"));
    }

    #[test]
    fn test_x_resets_only_a_binding_of_the_users_own() {
        let mut editor = rebinding();
        editor.on_key(&press(KeyCode::Char('x')));
        assert!(
            editor.take_requests().is_empty(),
            "a built-in binding has nothing to reset"
        );
        assert!(editor.take_notice().is_some());
        editor.commands[0].source = Source::User;
        editor.on_key(&press(KeyCode::Char('x')));
        assert_eq!(editor.take_requests(), [rebind(&[], None)]);
    }

    #[test]
    fn test_a_click_selects_its_command_and_works_its_button() {
        let mut editor = rebinding();
        editor.clicked("select:1");
        assert_eq!(editor.selected_command().unwrap().action, "open_settings");
        assert_eq!(
            editor.clicked("run:0"),
            EditorOutcome::RunAction("new_tab".to_string())
        );
        editor.clicked("change:0");
        assert!(editor.capture.is_some());
        editor.capture = None;
        editor.clicked("filter");
        assert_eq!(editor.focus, Focus::Filter);
        editor.clicked("remove:9");
        assert!(editor.take_requests().is_empty(), "no such row");
    }

    #[test]
    fn test_the_page_draws_the_keys_as_caps_and_the_selected_command_s_buttons() {
        let mut editor = rebinding();
        let html = editor.html();
        assert!(html.contains("<span class=\"kbd\">C-S-t</span>"));
        assert!(html.contains("data-action=\"change:0\""));
        assert!(
            !html.contains("data-action=\"change:1\""),
            "only the selected row's"
        );
        type_filter(&mut editor, "i");
        type_filter(&mut editor, "tab");
        assert!(
            editor.html().contains("<span class=\"lit\">"),
            "what matched is lit"
        );
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
        let mut editor = KeysEditor::new(&Keymap::default(), entries(""), &HashMap::new());
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
        let editor = KeysEditor::from_rows(vec![
            row("new_tab", "C-S-t", "Tab: New Tab"),
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
            editor.contested.contains(&index("new_tab"))
                && editor.contested.contains(&index("other"))
        );
        assert!(
            !editor.contested.contains(&index("git_tab")),
            "another block's chord is its own"
        );
        assert!(editor.html().contains("row selected contested"));
    }

    fn every_command() -> KeysEditor {
        KeysEditor::new(
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
            assert_eq!(
                editor.take_notice().map(|notice| notice.text),
                Some(BUILT_IN.to_string()),
                "{key}"
            );
        }
    }

    #[test]
    fn test_every_key_of_the_editor_is_spelled_right() {
        assert_eq!(table::invalid_specs(&KEYS), Vec::<&str>::new());
    }
}
