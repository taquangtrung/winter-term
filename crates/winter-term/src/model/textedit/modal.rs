//! What a key does to the text: the Vim grammar of Normal, Visual and Replace,
//! and an Emacs-style Insert mode, since that is what the shell's own line is.
//!
//! The engine edits a [`TextBuffer`] it is lent one key at a time, through an
//! [`EditView`] that also carries the marks and the scroll of whatever the
//! text is shown in. Anything that is not an edit to the text (saving,
//! quitting, asking what to search for, the list of open files) comes back as
//! an [`EditOutcome`] for the host to carry out, so the same keys drive a file
//! in the editor, a column of names in a listing, and a one-line answer.

use std::collections::HashMap;

use crate::model::input::{Key, KeyCode, Mods, TextObject};
use crate::model::textedit::buffer::{Register, TextBuffer, WordCase};
use crate::model::textedit::kill::{KillRing, Merge};
use crate::model::textedit::search::Search;
use crate::model::tool::{ToolNotice, ToolStatus};
use crate::model::vim::motion::{CursorMove, FindChar};
use crate::model::vim::nav::{bare_motion, VimKey, VimNav};
use crate::model::vim::objects::{
    bracket_object, paragraph_object, quote_object, sentence_object, word_object, Span,
};
use crate::model::vim::words::{next_word_start, prev_word_start};

// ========================================================================
// Constants
// ========================================================================

/// What a screenful means to the paging keys, as a fraction of the view.
const HALF_PAGE: usize = 2;

/// How many lines a screen-wise Emacs scroll keeps in view, to keep the place.
const PAGE_OVERLAP: usize = 2;

/// The numbered registers a big delete shifts through, newest in `1`.
const NUMBERED_REGISTERS: [char; 9] = ['1', '2', '3', '4', '5', '6', '7', '8', '9'];

/// The `g` commands: the key, what it does, and what the key hints call it.
/// Both the keys and the hints read this, so neither can list one the other
/// does not have.
const GOTO: &[(char, Goto, &str)] = &[
    ('b', Goto::Buffers, "list the open files"),
    ('g', Goto::Motion(CursorMove::Top), "the first line"),
    (
        'e',
        Goto::Motion(CursorMove::WordEndBack),
        "the end of the word before",
    ),
    (
        'E',
        Goto::Motion(CursorMove::WordEndBackBig),
        "the end of the WORD before",
    ),
    (
        '_',
        Goto::Motion(CursorMove::LastNonBlank),
        "the last character with ink on it",
    ),
];

/// The `z` placements of the cursor's line in the view.
const SCROLL: &[(char, ScrollTo, &str)] = &[
    ('z', ScrollTo::Middle, "this line to the middle"),
    ('t', ScrollTo::Top, "this line to the top"),
    ('b', ScrollTo::Bottom, "this line to the bottom"),
];

/// The `Z` endings: whether each writes the text first.
const LEAVE: &[(char, bool, &str)] = &[
    ('Z', true, "write and close"),
    ('Q', false, "close, discarding edits"),
];

/// What is said when an edit key is pressed in a read-only buffer.
const READ_ONLY_MESSAGE: &str = "buffer is read-only";

// ========================================================================
// Data Structures
// ========================================================================

/// Which of the modes has the keyboard.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EditMode {
    /// Typing goes into the text.
    Insert,
    /// Keys are commands.
    #[default]
    Normal,
    /// Typing goes into the text, over what is already there.
    Replace,
    /// Keys are commands, and they act on what is selected.
    Visual,
}

/// What a key asked of the host, beyond the edit it made to the text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditOutcome {
    /// `Esc` in a field that is only ever typed into: the answer is withdrawn.
    Cancel,
    /// The key was spent on the text, the cursor, or a command part-typed.
    Consumed,
    /// `C-o`: hand the text to a fuller editor.
    Escalate,
    /// No binding here, so the host may have one.
    Ignored,
    /// `gb`: show the open files to choose from.
    ListBuffers,
    /// `"+p`: the host reads the clipboard and hands it to [`ModalEditor::paste_text`].
    Paste,
    /// `ZZ` or `ZQ`: finish, writing the text first or throwing it away.
    Quit {
        /// Whether to write the text first (`ZZ`) rather than discard it.
        save: bool,
    },
    /// `C-s`: write the text.
    Save,
    /// `/` or `?`: ask what to look for, and hand it to [`ModalEditor::start_search`].
    Search {
        /// True for `/`, which searches down the text.
        forward: bool,
    },
    /// `]b` or `[b`: on to the next open file, or back to the one before.
    StepBuffer {
        /// True for `]b`.
        forward: bool,
    },
    /// `Enter` while typing into text whose lines are fixed: the edit is done.
    Submit,
    /// `"+y` and the like: put this on the clipboard.
    Yank(String),
}

/// The places worth going back to in one text: the marks `m` sets, and where
/// the cursor was before the last jump.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Marks {
    /// Where `''` goes: where the cursor was before the last jump.
    pub jump: Option<(usize, usize)>,
    /// The marks `m` has set, as the position each was set at.
    named: HashMap<char, (usize, usize)>,
}

/// What the engine is lent for one key: the text, its marks, and the window
/// it is read through, which the paging and scrolling keys measure against.
#[derive(Debug)]
pub struct EditView<'a> {
    /// The text the key edits.
    pub buffer: &'a mut TextBuffer,
    /// Where `m` sets marks in it and `''` goes back to.
    pub marks: &'a mut Marks,
    /// The lines of other texts open beside it, which `M-/` completes from
    /// after the text's own words.
    pub others: &'a [&'a [String]],
    /// The first line in view, which the scroll placements move.
    pub scroll: &'a mut usize,
    /// How many lines are in view.
    pub viewport: usize,
}

/// The state of modal editing between keys: the mode, whatever command is
/// part-typed, and what outlives a single text (registers, the last search,
/// the last change), so one yank reaches every file it is used over.
#[derive(Clone, Debug, Default)]
pub struct ModalEditor {
    /// Where a Visual selection started, for as long as there is one.
    anchor: Option<Anchor>,
    /// The `M-/` completion the last key put in, which the next one cycles.
    completion: Option<Completion>,
    /// A count being typed ahead of a command, as digits so far.
    count: Option<usize>,
    /// The last char search, for `;` and `,` to do again.
    find: Option<FindChar>,
    /// What the unnamed register held before a `"_` command, which puts it
    /// back: the black hole takes nothing.
    held: Option<Register>,
    /// Whether this is a field that is only ever typed into, where `Esc`
    /// withdraws the answer rather than stepping back to Normal.
    insert_only: bool,
    /// Whether the last key was a kill, so the next kill joins it.
    kill_chain: bool,
    /// What the kills and the unnamed deletes and yanks took, for `C-y` and
    /// `M-y`.
    kills: KillRing,
    /// The keys of the last command that changed the text, for `.`.
    last_change: Vec<Key>,
    mode: EditMode,
    /// What the last command reported, until the host takes it.
    notice: Option<ToolNotice>,
    nav: VimNav,
    /// An operator waiting for the motion or object that says how far it
    /// reaches.
    operator: Option<Operator>,
    /// A command part-way through being typed, waiting on its next key.
    pending: Option<Pending>,
    /// Whether this editor rejects edits because its text is read-only.
    read_only: bool,
    /// The keys of the command being typed, as they are pressed.
    record: Vec<Key>,
    /// The register the next command reads or writes, from a `"` prefix.
    register: Option<char>,
    /// The named registers yanks and deletes go to.
    registers: HashMap<char, Register>,
    /// Whether the keys arriving are a replay of the last change, so the
    /// replay is not recorded as a change of its own.
    replaying: bool,
    /// The text revision the command being typed started from, which is how a
    /// command that changed something is told from one that did not.
    revision: usize,
    /// What the last `/` or `?` looked for.
    search: Option<Search>,
    /// Where the last key's yank put its text, for `M-y` to swap it out.
    yanked: Option<Yanked>,
}

/// Where a Visual selection was started, which is the end of it the cursor is
/// not on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Anchor {
    col: usize,
    /// Whether the selection covers whole lines (`V` rather than `v`).
    linewise: bool,
    row: usize,
}

/// An `M-/` completion in progress: the word typed, the words it could
/// become, and which of them is in.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Completion {
    candidates: Vec<String>,
    /// Which candidate is in, or `None` for the word as it was typed.
    index: Option<usize>,
    row: usize,
    /// The column the word starts at.
    start: usize,
    typed: String,
}

/// Where a yank put its text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Yanked {
    end: (usize, usize),
    start: (usize, usize),
}

/// What a `g` command does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Goto {
    /// List the open files.
    Buffers,
    /// Move, or reach over, as a motion does.
    Motion(CursorMove),
}

/// Where a `z` command puts the cursor's line in the view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScrollTo {
    Bottom,
    Middle,
    Top,
}

/// A command part-way through being typed, waiting on the key that says what
/// it acts on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pending {
    /// `[` or `]`, awaiting the key that says which list to step through.
    Bracket(BracketWait),
    /// `f`/`F`/`t`/`T`, awaiting the character to look for.
    Find(FindWait),
    /// `g`, awaiting the key that says which of its commands is meant.
    Goto,
    /// `'` or `` ` ``, awaiting the mark to go to.
    Jump(MarkJump),
    /// `Z`, awaiting the second key that says save or discard.
    Leave,
    /// `m`, awaiting the letter the mark is named by.
    Mark,
    /// An operator, awaiting the motion that says how far it reaches.
    Motion,
    /// An operator's `i` or `a`, awaiting the object it takes.
    Object(ObjectWait),
    /// `"`, awaiting the register the next command reads or writes.
    Register,
    /// `r`, awaiting the character to write.
    Replace,
    /// `z`, awaiting the key that says where the cursor's line goes.
    Scroll,
}

/// A `[` or `]` waiting for what it steps through.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BracketWait {
    /// True after `]`, which steps on rather than back.
    pub forward: bool,
}

/// A char search waiting for its target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FindWait {
    /// True for `f`/`t`, false for `F`/`T`.
    pub forward: bool,
    /// True for `t`/`T`, which stop one short of the target.
    pub till: bool,
}

/// A jump to a mark waiting for the letter naming it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MarkJump {
    /// True after `` ` ``, which restores the column as well as the line.
    pub exact: bool,
}

/// A text object waiting for the key that names it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectWait {
    /// True after `a`, false after `i`.
    pub around: bool,
}

/// What an operator does to the text it reaches over.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Operator {
    /// How many times over, from the count typed before it. One where none
    /// was typed, which is what tells `G` from `20G`.
    count: usize,
    /// What it does to the text it reaches over.
    kind: OperatorKind,
    /// The register it reads or writes, from a `"` prefix.
    register: Option<char>,
}

/// The operators the engine carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OperatorKind {
    /// `c`: take the text and start typing in its place.
    Change,
    /// `d`: take the text out.
    Delete,
    /// `>`: push the lines it covers one level right.
    Indent,
    /// `<`: pull them one level back.
    Outdent,
    /// `y`: copy the text, leaving it where it is.
    Yank,
}

