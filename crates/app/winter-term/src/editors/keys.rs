//! Keys: every command, sorted by name, beside the chord bound to it, where
//! that binding comes from, and the id it is known by, under a filter line
//! that narrows the list as it is typed into, or, with `M-k`, to the commands
//! bound to the keys pressed. The list itself is a read-only text editor pane
//! supporting the full set of modal motions, where `c` captures new keys for
//! the command on the cursor's line, saved to the user's `keybindings.kdl`,
//! and `x` takes that binding away again.

use std::collections::{HashMap, HashSet};

use crate::config::schema::WINDOW_BLOCK;
use crate::editors::{key_tables, COMMAND_BLOCKS};
use winter_model::editor::{
    menu_groups, present, EditorHint, EditorMenuItem, EditorOutcome, EditorRequest, EditorStatus,
    Notice, Page, PromptMode, PromptReply, PromptRequest, ScrollPosition, SearchReport,
};
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed};
use winter_model::input::{format_sequence, Key, KeyCode, Keymap, Mods, MAX_SEQUENCE_LEN};
use winter_model::list::Viewport;
use winter_model::paint::{
    row_height, wrapped_lines, Caret, DiagnosticLevel, Frame, Point, Row, Span, Style,
};
use winter_model::textedit::buffer::TextBuffer;
use winter_model::textedit::field::TextField;
use winter_model::textedit::modal::{EditMode, EditOutcome, EditView, Marks, ModalEditor};
use winter_model::vim::nav::VimNav;
use winter_picker::query::Query;

// ========================================================================
// Constants
// ========================================================================

/// The question the editor asks for forward search.
const ASK_SEARCH: &str = "search";

/// The question the editor asks for backward search.
const ASK_SEARCH_BACK: &str = "search-back";

/// Joins the keys of one entry, in its keybinding column.
const CHORD_SEPARATOR: &str = " / ";

/// The cells kept between one column and the next.
const COLUMN_GAP: usize = 2;

/// How wide the columns are laid out for before the first paint says how
/// wide the pane is.
const DEFAULT_COLS: usize = 100;

/// What marks a column's text cut short to fit it.
const ELLIPSIS: char = '…';

/// What the list says when the filter lets nothing through.
const EMPTY_FILTERED: &str = "No command matches the filter";

/// What stands before the filter's text on its line.
const FILTER_LABEL: &str = "Filter: ";

/// What the filter line says while nothing is typed into it.
const FILTER_PLACEHOLDER: &str = "type a command's name, id, or keybinding, or record keys";

/// What the filter line says while nothing is typed into it and the keys
/// are over the list, in Normal mode.
const FILTER_PLACEHOLDER_NORMAL: &str = "press i to filter by a command's name, id, or keybinding";

/// The row the filter line is on.
const FILTER_ROW: usize = 1;

/// What a key listed for reading only says when asked to change.
const BUILT_IN: &str = "Vim's motions are built in, so they cannot be rebound";

/// What the capture line says after the keys pressed so far.
const CAPTURE_HELP: &str = "RET saves, Esc cancels, BS removes the last key";

/// What stands before the command a binding is being captured for.
const CAPTURE_LABEL: &str = "New keys for ";

/// What the capture line says of a chord another command is bound to.
const CAPTURE_TAKEN: &str = "also bound to";

/// What stands before the recorded keys on the filter line.
const RECORD_LABEL: &str = "Recording keys";

/// What the filter line says while recording, before a key is pressed.
const RECORD_PLACEHOLDER: &str = "press the keys to look up";

/// The rows above the first command: the heading, the filter line, and the
/// column headings.
const HEADER_ROWS: usize = 3;

/// The editor's heading.
const HEADING: &str = "Keyboard Shortcuts";

/// The column headings.
const HEADING_CHORD: &str = "Keybinding";
const HEADING_COMMAND: &str = "Command";
const HEADING_ID: &str = "Id";
const HEADING_SOURCE: &str = "Source";

/// Left margin every row starts at.
const LEFT_PAD: usize = 2;

/// The widest the keybinding column grows, however long a chord is.
const MAX_CHORD_WIDTH: usize = 24;

/// The widest the id column grows, however long an id is.
const MAX_ID_WIDTH: usize = 36;

/// The narrowest the keybinding column is laid out, so its heading fits.
const MIN_CHORD_WIDTH: usize = 14;

/// The least a command's name is left once the columns beside it are laid
/// out; past it, the id column and then the source column are dropped.
const MIN_COMMAND_WIDTH: usize = 28;

/// The menu's entries typed as Vim keys: the row copied through the `+`
/// register, so it reaches other windows, and a search.
const MENU_COPY_ROW: (&str, &str) = ("\" + y y", "Copy Row");
const MENU_SEARCH: (&str, &str) = ("/", "Find");

/// Shown in the chord column for a command reachable only from the palette.
const NO_CHORD: &str = "-";

/// What the source column says of each kind of binding.
const SOURCE_DEFAULT: &str = "default";
const SOURCE_USER: &str = "user";

/// The source column's width: its longest word and the gap after it.
const SOURCE_WIDTH: usize = SOURCE_DEFAULT.len() + COLUMN_GAP;

/// The editor's title.
const TITLE: &str = "Keys";

