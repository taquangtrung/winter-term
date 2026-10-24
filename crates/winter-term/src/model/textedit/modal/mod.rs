//! What a key does to the text: the Vim grammar of Normal, Visual and Replace,
//! and an Emacs-style Insert mode, since that is what the shell's own line is.
//!
//! The engine edits a [`TextBuffer`] it is lent one key at a time, through an
//! [`EditView`] that also carries the marks and the scroll of whatever the
//! text is shown in. Anything that is not an edit to the text (saving,
//! quitting, asking what to search for, the list of open files) comes back as
//! an [`EditOutcome`] for the host to carry out, so the same keys drive a file
//! in the editor, a column of names in a listing, and a one-line answer.
//!
//! The engine's state and its dispatch live here; what each mode does with a
//! key lives beside it, one file per part of the grammar:
//!
//! - [`normal`]: Normal mode.
//! - [`insert`]: Insert and Replace.
//! - [`visual`]: Visual mode.
//! - [`pending`]: the key a part-typed command was waiting on.
//! - [`operators`]: operators and the text objects they take.
//! - [`motions`]: searching, and moving by motions.
//!
//! What a key means in the grammar it shares with the terminal's own modes
//! (counts, operators, registers, marks) is [`crate::model::vim::grammar`]'s.

mod insert;
mod motions;
mod normal;
mod operators;
mod pending;
mod visual;

use std::collections::HashMap;

use crate::model::editor::{EditorStatus, Notice, SearchReport};
use crate::model::input::{Key, KeyCode};
use crate::model::textedit::buffer::{Register, TextBuffer};
use crate::model::textedit::kill::KillRing;
use crate::model::textedit::search::Search;
use crate::model::vim::grammar::{
    OperatorKind, BLACK_HOLE_REGISTER, GOTO_MOTIONS, LINE_PLACEMENTS,
};
use crate::model::vim::motion::{CursorMove, FindChar};
use crate::model::vim::nav::VimNav;

// ========================================================================
// Constants
// ========================================================================

/// What a screenful means to the paging keys, as a fraction of the view.
const HALF_PAGE: usize = 2;

/// How many lines a screen-wise Emacs scroll keeps in view, to keep the place.
const PAGE_OVERLAP: usize = 2;

/// The registers only an editor of text has, beside the shared ones: the
/// black hole, and the last small delete (`-`).
const EDITOR_REGISTERS: [char; 2] = [BLACK_HOLE_REGISTER, '-'];

/// The numbered registers a big delete shifts through, newest in `1`.
const NUMBERED_REGISTERS: [char; 9] = ['1', '2', '3', '4', '5', '6', '7', '8', '9'];

/// The `g` command the editor adds to the [`GOTO_MOTIONS`] every surface
/// shares: the list of the open files.
const GOTO_BUFFERS: (char, &str) = ('b', "list the open files");

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
    notice: Option<Notice>,
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
    /// Whether a search ran or repeated since the host last asked.
    searched: bool,
    /// Whether the last key typed a character into a field, so the next one
    /// joins its undo step: a run of typing walks back a word at a time.
    typing_run: bool,
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
                let shared = GOTO_MOTIONS.iter().map(|(key, _, what)| (*key, *what));
                return Some(table_hint(std::iter::once(GOTO_BUFFERS).chain(shared)));
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
                    LINE_PLACEMENTS.iter().map(|(key, _, what)| (*key, *what)),
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

    /// What an editor editing `buffer` with these keys puts in the status bar:
    /// the mode, where the cursor is, `total` saying how much there is, and
    /// the register a `"` named for the next command.
    pub fn editor_status(&self, buffer: &TextBuffer, total: String) -> EditorStatus {
        let mut segments = vec![format!("{}:{}", buffer.row() + 1, buffer.col() + 1), total];
        segments.extend(self.register.map(|register| format!("\"{register}")));
        EditorStatus {
            mode: Some(self.mode),
            segments,
        }
    }

    /// Report `notice`, as the host does for what it carried out itself.
    pub fn report(&mut self, notice: Notice) {
        self.notice = Some(notice);
    }

    /// Take what the last command reported, for the status bar, leaving
    /// nothing reported.
    pub fn take_notice(&mut self) -> Option<Notice> {
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

    /// The search run or repeated since this was last asked, with the
    /// cursor's place among the matches in `buffer`.
    pub fn take_search(&mut self, buffer: &TextBuffer) -> Option<SearchReport> {
        if !std::mem::take(&mut self.searched) {
            return None;
        }
        let at = (buffer.row(), buffer.col());
        Some(self.search.as_ref()?.report(buffer.lines(), at))
    }

    /// Put text the host read out of the clipboard in at the cursor: the
    /// answer to an [`EditOutcome::Paste`], and what a pasted chord brings.
    pub fn paste_text(&mut self, view: EditView<'_>, text: String) {
        if self.read_only {
            self.notice = Some(Notice::error(READ_ONLY_MESSAGE));
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
            self.ed.notice = Some(Notice::error("nothing to repeat"));
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
    editor: usize,
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
        CursorMove::PageDown => buffer.move_down(editor, false),
        CursorMove::PageUp => buffer.move_up(editor, false),
        CursorMove::HalfPageDown => buffer.move_down(editor / HALF_PAGE, false),
        CursorMove::HalfPageUp => buffer.move_up(editor / HALF_PAGE, false),
        CursorMove::ScreenTop => buffer.move_to_line(scroll),
        CursorMove::ScreenMiddle => buffer.move_to_line(scroll + editor / HALF_PAGE),
        CursorMove::ScreenBottom => buffer.move_to_line(scroll + editor.saturating_sub(1)),
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
    use crate::model::vim::grammar::MAX_COUNT;

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
            Some(Notice::error(READ_ONLY_MESSAGE))
        );
        ed.assert("hello |world");
        ed.keys("i");
        assert_eq!(
            ed.modal.take_notice(),
            Some(Notice::error(READ_ONLY_MESSAGE))
        );
        assert_eq!(ed.modal.mode(), EditMode::Normal);
        ed.keys("y $");
        assert_eq!(
            ed.modal.registers.get(&'0').map(|r| r.text.as_str()),
            Some("world")
        );
    }

    #[test]
    fn test_a_count_typed_too_long_is_clamped_not_overflowed() {
        // Regression: the count multiplied without a cap, so twenty-odd
        // digits overflowed and a long one ran its command that many times.
        let mut h = Harness::new("|abc");
        h.keys(&"9 ".repeat(25));
        assert_eq!(h.modal.count, Some(MAX_COUNT));
        h.keys("x");
        h.assert("|");
    }

    #[test]
    fn test_counts_either_side_of_an_operator_multiply_within_the_cap() {
        let mut h = Harness::new("|a b c d e f");
        h.keys("2 d 3 w");
        h.assert("|f");
    }

    #[test]
    fn test_incrementing_the_largest_number_leaves_it_and_says_so() {
        // Regression: `i64::MAX` plus one overflowed (a panic in debug builds,
        // the most negative number in release).
        let mut h = Harness::new("|9223372036854775807");
        h.keys("<C-a>");
        h.assert("|9223372036854775807");
        h.keys("<C-x>");
        h.assert("922337203685477580|6");
    }

    #[test]
    fn test_a_key_that_names_no_register_or_mark_sets_none() {
        let mut h = Harness::new("|abc");
        h.keys("\" % y y");
        assert_eq!(h.register('%'), None);
        h.keys("m . ` .");
        assert!(h.marks.named.is_empty());
    }
}