/// How far an operator reaches, and how the text there is taken.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Reach {
    /// Whether the cell landed on goes with it, as `e` and `$` do.
    inclusive: bool,
    /// Whether it takes whole lines, as `j` and `G` do.
    linewise: bool,
    /// Where the motion or object ended.
    to: (usize, usize),
}

/// The text a Visual selection covers, ordered from its first point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Selection {
    /// The last point selected, taken with the selection.
    pub end: (usize, usize),
    /// Whether whole lines are selected.
    pub linewise: bool,
    /// The first point selected.
    pub start: (usize, usize),
}

/// The engine and the view it was lent, for the length of one key.
struct Session<'a> {
    /// Whether the key before this one was a kill.
    chained: bool,
    /// The completion the key before this one put in, if it did.
    completing: Option<Completion>,
    ed: &'a mut ModalEditor,
    view: EditView<'a>,
    /// Where the key before this one yanked to, if it did.
    yanked: Option<Yanked>,
}

// ========================================================================
// ModalEditor: state
// ========================================================================

impl ModalEditor {
    /// An engine in Normal mode, keys first and typing second.
    pub fn new() -> Self {
        Self::default()
    }

    /// An engine for a field that is only ever typed into: it starts in
    /// Insert, and `Esc` withdraws the answer rather than leaving Insert.
    pub fn insert_only() -> Self {
        Self {
            insert_only: true,
            mode: EditMode::Insert,
            ..Self::default()
        }
    }

    /// An engine for read-only text: motions and yanks work, but edits are
    /// rejected.
    pub fn read_only() -> Self {
        Self {
            read_only: true,
            ..Self::default()
        }
    }

    /// The same engine, starting in `mode`.
    pub fn with_mode(mut self, mode: EditMode) -> Self {
        self.mode = mode;
        self
    }

    /// Mark this engine as read-only or writable.
    pub fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Set whether this engine rejects edits because the text is read-only.
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    /// Whether this engine rejects edits because the text is read-only.
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Which mode has the keyboard.
    pub fn mode(&self) -> EditMode {
        self.mode
    }

    /// Whether the caret sits between characters rather than on one, as it
    /// does while typing.
    pub fn is_typing(&self) -> bool {
        matches!(self.mode, EditMode::Insert | EditMode::Replace)
    }

    /// Whether nothing is part-typed in Normal mode, so a key the host keeps
    /// for itself (`q` to quit, say) cannot be the second half of a command.
    pub fn is_idle(&self) -> bool {
        self.mode == EditMode::Normal
            && self.pending.is_none()
            && self.count.is_none()
            && self.register.is_none()
            && !self.nav.in_sequence()
    }

    /// The command waiting on its next key, for a host listing what it takes.
    pub fn pending(&self) -> Option<Pending> {
        self.pending
    }

    /// What the operator waiting on its motion is called, for the same list.
    pub fn operator_name(&self) -> Option<&'static str> {
        self.operator.map(|operator| match operator.kind {
            OperatorKind::Change => "change",
            OperatorKind::Delete => "delete",
            OperatorKind::Indent => "indent",
            OperatorKind::Outdent => "outdent",
            OperatorKind::Yank => "yank",
        })
    }

    /// What the command waiting on its next key takes, as key and meaning,
    /// for the host's key hints.
    pub fn hint(&self) -> Option<Vec<(String, String)>> {
        let name = self.operator_name().unwrap_or("act");
        let pairs: Vec<(&str, String)> = match self.pending? {
            Pending::Bracket(_) => vec![("b", "the next open file".to_string())],
            Pending::Find(_) => vec![("<char>", "to the next one on this line".to_string())],
            Pending::Goto => {
                return Some(table_hint(GOTO.iter().map(|(key, _, what)| (*key, *what))));
            }
            Pending::Jump(_) => vec![
                ("<letter>", "to that mark".to_string()),
                ("' or `", "back to where the last jump started".to_string()),
            ],
            Pending::Leave => {
                return Some(table_hint(LEAVE.iter().map(|(key, _, what)| (*key, *what))));
            }
            Pending::Mark => vec![("<letter>", "name this place".to_string())],
            Pending::Motion => vec![
                ("<motion>", format!("{name} over it")),
                ("i or a", format!("{name} a text object")),
                ("same key", format!("{name} whole lines")),
            ],
            Pending::Object(_) => vec![
                ("w or W", "a word".to_string()),
                ("p or s", "a paragraph, a sentence".to_string()),
                ("\" ' ` ( [ {", "what they hold".to_string()),
            ],
            Pending::Register => vec![
                (
                    "<letter>",
                    "use that register, or append to it in capitals".to_string(),
                ),
                (
                    "0 or 1-9",
                    "the last yank, the last big deletes".to_string(),
                ),
                ("-", "the last small delete".to_string()),
                ("_", "the black hole, which keeps nothing".to_string()),
                ("+", "the system clipboard".to_string()),
            ],
            Pending::Replace => vec![("<char>", "replace with it".to_string())],
            Pending::Scroll => {
                return Some(table_hint(
                    SCROLL.iter().map(|(key, _, what)| (*key, *what)),
                ));
            }
        };
        Some(
            pairs
                .into_iter()
                .map(|(key, what)| (key.to_string(), what))
                .collect(),
        )
    }

    /// The register a `"` prefix named for the next command, if one did.
    pub fn register_prefix(&self) -> Option<char> {
        self.register
    }

    /// What a tool editing `buffer` with these keys puts in the status bar:
    /// the mode, where the cursor is, `total` saying how much there is, and
    /// the register a `"` named for the next command.
    pub fn tool_status(&self, buffer: &TextBuffer, total: String) -> ToolStatus {
        let mut segments = vec![format!("{}:{}", buffer.row() + 1, buffer.col() + 1), total];
        segments.extend(self.register.map(|register| format!("\"{register}")));
        ToolStatus {
            mode: Some(self.mode),
            segments,
        }
    }

    /// Report `notice`, as the host does for what it carried out itself.
    pub fn report(&mut self, notice: ToolNotice) {
        self.notice = Some(notice);
    }

    /// Take what the last command reported, for the status bar, leaving
    /// nothing reported.
    pub fn take_notice(&mut self) -> Option<ToolNotice> {
        self.notice.take()
    }

    /// Forget anything part-typed and go back to Normal, over text that has
    /// just been put in front of the keys: a command half-typed over one text
    /// means nothing over the next.
    pub fn reset(&mut self, buffer: &TextBuffer) {
        self.anchor = None;
        self.count = None;
        self.operator = None;
        self.pending = None;
        self.mode = match self.insert_only {
            true => EditMode::Insert,
            false => EditMode::Normal,
        };
        self.revision = buffer.revision();
    }

    /// Start selecting from where the cursor is, as `v` and `V` do.
    pub fn start_visual(&mut self, buffer: &TextBuffer, linewise: bool) {
        self.anchor = Some(Anchor {
            col: buffer.col(),
            linewise,
            row: buffer.row(),
        });
        self.mode = EditMode::Visual;
    }

    /// Whether a selection is being made.
    pub fn is_selecting(&self) -> bool {
        self.anchor.is_some()
    }

    /// Stop selecting, as a click elsewhere does.
    pub fn clear_selection(&mut self) {
        self.anchor = None;
        if self.mode == EditMode::Visual {
            self.mode = EditMode::Normal;
        }
    }

    /// The text the selection covers in `buffer`, ordered from its first point.
    pub fn selection(&self, buffer: &TextBuffer) -> Option<Selection> {
        let anchor = self.anchor?;
        let here = (buffer.row(), buffer.col());
        let there = (anchor.row, anchor.col);
        let (start, end) = match here < there {
            true => (here, there),
            false => (there, here),
        };
        Some(Selection {
            end,
            linewise: anchor.linewise,
            start,
        })
    }
}

// ========================================================================
// ModalEditor: keys
// ========================================================================

impl ModalEditor {
    /// One key, in whichever mode the engine is in, with what it changes kept
    /// for `.` to do again.
    pub fn on_key(&mut self, view: EditView<'_>, key: &Key) -> EditOutcome {
        Session::new(self, view).dispatch(key)
    }

    /// Look for `pattern` from the cursor, and stay where it was found: the
    /// answer to an [`EditOutcome::Search`].
    pub fn start_search(&mut self, view: EditView<'_>, pattern: String, forward: bool) {
        if pattern.is_empty() {
            return;
        }
        self.search = Some(Search::new(pattern, forward));
        Session::new(self, view).search_step(false);
    }

    /// Put text the host read out of the clipboard in at the cursor: the
    /// answer to an [`EditOutcome::Paste`], and what a pasted chord brings.
    pub fn paste_text(&mut self, view: EditView<'_>, text: String) {
        if self.read_only {
            self.notice = Some(ToolNotice::error(READ_ONLY_MESSAGE));
            return;
        }
        Session::new(self, view).paste_text(text);
    }
}

// ========================================================================
// Session: dispatch
// ========================================================================

impl<'a> Session<'a> {
    fn new(ed: &'a mut ModalEditor, view: EditView<'a>) -> Self {
        Self {
            chained: false,
            completing: None,
            ed,
            view,
            yanked: None,
        }
    }
}

impl Session<'_> {
    fn buffer(&self) -> &TextBuffer {
        self.view.buffer
    }

    fn buffer_mut(&mut self) -> &mut TextBuffer {
        self.view.buffer
    }

    fn dispatch(&mut self, key: &Key) -> EditOutcome {
        // What the key before left for this one to carry on: a kill to join,
        // a yank to swap, a completion to cycle. Any other key drops them.
        self.chained = std::mem::take(&mut self.ed.kill_chain);
        self.yanked = self.ed.yanked.take();
        self.completing = self.ed.completion.take();
        if !self.ed.replaying {
            self.ed.record.push(*key);
        }
        let outcome = match self.ed.mode {
            EditMode::Insert => self.on_insert_key(key),
            EditMode::Normal => self.on_normal_key(key),
            EditMode::Replace => self.on_replace_key(key),
            EditMode::Visual => self.on_visual_key(key),
        };
        self.close_command();
        outcome
    }

    /// A command is over once nothing is part-typed and the keys are commands
    /// again. What it changed, if anything, is what `.` now does again.
    fn close_command(&mut self) {
        if self.ed.mode != EditMode::Normal || self.ed.pending.is_some() || self.ed.count.is_some()
        {
            return;
        }
        if self.buffer().revision() != self.ed.revision {
            if !self.ed.replaying {
                self.ed.last_change = self.ed.record.clone();
            }
            self.ed.revision = self.buffer().revision();
        }
        self.ed.record.clear();
    }

    /// Type the last command that changed the text again (`.`).
    fn repeat_change(&mut self) -> EditOutcome {
        if self.ed.last_change.is_empty() {
            self.ed.notice = Some(ToolNotice::error("nothing to repeat"));
            return EditOutcome::Consumed;
        }
        let keys = self.ed.last_change.clone();
        self.ed.replaying = true;
        for key in &keys {
            self.dispatch(key);
        }
        self.ed.replaying = false;
        self.ed.revision = self.buffer().revision();
        EditOutcome::Consumed
    }

    /// The count typed before a command, defaulting to one, and spent in the
    /// reading: a count belongs to the command that follows it.
    fn take_count(&mut self) -> usize {
        self.ed.count.take().unwrap_or(1)
    }
}