/// The editor's own keys in the list, ahead of the read-only editor's.
const KEYS: KeyTable<Command> = KeyTable {
    block: "keys",
    bindings: &[
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
    /// Capture new keys for the command on the cursor's line.
    Change,
    /// Put the keys in the filter line.
    Filter,
    /// In the filter, start or stop recording the keys to look up, as VS
    /// Code's keyboard shortcuts editor has it.
    Record,
    /// Take the user's own binding of the command on the cursor's line away,
    /// so its built-in one is in force again.
    Reset,
    /// Run the command on the cursor's line, in Normal mode.
    Run,
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

/// A read-only editor listing what Winter can do and how to ask for it.
#[derive(Clone, Debug)]
pub struct KeysEditor {
    /// The listed lines: one per command the filter lets through.
    buffer: TextBuffer,
    /// The new keys being pressed for a command's binding, while they are.
    capture: Option<Capture>,
    /// How the columns are laid out for the pane's width.
    columns: Columns,
    /// Every command, sorted by name.
    commands: Vec<CommandRow>,
    /// The commands whose chord another command of the same block is bound
    /// to as well, as indices into `commands`, marked as a warning: only
    /// one of them runs.
    contested: HashSet<usize>,
    /// The filter line.
    filter: TextField,
    focus: Focus,
    hscroll: usize,
    /// The pane width the lines were last laid out for.
    laid_out_cols: usize,
    marks: Marks,
    modal: ModalEditor,
    /// A report of the editor's own, ahead of the editor's.
    notice: Option<Notice>,
    painted: Vec<Painted>,
    /// What the editor asks of the window: a binding saved to the user's
    /// keybindings.
    requests: Vec<EditorRequest>,
    /// The keys pressed so far while recording them into the filter, which
    /// then lets through only the commands bound to them; `None` while
    /// the filter is typed into.
    recording: Option<Vec<Key>>,
    /// The commands the filter lets through, in order, each with where it
    /// matched.
    shown: Vec<ShownRow>,
    /// The window over the listed lines.
    view: Viewport,
}

/// The keys pressed so far for command `index`'s new binding.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Capture {
    index: usize,
    keys: Vec<Key>,
}

/// How wide each column is, in cells, its gap included; zero for one the
/// pane is too narrow to show.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Columns {
    chord: usize,
    command: usize,
    id: usize,
    source: usize,
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

/// What one screen row of a painted frame holds for click targeting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Painted {
    line: usize,
    offset: usize,
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
    /// keeping what the filter says and the cursor on the command it was on.
    pub fn refresh(
        &mut self,
        keymap: &Keymap,
        commands: Vec<WindowCommand>,
        user: &HashMap<String, HashMap<String, String>>,
    ) {
        let was_on = self
            .selected_command()
            .map(|command| command.action.clone());
        self.commands = Self::new(keymap, commands, user).commands;
        self.apply_filter();
        let row = self
            .shown
            .iter()
            .position(|shown| Some(&self.commands[shown.index].action) == was_on.as_ref());
        if let Some(row) = row {
            self.buffer.move_to(row, 0);
        }
    }

    /// The editor listing `commands`, one per line, sorted by name, the
    /// keys over the list.
    fn from_rows(mut commands: Vec<CommandRow>) -> Self {
        commands.sort_by_cached_key(|command| command.label.to_lowercase());
        let shown = (0..commands.len())
            .map(|index| ShownRow {
                index,
                matched: Matched::default(),
            })
            .collect();
        let mut editor = Self {
            buffer: TextBuffer::new(Vec::new()),
            capture: None,
            contested: HashSet::new(),
            columns: Columns::default(),
            commands,
            filter: TextField::line(""),
            focus: Focus::List,
            hscroll: 0,
            laid_out_cols: 0,
            marks: Marks::default(),
            modal: ModalEditor::read_only(),
            notice: None,
            painted: Vec::new(),
            recording: None,
            requests: Vec::new(),
            shown,
            view: Viewport::new(),
        };
        editor.lay_out(DEFAULT_COLS);
        editor
    }

    /// The index, among every command sorted by name, of the one on the
    /// cursor's line.
    pub fn selected(&self) -> usize {
        self.shown
            .get(self.buffer.row())
            .map_or(0, |shown| shown.index)
    }

    /// The command on the cursor's line, while the filter lets any through.
    fn selected_command(&self) -> Option<&CommandRow> {
        let shown = self.shown.get(self.buffer.row())?;
        self.commands.get(shown.index)
    }

    /// Let through only the commands the filter's text matches by name, id,
    /// or chord, keeping the cursor on the command it was on where that is
    /// still listed.
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
        let row = self
            .shown
            .iter()
            .position(|shown| shown.index == was_on)
            .unwrap_or(0);
        self.lay_out(self.laid_out_cols.max(1));
        self.buffer.move_to(row, 0);
    }

    /// Lay the listed lines out in columns for a pane `cols` cells wide,
    /// keeping the cursor on its line.
    fn lay_out(&mut self, cols: usize) {
        let row = self.buffer.row();
        let listed: Vec<&CommandRow> = self
            .shown
            .iter()
            .map(|shown| &self.commands[shown.index])
            .collect();
        self.columns = Columns::fit(cols, &listed);
        let lines = listed
            .iter()
            .map(|command| command_line(command, &self.columns))
            .collect();
        self.buffer = TextBuffer::new(lines);
        self.buffer
            .move_to(row.min(self.shown.len().saturating_sub(1)), 0);
        self.laid_out_cols = cols;
        self.contested = contested(&self.commands);
    }

    /// The heading, and how many commands are listed.
    fn heading_row(&self, cols: usize) -> Row {
        let count = match self.shown.len() == self.commands.len() {
            true => format!("{} commands", self.commands.len()),
            false => format!("{} of {} commands", self.shown.len(), self.commands.len()),
        };
        let used = LEFT_PAD + HEADING.chars().count();
        let gap = cols
            .saturating_sub(used + count.chars().count() + LEFT_PAD)
            .max(COLUMN_GAP);
        vec![
            Span::new(Style::Header, format!("{}{HEADING}", pad(LEFT_PAD))),
            Span::new(Style::Dim, format!("{}{count}", pad(gap))),
        ]
    }

    /// The filter line: its label, and what is typed or, while nothing is,
    /// what it is for.
    fn filter_row(&self) -> Row {
        let (label, placeholder, style) = match self.recording.is_some() {
            true => (self.filter_label(), RECORD_PLACEHOLDER, Style::Accent),
            false if self.focus == Focus::List => {
                (self.filter_label(), FILTER_PLACEHOLDER_NORMAL, Style::Dim)
            }
            false => (self.filter_label(), FILTER_PLACEHOLDER, Style::Dim),
        };
        let label = Span::new(style, format!("{}{label}", pad(LEFT_PAD)));
        match self.filter.text().is_empty() {
            true => vec![label, Span::new(Style::Dim, placeholder)],
            false => vec![label, Span::new(Style::Normal, self.filter.text())],
        }
    }

    /// Whether the keys are typed into the filter line or captured on it,
    /// which puts the caret there.
    fn is_typing(&self) -> bool {
        self.focus == Focus::Filter || self.capture.is_some()
    }

    /// The capture line: the command whose keys are being pressed, the keys
    /// so far, the commands those keys are bound to already, and what the
    /// keys that end it do.
    fn capture_row(&self) -> Row {
        let Some(capture) = &self.capture else {
            return Row::new();
        };
        let label = self
            .commands
            .get(capture.index)
            .map_or("", |command| command.label.as_str());
        let typed = format_sequence(&capture.keys);
        let mut row = vec![
            Span::new(
                Style::Accent,
                format!("{}{CAPTURE_LABEL}{label}: ", pad(LEFT_PAD)),
            ),
            Span::new(Style::Normal, typed.clone()),
        ];
        let taken: Vec<&str> = self
            .commands
            .iter()
            .enumerate()
            .filter(|(index, command)| {
                *index != capture.index
                    && !typed.is_empty()
                    && command.chords().any(|chord| chord == typed)
            })
            .map(|(_, command)| command.label.as_str())
            .collect();
        if !taken.is_empty() {
            let warning = format!("{}{CAPTURE_TAKEN} {}", pad(COLUMN_GAP), taken.join(", "));
            row.push(Span::new(
                Style::Diagnostic(DiagnosticLevel::Warning),
                warning,
            ));
        }
        row.push(Span::new(
            Style::Dim,
            format!("{}{CAPTURE_HELP}", pad(COLUMN_GAP)),
        ));
        row
    }

    /// Start capturing new keys for the command on the cursor's line.
    fn start_capture(&mut self) {
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
                let (index, keys) = (capture.index, std::mem::take(&mut capture.keys));
                self.capture = None;
                match keys.is_empty() {
                    true => {
                        self.notice =
                            Some(Notice::info("no keys pressed, so the binding is unchanged"))
                    }
                    false => self.save_binding(index, vec![format_sequence(&keys)]),
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

    /// Take the user's own binding of the command on the cursor's line away,
    /// so its built-in one is in force again; say so where there is none.
    fn reset_binding(&mut self) {
        let Some(command) = self.selected_command() else {
            return;
        };
        if !command.rebindable {
            self.notice = Some(Notice::info(BUILT_IN));
            return;
        }
        match command.source {
            Source::User => self.save_binding(self.selected(), Vec::new()),
            Source::Default | Source::Unbound => {
                self.notice = Some(Notice::info("already the built-in binding"));
            }
        }
    }

    /// Ask the window to bind command `index` to `chords` in the user's
    /// keybindings, or with none to take their binding of it away, and show
    /// the new binding at once rather than once the file is read again.
    fn save_binding(&mut self, index: usize, chords: Vec<String>) {
        let Some(command) = self.commands.get_mut(index) else {
            return;
        };
        self.requests.push(EditorRequest::Rebind {
            block: command.block.to_string(),
            chords: chords.clone(),
            command: command.action.clone(),
        });
        if let Some(chord) = chords.first() {
            command.chord = chord.clone();
            command.source = Source::User;
        }
        self.lay_out(self.laid_out_cols.max(1));
    }

    /// What stands before the filter's text: while recording, saying which
    /// key stops it, as bound now.
    fn filter_label(&self) -> String {
        if self.recording.is_none() {
            return FILTER_LABEL.to_string();
        }
        match table::menu_typed(&KEYS, Command::Record) {
            Some((keys, _)) => format!("{RECORD_LABEL} ({} to stop): ", format_sequence(&keys)),
            None => format!("{RECORD_LABEL}: "),
        }
    }

    /// Start recording keys into the filter, or stop and clear it.
    fn toggle_recording(&mut self) {
        self.recording = match self.recording {
            Some(_) => None,
            None => Some(Vec::new()),
        };
        self.filter = TextField::line("");
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
        self.filter = TextField::line(&format_sequence(keys));
        self.apply_filter();
    }

    /// The column headings, laid out as the lines are.
    fn headings_row(&self) -> Row {
        let headings = [
            (HEADING_COMMAND, self.columns.command),
            (HEADING_CHORD, self.columns.chord),
            (HEADING_SOURCE, self.columns.source),
            (HEADING_ID, self.columns.id),
        ];
        let text: String = headings
            .iter()
            .map(|(heading, width)| fit(heading, *width))
            .collect();
        vec![Span::new(
            Style::Header,
            format!("{}{}", pad(LEFT_PAD), text.trim_end()),
        )]
    }

    /// Run the command on the cursor's line, or say where an editor's own one
    /// runs instead.
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

    /// Listed line `index`, `line`, styled by its columns: the name plain,
    /// the chord in the accent, the source and the id receded, what the
    /// filter matched lit, and the line the cursor is on marked while the
    /// keys are in the filter.
    fn command_row(&self, index: usize, line: &str) -> Row {
        let marked = self.selected_columns(index);
        let from = line.chars().count().min(self.hscroll);
        let marked = marked.map(|(s, e)| (s.saturating_sub(from), e.saturating_sub(from)));
        let picked = self.focus == Focus::Filter && index == self.buffer.row();
        let lit = self
            .shown
            .get(index)
            .map(|shown| self.lit_columns(&shown.matched))
            .unwrap_or_default();
        let is_contested = self
            .shown
            .get(index)
            .is_some_and(|shown| self.contested.contains(&shown.index));
        let mut spans: Vec<Span> = Vec::new();
        for (col, c) in line.chars().skip(from).enumerate() {
            let actual_col = col + from;
            let selected = picked || marked.is_some_and(|(start, end)| col >= start && col < end);
            let style = match self.columns.style_at(actual_col) {
                Style::Accent if is_contested => Style::Diagnostic(DiagnosticLevel::Warning),
                style => style,
            };
            let is_lit = lit.contains(&actual_col);
            match spans.last_mut() {
                Some(last)
                    if last.style == style && last.selected == selected && last.lit == is_lit =>
                {
                    last.text.push(c)
                }
                _ => {
                    let span = Span::new(style, c.to_string()).lit(is_lit);
                    spans.push(if selected { span.selected() } else { span });
                }
            }
        }
        if spans.is_empty() {
            spans.push(Span::plain(String::new()));
        }
        spans
    }

    /// The line columns the filter matched, each column's matches kept to
    /// the part of it that shows.
    fn lit_columns(&self, matched: &Matched) -> Vec<usize> {
        let starts = self.columns.starts();
        let in_column = |positions: &[usize], start: usize, width: usize| {
            let room = width.saturating_sub(COLUMN_GAP);
            positions
                .iter()
                .filter(move |at| **at < room)
                .map(move |at| start + at)
                .collect::<Vec<_>>()
        };
        let mut lit = in_column(&matched.label, starts.command, self.columns.command);
        lit.extend(in_column(&matched.chord, starts.chord, self.columns.chord));
        lit.extend(in_column(&matched.action, starts.id, self.columns.id));
        lit
    }

    fn selected_columns(&self, index: usize) -> Option<(usize, usize)> {
        let selection = self.modal.selection(&self.buffer)?;
        if index < selection.start.0 || index > selection.end.0 {
            return None;
        }
        let width = self
            .buffer
            .lines()
            .get(index)
            .map_or(0, |l| l.chars().count());
        if selection.linewise {
            return Some((0, width.max(1)));
        }
        let start = match index == selection.start.0 {
            true => selection.start.1,
            false => 0,
        };
        let end = match index == selection.end.0 {
            true => selection.end.1 + 1,
            false => width.max(1),
        };
        Some((start, end))
    }

    fn follow_cursor(&mut self, cols: usize, wrap: bool) {
        if wrap || cols == 0 {
            self.hscroll = 0;
            return;
        }
        let col = self.buffer.col();
        if col < self.hscroll {
            self.hscroll = col;
        } else if col >= self.hscroll + cols {
            self.hscroll = col + 1 - cols;
        }
    }

    fn remember_layout(&mut self, lines: &[usize], cols: usize, wrap: bool) {
        let mut painted = vec![
            Painted {
                line: lines.first().copied().unwrap_or(0),
                offset: 0,
            };
            HEADER_ROWS
        ];
        for &line in lines {
            let text = self.buffer.lines().get(line).map_or("", String::as_str);
            let chars: Vec<char> = text.chars().collect();
            for (start, _) in wrapped_lines(&chars, cols, wrap, 0) {
                painted.push(Painted {
                    line,
                    offset: match wrap {
                        true => start,
                        false => self.hscroll,
                    },
                });
            }
        }
        self.painted = painted;
    }

    fn point_at(&mut self, at: Point) -> EditorOutcome {
        let Some(spot) = self.painted.get(at.row).copied() else {
            return EditorOutcome::Ignored;
        };
        let col = spot.offset + at.col;
        if at.drag {
            if !self.modal.is_selecting() {
                self.modal.start_visual(&self.buffer, false);
            }
        } else {
            self.modal.clear_selection();
        }
        self.buffer.move_to(spot.line, col);
        EditorOutcome::Consumed
    }

    /// A key in Insert mode, typed into the filter, the list narrowed to
    /// match: `RET` runs the command the cursor is on, and `Esc` goes back
    /// to Normal mode over the list, keeping what is typed. Nothing else
    /// leaves Insert mode.
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
        match self.filter.on_key(&key.typed(), 1) {
            EditOutcome::Submit => return self.run_row(),
            // `Esc` leaves Insert mode, as in Vim, keeping what is typed.
            EditOutcome::Cancel => {
                self.filter = TextField::line(&before);
                self.focus = Focus::List;
            }
            _ => {}
        }
        if self.filter.text() != before {
            self.apply_filter();
        }
        EditorOutcome::Consumed
    }

    /// A key in Normal mode over the list: the editor's own first, `i` the
    /// only one into Insert mode in the filter, then the read-only editor's
    /// motions.
    fn list_key(&mut self, key: &Key) -> EditorOutcome {
        let normal = self.modal.mode() == EditMode::Normal;
        match table::find(&KEYS, key) {
            Some(Command::Run) if normal => return self.run_row(),
            Some(Command::Filter) if normal => {
                self.focus = Focus::Filter;
                return EditorOutcome::Consumed;
            }
            Some(Command::Change) if normal => {
                self.start_capture();
                return EditorOutcome::Consumed;
            }
            Some(Command::Reset) if normal => {
                self.reset_binding();
                return EditorOutcome::Consumed;
            }
            _ => {}
        }

        let reg_before = self.buffer.register().clone();
        let view = EditView {
            buffer: &mut self.buffer,
            marks: &mut self.marks,
            others: &[],
            viewport: self.view.rows(),
            shown: self.view.shown(),
        };
        let outcome = self.modal.on_key(view, key);
        let reg_after = self.buffer.register();
        if reg_after != &reg_before && !reg_after.text.is_empty() {
            return EditorOutcome::Yank(reg_after.text.clone());
        }
        match outcome {
            EditOutcome::Consumed => EditorOutcome::Consumed,
            EditOutcome::Ignored => EditorOutcome::Ignored,
            EditOutcome::Search { forward } => EditorOutcome::Prompt(PromptRequest {
                initial: String::new(),
                label: if forward {
                    "/".to_string()
                } else {
                    "?".to_string()
                },
                mode: PromptMode::Search { reverse: !forward },
                tag: if forward { ASK_SEARCH } else { ASK_SEARCH_BACK },
                title: None,
            }),
            EditOutcome::Yank(text) => EditorOutcome::Yank(text),
            EditOutcome::Quit { .. } => EditorOutcome::Close,
            EditOutcome::Place(place) => {
                self.view.request(place);
                EditorOutcome::Consumed
            }
            _ => EditorOutcome::Consumed,
        }
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
// Columns
// ========================================================================

impl Columns {
    /// The columns for `listed` in a pane `cols` cells wide: the chord's
    /// as wide as the longest chord needs, within bounds, the source's and
    /// the id's only where the name keeps its least width beside them, and
    /// the name's the rest.
    fn fit(cols: usize, listed: &[&CommandRow]) -> Self {
        let room = cols.saturating_sub(LEFT_PAD);
        let longest = |text: fn(&CommandRow) -> &str| {
            listed
                .iter()
                .map(|command| text(command).chars().count())
                .max()
                .unwrap_or(0)
        };
        let chord = (longest(|command| &command.chord) + COLUMN_GAP)
            .clamp(MIN_CHORD_WIDTH, MAX_CHORD_WIDTH);
        let id = (longest(|command| &command.action) + COLUMN_GAP).min(MAX_ID_WIDTH);
        let fits = |taken: usize| room >= taken + MIN_COMMAND_WIDTH;
        let (source, id) = match (fits(chord + SOURCE_WIDTH + id), fits(chord + SOURCE_WIDTH)) {
            (true, _) => (SOURCE_WIDTH, id),
            (false, true) => (SOURCE_WIDTH, 0),
            (false, false) => (0, 0),
        };
        Self {
            chord,
            command: room
                .saturating_sub(chord + source + id)
                .max(MIN_COMMAND_WIDTH),
            id,
            source,
        }
    }

    /// Where each column starts on a line, its left margin counted.
    fn starts(&self) -> Columns {
        let command = LEFT_PAD;
        let chord = command + self.command;
        let source = chord + self.chord;
        Columns {
            chord,
            command,
            id: source + self.source,
            source,
        }
    }

    /// The style of the character at line column `col`, by the column it
    /// falls in.
    fn style_at(&self, col: usize) -> Style {
        let starts = self.starts();
        match col {
            col if col >= starts.source => Style::Dim,
            col if col >= starts.chord => Style::Accent,
            _ => Style::Normal,
        }
    }
}

// ========================================================================
// Editor
// ========================================================================

impl Page for KeysEditor {
    /// The command under the pointer to run or copy, then a search, then
    /// closing.
    fn context_items(&self) -> Vec<EditorMenuItem> {
        let vim = |(spec, label): (&str, &str)| EditorMenuItem::spec(spec, label);
        menu_groups([
            present([
                EditorMenuItem::bound(&KEYS, Command::Run),
                vim(MENU_COPY_ROW),
            ]),
            present([vim(MENU_SEARCH)]),
        ])
    }

    fn title(&self) -> String {
        TITLE.to_string()
    }

    /// In the filter, on the character after what is typed, in the shape
    /// a terminal's cursor takes while typing; over the list, on the cursor's
    /// line while any is listed.
    fn caret(&self) -> Option<Caret> {
        if self.capture.is_some() {
            let typed: usize = self
                .capture_row()
                .iter()
                .take(2)
                .map(|span| span.text.chars().count())
                .sum();
            return Some(Caret {
                col: typed,
                insert: true,
            });
        }
        match self.focus {
            Focus::Filter => Some(Caret {
                col: LEFT_PAD + self.filter_label().chars().count() + self.filter.col(),
                insert: true,
            }),
            Focus::List if self.shown.is_empty() => None,
            Focus::List => Some(Caret {
                col: self.buffer.col().saturating_sub(self.hscroll),
                insert: false,
            }),
        }
    }

    fn hint(&self) -> Option<EditorHint> {
        Some(EditorHint {
            items: self.modal.hint()?,
            menu: false,
            title: "keys".to_string(),
        })
    }

    /// A press on the filter line puts the keys there; one on the list,
    /// over it, on the line pressed.
    fn on_mouse(&mut self, at: Point) -> EditorOutcome {
        self.view.release();
        if at.row == FILTER_ROW {
            self.focus = Focus::Filter;
            return EditorOutcome::Consumed;
        }
        if at.row < HEADER_ROWS {
            return EditorOutcome::Consumed;
        }
        self.focus = Focus::List;
        self.point_at(at)
    }

    fn set_scroll_margin(&mut self, rows: usize) {
        self.view.set_margin(rows);
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        self.view.wheel(lines);
        EditorOutcome::Consumed
    }

    fn scroll_position(&self) -> Option<ScrollPosition> {
        self.view.scroll_position(self.buffer.lines().len())
    }

    fn scroll_to(&mut self, offset: usize) -> EditorOutcome {
        self.view.scroll_to(offset, self.buffer.lines().len());
        EditorOutcome::Consumed
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> Frame {
        if cols != self.laid_out_cols {
            self.lay_out(cols);
        }
        let visible = rows.saturating_sub(HEADER_ROWS);
        self.view.set_rows(visible);
        self.follow_cursor(cols, wrap);
        let total = self.buffer.lines().len();
        let lines_of = self.buffer.lines();
        let placed = self.view.place(self.buffer.row(), total, visible, |index| {
            row_height(
                lines_of.get(index).map_or("", String::as_str),
                cols,
                wrap,
                0,
            )
        });
        let window = placed.window;
        let mut frame_rows = vec![
            self.heading_row(cols),
            match self.capture.is_some() {
                true => self.capture_row(),
                false => self.filter_row(),
            },
            self.headings_row(),
        ];
        let lines: Vec<usize> = (window.start..(window.start + window.count).min(total)).collect();
        for &index in &lines {
            let line = self.buffer.lines().get(index).map_or("", String::as_str);
            frame_rows.push(self.command_row(index, line));
        }
        self.remember_layout(&lines, cols, wrap);
        if self.shown.is_empty() {
            if !self.commands.is_empty() {
                frame_rows.push(vec![Span::new(
                    Style::Dim,
                    format!("{}{EMPTY_FILTERED}", pad(LEFT_PAD)),
                )]);
            }
            let mut content = Frame::new(frame_rows);
            content.cursor_line = self.is_typing().then_some(FILTER_ROW);
            return content;
        }
        let mut content = placed.apply(Frame::new(frame_rows), HEADER_ROWS);
        if self.is_typing() {
            content.cursor_line = Some(FILTER_ROW);
        }
        content
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice
            .take()
            .or_else(|| self.filter.take_notice())
            .or_else(|| self.modal.take_notice())
    }

    fn take_requests(&mut self) -> Vec<EditorRequest> {
        std::mem::take(&mut self.requests)
    }

    fn take_search(&mut self) -> Option<SearchReport> {
        self.modal.take_search(&self.buffer)
    }

    /// Typing while the keys are in the filter, else the list's own mode;
    /// and how many commands are listed.
    fn status(&self) -> EditorStatus {
        let total = match self.shown.len() == self.commands.len() {
            true => format!("{} commands", self.commands.len()),
            false => format!("{} of {} commands", self.shown.len(), self.commands.len()),
        };
        match self.focus {
            _ if self.capture.is_some() => EditorStatus {
                mode: Some(EditMode::Insert),
                segments: vec![total],
            },
            Focus::Filter => EditorStatus {
                mode: Some(EditMode::Insert),
                segments: vec![total],
            },
            Focus::List => self.modal.editor_status(&self.buffer, total),
        }
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        self.view.release();
        if self.capture.is_some() {
            return self.capture_key(key);
        }
        match self.focus {
            Focus::Filter => self.filter_key(key),
            Focus::List => self.list_key(key),
        }
    }

    fn keys(&self) -> Vec<KeyHelp> {
        let mut rows = table::help(&KEYS);
        rows.extend(VimNav::help());
        rows
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        self.view.release();
        if let Some(pattern) = reply.answer {
            let forward = reply.tag == ASK_SEARCH;
            let view = EditView {
                buffer: &mut self.buffer,
                marks: &mut self.marks,
                others: &[],
                viewport: self.view.rows(),
                shown: self.view.shown(),
            };
            self.modal.start_search(view, pattern, forward);
        }
        EditorOutcome::Consumed
    }

    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> EditorOutcome {
        if (tag == ASK_SEARCH || tag == ASK_SEARCH_BACK) && !query.is_empty() {
            let forward = tag == ASK_SEARCH;
            let view = EditView {
                buffer: &mut self.buffer,
                marks: &mut self.marks,
                others: &[],
                viewport: self.view.rows(),
                shown: self.view.shown(),
            };
            self.modal.start_search(view, query.to_string(), forward);
        }
        EditorOutcome::Consumed
    }
}

// ========================================================================
// Helpers
// ========================================================================

/// Command `command`'s line, laid out in `columns`.
fn command_line(command: &CommandRow, columns: &Columns) -> String {
    let chord = match command.chord.is_empty() {
        true => NO_CHORD,
        false => &command.chord,
    };
    let source = match command.source {
        Source::Default => SOURCE_DEFAULT,
        Source::Unbound => "",
        Source::User => SOURCE_USER,
    };
    let line = [
        fit(&command.label, columns.command),
        fit(chord, columns.chord),
        fit(source, columns.source),
        fit(&command.action, columns.id),
    ]
    .concat();
    format!("{}{}", pad(LEFT_PAD), line.trim_end())
}

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

/// `text` in a column `width` cells wide: padded out to it, or cut short
/// with an ellipsis so the column's gap stays clear. Nothing for a column
/// of no width.
fn fit(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let room = width.saturating_sub(COLUMN_GAP);
    let length = text.chars().count();
    match length > room {
        true => {
            let kept: String = text.chars().take(room.saturating_sub(1)).collect();
            format!("{kept}{ELLIPSIS}{}", pad(width - room))
        }
        false => format!("{text}{}", pad(width - length)),
    }
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

fn pad(width: usize) -> String {
    " ".repeat(width)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use winter_model::input::KeyCode;

    fn editor_with_rows(count: usize) -> KeysEditor {
        let commands: Vec<CommandRow> = (0..count)
            .map(|i| CommandRow {
                action: format!("action_{i}"),
                block: WINDOW_BLOCK,
                chord: String::new(),
                label: format!("command {i}"),
                rebindable: true,
                runs_in: None,
                source: Source::Unbound,
            })
            .collect();
        KeysEditor::from_rows(commands)
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

    #[test]
    fn test_the_menu_runs_copies_and_finds() {
        let editor = editor_with_rows(3);
        for item in editor
            .context_items()
            .into_iter()
            .filter(|item| !item.is_separator())
        {
            let mut probe = editor_with_rows(3);
            let mut last = EditorOutcome::Ignored;
            for key in item.keys() {
                last = probe.on_key(key);
            }
            assert_ne!(last, EditorOutcome::Ignored, "{}", item.label);
            if item.label == "Copy Row" {
                assert!(matches!(last, EditorOutcome::Yank(_)), "{last:?}");
            }
        }
    }

    #[test]
    fn test_the_emacs_buffer_ends_reach_the_first_and_last_command() {
        let alt = |c: char| Key {
            alt: true,
            code: KeyCode::Char(c),
            ctrl: false,
            shift: false,
            sup: false,
        };
        let mut editor = editor_with_rows(5);
        editor.on_key(&Key {
            shift: true,
            ..alt('>')
        });
        assert_eq!(editor.selected(), 4);
        editor.on_key(&Key {
            shift: true,
            ..alt('<')
        });
        assert_eq!(editor.selected(), 0);
    }

    #[test]
    fn test_cursor_stops_on_the_last_command() {
        let mut editor = editor_with_rows(2);
        for _ in 0..5 {
            editor.on_key(&press(KeyCode::Char('j')));
        }
        assert_eq!(editor.selected(), 1);
    }

    #[test]
    fn test_up_past_the_first_command_stays_in_normal_mode() {
        // Regression: `k` on the first line went into the filter, in
        // Insert mode, and typed the next `k` into it.
        let mut editor = editor_with_rows(2);
        editor.on_key(&press(KeyCode::Char('j')));
        for _ in 0..3 {
            editor.on_key(&press(KeyCode::Char('k')));
            editor.on_key(&press(KeyCode::Up));
        }
        assert_eq!((editor.selected(), editor.focus), (0, Focus::List));
        assert_eq!(editor.filter.text(), "");
    }

    #[test]
    fn test_empty_editor_keeps_the_cursor_at_zero() {
        let mut editor = editor_with_rows(0);
        editor.on_key(&press(KeyCode::Char('j')));
        assert_eq!(editor.selected(), 0);
    }

    fn search(editor: &mut KeysEditor, text: &str) {
        let outcome = editor.on_key(&press(KeyCode::Char('/')));
        let EditorOutcome::Prompt(request) = outcome else {
            panic!("expected a prompt, got {outcome:?}");
        };
        editor.on_prompt(PromptReply {
            answer: Some(text.to_string()),
            tag: request.tag,
        });
    }

    #[test]
    fn test_a_search_moves_the_cursor_to_the_matching_command() {
        let mut editor = editor_with_rows(5);
        search(&mut editor, "command 3");
        assert_eq!(editor.selected(), 3);
    }

    #[test]
    fn test_the_repeat_keys_step_through_the_matches_in_both_directions() {
        let mut editor = editor_with_rows(5);
        search(&mut editor, "command");
        assert_eq!(editor.selected(), 0);

        editor.on_key(&press(KeyCode::Char('n')));
        assert_eq!(editor.selected(), 1);

        editor.on_key(&press(KeyCode::Char('N')));
        assert_eq!(editor.selected(), 0);
    }

    #[test]
    fn test_a_search_finds_a_command_by_the_chord_beside_it() {
        let commands = vec![
            row("new_tab", "", "Tab: New Tab"),
            row("git_page", "C-S-g", "Git: Status"),
        ];
        let mut editor = KeysEditor::from_rows(commands);
        editor.on_key(&press(KeyCode::Char('G')));
        search(&mut editor, "c-s-g");
        assert_eq!(editor.commands[editor.selected()].action, "git_page");
    }

    #[test]
    fn test_enter_hands_back_the_command_under_the_cursor() {
        let mut editor = editor_with_rows(3);
        editor.on_key(&press(KeyCode::Char('j')));
        assert_eq!(
            editor.on_key(&press(KeyCode::Enter)),
            EditorOutcome::RunAction("action_1".to_string())
        );
    }

    #[test]
    fn test_enter_with_nothing_listed_runs_nothing() {
        let mut editor = editor_with_rows(0);
        assert_eq!(
            editor.on_key(&press(KeyCode::Enter)),
            EditorOutcome::Consumed
        );
    }

    #[test]
    fn test_a_search_with_nothing_to_find_reports_it() {
        let mut editor = editor_with_rows(2);
        editor.on_key(&press(KeyCode::Char('n')));
        assert!(editor.take_notice().is_some());

        search(&mut editor, "absent");
        assert!(editor.take_notice().is_some());
        assert_eq!(editor.selected(), 0);
    }

    #[test]
    fn test_an_empty_list_has_no_cursor_line() {
        let mut editor = editor_with_rows(0);
        assert_eq!(editor.content(20, 80, false).cursor_line, None);
    }

    #[test]
    fn test_cursor_line_sits_on_the_selected_command() {
        let mut editor = editor_with_rows(3);
        editor.on_key(&press(KeyCode::Char('j')));
        assert_eq!(
            editor.content(40, 80, false).cursor_line,
            Some(HEADER_ROWS + 1)
        );
    }

    #[test]
    fn test_vim_motions_work_in_keybinding_view() {
        let mut editor = editor_with_rows(10);
        // G goes to the last line
        editor.on_key(&press(KeyCode::Char('G')));
        assert_eq!(editor.selected(), 9);

        // gg goes to the first line (landing at first non-blank column 2)
        editor.on_key(&press(KeyCode::Char('g')));
        editor.on_key(&press(KeyCode::Char('g')));
        assert_eq!(editor.selected(), 0);
        assert_eq!(editor.buffer.col(), 2);

        // 0 moves to column 0
        editor.on_key(&press(KeyCode::Char('0')));
        assert_eq!(editor.buffer.col(), 0);

        // w moves forward words on the line
        editor.on_key(&press(KeyCode::Char('w')));
        assert!(editor.buffer.col() > 0);
    }

    #[test]
    fn test_read_only_protection_in_keybinding_view() {
        let mut editor = editor_with_rows(3);
        let orig_line = editor.buffer.lines()[0].clone();

        // Typing 'o', 'x', 'd' should not change buffer ('i' types into
        // the filter instead).
        editor.on_key(&press(KeyCode::Char('o')));
        assert!(editor.take_notice().is_some(), "refused as read-only");
        editor.on_key(&press(KeyCode::Char('x')));
        assert!(editor.take_notice().is_some(), "refused as read-only");
        editor.on_key(&press(KeyCode::Char('d')));
        assert!(editor.take_notice().is_some(), "refused as read-only");

        assert_eq!(editor.buffer.lines()[0], orig_line);
    }

    #[test]
    fn test_visual_mode_and_yank_in_keybinding_view() {
        let mut editor = editor_with_rows(3);
        // Start visual mode
        editor.on_key(&press(KeyCode::Char('v')));
        assert_eq!(editor.modal.mode(), EditMode::Visual);

        // Yank selection
        let outcome = editor.on_key(&press(KeyCode::Char('y')));
        assert_eq!(editor.modal.mode(), EditMode::Normal);
        assert!(matches!(outcome, EditorOutcome::Yank(_)));
    }

    #[test]
    fn test_q_does_not_close_keybinding_view() {
        let mut editor = editor_with_rows(3);
        assert_ne!(
            editor.on_key(&press(KeyCode::Char('q'))),
            EditorOutcome::Close
        );
    }

    #[test]
    fn test_an_editor_command_is_listed_with_its_chord_and_runs_only_in_its_editor() {
        let mut editor = KeysEditor::new(
            &Keymap::default(),
            crate::window::commands::window_commands(&Keymap::default()),
            &HashMap::new(),
        );
        let row = editor
            .commands
            .iter()
            .position(|row| row.action == "open_remote")
            .expect("the Git editor's command is listed");
        assert_eq!(editor.commands[row].chord, "M-g r");
        editor.buffer.move_to(row, 0);
        assert_eq!(
            editor.on_key(&press(KeyCode::Enter)),
            EditorOutcome::Consumed
        );
        assert!(editor.take_notice().is_some(), "says where it runs");
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

    #[test]
    fn test_the_commands_are_sorted_by_name_whatever_their_case() {
        let editor = KeysEditor::from_rows(vec![
            row("b", "", "beta"),
            row("a", "", "Alpha"),
            row("c", "", "Gamma"),
        ]);
        assert_eq!(listed(&editor), ["a", "b", "c"]);
    }

    #[test]
    fn test_the_filter_narrows_the_list_by_name_id_or_chord() {
        let mut editor = filtering();
        type_filter(&mut editor, "split");
        assert_eq!(listed(&editor), ["split_vertical"], "by name");
        editor.filter = TextField::line("");
        type_filter(&mut editor, "new_tab");
        assert_eq!(listed(&editor), ["new_tab"], "by id");
        editor.filter = TextField::line("");
        type_filter(&mut editor, "C-,");
        assert_eq!(listed(&editor), ["open_settings"], "by chord");
        assert!(
            editor
                .buffer
                .lines()
                .iter()
                .all(|line| !line.contains("New Tab")),
            "lines follow the filter"
        );
    }

    #[test]
    fn test_enter_in_the_filter_runs_the_first_command_it_lets_through() {
        let mut editor = filtering();
        type_filter(&mut editor, "settings");
        assert_eq!(
            editor.on_key(&press(KeyCode::Enter)),
            EditorOutcome::RunAction("open_settings".to_string())
        );
    }

    #[test]
    fn test_escape_leaves_insert_mode_keeping_the_filter() {
        let mut editor = filtering();
        type_filter(&mut editor, "tab");
        assert_eq!(editor.shown.len(), 1);
        editor.on_key(&press(KeyCode::Escape));
        assert_eq!(editor.focus, Focus::List);
        assert_eq!(
            (editor.filter.text(), editor.shown.len()),
            ("tab", 1),
            "still filtered"
        );
        // Back in Insert mode, `C-u` clears it.
        editor.on_key(&press(KeyCode::Char('i')));
        editor.on_key(&Key {
            ctrl: true,
            ..press(KeyCode::Char('u'))
        });
        assert_eq!((editor.filter.text(), editor.shown.len()), ("", 3));
    }

    #[test]
    fn test_only_escape_leaves_insert_mode() {
        let mut editor = filtering();
        type_filter(&mut editor, "t");
        assert!(editor.shown.len() > 1);
        for code in [KeyCode::Down, KeyCode::Tab] {
            editor.on_key(&press(code));
            assert_eq!(editor.focus, Focus::Filter, "{code:?} stays in Insert mode");
        }
        editor.on_key(&press(KeyCode::Escape));
        assert_eq!(editor.focus, Focus::List);
        editor.on_key(&press(KeyCode::Char('j')));
        assert_eq!(
            editor.buffer.row(),
            1,
            "motions move over the filtered lines"
        );
    }

    #[test]
    fn test_a_filter_matching_nothing_lists_nothing_and_runs_nothing() {
        let mut editor = filtering();
        type_filter(&mut editor, "zzzz");
        assert!(editor.shown.is_empty());
        assert_eq!(
            editor.on_key(&press(KeyCode::Enter)),
            EditorOutcome::Consumed
        );
        let frame = editor.content(20, 100, false);
        let text: String = frame
            .rows
            .iter()
            .flatten()
            .map(|span| span.text.as_str())
            .collect();
        assert!(text.contains(EMPTY_FILTERED), "{text}");
    }

    #[test]
    fn test_the_source_says_whose_binding_it_is() {
        let user = HashMap::from([(
            WINDOW_BLOCK.to_string(),
            HashMap::from([("C-S-n".to_string(), "new_tab".to_string())]),
        )]);
        let commands = vec![
            WindowCommand {
                action: "new_tab".to_string(),
                chord: "C-S-n".to_string(),
                label: "Tab: New Tab".to_string(),
            },
            WindowCommand {
                action: "close_tab".to_string(),
                chord: "C-w".to_string(),
                label: "Tab: Close".to_string(),
            },
            WindowCommand {
                action: "reload".to_string(),
                chord: String::new(),
                label: "Window: Reload".to_string(),
            },
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
        assert_eq!(
            editor.focus,
            Focus::List,
            "the editor opens in Normal mode, over the list"
        );
    }

    #[test]
    fn test_a_narrow_pane_drops_the_id_then_the_source_column() {
        let command = row("split_vertical", "S-M--", "Pane: Split Vertically");
        let wide = Columns::fit(120, &[&command]);
        assert!(wide.id > 0 && wide.source > 0);
        let middle = Columns::fit(60, &[&command]);
        assert_eq!((middle.id, middle.source), (0, SOURCE_WIDTH));
        let narrow = Columns::fit(40, &[&command]);
        assert_eq!((narrow.id, narrow.source), (0, 0));
        assert!(narrow.command >= MIN_COMMAND_WIDTH);
    }

    #[test]
    fn test_text_too_long_for_its_column_ends_in_an_ellipsis_before_the_gap() {
        assert_eq!(fit("abcdefgh", 6), "abc\u{2026}  ");
        assert_eq!(fit("ab", 6), "ab    ");
        assert_eq!(fit("anything", 0), "");
    }

    #[test]
    fn test_the_caret_is_in_the_filter_after_what_is_typed() {
        let mut editor = filtering();
        type_filter(&mut editor, "ta");
        let caret = editor.caret().unwrap();
        assert_eq!(
            caret,
            Caret {
                col: LEFT_PAD + FILTER_LABEL.len() + 2,
                insert: true
            }
        );
        assert_eq!(editor.content(20, 100, false).cursor_line, Some(FILTER_ROW));
    }

    #[test]
    fn test_recorded_keys_filter_to_the_commands_bound_to_them() {
        let mut editor = KeysEditor::from_rows(vec![
            row("new_tab", "C-S-t", "Tab: New Tab"),
            row("open_remote", "M-g r", "Git: Open Remote"),
            row("goto_line", "M-g g", "Go to Line"),
            row("reopen", "C-S-t C-S-t", "Tab: Reopen"),
        ]);
        editor.on_key(&press(KeyCode::Char('i')));
        let alt = |c| Key {
            alt: true,
            ..press(KeyCode::Char(c))
        };
        editor.on_key(&alt('k'));
        assert!(editor.recording.is_some());
        // `M-g` is the start of two sequences; `r` after it, of one.
        editor.on_key(&alt('g'));
        assert_eq!(listed(&editor), ["open_remote", "goto_line"], "by name");
        editor.on_key(&press(KeyCode::Char('r')));
        assert_eq!(listed(&editor), ["open_remote"]);
        assert_eq!(editor.filter.text(), "M-g r");
        // Stopping clears the filter and lists everything again.
        editor.on_key(&alt('k'));
        assert_eq!((editor.recording.is_some(), editor.shown.len()), (false, 4));
    }

    #[test]
    fn test_a_recorded_key_matches_whole_chords_not_their_letters() {
        let mut editor = KeysEditor::from_rows(vec![
            row("new_tab", "C-S-t", "Tab: New Tab"),
            row("toggle", "t", "Toggle"),
        ]);
        editor.on_key(&press(KeyCode::Char('i')));
        editor.on_key(&Key {
            alt: true,
            ..press(KeyCode::Char('k'))
        });
        editor.on_key(&press(KeyCode::Char('t')));
        assert_eq!(listed(&editor), ["toggle"], "a plain t is not C-S-t");
    }

    fn ctrl_shift(c: char) -> Key {
        Key {
            ctrl: true,
            shift: true,
            ..press(KeyCode::Char(c))
        }
    }

    /// Two commands, `new_tab` first by name.
    fn rebinding() -> KeysEditor {
        KeysEditor::from_rows(vec![
            row("open_settings", "C-,", "Settings: Open"),
            row("new_tab", "C-S-t", "New Tab"),
        ])
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
        assert_eq!(
            editor.take_requests(),
            [EditorRequest::Rebind {
                block: WINDOW_BLOCK.to_string(),
                chords: vec!["C-S-n".to_string()],
                command: "new_tab".to_string(),
            }]
        );
        let command = &editor.commands[0];
        assert_eq!(
            (command.chord.as_str(), command.source),
            ("C-S-n", Source::User),
            "shown at once"
        );
        assert!(editor.capture.is_none());
    }

    #[test]
    fn test_escape_while_capturing_changes_nothing() {
        let mut editor = rebinding();
        editor.on_key(&press(KeyCode::Char('c')));
        editor.on_key(&ctrl_shift('n'));
        editor.on_key(&press(KeyCode::Escape));
        assert!(editor.capture.is_none());
        assert!(editor.take_requests().is_empty());
        assert_eq!(editor.commands[0].chord, "C-S-t");
    }

    #[test]
    fn test_enter_with_no_keys_captured_saves_nothing() {
        let mut editor = rebinding();
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
        let warned = editor.capture_row().iter().any(|span| {
            span.style == Style::Diagnostic(DiagnosticLevel::Warning)
                && span.text.contains("Settings: Open")
        });
        assert!(warned, "{:?}", editor.capture_row());
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
        assert_eq!(
            editor.take_requests(),
            [EditorRequest::Rebind {
                block: WINDOW_BLOCK.to_string(),
                chords: Vec::new(),
                command: "new_tab".to_string(),
            }]
        );
    }

    #[test]
    fn test_a_refresh_keeps_the_filter_and_the_cursors_command() {
        let entries = |chord: &str| {
            vec![
                WindowCommand {
                    action: "a_tab".to_string(),
                    chord: String::new(),
                    label: "Tab: A".to_string(),
                },
                WindowCommand {
                    action: "b_tab".to_string(),
                    chord: chord.to_string(),
                    label: "Tab: B".to_string(),
                },
                WindowCommand {
                    action: "other".to_string(),
                    chord: String::new(),
                    label: "Other".to_string(),
                },
            ]
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
        let mut shared = row("other", "C-S-t", "Other Tab");
        let mut elsewhere = row("git_tab", "C-S-t", "Git: Tab");
        elsewhere.block = "git";
        shared.label = "Another Tab".to_string();
        let mut editor = KeysEditor::from_rows(vec![
            row("new_tab", "C-S-t", "Tab: New Tab"),
            shared,
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
        let frame = editor.content(20, 100, false);
        let warned = frame.rows.iter().flatten().any(|span| {
            span.style == Style::Diagnostic(DiagnosticLevel::Warning) && span.text.contains("C-S-t")
        });
        assert!(warned);
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
        let row = editor
            .commands
            .iter()
            .position(|command| command.block == "vim" && command.action == "down")
            .expect("Vim's motions are listed");
        assert!(!editor.commands[row].rebindable);
        editor.buffer.move_to(row, 0);
        for key in ['c', 'x'] {
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
    fn test_a_key_of_several_is_found_by_recording_any_of_them() {
        let mut editor = KeysEditor::from_rows(vec![
            row("down", "j / <down>", "Move down"),
            row("up", "k", "Move up"),
        ]);
        editor.on_key(&press(KeyCode::Char('i')));
        editor.on_key(&Key {
            alt: true,
            ..press(KeyCode::Char('k'))
        });
        editor.on_key(&press(KeyCode::Down));
        assert_eq!(listed(&editor), ["down"]);
        let matched = &editor.shown[0].matched.chord;
        assert_eq!(
            matched.first(),
            Some(&"j / ".chars().count()),
            "lit on the key that matched"
        );
    }

    #[test]
    fn test_every_key_of_the_editor_is_spelled_right() {
        assert_eq!(table::invalid_specs(&KEYS), Vec::<&str>::new());
    }
}