// ========================================================================
// Session: Normal mode
// ========================================================================

impl Session<'_> {
    /// Normal mode: motions, operators, and the commands that change text.
    fn on_normal_key(&mut self, key: &Key) -> EditOutcome {
        if let Some(pending) = self.ed.pending {
            return self.resolve_pending(pending, key);
        }
        // An Alt chord belongs to the window: `M-p` moves focus rather than
        // pasting. Only the two buffer ends the shared layer claims stay here.
        if key.alt {
            return self.shared_motion(key);
        }
        if key.ctrl {
            return self.on_ctrl_key(key);
        }
        if self.take_digit(key) {
            return EditOutcome::Consumed;
        }
        // A count or a register typed and then thought better of: `Esc` drops
        // it rather than closing a file over a half-typed command.
        if key.glyph() == KeyCode::Escape && (self.ed.count.is_some() || self.ed.register.is_some())
        {
            self.ed.count = None;
            self.ed.register = None;
            return EditOutcome::Consumed;
        }
        if self.ed.read_only {
            if let KeyCode::Char(
                'i' | 'a' | 'I' | 'A' | 'o' | 'O' | 'R' | 's' | 'S' | 'x' | 'X' | 'C' | 'D' | 'r'
                | '~' | 'J' | 'p' | 'P' | 'u' | '.' | 'd' | 'c' | '>' | '<',
            ) = key.glyph()
            {
                self.ed.notice = Some(ToolNotice::error(READ_ONLY_MESSAGE));
                return EditOutcome::Consumed;
            }
        }
        match key.glyph() {
            KeyCode::Char('i') => self.enter_insert(),
            KeyCode::Char('a') => {
                self.buffer_mut().move_right(1, true);
                self.enter_insert()
            }
            KeyCode::Char('I') => {
                self.buffer_mut().move_first_non_blank();
                self.enter_insert()
            }
            KeyCode::Char('A') => {
                self.buffer_mut().move_line_end(true);
                self.enter_insert()
            }
            KeyCode::Char('o') | KeyCode::Char('O') => {
                let below = key.glyph() == KeyCode::Char('o');
                self.buffer_mut().snapshot();
                self.buffer_mut().open_line(below);
                self.ed.mode = EditMode::Insert;
                EditOutcome::Consumed
            }
            KeyCode::Char('R') => {
                self.buffer_mut().snapshot();
                self.ed.mode = EditMode::Replace;
                EditOutcome::Consumed
            }
            KeyCode::Char('v') => self.start_visual(false),
            KeyCode::Char('V') => self.start_visual(true),
            KeyCode::Char('x') => {
                let count = self.take_count();
                self.buffer_mut().snapshot();
                self.buffer_mut().delete_under(count);
                self.store_take()
            }
            KeyCode::Char('X') => {
                let count = self.take_count();
                self.buffer_mut().snapshot();
                let (row, col) = (self.buffer().row(), self.buffer().col());
                self.buffer_mut()
                    .take_between((row, col.saturating_sub(count)), (row, col), false);
                self.store_take()
            }
            KeyCode::Char('s') => {
                let count = self.take_count();
                self.buffer_mut().snapshot();
                self.buffer_mut().delete_under(count);
                self.ed.mode = EditMode::Insert;
                EditOutcome::Consumed
            }
            KeyCode::Char('S') => {
                let operator = self.operator_of(OperatorKind::Change);
                self.operate_on_lines(operator)
            }
            KeyCode::Char('C') => {
                let operator = self.operator_of(OperatorKind::Change);
                self.line_tail(operator)
            }
            KeyCode::Char('D') => {
                let operator = self.operator_of(OperatorKind::Delete);
                self.line_tail(operator)
            }
            KeyCode::Char('r') => {
                self.ed.pending = Some(Pending::Replace);
                EditOutcome::Consumed
            }
            KeyCode::Char('~') => {
                let count = self.take_count();
                self.buffer_mut().snapshot();
                self.buffer_mut().toggle_case(count);
                EditOutcome::Consumed
            }
            KeyCode::Char('J') => {
                let count = self.take_count();
                self.buffer_mut().snapshot();
                self.buffer_mut().join_lines(count);
                EditOutcome::Consumed
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                let after = key.glyph() == KeyCode::Char('p');
                self.paste(after)
            }
            KeyCode::Char('u') => {
                if !self.buffer_mut().undo() {
                    self.ed.notice = Some(ToolNotice::error("nothing to undo"));
                }
                // An undo is not a change to be done again: `.` still means
                // whatever changed the text last.
                self.ed.revision = self.buffer().revision();
                EditOutcome::Consumed
            }
            KeyCode::Char('.') => self.repeat_change(),
            KeyCode::Char('d') => self.start_operator(OperatorKind::Delete),
            KeyCode::Char('c') => self.start_operator(OperatorKind::Change),
            KeyCode::Char('y') => self.start_operator(OperatorKind::Yank),
            KeyCode::Char('>') => self.start_operator(OperatorKind::Indent),
            KeyCode::Char('<') => self.start_operator(OperatorKind::Outdent),
            KeyCode::Char('"') => {
                self.ed.pending = Some(Pending::Register);
                EditOutcome::Consumed
            }
            KeyCode::Char('m') => {
                self.ed.pending = Some(Pending::Mark);
                EditOutcome::Consumed
            }
            KeyCode::Char('\'') => {
                self.ed.pending = Some(Pending::Jump(MarkJump { exact: false }));
                EditOutcome::Consumed
            }
            KeyCode::Char('`') => {
                self.ed.pending = Some(Pending::Jump(MarkJump { exact: true }));
                EditOutcome::Consumed
            }
            KeyCode::Char('f') | KeyCode::Char('F') | KeyCode::Char('t') | KeyCode::Char('T') => {
                self.ed.pending = Some(Pending::Find(find_wait(key)));
                EditOutcome::Consumed
            }
            KeyCode::Char(';') | KeyCode::Char(',') => {
                let reverse = key.glyph() == KeyCode::Char(',');
                self.repeat_find(reverse)
            }
            KeyCode::Char('/') | KeyCode::Char('?') => EditOutcome::Search {
                forward: key.glyph() == KeyCode::Char('/'),
            },
            KeyCode::Char('n') | KeyCode::Char('N') => {
                let reverse = key.glyph() == KeyCode::Char('N');
                self.search_step(reverse)
            }
            KeyCode::Char('*') | KeyCode::Char('#') => {
                let forward = key.glyph() == KeyCode::Char('*');
                self.search_word(forward)
            }
            KeyCode::Char('g') => {
                self.ed.pending = Some(Pending::Goto);
                EditOutcome::Consumed
            }
            KeyCode::Char('[') | KeyCode::Char(']') => {
                let forward = key.glyph() == KeyCode::Char(']');
                self.ed.pending = Some(Pending::Bracket(BracketWait { forward }));
                EditOutcome::Consumed
            }
            KeyCode::Char('z') => {
                self.ed.pending = Some(Pending::Scroll);
                EditOutcome::Consumed
            }
            KeyCode::Char('Z') => {
                self.ed.pending = Some(Pending::Leave);
                EditOutcome::Consumed
            }
            _ => self.shared_motion(key),
        }
    }

    /// The Ctrl chords: saving, redo, the number keys, and the paging motions.
    fn on_ctrl_key(&mut self, key: &Key) -> EditOutcome {
        // Only the bare Ctrl chords are these; `C-S-d` is the window's.
        if key.mods() != Mods::CTRL {
            return self.shared_motion(key);
        }
        if self.ed.read_only && matches!(key.glyph(), KeyCode::Char('s' | 'r' | 'a' | 'x')) {
            self.ed.notice = Some(ToolNotice::error(READ_ONLY_MESSAGE));
            return EditOutcome::Consumed;
        }
        match key.glyph() {
            KeyCode::Char('s') => EditOutcome::Save,
            KeyCode::Char('r') => {
                if !self.buffer_mut().redo() {
                    self.ed.notice = Some(ToolNotice::error("nothing to redo"));
                }
                self.ed.revision = self.buffer().revision();
                EditOutcome::Consumed
            }
            KeyCode::Char('a') | KeyCode::Char('x') => {
                let count = self.take_count() as i64;
                let by = match key.glyph() == KeyCode::Char('a') {
                    true => count,
                    false => -count,
                };
                self.buffer_mut().snapshot();
                if !self.buffer_mut().add_to_number(by) {
                    self.ed.notice = Some(ToolNotice::error("no number here"));
                }
                EditOutcome::Consumed
            }
            KeyCode::Char('o') => EditOutcome::Escalate,
            _ => self.shared_motion(key),
        }
    }

    /// A key the engine does not claim, offered to the shared Vim layer so a
    /// motion means here what it means over every other surface.
    fn shared_motion(&mut self, key: &Key) -> EditOutcome {
        let typed = self.ed.count.take();
        match self.ed.nav.key(key) {
            VimKey::Motion(motion) => {
                self.jump_for(motion);
                self.apply_motion(motion, typed);
                EditOutcome::Consumed
            }
            VimKey::Pending => EditOutcome::Consumed,
            VimKey::Unhandled => EditOutcome::Ignored,
        }
    }

    /// A digit typed before a command, which belongs to the command that
    /// follows it. A leading zero is the motion to column one instead.
    fn take_digit(&mut self, key: &Key) -> bool {
        let KeyCode::Char(c) = key.glyph() else {
            return false;
        };
        if !c.is_ascii_digit() || (c == '0' && self.ed.count.is_none()) {
            return false;
        }
        let digit = c as usize - '0' as usize;
        self.ed.count = Some(self.ed.count.unwrap_or(0) * 10 + digit);
        true
    }
}

// ========================================================================
// Session: Insert and Replace
// ========================================================================

impl Session<'_> {
    /// Insert mode: an Emacs-style field, since that is what the shell's own
    /// line is, with the chords that edit one.
    fn on_insert_key(&mut self, key: &Key) -> EditOutcome {
        // Typing is no reason to lose the window chords: `M-h` moves focus,
        // it does not type an `h`. The Emacs Alt chords below leave those, and
        // `M-l` with them, alone.
        if key.alt {
            return match key.ctrl {
                true => EditOutcome::Ignored,
                false => self.on_insert_alt_key(key),
            };
        }
        if key.ctrl {
            return self.on_insert_ctrl_key(key);
        }
        match key.glyph() {
            KeyCode::Escape if self.ed.insert_only => EditOutcome::Cancel,
            KeyCode::Escape => self.leave_insert(),
            // Text whose lines stand for something cannot take a new one, so
            // `Enter` there is the answer being given.
            KeyCode::Enter if self.buffer().is_fixed() => EditOutcome::Submit,
            KeyCode::Enter => {
                self.buffer_mut().insert_newline();
                EditOutcome::Consumed
            }
            KeyCode::Backspace => {
                self.buffer_mut().backspace();
                EditOutcome::Consumed
            }
            KeyCode::Delete => {
                self.buffer_mut().delete_under(1);
                EditOutcome::Consumed
            }
            KeyCode::Tab => {
                self.buffer_mut().insert_char('\t');
                EditOutcome::Consumed
            }
            KeyCode::Space => {
                self.buffer_mut().insert_char(' ');
                EditOutcome::Consumed
            }
            KeyCode::Char(c) => {
                self.buffer_mut().insert_char(c);
                EditOutcome::Consumed
            }
            KeyCode::Left => {
                self.buffer_mut().move_left(1);
                EditOutcome::Consumed
            }
            KeyCode::Right => {
                self.buffer_mut().move_right(1, true);
                EditOutcome::Consumed
            }
            KeyCode::Up => {
                self.buffer_mut().move_up(1, true);
                EditOutcome::Consumed
            }
            KeyCode::Down => {
                self.buffer_mut().move_down(1, true);
                EditOutcome::Consumed
            }
            KeyCode::Home => {
                self.buffer_mut().move_line_start();
                EditOutcome::Consumed
            }
            KeyCode::End => {
                self.buffer_mut().move_line_end(true);
                EditOutcome::Consumed
            }
            _ => EditOutcome::Ignored,
        }
    }

    /// The Ctrl chords of Insert mode: the Emacs ones for moving by character
    /// and line, for killing and yanking, and for editing around the cursor.
    fn on_insert_ctrl_key(&mut self, key: &Key) -> EditOutcome {
        if key.mods() != Mods::CTRL {
            return EditOutcome::Ignored;
        }
        let KeyCode::Char(c) = key.glyph() else {
            return EditOutcome::Ignored;
        };
        match c {
            's' => return EditOutcome::Save,
            'a' => self.buffer_mut().move_line_start(),
            'e' => self.buffer_mut().move_line_end(true),
            'b' => self.buffer_mut().move_left(1),
            'f' => self.buffer_mut().move_right(1, true),
            'n' => self.buffer_mut().move_down(1, true),
            'p' => self.buffer_mut().move_up(1, true),
            'd' => self.buffer_mut().delete_forward(),
            'h' => self.buffer_mut().backspace(),
            'k' => self.kill(true, TextBuffer::kill_to_line_end),
            'o' => self.buffer_mut().open_line_here(),
            't' => self.buffer_mut().transpose_chars(),
            'y' => self.yank_kill(),
            'v' => {
                let tool = self.insert_tool();
                self.buffer_mut().move_down(tool, true);
            }
            'l' => self.center_on_cursor(),
            'u' => self.kill(false, |buffer| buffer.take_to(0, false)),
            'w' => {
                let chars = self.buffer().chars();
                let to = prev_word_start(&chars, self.buffer().col(), false).unwrap_or(0);
                self.kill(false, |buffer| buffer.take_to(to, false));
            }
            _ => return EditOutcome::Ignored,
        }
        EditOutcome::Consumed
    }

    /// The Alt chords of Insert mode: the Emacs word and buffer motions and the
    /// edits that work a word at a time. The letters the window keeps (`h`
    /// `j` `k` `l` and the digits) are not among them.
    fn on_insert_alt_key(&mut self, key: &Key) -> EditOutcome {
        match (key.mods(), key.glyph()) {
            (Mods::ALT, KeyCode::Char('b')) => self.buffer_mut().move_emacs_word(false),
            (Mods::ALT, KeyCode::Char('f')) => self.buffer_mut().move_emacs_word(true),
            (Mods::ALT, KeyCode::Char('d')) => {
                self.kill(true, |buffer| buffer.kill_emacs_word(true))
            }
            (Mods::ALT, KeyCode::Backspace) => {
                self.kill(false, |buffer| buffer.kill_emacs_word(false))
            }
            (Mods::ALT, KeyCode::Char('y')) => self.yank_pop(),
            (Mods::ALT, KeyCode::Char('/')) => self.complete(),
            (Mods::ALT, KeyCode::Char('t')) => self.buffer_mut().transpose_words(),
            (Mods::ALT, KeyCode::Char('u')) => self.buffer_mut().change_word_case(WordCase::Upper),
            (Mods::ALT, KeyCode::Char('c')) => {
                self.buffer_mut().change_word_case(WordCase::Capitalize)
            }
            (Mods::ALT, KeyCode::Char('m')) => self.buffer_mut().move_first_non_blank(),
            (Mods::ALT, KeyCode::Char('v')) => {
                let tool = self.insert_tool();
                self.buffer_mut().move_up(tool, true);
            }
            (Mods::ALT_SHIFT, KeyCode::Char('<')) => self.buffer_mut().move_text_start(),
            (Mods::ALT_SHIFT, KeyCode::Char('>')) => self.buffer_mut().move_text_end(),
            _ => return EditOutcome::Ignored,
        }
        EditOutcome::Consumed
    }

    /// How far a screen-wise Emacs scroll moves: a page, less the lines that
    /// stay in view to keep the place.
    fn insert_tool(&self) -> usize {
        self.view.viewport.saturating_sub(PAGE_OVERLAP).max(1)
    }

    /// Scroll so the cursor's line sits in the middle of the view (`C-l`).
    fn center_on_cursor(&mut self) {
        let row = self.buffer().row();
        *self.view.scroll = row.saturating_sub(self.view.viewport / HALF_PAGE);
    }

    /// Run an Emacs kill, and keep what it took on the kill ring: joined onto
    /// the last kill when the key before was one too, after it for a kill
    /// forward and before it for a kill back, so the run yanks back whole.
    fn kill(&mut self, forward: bool, edit: impl FnOnce(&mut TextBuffer)) {
        let before = self.buffer().revision();
        edit(self.buffer_mut());
        // A kill with nothing to take keeps the run going, as in Emacs.
        self.ed.kill_chain = self.chained;
        if self.buffer().revision() == before {
            return;
        }
        let merge = match (self.chained, forward) {
            (false, _) => Merge::New,
            (true, true) => Merge::Append,
            (true, false) => Merge::Prepend,
        };
        let taken = self.buffer().register().clone();
        self.ed.kills.kill(taken, merge);
        // The unnamed register holds the whole run, so `p` and `C-y` agree.
        if let Some(run) = self.ed.kills.yank() {
            self.buffer_mut().set_register(run);
        }
        self.ed.kill_chain = true;
    }

    /// `C-y`: put the newest kill back at the caret, or the unnamed register
    /// where nothing has been killed yet.
    fn yank_kill(&mut self) {
        if let Some(newest) = self.ed.kills.yank() {
            self.buffer_mut().set_register(newest);
        }
        self.put_yank();
    }

    /// Yank the register at the caret, remembering where it went for `M-y`.
    fn put_yank(&mut self) {
        let start = (self.buffer().row(), self.buffer().col());
        self.buffer_mut().yank();
        let end = (self.buffer().row(), self.buffer().col());
        if end != start {
            self.ed.yanked = Some(Yanked { end, start });
        }
    }

    /// `M-y` straight after a yank: swap what it put in for the kill before.
    fn yank_pop(&mut self) {
        let Some(yanked) = self.yanked else {
            self.ed.notice = Some(ToolNotice::error("the last command was not a yank"));
            return;
        };
        let Some(older) = self.ed.kills.yank_pop() else {
            return;
        };
        self.buffer_mut()
            .take_between(yanked.start, yanked.end, false);
        self.buffer_mut()
            .move_to_typing(yanked.start.0, yanked.start.1);
        self.buffer_mut().set_register(older);
        self.put_yank();
    }

    /// `M-/`: complete the word before the caret from the words around it,
    /// nearest first, then from the other open texts. Pressed again it puts
    /// in the next candidate, and after the last, the word as it was typed.
    fn complete(&mut self) {
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let continuing = self
            .completing
            .take()
            .filter(|done| done.row == row && done.start + current(done).chars().count() == col);
        let mut completion = match continuing {
            Some(completion) => completion,
            None => {
                let chars = self.buffer().chars();
                let mut start = col.min(chars.len());
                while start > 0 && is_word_char(chars[start - 1]) {
                    start -= 1;
                }
                let typed: String = chars[start..col.min(chars.len())].iter().collect();
                if typed.is_empty() {
                    return;
                }
                let candidates = completion_candidates(
                    self.buffer().lines(),
                    (row, start),
                    &typed,
                    self.view.others,
                );
                if candidates.is_empty() {
                    self.ed.notice =
                        Some(ToolNotice::error(format!("nothing completes `{typed}`")));
                    return;
                }
                Completion {
                    candidates,
                    index: None,
                    row,
                    start,
                    typed,
                }
            }
        };
        completion.index = match completion.index {
            None => Some(0),
            Some(at) if at + 1 < completion.candidates.len() => Some(at + 1),
            Some(_) => None,
        };
        let kept = self.buffer().register().clone();
        self.buffer_mut()
            .take_between((row, completion.start), (row, col), false);
        self.buffer_mut().move_to_typing(row, completion.start);
        for c in current(&completion).chars() {
            self.buffer_mut().insert_char(c);
        }
        self.buffer_mut().set_register(kept);
        self.ed.completion = Some(completion);
    }

    /// Replace mode (`R`): every character typed writes over the one under
    /// the cursor rather than pushing it along.
    fn on_replace_key(&mut self, key: &Key) -> EditOutcome {
        if key.alt || key.ctrl {
            return EditOutcome::Ignored;
        }
        match key.glyph() {
            KeyCode::Escape => self.leave_insert(),
            KeyCode::Backspace => {
                self.buffer_mut().move_left(1);
                EditOutcome::Consumed
            }
            KeyCode::Char(c) => {
                self.buffer_mut().replace_char_over(c);
                EditOutcome::Consumed
            }
            _ => self.on_insert_key(key),
        }
    }

    /// Back to Normal, with the cursor stepped onto the character it was
    /// typing past, which is where Vim leaves it.
    fn leave_insert(&mut self) -> EditOutcome {
        self.ed.mode = EditMode::Normal;
        self.buffer_mut().move_left(1);
        self.buffer_mut().clamp(false);
        EditOutcome::Consumed
    }

    /// Start typing, with an undo step open so the whole run walks back at
    /// once rather than a character at a time.
    fn enter_insert(&mut self) -> EditOutcome {
        self.buffer_mut().snapshot();
        self.ed.mode = EditMode::Insert;
        EditOutcome::Consumed
    }
}

// ========================================================================
// Session: Visual mode
// ========================================================================

impl Session<'_> {
    /// Start selecting from where the cursor is (`v`/`V`).
    fn start_visual(&mut self, linewise: bool) -> EditOutcome {
        self.ed.start_visual(self.view.buffer, linewise);
        EditOutcome::Consumed
    }

    /// Stop selecting, leaving the cursor where it is.
    fn leave_visual(&mut self) -> EditOutcome {
        self.ed.anchor = None;
        self.ed.mode = EditMode::Normal;
        self.buffer_mut().clamp(false);
        EditOutcome::Consumed
    }

    fn selection(&self) -> Option<Selection> {
        self.ed.selection(self.view.buffer)
    }

    /// Visual mode: the motions move one end of the selection, and an
    /// operator acts on everything between the two.
    fn on_visual_key(&mut self, key: &Key) -> EditOutcome {
        if let Some(pending) = self.ed.pending {
            return self.resolve_pending(pending, key);
        }
        if key.alt {
            return self.shared_motion(key);
        }
        if key.ctrl {
            return self.on_ctrl_key(key);
        }
        if self.take_digit(key) {
            return EditOutcome::Consumed;
        }
        let Some(selection) = self.selection() else {
            return self.leave_visual();
        };
        if self.ed.read_only {
            if let KeyCode::Char('d' | 'x' | 'c' | 's' | '>' | '<' | 'J' | '~' | 'p' | 'P' | 'r') =
                key.glyph()
            {
                self.ed.notice = Some(ToolNotice::error(READ_ONLY_MESSAGE));
                return EditOutcome::Consumed;
            }
        }
        match key.glyph() {
            KeyCode::Escape | KeyCode::Char('q') => self.leave_visual(),
            KeyCode::Char('v') => match selection.linewise {
                true => self.start_visual(false),
                false => self.leave_visual(),
            },
            KeyCode::Char('V') => match selection.linewise {
                true => self.leave_visual(),
                false => self.start_visual(true),
            },
            KeyCode::Char('o') => {
                let anchor = self.ed.anchor.take();
                if let Some(anchor) = anchor {
                    self.ed.anchor = Some(Anchor {
                        col: self.buffer().col(),
                        linewise: anchor.linewise,
                        row: self.buffer().row(),
                    });
                    self.buffer_mut().move_to(anchor.row, anchor.col);
                }
                EditOutcome::Consumed
            }
            KeyCode::Char('d') | KeyCode::Char('x') => {
                self.operate_on_selection(selection, OperatorKind::Delete)
            }
            KeyCode::Char('c') | KeyCode::Char('s') => {
                self.operate_on_selection(selection, OperatorKind::Change)
            }
            KeyCode::Char('y') => self.operate_on_selection(selection, OperatorKind::Yank),
            KeyCode::Char('>') => self.operate_on_selection(selection, OperatorKind::Indent),
            KeyCode::Char('<') => self.operate_on_selection(selection, OperatorKind::Outdent),
            KeyCode::Char('J') => {
                self.buffer_mut().snapshot();
                self.buffer_mut().move_to(selection.start.0, 0);
                let rows = selection.end.0 - selection.start.0;
                self.buffer_mut().join_lines(rows.max(1));
                self.leave_visual()
            }
            KeyCode::Char('~') => {
                self.buffer_mut().snapshot();
                self.flip_selection(selection);
                self.leave_visual()
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                // What the selection held is not what goes back in its place,
                // so the register survives the delete that makes room.
                let register = self.buffer().register().clone();
                self.buffer_mut().snapshot();
                self.cut_selection(selection, false);
                self.buffer_mut().set_register(register);
                let linewise = self.buffer().register().linewise;
                self.buffer_mut().paste(linewise && selection.linewise);
                self.leave_visual()
            }
            KeyCode::Char('r') => {
                self.ed.pending = Some(Pending::Replace);
                EditOutcome::Consumed
            }
            KeyCode::Char('i') | KeyCode::Char('a') => {
                let around = key.glyph() == KeyCode::Char('a');
                self.ed.pending = Some(Pending::Object(ObjectWait { around }));
                EditOutcome::Consumed
            }
            KeyCode::Char('"') => {
                self.ed.pending = Some(Pending::Register);
                EditOutcome::Consumed
            }
            KeyCode::Char('f') | KeyCode::Char('F') | KeyCode::Char('t') | KeyCode::Char('T') => {
                self.ed.pending = Some(Pending::Find(find_wait(key)));
                EditOutcome::Consumed
            }
            KeyCode::Char('g') => {
                self.ed.pending = Some(Pending::Goto);
                EditOutcome::Consumed
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                let reverse = key.glyph() == KeyCode::Char('N');
                self.search_step(reverse)
            }
            _ => self.shared_motion(key),
        }
    }

    /// Act on what is selected, and stop selecting: an operator is the end of
    /// a Visual run whichever one it is.
    fn operate_on_selection(&mut self, selection: Selection, kind: OperatorKind) -> EditOutcome {
        let register = self.ed.register.take();
        self.buffer_mut().snapshot();
        match kind {
            OperatorKind::Indent | OperatorKind::Outdent => {
                self.buffer_mut().move_to(selection.start.0, 0);
                let rows = selection.end.0 - selection.start.0 + 1;
                self.buffer_mut()
                    .shift_lines(rows, kind == OperatorKind::Indent);
                self.leave_visual()
            }
            OperatorKind::Yank => {
                self.cut_selection(selection, true);
                let outcome = self.store_register(register, true);
                self.leave_visual();
                outcome
            }
            OperatorKind::Delete => {
                self.cut_selection(selection, false);
                let outcome = self.store_register(register, false);
                self.leave_visual();
                outcome
            }
            OperatorKind::Change => {
                match selection.linewise {
                    true => {
                        self.buffer_mut().move_to(selection.start.0, 0);
                        self.buffer_mut()
                            .clear_lines(selection.end.0 - selection.start.0 + 1);
                    }
                    false => self.cut_selection(selection, false),
                }
                let outcome = self.store_register(register, false);
                self.ed.anchor = None;
                self.ed.mode = EditMode::Insert;
                outcome
            }
        }
    }

    /// Take the selected text into the register, removing it unless `keep`.
    fn cut_selection(&mut self, selection: Selection, keep: bool) {
        if selection.linewise {
            self.buffer_mut()
                .take_rows(selection.start.0, selection.end.0, keep);
            return;
        }
        // The cursor's own cell is inside the selection, so the range runs one
        // past it: a Visual selection is inclusive at both ends.
        let end = (selection.end.0, selection.end.1 + 1);
        self.buffer_mut().take_between(selection.start, end, keep);
    }

    /// Flip the case of every character selected (`~`).
    fn flip_selection(&mut self, selection: Selection) {
        for row in selection.start.0..=selection.end.0 {
            let (from, to) = self.selected_on(selection, row);
            if from >= to {
                continue;
            }
            self.buffer_mut().move_to(row, from);
            self.buffer_mut().toggle_case(to - from);
        }
        self.buffer_mut()
            .move_to(selection.start.0, selection.start.1);
    }

    /// Write one character over every character selected (`r`).
    fn fill_selection(&mut self, selection: Selection, c: char) {
        for row in selection.start.0..=selection.end.0 {
            let (from, to) = self.selected_on(selection, row);
            for col in from..to {
                self.buffer_mut().move_to(row, col);
                self.buffer_mut().replace_char(c);
            }
        }
        self.buffer_mut()
            .move_to(selection.start.0, selection.start.1);
    }

    /// The half-open range of columns `selection` covers on `row`.
    fn selected_on(&self, selection: Selection, row: usize) -> (usize, usize) {
        let len = self.buffer().line_len_of(row);
        let from = match row == selection.start.0 && !selection.linewise {
            true => selection.start.1,
            false => 0,
        };
        let to = match row == selection.end.0 && !selection.linewise {
            true => (selection.end.1 + 1).min(len),
            false => len,
        };
        (from, to)
    }
}

// ========================================================================
// Session: pending commands
// ========================================================================

impl Session<'_> {
    /// The second key of a command: the character `r` writes, the motion an
    /// operator reaches over, the object it takes, or the letter naming a
    /// register or a mark.
    fn resolve_pending(&mut self, pending: Pending, key: &Key) -> EditOutcome {
        self.ed.pending = None;
        if key.glyph() == KeyCode::Escape {
            self.ed.operator = None;
            self.ed.count = None;
            return EditOutcome::Consumed;
        }
        match pending {
            // `]b` and `[b` step through the open files, the way they step
            // through the blocks of a shell's output.
            Pending::Bracket(wait) => match key.glyph() {
                KeyCode::Char('b') => EditOutcome::StepBuffer {
                    forward: wait.forward,
                },
                _ => EditOutcome::Consumed,
            },
            Pending::Find(wait) => self.resolve_find(wait, key),
            Pending::Goto => self.resolve_goto(key),
            Pending::Jump(jump) => self.jump_to_mark(jump, key),
            Pending::Leave => match table_entry(LEAVE, key) {
                Some(save) => EditOutcome::Quit { save },
                None => EditOutcome::Consumed,
            },
            Pending::Mark => {
                if let KeyCode::Char(c) = key.glyph() {
                    let at = (self.buffer().row(), self.buffer().col());
                    self.view.marks.named.insert(c, at);
                }
                EditOutcome::Consumed
            }
            Pending::Motion => self.resolve_motion(key),
            Pending::Object(wait) => self.resolve_object(wait, key),
            Pending::Register => {
                if let KeyCode::Char(c) = key.glyph() {
                    self.ed.register = Some(c);
                    if c == '_' {
                        self.ed.held = Some(self.buffer().register().clone());
                    }
                }
                EditOutcome::Consumed
            }
            Pending::Replace => self.resolve_replace(key),
            Pending::Scroll => {
                let at = match table_entry(SCROLL, key) {
                    Some(ScrollTo::Middle) => self.view.viewport / HALF_PAGE,
                    Some(ScrollTo::Top) => 0,
                    Some(ScrollTo::Bottom) => self.view.viewport.saturating_sub(1),
                    None => return EditOutcome::Consumed,
                };
                *self.view.scroll = self.buffer().row().saturating_sub(at);
                EditOutcome::Consumed
            }
        }
    }

    /// `r`: write one character over the one under the cursor, or over every
    /// character of a selection.
    fn resolve_replace(&mut self, key: &Key) -> EditOutcome {
        let c = match key.glyph() {
            KeyCode::Char(c) => c,
            KeyCode::Space => ' ',
            _ => return EditOutcome::Consumed,
        };
        self.buffer_mut().snapshot();
        match self.selection() {
            Some(selection) => {
                self.fill_selection(selection, c);
                self.leave_visual()
            }
            None => {
                self.buffer_mut().replace_char(c);
                EditOutcome::Consumed
            }
        }
    }

    /// The `g` commands the engine carries, as [`GOTO`] lists them.
    fn resolve_goto(&mut self, key: &Key) -> EditOutcome {
        let motion = match table_entry(GOTO, key) {
            Some(Goto::Motion(motion)) => motion,
            Some(Goto::Buffers) => {
                self.ed.operator = None;
                return EditOutcome::ListBuffers;
            }
            None => {
                self.ed.operator = None;
                return EditOutcome::Consumed;
            }
        };
        let typed = self.ed.count.take();
        self.jump_for(motion);
        match self.ed.operator.take() {
            Some(operator) => self.operate_over(operator, motion, typed),
            None => {
                self.apply_motion(motion, typed);
                EditOutcome::Consumed
            }
        }
    }

    /// The character a char search was waiting for, which now stands as the
    /// search `;` and `,` repeat.
    fn resolve_find(&mut self, wait: FindWait, key: &Key) -> EditOutcome {
        let ch = match key.glyph() {
            KeyCode::Char(ch) => ch,
            KeyCode::Space => ' ',
            _ => {
                self.ed.operator = None;
                return EditOutcome::Consumed;
            }
        };
        let find = FindChar {
            ch,
            forward: wait.forward,
            till: wait.till,
        };
        self.ed.find = Some(find);
        self.run_find(find)
    }

    /// The last char search again (`;`), or the other way (`,`).
    fn repeat_find(&mut self, reverse: bool) -> EditOutcome {
        let Some(find) = self.ed.find else {
            return EditOutcome::Consumed;
        };
        let find = match reverse {
            true => find.reversed(),
            false => find,
        };
        self.run_find(find)
    }

    /// Move to where a char search lands, or act over it where an operator is
    /// waiting on one.
    fn run_find(&mut self, find: FindChar) -> EditOutcome {
        let count = self.take_count();
        let operator = self.ed.operator.take();
        let mut probe = self.buffer().clone();
        if !probe.move_to_char(find, count) {
            self.ed.notice = Some(ToolNotice::error(format!("no `{}` on this line", find.ch)));
            return EditOutcome::Consumed;
        }
        let to = (probe.row(), probe.col());
        match operator {
            Some(operator) => self.operate(
                operator,
                Reach {
                    // A char search takes the cell it stops on, which is what
                    // makes `df,` reach through the comma and `dt,` up to it.
                    inclusive: true,
                    linewise: false,
                    to,
                },
            ),
            None => {
                self.buffer_mut().move_to(to.0, to.1);
                EditOutcome::Consumed
            }
        }
    }

    /// Go to a mark (`'a`, `` `a ``), or back to where the last jump started
    /// (`''`).
    fn jump_to_mark(&mut self, jump: MarkJump, key: &Key) -> EditOutcome {
        let KeyCode::Char(c) = key.glyph() else {
            return EditOutcome::Consumed;
        };
        let at = match c {
            '\'' | '`' => self.view.marks.jump,
            _ => self.view.marks.named.get(&c).copied(),
        };
        let Some((row, col)) = at else {
            self.ed.notice = Some(ToolNotice::error(format!("mark `{c}` is not set")));
            return EditOutcome::Consumed;
        };
        self.push_jump();
        match jump.exact {
            true => self.buffer_mut().move_to(row, col),
            false => self.buffer_mut().move_to_line(row),
        }
        EditOutcome::Consumed
    }
}

// ========================================================================
// Session: operators
// ========================================================================

impl Session<'_> {
    /// An operator with the count and register typed before it.
    fn operator_of(&mut self, kind: OperatorKind) -> Operator {
        Operator {
            count: self.take_count(),
            kind,
            register: self.ed.register.take(),
        }
    }

    /// Open an operator, which now waits for the motion or object saying how
    /// far it reaches.
    fn start_operator(&mut self, kind: OperatorKind) -> EditOutcome {
        self.ed.operator = Some(self.operator_of(kind));
        self.ed.pending = Some(Pending::Motion);
        EditOutcome::Consumed
    }

    /// The key after an operator: its own key doubles it over whole lines, an
    /// `i` or `a` opens a text object, and anything else is its motion.
    fn resolve_motion(&mut self, key: &Key) -> EditOutcome {
        let Some(operator) = self.ed.operator else {
            return EditOutcome::Consumed;
        };
        if self.take_digit(key) {
            self.ed.pending = Some(Pending::Motion);
            return EditOutcome::Consumed;
        }
        match key.glyph() {
            KeyCode::Char('i') | KeyCode::Char('a') => {
                let around = key.glyph() == KeyCode::Char('a');
                self.ed.pending = Some(Pending::Object(ObjectWait { around }));
                return EditOutcome::Consumed;
            }
            KeyCode::Char('f') | KeyCode::Char('F') | KeyCode::Char('t') | KeyCode::Char('T') => {
                self.ed.pending = Some(Pending::Find(find_wait(key)));
                return EditOutcome::Consumed;
            }
            KeyCode::Char('g') => {
                self.ed.pending = Some(Pending::Goto);
                return EditOutcome::Consumed;
            }
            _ => {}
        }
        self.ed.operator = None;
        // A count either side of the operator counts: `2d3w` is six words.
        let typed = match (self.ed.count.take(), operator.count) {
            (Some(count), one) => Some(count * one),
            (None, one) if one > 1 => Some(one),
            (None, _) => None,
        };
        if key.glyph() == doubled_key(operator.kind) {
            let count = typed.unwrap_or(1);
            return self.operate_on_lines(Operator { count, ..operator });
        }
        let Some(motion) = bare_motion(key) else {
            return EditOutcome::Consumed;
        };
        self.operate_over(operator, motion, typed)
    }

    /// Act over what a motion covers from the cursor.
    fn operate_over(
        &mut self,
        operator: Operator,
        motion: CursorMove,
        typed: Option<usize>,
    ) -> EditOutcome {
        let count = typed.unwrap_or(1);
        let mut probe = self.buffer().clone();
        move_cursor(
            &mut probe,
            motion,
            typed,
            self.view.viewport.max(1),
            *self.view.scroll,
        );
        let (linewise, mut inclusive) = motion_kind(motion);
        let mut to = (probe.row(), probe.col());
        // `cw` stops at the end of the word rather than the start of the next,
        // so that changing a word does not swallow the space after it.
        let word_motion = match motion {
            CursorMove::WordForward => Some(false),
            CursorMove::WordForwardBig => Some(true),
            _ => None,
        };
        if let (OperatorKind::Change, Some(big)) = (operator.kind, word_motion) {
            let mut word = self.buffer().clone();
            word.move_word_end(count, big);
            to = (word.row(), word.col());
            inclusive = true;
        } else if let Some(big) = word_motion {
            // Vim's other `w` rule: from the last word on a line an operator
            // reaches to the line's end, not into the next line, and not one
            // short of the last character.
            let here = (self.buffer().row(), self.buffer().col());
            let last_word =
                count == 1 && next_word_start(&self.buffer().chars(), here.1, big).is_none();
            if last_word || to.0 != here.0 {
                to = (here.0, self.buffer().line_len());
                inclusive = false;
            }
        }
        self.operate(
            operator,
            Reach {
                inclusive,
                linewise,
                to,
            },
        )
    }

    /// Act over everything between the cursor and where a motion or an object
    /// ended.
    fn operate(&mut self, operator: Operator, reach: Reach) -> EditOutcome {
        self.buffer_mut().snapshot();
        let here = (self.buffer().row(), self.buffer().col());
        if reach.linewise {
            let (first, last) = order(here.0, reach.to.0);
            return self.operate_on_rows(operator, first, last);
        }
        let (start, end) = match here <= reach.to {
            true => (here, reach.to),
            false => (reach.to, here),
        };
        let end = match reach.inclusive {
            true => (end.0, end.1 + 1),
            false => end,
        };
        match operator.kind {
            OperatorKind::Indent | OperatorKind::Outdent => {
                let (first, last) = order(here.0, reach.to.0);
                self.operate_on_rows(operator, first, last)
            }
            OperatorKind::Yank => {
                self.buffer_mut().take_between(start, end, true);
                self.store_register(operator.register, true)
            }
            OperatorKind::Delete => {
                self.buffer_mut().take_between(start, end, false);
                self.store_register(operator.register, false)
            }
            OperatorKind::Change => {
                self.buffer_mut().take_between(start, end, false);
                let outcome = self.store_register(operator.register, false);
                self.ed.mode = EditMode::Insert;
                outcome
            }
        }
    }

    /// The doubled form: `dd`, `yy`, `cc`, `>>`, `<<`.
    fn operate_on_lines(&mut self, operator: Operator) -> EditOutcome {
        self.buffer_mut().snapshot();
        let first = self.buffer().row();
        let last = (first + operator.count.max(1) - 1).min(self.buffer().lines().len() - 1);
        self.operate_on_rows(operator, first, last)
    }

    /// Act on whole rows, which is what every linewise operator comes down to.
    fn operate_on_rows(&mut self, operator: Operator, first: usize, last: usize) -> EditOutcome {
        match operator.kind {
            OperatorKind::Indent | OperatorKind::Outdent => {
                self.buffer_mut().move_to(first, 0);
                self.buffer_mut()
                    .shift_lines(last - first + 1, operator.kind == OperatorKind::Indent);
                EditOutcome::Consumed
            }
            OperatorKind::Yank => {
                self.buffer_mut().take_rows(first, last, true);
                self.store_register(operator.register, true)
            }
            OperatorKind::Delete => {
                self.buffer_mut().take_rows(first, last, false);
                self.store_register(operator.register, false)
            }
            OperatorKind::Change => {
                self.buffer_mut().move_to(first, 0);
                self.buffer_mut().clear_lines(last - first + 1);
                let outcome = self.store_register(operator.register, false);
                self.ed.mode = EditMode::Insert;
                outcome
            }
        }
    }

    /// `C` and `D`: change or delete to the end of the line.
    fn line_tail(&mut self, operator: Operator) -> EditOutcome {
        let to = (self.buffer().row(), self.buffer().line_len());
        self.operate(
            operator,
            Reach {
                inclusive: false,
                linewise: false,
                to,
            },
        )
    }

    /// Put what a yank or a delete took where the register prefix asked for,
    /// the way Vim files it. The clipboard registers leave the engine, so a
    /// yank there reaches every other window; the black hole keeps nothing,
    /// not even in the unnamed register; a capital appends to its register.
    /// With no prefix, a yank also goes to `0`, a delete of lines or across
    /// them to `1` (shifting the older ones up to `9`), a smaller one to `-`,
    /// and either to the kill ring, so `C-y` reaches it.
    fn store_register(&mut self, register: Option<char>, yank: bool) -> EditOutcome {
        let taken = self.buffer().register().clone();
        match register {
            Some('_') => {
                if let Some(held) = self.ed.held.take() {
                    self.buffer_mut().set_register(held);
                }
            }
            Some('+') | Some('*') => return EditOutcome::Yank(taken.text),
            Some(c) if c.is_ascii_uppercase() => {
                let lower = c.to_ascii_lowercase();
                let joined = match self.ed.registers.remove(&lower) {
                    Some(old) if old.linewise || taken.linewise => Register {
                        linewise: true,
                        text: format!("{}\n{}", old.text, taken.text),
                    },
                    Some(old) => Register {
                        linewise: false,
                        text: format!("{}{}", old.text, taken.text),
                    },
                    None => taken,
                };
                self.ed.registers.insert(lower, joined);
            }
            Some(c) => {
                self.ed.registers.insert(c, taken);
            }
            None => {
                if yank {
                    self.ed.registers.insert('0', taken.clone());
                } else if taken.linewise || taken.text.contains('\n') {
                    for pair in NUMBERED_REGISTERS.windows(2).rev() {
                        if let Some(older) = self.ed.registers.remove(&pair[0]) {
                            self.ed.registers.insert(pair[1], older);
                        }
                    }
                    self.ed
                        .registers
                        .insert(NUMBERED_REGISTERS[0], taken.clone());
                } else {
                    self.ed.registers.insert('-', taken.clone());
                }
                self.ed.kills.kill(taken, Merge::New);
            }
        }
        EditOutcome::Consumed
    }

    /// Store what a bare `x` or `X` took, which has no operator to carry the
    /// register prefix for it.
    fn store_take(&mut self) -> EditOutcome {
        let register = self.ed.register.take();
        self.store_register(register, false)
    }

    /// Put a register's text back (`p`/`P`), from the clipboard where the
    /// prefix named it, which the host has to read for us.
    fn paste(&mut self, after: bool) -> EditOutcome {
        let count = self.take_count();
        match self.ed.register.take() {
            Some('+') | Some('*') => return EditOutcome::Paste,
            Some('_') => return EditOutcome::Consumed,
            Some(c) => match self.ed.registers.get(&c.to_ascii_lowercase()).cloned() {
                Some(register) => self.buffer_mut().set_register(register),
                None => {
                    self.ed.notice = Some(ToolNotice::error(format!("register `{c}` is empty")));
                    return EditOutcome::Consumed;
                }
            },
            None => {}
        }
        self.buffer_mut().snapshot();
        for _ in 0..count {
            self.buffer_mut().paste(after);
        }
        EditOutcome::Consumed
    }

    /// Put text read out of the clipboard in at the cursor: after the
    /// character under it in Normal mode, and at the caret while typing, which
    /// it then steps past the way typing it would.
    fn paste_text(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        // Text that ends in a newline is whole lines, however it was copied.
        let linewise = text.ends_with('\n');
        let kept = self.buffer().register().clone();
        self.buffer_mut().set_register(Register {
            linewise,
            text: text.trim_end_matches('\n').to_string(),
        });
        self.buffer_mut().snapshot();
        // Whole lines go in below the cursor's whichever the mode; text goes in
        // at the caret while typing, and after the character under it else.
        let at_caret = self.ed.is_typing() && (!linewise || self.buffer().is_fixed());
        self.buffer_mut().paste(!at_caret);
        if at_caret {
            self.buffer_mut().move_right(1, true);
        }
        self.buffer_mut().set_register(kept);
    }
}

// ========================================================================
// Session: text objects
// ========================================================================

impl Session<'_> {
    /// The object key after an `i` or an `a`: it either says how far a waiting
    /// operator reaches, or what a Visual selection now covers.
    fn resolve_object(&mut self, wait: ObjectWait, key: &Key) -> EditOutcome {
        let operator = self.ed.operator.take();
        let Some(object) = TextObject::of_key(key.glyph()) else {
            return EditOutcome::Consumed;
        };
        let Some((span, linewise)) = self.object_span(object, wait.around) else {
            self.ed.notice = Some(ToolNotice::error("no object here"));
            return EditOutcome::Consumed;
        };
        if let Some(operator) = operator {
            self.buffer_mut().move_to(span.start.0, span.start.1);
            return self.operate(
                operator,
                Reach {
                    inclusive: true,
                    linewise,
                    to: span.end,
                },
            );
        }
        // In Visual mode the object becomes the selection.
        self.ed.anchor = Some(Anchor {
            col: span.start.1,
            linewise: linewise || self.ed.anchor.is_some_and(|anchor| anchor.linewise),
            row: span.start.0,
        });
        self.buffer_mut().move_to(span.end.0, span.end.1);
        EditOutcome::Consumed
    }

    /// Where an object at the cursor starts and ends, and whether it covers
    /// whole lines.
    fn object_span(&self, object: TextObject, around: bool) -> Option<(Span, bool)> {
        let row = self.buffer().row();
        let col = self.buffer().col();
        let line = self.buffer().chars();
        let over_line = |range: Option<(usize, usize)>| {
            range.map(|(start, end)| {
                (
                    Span {
                        end: (row, end),
                        start: (row, start),
                    },
                    false,
                )
            })
        };
        match object {
            TextObject::Word => over_line(word_object(&line, col, false, around)),
            TextObject::WordBig => over_line(word_object(&line, col, true, around)),
            TextObject::Quotes(quote) => over_line(quote_object(&line, col, quote, around)),
            TextObject::Sentence => over_line(sentence_object(&line, col, around)),
            TextObject::Brackets(open, close) => {
                bracket_object(self.buffer(), (row, col), open, close, around)
                    .map(|span| (span, false))
            }
            TextObject::Paragraph => paragraph_object(self.buffer(), row, around).map(|(a, b)| {
                (
                    Span {
                        end: (b, 0),
                        start: (a, 0),
                    },
                    true,
                )
            }),
        }
    }
}

// ========================================================================
// Session: search
// ========================================================================

impl Session<'_> {
    /// The next match of the last search (`n`), or the one the other way
    /// (`N`).
    fn search_step(&mut self, reverse: bool) -> EditOutcome {
        let Some(search) = self.ed.search.clone() else {
            self.ed.notice = Some(ToolNotice::error("nothing has been searched for"));
            return EditOutcome::Consumed;
        };
        let forward = search.forward != reverse;
        let at = (self.buffer().row(), self.buffer().col());
        match search.next(self.buffer().lines(), at, forward) {
            Some((row, col)) => {
                self.push_jump();
                self.buffer_mut().move_to(row, col);
            }
            None => {
                self.ed.notice = Some(ToolNotice::error(format!(
                    "`{}` is not in the file",
                    search.pattern
                )))
            }
        }
        EditOutcome::Consumed
    }

    /// Look for the word under the cursor (`*`/`#`).
    fn search_word(&mut self, forward: bool) -> EditOutcome {
        let line = self.buffer().chars();
        let Some((start, end)) = word_object(&line, self.buffer().col(), false, false) else {
            return EditOutcome::Consumed;
        };
        let word: String = line[start..=end].iter().collect();
        if word.trim().is_empty() {
            return EditOutcome::Consumed;
        }
        self.ed.search = Some(Search::new(word, forward));
        self.search_step(false)
    }
}

// ========================================================================
// Session: motions
// ========================================================================

impl Session<'_> {
    /// Move the cursor as `motion` says, in the terms the view gives them.
    fn apply_motion(&mut self, motion: CursorMove, typed: Option<usize>) {
        let row = self.buffer().row();
        let viewport = self.view.viewport;
        match motion {
            // The scroll placements move the window rather than the cursor,
            // so they are the view's to answer, not the text's.
            CursorMove::LineToTop => *self.view.scroll = row,
            CursorMove::LineToCenter => {
                *self.view.scroll = row.saturating_sub(viewport / HALF_PAGE)
            }
            CursorMove::LineToBottom => {
                *self.view.scroll = row.saturating_sub(viewport.saturating_sub(1));
            }
            _ => {
                let (tool, scroll) = (viewport.max(1), *self.view.scroll);
                move_cursor(self.view.buffer, motion, typed, tool, scroll);
            }
        }
    }

    /// Remember where a jump started, so `''` can go back to it.
    fn push_jump(&mut self) {
        self.view.marks.jump = Some((self.buffer().row(), self.buffer().col()));
    }

    /// Remember the cursor ahead of the motions that carry the eye a long way.
    fn jump_for(&mut self, motion: CursorMove) {
        let jumps = matches!(
            motion,
            CursorMove::Bottom
                | CursorMove::MatchingBracket
                | CursorMove::PageDown
                | CursorMove::PageUp
                | CursorMove::ParagraphBack
                | CursorMove::ParagraphForward
                | CursorMove::Top
        );
        if jumps {
            self.push_jump();
        }
    }
}

// ========================================================================
// Helpers
// ========================================================================

/// Move `buffer`'s cursor as `motion` says. The screen motions and the paging
/// ones measure against the view, so they need what it is showing. `typed` is
/// the count that was typed before the motion, which a few of them read as
/// something other than how many times to do it.
fn move_cursor(
    buffer: &mut TextBuffer,
    motion: CursorMove,
    typed: Option<usize>,
    tool: usize,
    scroll: usize,
) {
    let count = typed.unwrap_or(1);
    match motion {
        CursorMove::Left => buffer.move_left(count),
        CursorMove::Right => buffer.move_right(count, false),
        CursorMove::Down => buffer.move_down(count, false),
        CursorMove::Up => buffer.move_up(count, false),
        CursorMove::LineStart => buffer.move_line_start(),
        CursorMove::LineEnd => buffer.move_line_end(false),
        CursorMove::FirstNonBlank => buffer.move_first_non_blank(),
        CursorMove::LastNonBlank => buffer.move_last_non_blank(),
        CursorMove::WordForward => buffer.move_word_forward(count, false),
        CursorMove::WordForwardBig => buffer.move_word_forward(count, true),
        CursorMove::WordBack => buffer.move_word_back(count, false),
        CursorMove::WordBackBig => buffer.move_word_back(count, true),
        CursorMove::WordEnd => buffer.move_word_end(count, false),
        CursorMove::WordEndBig => buffer.move_word_end(count, true),
        CursorMove::Top => buffer.move_top(),
        // `20G` is line twenty; `G` on its own is the last line.
        CursorMove::Bottom => match typed {
            Some(line) => buffer.move_to_line(line.saturating_sub(1)),
            None => buffer.move_bottom(),
        },
        CursorMove::PageDown => buffer.move_down(tool, false),
        CursorMove::PageUp => buffer.move_up(tool, false),
        CursorMove::HalfPageDown => buffer.move_down(tool / HALF_PAGE, false),
        CursorMove::HalfPageUp => buffer.move_up(tool / HALF_PAGE, false),
        CursorMove::ScreenTop => buffer.move_to_line(scroll),
        CursorMove::ScreenMiddle => buffer.move_to_line(scroll + tool / HALF_PAGE),
        CursorMove::ScreenBottom => buffer.move_to_line(scroll + tool.saturating_sub(1)),
        CursorMove::ParagraphBack => buffer.move_paragraph(count, false),
        CursorMove::ParagraphForward => buffer.move_paragraph(count, true),
        CursorMove::MatchingBracket => buffer.move_matching_bracket(),
        CursorMove::WordEndBack => buffer.move_word_end_back(count, false),
        CursorMove::WordEndBackBig => buffer.move_word_end_back(count, true),
        // Where the cursor's line sits in the view is the view's to answer,
        // since the text knows nothing of the window it is read through.
        CursorMove::LineToBottom | CursorMove::LineToCenter | CursorMove::LineToTop => {}
    }
}

/// What `key` stands for in one of the command tables.
fn table_entry<T: Copy>(table: &[(char, T, &str)], key: &Key) -> Option<T> {
    let KeyCode::Char(c) = key.glyph() else {
        return None;
    };
    table
        .iter()
        .find(|(entry, _, _)| *entry == c)
        .map(|(_, value, _)| *value)
}

/// A command table as the key hints list it.
fn table_hint<'t>(entries: impl Iterator<Item = (char, &'t str)>) -> Vec<(String, String)> {
    entries
        .map(|(key, what)| (key.to_string(), what.to_string()))
        .collect()
}

/// Whether `c` is part of a word for completion: letters, digits, `_`.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The word a completion has put in.
fn current(completion: &Completion) -> &str {
    completion
        .index
        .and_then(|at| completion.candidates.get(at))
        .map_or(completion.typed.as_str(), String::as_str)
}

/// The words `typed` could complete to, each once, nearest first: back along
/// the cursor's line and up the text, then on down it from the cursor, then
/// through `others`.
fn completion_candidates(
    lines: &[String],
    at: (usize, usize),
    typed: &str,
    others: &[&[String]],
) -> Vec<String> {
    let (row, col) = at;
    let words_of = |line: &str| -> Vec<String> {
        line.split(|c: char| !is_word_char(c))
            .filter(|word| word.len() > typed.len() && word.starts_with(typed))
            .map(str::to_string)
            .collect()
    };
    let line: Vec<char> = lines
        .get(row)
        .map(|l| l.chars().collect())
        .unwrap_or_default();
    let before: String = line[..col.min(line.len())].iter().collect();
    let after: String = line[col.min(line.len())..].iter().collect();
    let mut found: Vec<String> = Vec::new();
    let mut take = |words: Vec<String>| {
        for word in words {
            if !found.contains(&word) {
                found.push(word);
            }
        }
    };
    take(words_of(&before).into_iter().rev().collect());
    for line in lines[..row.min(lines.len())].iter().rev() {
        take(words_of(line).into_iter().rev().collect());
    }
    take(words_of(&after));
    for line in lines.iter().skip(row + 1) {
        take(words_of(line));
    }
    for text in others {
        for line in text.iter() {
            take(words_of(line));
        }
    }
    found
}

/// The direction and the stopping rule a char search key names.
fn find_wait(key: &Key) -> FindWait {
    FindWait {
        forward: matches!(key.glyph(), KeyCode::Char('f') | KeyCode::Char('t')),
        till: matches!(key.glyph(), KeyCode::Char('t') | KeyCode::Char('T')),
    }
}

/// The key that doubles an operator over whole lines.
fn doubled_key(kind: OperatorKind) -> KeyCode {
    match kind {
        OperatorKind::Change => KeyCode::Char('c'),
        OperatorKind::Delete => KeyCode::Char('d'),
        OperatorKind::Indent => KeyCode::Char('>'),
        OperatorKind::Outdent => KeyCode::Char('<'),
        OperatorKind::Yank => KeyCode::Char('y'),
    }
}

/// Whether a motion takes whole lines, and whether it takes the cell it lands
/// on with it. Vim's two classifications, which are what tell `dj` from `dw`
/// and `de` from `db`.
fn motion_kind(motion: CursorMove) -> (bool, bool) {
    use CursorMove as M;
    match motion {
        M::Bottom
        | M::Down
        | M::HalfPageDown
        | M::HalfPageUp
        | M::PageDown
        | M::PageUp
        | M::ScreenBottom
        | M::ScreenMiddle
        | M::ScreenTop
        | M::Top
        | M::Up => (true, false),
        M::LineEnd | M::LastNonBlank | M::MatchingBracket | M::WordEnd | M::WordEndBig => {
            (false, true)
        }
        M::FirstNonBlank
        | M::Left
        | M::LineStart
        | M::LineToBottom
        | M::LineToCenter
        | M::LineToTop
        | M::ParagraphBack
        | M::ParagraphForward
        | M::Right
        | M::WordBack
        | M::WordBackBig
        | M::WordEndBack
        | M::WordEndBackBig
        | M::WordForward
        | M::WordForwardBig => (false, false),
    }
}

/// Two rows in the order they come in the text.
fn order(one: usize, other: usize) -> (usize, usize) {
    match one <= other {
        true => (one, other),
        false => (other, one),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Where the cursor sits in a spec string: `"foo |bar"` is the cursor on
    /// the `b`, or before it while typing.
    const CURSOR: char = '|';

    /// An engine over a buffer, driven by key notation and checked against a
    /// spec of the text with the cursor marked in it.
    struct Harness {
        buffer: TextBuffer,
        marks: Marks,
        modal: ModalEditor,
        outcomes: Vec<EditOutcome>,
        scroll: usize,
    }

    impl Harness {
        fn new(spec: &str) -> Self {
            let (lines, (row, col)) = parse_spec(spec);
            let mut buffer = TextBuffer::new(lines);
            buffer.move_to(row, col);
            Self {
                buffer,
                marks: Marks::default(),
                modal: ModalEditor::new(),
                outcomes: Vec::new(),
                scroll: 0,
            }
        }

        /// Press `keys`, written as space-separated characters and `<...>`
        /// names: `<esc>`, `<cr>`, `<bs>`, `<C-k>`, `<M-y>`.
        fn keys(&mut self, keys: &str) -> &mut Self {
            for key in keys.split_whitespace().map(parse_key) {
                let view = EditView {
                    buffer: &mut self.buffer,
                    marks: &mut self.marks,
                    others: &[],
                    scroll: &mut self.scroll,
                    viewport: 10,
                };
                let outcome = self.modal.on_key(view, &key);
                self.outcomes.push(outcome);
            }
            self
        }

        fn assert(&self, spec: &str) {
            let mut lines: Vec<String> = self.buffer.lines().to_vec();
            let (row, col) = (self.buffer.row(), self.buffer.col());
            let mut chars: Vec<char> = lines[row].chars().collect();
            chars.insert(col.min(chars.len()), CURSOR);
            lines[row] = chars.into_iter().collect();
            assert_eq!(lines.join("\n"), spec);
        }

        fn register(&self, name: char) -> Option<&str> {
            self.modal.registers.get(&name).map(|r| r.text.as_str())
        }
    }

    fn parse_spec(spec: &str) -> (Vec<String>, (usize, usize)) {
        let mut at = (0, 0);
        let lines = spec
            .split('\n')
            .enumerate()
            .map(|(row, line)| {
                if let Some(col) = line.chars().position(|c| c == CURSOR) {
                    at = (row, col);
                }
                line.chars().filter(|c| *c != CURSOR).collect()
            })
            .collect();
        (lines, at)
    }

    fn parse_key(token: &str) -> Key {
        let plain = |code| Key {
            alt: false,
            code,
            ctrl: false,
            shift: false,
            sup: false,
        };
        let Some(name) = token.strip_prefix('<').and_then(|t| t.strip_suffix('>')) else {
            return plain(KeyCode::Char(token.chars().next().unwrap_or(' ')));
        };
        match name {
            "esc" => plain(KeyCode::Escape),
            "cr" => plain(KeyCode::Enter),
            "bs" => plain(KeyCode::Backspace),
            "space" => plain(KeyCode::Char(' ')),
            _ => {
                let (modifier, rest) = name.split_at(2);
                let code = match rest {
                    "bs" => KeyCode::Backspace,
                    _ => KeyCode::Char(rest.chars().next().unwrap_or(' ')),
                };
                Key {
                    alt: modifier == "M-",
                    ctrl: modifier == "C-",
                    ..plain(code)
                }
            }
        }
    }

    #[test]
    fn test_the_harness_drives_an_operator_and_reads_the_cursor_back() {
        Harness::new("foo |bar baz").keys("d w").assert("foo |baz");
    }

    #[test]
    fn test_consecutive_kills_yank_back_as_one_and_a_motion_ends_the_run() {
        let mut ed = Harness::new("|one two three");
        ed.keys("i <M-d> <M-d>").assert("| three");
        ed.keys("<C-y>").assert("one two| three");

        // A motion between kills starts a new entry, and `M-y` reaches back
        // past it to the run before.
        ed.keys("<C-e> <M-bs> <C-a> <M-d> <C-y>")
            .assert("one| two ");
        ed.keys("<M-y>").assert("three| two ");
        ed.keys("<M-y>").assert("one two| two ");
    }

    #[test]
    fn test_kill_to_line_end_twice_takes_the_line_and_its_break() {
        let mut ed = Harness::new("|abc\ndef");
        ed.keys("i <C-k> <C-k>").assert("|def");
        ed.keys("<C-y>").assert("abc\n|def");
    }

    #[test]
    fn test_the_black_hole_keeps_nothing_and_leaves_the_unnamed_register_alone() {
        let mut ed = Harness::new("|keep\ndrop\nend");
        ed.keys("y y j \" _ d d").assert("keep\n|end");
        ed.keys("p").assert("keep\nend\n|keep");
    }

    #[test]
    fn test_deletes_fill_the_numbered_and_small_registers_and_yanks_fill_zero() {
        let mut ed = Harness::new("|one\ntwo\nthree word");
        ed.keys("y y d d d d");
        assert_eq!(ed.register('0'), Some("one"), "the yank");
        assert_eq!(ed.register('1'), Some("two"), "the newest big delete");
        assert_eq!(ed.register('2'), Some("one"), "shifted up");
        ed.keys("d w");
        assert_eq!(ed.register('-'), Some("three "), "a delete inside a line");
        assert_eq!(
            ed.register('1'),
            Some("two"),
            "which leaves the numbered alone"
        );
    }

    #[test]
    fn test_a_capital_register_appends_to_its_lower_case_one() {
        let mut ed = Harness::new("|ab cd");
        ed.keys("\" a y w w \" A y w");
        assert_eq!(ed.register('a'), Some("ab cd"));
        ed.keys("0 \" a P").assert("ab c|dab cd");
    }

    #[test]
    fn test_alt_slash_completes_from_the_nearest_word_and_cycles_back_to_what_was_typed() {
        let mut ed = Harness::new("alpha\nalbum |");
        ed.keys("a a l <M-/>").assert("alpha\nalbum album|");
        ed.keys("<M-/>").assert("alpha\nalbum alpha|");
        ed.keys("<M-/>").assert("alpha\nalbum al|");
    }

    #[test]
    fn test_every_listed_goto_key_does_something_and_none_is_left_unlisted() {
        // The hints and the keys read one table, so a `g` command the hints
        // name is one the keys carry out.
        let mut ed = Harness::new("one two\nthree |four");
        ed.keys("g");
        let hinted: Vec<String> = ed
            .modal
            .hint()
            .unwrap_or_default()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert!(
            hinted.contains(&"E".to_string()),
            "gE is listed: {hinted:?}"
        );
        ed.keys("E").assert("one two\nthre|e four");
    }

    #[test]
    fn test_read_only_editor_allows_motions_and_yanks_but_rejects_edits() {
        let mut ed = Harness::new("|hello world");
        ed.modal.set_read_only(true);
        ed.keys("w").assert("hello |world");
        ed.keys("x");
        assert_eq!(
            ed.modal.take_notice(),
            Some(ToolNotice::error(READ_ONLY_MESSAGE))
        );
        ed.assert("hello |world");
        ed.keys("i");
        assert_eq!(
            ed.modal.take_notice(),
            Some(ToolNotice::error(READ_ONLY_MESSAGE))
        );
        assert_eq!(ed.modal.mode(), EditMode::Normal);
        ed.keys("y $");
        assert_eq!(
            ed.modal.registers.get(&'0').map(|r| r.text.as_str()),
            Some("world")
        );
    }
}
