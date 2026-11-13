//! Diff: texts compared region by region, and merged, as Emacs' ediff
//! does it. Two texts side by side (or three, against the middle one), or
//! two merged into a third against the ancestor they share; the regions
//! they differ in stepped through with `n` and `p`, each copied from one
//! text to another (`a`, `b`, or `ab`, `ca` ... with three), a merge region
//! given one side, the other, both (`+`), or put back (`r`); whitespace and
//! case left out of the comparison (`##`, `#c`); the layout turned between
//! columns and bands (`|`); each text edited in place (`e` and its letter,
//! `C-c C-c` back) and written back (`w` and its letter).
//!
//! - `lines`: where texts differ, line by line.
//! - `model`: the texts and their regions.
//! - `paint`: how the session is drawn.

pub(crate) mod lines;
mod model;
mod paint;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use winter_model::editor::{
    menu_groups, present, CompareRequest, DiffRequest, Editor, EditorMenuItem, EditorOutcome,
    EditorStatus, Notice, Page, PromptMode, PromptReply, PromptRequest, ScrollPosition,
};
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed, TableMatch};
use winter_model::input::{format_sequence, Key};
use winter_model::list::Viewport;
use winter_model::paint::{Caret, Frame, IconKind, Point, Row, Span, Style};
use winter_model::text_file::{self, FileShape};
use winter_syntax::LineState;

use cherry::{DocumentId, Edit, Handled, HostRequest, Mode, NoticeLevel, ViewId};
use winter_cherry::clock;

use super::{edit_mode_of, keys, session};
use model::{Aligned, Kind, Session, Side, SideText, Variant, A, ANCESTOR, B, C};
use paint::{Column, Widths, DIVIDER};

// ========================================================================
// Constants
// ========================================================================

/// Prompt tags: the region to jump to, and leaving with edits unwritten.
/// Numbers each output diff's copies, so two diffs of files with the same
/// name, at once in this process, do not write over each other's.
static COPY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const ASK_JUMP: &str = "diff-jump";
const ASK_QUIT: &str = "diff-quit";

/// How many columns `<` and `>` scroll sideways.
const HSCROLL_STEP: usize = 8;

/// How far down the pane the current region is brought.
const REGION_LIFT: usize = 3;

/// The keys of every session, after the ones of its kind: stepping through
/// the regions, scrolling every text together, the view, and what is done to
/// one text named by its letter.
const KEYS: KeyTable<Act> = KeyTable {
    block: "diff",
    bindings: &[
        Binding {
            action: Act::Next,
            id: "next_region",
            keys: &["n", "] c"],
            label: "Next region",
        },
        Binding {
            action: Act::Previous,
            id: "previous_region",
            keys: &["p", "DEL", "[ c"],
            label: "Previous region",
        },
        Binding {
            action: Act::Jump,
            id: "jump_to_region",
            keys: &["J"],
            label: "Jump to region number\u{2026}",
        },
        Binding {
            action: Act::ScrollDown,
            id: "scroll_down",
            keys: &["j", "<down>", "C-e"],
            label: "Scroll down a line",
        },
        Binding {
            action: Act::ScrollUp,
            id: "scroll_up",
            keys: &["k", "<up>", "C-y"],
            label: "Scroll up a line",
        },
        Binding {
            action: Act::PageDown,
            id: "page_down",
            keys: &["SPC", "v", "<next>", "C-v", "C-f"],
            label: "Scroll down a page",
        },
        Binding {
            action: Act::PageUp,
            id: "page_up",
            keys: &["S-SPC", "V", "<prior>", "C-b", "M-v"],
            label: "Scroll up a page",
        },
        Binding {
            action: Act::HalfPageDown,
            id: "half_page_down",
            keys: &["C-d"],
            label: "Scroll down half a page",
        },
        Binding {
            action: Act::HalfPageUp,
            id: "half_page_up",
            keys: &["C-u"],
            label: "Scroll up half a page",
        },
        Binding {
            action: Act::Bottom,
            id: "bottom",
            keys: &["G", "<end>"],
            label: "Scroll to the end",
        },
        Binding {
            action: Act::Top,
            id: "top",
            keys: &["g g", "<home>"],
            label: "Scroll to the top",
        },
        Binding {
            action: Act::ScrollRight,
            id: "scroll_right",
            keys: &["<", "<right>"],
            label: "Scroll sideways, to the right",
        },
        Binding {
            action: Act::ScrollLeft,
            id: "scroll_left",
            keys: &[">", "<left>"],
            label: "Scroll sideways, to the left",
        },
        Binding {
            action: Act::Restore(A),
            id: "restore_a",
            keys: &["r a"],
            label: "Put the region in A back as it was",
        },
        Binding {
            action: Act::Restore(B),
            id: "restore_b",
            keys: &["r b"],
            label: "Put the region in B back as it was",
        },
        Binding {
            action: Act::Restore(C),
            id: "restore_c",
            keys: &["r c"],
            label: "Put the region in C back as it was",
        },
        Binding {
            action: Act::DiffPut,
            id: "diff_put",
            keys: &["d p"],
            label: "Copy the region from the text being edited to the other",
        },
        Binding {
            action: Act::DiffObtain,
            id: "diff_obtain",
            keys: &["d o"],
            label: "Copy the region from the other text into the one being edited",
        },
        Binding {
            action: Act::Recompute,
            id: "recompute",
            keys: &["!"],
            label: "Work the regions out again",
        },
        Binding {
            action: Act::ToggleWhitespace,
            id: "toggle_whitespace",
            keys: &["# #"],
            label: "Leave whitespace out of the comparison, or count it",
        },
        Binding {
            action: Act::ToggleCase,
            id: "toggle_case",
            keys: &["# c"],
            label: "Leave case out of the comparison, or count it",
        },
        Binding {
            action: Act::ToggleRefine,
            id: "toggle_changed_words",
            keys: &["@"],
            label: "Show the changed words of the current region, or not",
        },
        Binding {
            action: Act::ToggleLayout,
            id: "toggle_layout",
            keys: &["|"],
            label: "Columns or stacked bands",
        },
        Binding {
            action: Act::Swap,
            id: "swap",
            keys: &["~"],
            label: "Swap A and B",
        },
        Binding {
            action: Act::ToggleAncestor,
            id: "toggle_ancestor",
            keys: &["/"],
            label: "Merge: show the ancestor too, or not",
        },
        Binding {
            action: Act::Edit(A),
            id: "edit_a",
            keys: &["e a"],
            label: "Edit A in place; C-c C-c or C-S-c stops",
        },
        Binding {
            action: Act::Edit(B),
            id: "edit_b",
            keys: &["e b"],
            label: "Edit B in place; C-c C-c or C-S-c stops",
        },
        Binding {
            action: Act::Edit(C),
            id: "edit_c",
            keys: &["e c"],
            label: "Edit C in place; C-c C-c or C-S-c stops",
        },
        Binding {
            action: Act::Write(A),
            id: "write_a",
            keys: &["w a"],
            label: "Write A back to its file",
        },
        Binding {
            action: Act::Write(B),
            id: "write_b",
            keys: &["w b"],
            label: "Write B back to its file",
        },
        Binding {
            action: Act::Write(C),
            id: "write_c",
            keys: &["w c"],
            label: "Write C back to its file",
        },
        Binding {
            action: Act::DiffOutput,
            id: "show_as_diff",
            keys: &["D"],
            label: "Show the differences as a diff",
        },
        Binding {
            action: Act::Status,
            id: "status",
            keys: &["i"],
            label: "Say where the session stands",
        },
        Binding {
            action: Act::Help,
            id: "help",
            keys: &["?"],
            label: "List the keys of this tab",
        },
        Binding {
            action: Act::Quit,
            id: "quit",
            keys: &["q"],
            label: "Quit",
        },
    ],
    configurable: true,
    title: "Diff",
};

/// A comparison of two texts' own keys: a region copied across.
const COMPARE_KEYS: KeyTable<Act> = KeyTable {
    block: "diff.compare",
    bindings: &[
        Binding {
            action: Act::Copy(A, B),
            id: "copy_a_to_b",
            keys: &["a"],
            label: "Copy the region from A to B",
        },
        Binding {
            action: Act::Copy(B, A),
            id: "copy_b_to_a",
            keys: &["b"],
            label: "Copy the region from B to A",
        },
    ],
    configurable: true,
    title: "Diff: Two Texts",
};

/// A comparison of three texts' own keys: a region copied from one text,
/// named first, to another.
const COMPARE3_KEYS: KeyTable<Act> = KeyTable {
    block: "diff.compare3",
    bindings: &[
        Binding {
            action: Act::Copy(A, B),
            id: "copy_a_to_b",
            keys: &["a b"],
            label: "Copy the region from A to B",
        },
        Binding {
            action: Act::Copy(A, C),
            id: "copy_a_to_c",
            keys: &["a c"],
            label: "Copy the region from A to C",
        },
        Binding {
            action: Act::Copy(B, A),
            id: "copy_b_to_a",
            keys: &["b a"],
            label: "Copy the region from B to A",
        },
        Binding {
            action: Act::Copy(B, C),
            id: "copy_b_to_c",
            keys: &["b c"],
            label: "Copy the region from B to C",
        },
        Binding {
            action: Act::Copy(C, A),
            id: "copy_c_to_a",
            keys: &["c a"],
            label: "Copy the region from C to A",
        },
        Binding {
            action: Act::Copy(C, B),
            id: "copy_c_to_b",
            keys: &["c b"],
            label: "Copy the region from C to B",
        },
    ],
    configurable: true,
    title: "Diff: Three Texts",
};

/// A merge's own keys: which side a region keeps.
const MERGE_KEYS: KeyTable<Act> = KeyTable {
    block: "diff.merge",
    bindings: &[
        Binding {
            action: Act::Keep(Variant::A),
            id: "keep_a",
            keys: &["a"],
            label: "Keep A's",
        },
        Binding {
            action: Act::Keep(Variant::B),
            id: "keep_b",
            keys: &["b"],
            label: "Keep B's",
        },
        Binding {
            action: Act::Keep(Variant::Combined),
            id: "keep_both",
            keys: &["+"],
            label: "Keep both, A's first, between markers",
        },
        Binding {
            action: Act::Keep(Variant::Ancestor),
            id: "keep_ancestor",
            keys: &["o"],
            label: "Keep the ancestor's",
        },
        Binding {
            action: Act::RestoreMerge,
            id: "restore",
            keys: &["r"],
            label: "Put the region back as the merge first held it",
        },
        Binding {
            action: Act::StepConflicts,
            id: "conflicts_only",
            keys: &["$ $"],
            label: "Step only through regions still in conflict, or every one",
        },
    ],
    configurable: true,
    title: "Diff: Merge",
};

/// The keys while a text is edited in place, ahead of the editor's.
const EDIT_KEYS: KeyTable<Act> = KeyTable {
    block: "diff.edit",
    bindings: &[Binding {
        action: Act::StopEditing,
        id: "stop_editing",
        keys: &["C-c C-c", "C-S-c"],
        label: "Stop editing, back to the regions",
    }],
    configurable: true,
    title: "Diff: Editing in Place",
};

/// Every key table of this surface, for the Keys editor to list.
pub const KEY_TABLES: &[&dyn Listed] = &[
    &KEYS,
    &COMPARE_KEYS,
    &COMPARE3_KEYS,
    &MERGE_KEYS,
    &EDIT_KEYS,
];

// ========================================================================
// Data Structures
// ========================================================================

/// What a key of the session does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Act {
    Bottom,
    /// Copy the current region from the first text over the second.
    Copy(usize, usize),
    DiffObtain,
    DiffOutput,
    DiffPut,
    Edit(usize),
    HalfPageDown,
    HalfPageUp,
    Help,
    Jump,
    Keep(Variant),
    Next,
    PageDown,
    PageUp,
    Previous,
    Quit,
    Recompute,
    Restore(usize),
    RestoreMerge,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
    ScrollUp,
    Status,
    StepConflicts,
    StopEditing,
    Swap,
    ToggleAncestor,
    ToggleCase,
    ToggleLayout,
    ToggleRefine,
    ToggleWhitespace,
    Top,
    Write(usize),
}

/// How the texts are laid out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Layout {
    /// Side by side.
    Columns,
    /// One above another.
    Bands,
}

/// A text edited in place: its side, and the Cherry document and view it
/// is edited in, its lines taken back into the side after every key.
#[derive(Clone, Copy, Debug)]
struct Editing {
    document: DocumentId,
    side: usize,
    view: ViewId,
}

/// A diff session in a tab.
#[derive(Debug)]
pub struct DiffEditor {
    /// Whether the current region's changed words are drawn louder.
    auto_refine: bool,
    /// Where the caret was painted, while a text is edited: the screen row
    /// and column.
    caret: Option<(usize, usize)>,
    /// Each text's colors, and the revision they were worked out at.
    colors: [(Option<usize>, Vec<Vec<Style>>); 4],
    /// Whether `n` and `p` step only through merge regions in conflict.
    conflicts_only: bool,
    current: Option<usize>,
    /// The text being edited in place, as a Cherry document of its own.
    editing: Option<Editing>,
    /// The keys of a sequence of the edit keys typed so far (`C-c`), while
    /// they wait for the rest.
    edit_pending: Vec<Key>,
    hscroll: usize,
    layout: Layout,
    notice: Option<Notice>,
    /// The keys of a sequence typed so far (`w`, `#`, `a` with three
    /// texts), while they wait for the rest.
    pending: Vec<Key>,
    /// The window over the aligned rows: the first in view, and how many
    /// rows each text's window showed at the last paint.
    view: Viewport,
    session: Session,
    /// Whether a merge shows its ancestor as a text of its own.
    with_ancestor: bool,
}

// ========================================================================
// Editing
// ========================================================================

impl Editing {
    /// Closes the view and the document the text was edited in.
    fn close(self) {
        session::with(|session| {
            session.editor.close_view(self.view);
            session.editor.close_document(self.document);
        });
    }
}

// ========================================================================
// DiffEditor: making one
// ========================================================================

impl DiffEditor {
    /// A session over what `request` names.
    pub fn new(request: DiffRequest) -> Self {
        let side = |text: winter_model::editor::DiffText| {
            // The shape comes with the text, from where it was read: the
            // file is not read a second time to learn it.
            let shape = text.shape.unwrap_or(FileShape {
                trailing_newline: true,
                ..FileShape::default()
            });
            let lines = match text.lines.is_empty() {
                true => vec![String::new()],
                false => text.lines,
            };
            Side {
                text: SideText::new(lines),
                label: text.label,
                path: text.path,
                shape,
            }
        };
        let kind = match (&request.merge, request.texts.len()) {
            (Some(_), _) => Kind::Merge,
            (None, 3..) => Kind::Compare3,
            (None, _) => Kind::Compare,
        };
        let output = request.merge.and_then(|merge| merge.output);
        let sides: Vec<Side> = request.texts.into_iter().map(side).collect();
        let ancestor = request.ancestor.map(side);
        let session = Session::new(kind, sides, ancestor, output);
        let mut editor = Self {
            auto_refine: true,
            caret: None,
            colors: Default::default(),
            conflicts_only: false,
            current: None,
            editing: None,
            edit_pending: Vec::new(),
            hscroll: 0,
            layout: Layout::Columns,
            notice: None,
            pending: Vec::new(),
            view: Viewport::new(),
            session,
            with_ancestor: false,
        };
        editor.step(true);
        let count = editor.session.regions.len();
        editor.notice = Some(
            Notice::info(match count {
                0 => "no differences".to_string(),
                1 => "1 region differs".to_string(),
                n => format!("{n} regions differ"),
            })
            .unlogged(),
        );
        editor
    }
}

// ========================================================================
// DiffEditor: moving
// ========================================================================

impl DiffEditor {
    /// The texts shown, in order.
    fn shown(&self) -> Vec<usize> {
        self.session.shown(self.with_ancestor)
    }

    /// The rows set side by side.
    fn aligned(&self) -> Vec<Aligned> {
        self.session.aligned(&self.shown())
    }

    /// On to the next region, or back to the one before; only through
    /// merge regions in conflict while `$$` asks for those.
    fn step(&mut self, forward: bool) {
        let count = self.session.regions.len();
        if count == 0 {
            self.current = None;
            return;
        }
        let wanted = |index: usize| -> bool {
            !self.conflicts_only || {
                let region = &self.session.regions[index];
                region.conflict && region.variant == Some(Variant::Combined)
            }
        };
        let order: Vec<usize> = match (forward, self.current) {
            (true, None) => (0..count).collect(),
            (true, Some(at)) => (at + 1..count).collect(),
            (false, None) => (0..count).rev().collect(),
            (false, Some(at)) => (0..at).rev().collect(),
        };
        match order.into_iter().find(|index| wanted(*index)) {
            Some(index) => {
                self.current = Some(index);
                self.bring_into_view(index);
            }
            None => {
                let edge = if forward { "last" } else { "first" };
                self.notice = Some(Notice::info(format!("at the {edge} region")).unlogged());
            }
        }
    }

    /// Scroll region `index` into view, a few rows down the pane.
    fn bring_into_view(&mut self, index: usize) {
        let Some(row) = self
            .aligned()
            .iter()
            .position(|row| row.region == Some(index))
        else {
            return;
        };
        self.view.set_scroll(row.saturating_sub(REGION_LIFT));
    }

    /// Scroll by `delta` rows.
    fn scroll_by(&mut self, delta: isize) {
        let total = self.aligned().len();
        self.view.scroll_by(delta, total);
    }

    /// The region the cursor is in, or the current one, for a command.
    fn region(&mut self) -> Option<usize> {
        if self.current.is_none() {
            self.notice = Some(Notice::info("no region selected: n goes to the first").unlogged());
        }
        self.current
    }
}

// ========================================================================
// DiffEditor: the commands
// ========================================================================

impl DiffEditor {
    /// One key in control mode: the session's kind's keys first, then those
    /// of every session, a sequence waiting for its rest.
    fn control_key(&mut self, key: &Key) -> EditorOutcome {
        self.pending.push(*key);
        let found = match table::lookup(self.own_keys(), &self.pending) {
            TableMatch::Miss => table::lookup(&KEYS, &self.pending),
            found => found,
        };
        match found {
            TableMatch::Exact(act) => {
                self.pending.clear();
                self.run(act)
            }
            TableMatch::Prefix => EditorOutcome::Consumed,
            // The rest of a sequence that is none of the tables' is dropped.
            TableMatch::Miss if self.pending.len() > 1 => {
                self.pending.clear();
                EditorOutcome::Consumed
            }
            TableMatch::Miss => {
                self.pending.clear();
                EditorOutcome::Ignored
            }
        }
    }

    /// The keys of the session's kind, looked up ahead of every session's.
    fn own_keys(&self) -> &'static KeyTable<Act> {
        match self.session.kind {
            Kind::Compare => &COMPARE_KEYS,
            Kind::Compare3 => &COMPARE3_KEYS,
            Kind::Merge => &MERGE_KEYS,
        }
    }

    /// Carry out `act`, a key of the session's.
    fn run(&mut self, act: Act) -> EditorOutcome {
        let page = self.view.rows().max(1) as isize;
        let is_merge = self.session.kind == Kind::Merge;
        let exists = |side: usize| side < self.session.sides.len();
        match act {
            Act::Bottom => self.scroll_by(isize::MAX / 2),
            Act::Copy(from, to) if from != to && exists(from) && exists(to) => self.copy(from, to),
            Act::Copy(..) => {}
            Act::DiffObtain => self.diff_obtain(),
            Act::DiffOutput => return self.diff_output(),
            Act::DiffPut => self.diff_put(),
            Act::Edit(side) if exists(side) => self.edit(side),
            Act::HalfPageDown => self.scroll_by(page / 2),
            Act::HalfPageUp => self.scroll_by(-page / 2),
            Act::Help => return EditorOutcome::RunActionHere("tab_keys".to_string()),
            Act::Jump => {
                return EditorOutcome::Prompt(PromptRequest {
                    initial: String::new(),
                    label: format!("Region (1-{}): ", self.session.regions.len()),
                    mode: PromptMode::Text,
                    tag: ASK_JUMP,
                    title: None,
                })
            }
            Act::Keep(Variant::Ancestor) if self.session.ancestor.is_none() => {}
            Act::Keep(variant) => self.keep(variant),
            Act::Next => self.step(true),
            Act::PageDown => self.scroll_by(page),
            Act::PageUp => self.scroll_by(-page),
            Act::Previous => self.step(false),
            Act::Quit => return self.quit(),
            Act::Recompute => self.recompute(),
            Act::Restore(side) if exists(side) => {
                if let Some(index) = self.region() {
                    self.session.restore(index, side);
                }
            }
            Act::RestoreMerge => {
                if let Some(index) = self.region() {
                    self.session.restore(index, C);
                }
            }
            Act::ScrollDown => self.scroll_by(1),
            Act::ScrollLeft => self.hscroll = self.hscroll.saturating_sub(HSCROLL_STEP),
            Act::ScrollRight => self.hscroll += HSCROLL_STEP,
            Act::ScrollUp => self.scroll_by(-1),
            Act::Status => self.say_status(),
            Act::StepConflicts if is_merge => {
                self.conflicts_only = !self.conflicts_only;
                let state = if self.conflicts_only {
                    "only conflicts"
                } else {
                    "every region"
                };
                self.notice = Some(Notice::info(format!("stepping through {state}")).unlogged());
            }
            Act::StopEditing => return self.stop_editing(),
            Act::Swap => self.swap(),
            Act::ToggleAncestor => {
                if self.session.ancestor.is_some() {
                    self.with_ancestor = !self.with_ancestor;
                }
            }
            Act::ToggleCase => {
                self.session.compare.ignore_case = !self.session.compare.ignore_case;
                let state = if self.session.compare.ignore_case {
                    "left out"
                } else {
                    "counted"
                };
                self.notice = Some(Notice::info(format!("case {state}")).unlogged());
                self.recompute();
            }
            Act::ToggleLayout => {
                self.layout = match self.layout {
                    Layout::Columns => Layout::Bands,
                    Layout::Bands => Layout::Columns,
                };
            }
            Act::ToggleRefine => {
                self.auto_refine = !self.auto_refine;
                let state = if self.auto_refine { "on" } else { "off" };
                self.notice = Some(Notice::info(format!("changed words shown {state}")).unlogged());
            }
            Act::ToggleWhitespace => {
                self.session.compare.ignore_whitespace = !self.session.compare.ignore_whitespace;
                let state = if self.session.compare.ignore_whitespace {
                    "left out"
                } else {
                    "counted"
                };
                self.notice = Some(Notice::info(format!("whitespace {state}")).unlogged());
                self.recompute();
            }
            Act::Top => self.view.set_scroll(0),
            Act::Write(side) if exists(side) => self.write(side),
            Act::Edit(_) | Act::Restore(_) | Act::StepConflicts | Act::Write(_) => {}
        }
        EditorOutcome::Consumed
    }

    /// Copy the current region from text `from` over text `to`.
    fn copy(&mut self, from: usize, to: usize) {
        let Some(index) = self.region() else {
            return;
        };
        self.session.copy_region(index, from, to);
        self.notice = Some(
            Notice::info(format!(
                "region {} copied {} to {}",
                index + 1,
                letter_of(from),
                letter_of(to)
            ))
            .unlogged(),
        );
    }

    /// Vimdiff `dp` (diffput): copy from active side to other side.
    fn diff_put(&mut self) {
        let (from, to) = match self.editing_side() {
            Some(B) => (B, A),
            _ => (A, B),
        };
        self.copy(from, to);
    }

    /// Vimdiff `do` (diffget/obtain): copy other side into active side.
    fn diff_obtain(&mut self) {
        let (from, to) = match self.editing_side() {
            Some(B) => (A, B),
            _ => (B, A),
        };
        self.copy(from, to);
    }

    /// Have the current merge region keep `variant`, and go on to the next.
    fn keep(&mut self, variant: Variant) {
        let Some(index) = self.region() else {
            return;
        };
        self.session.choose(index, variant);
        self.step(true);
    }

    /// Work the regions out again, keeping the current one where it can.
    fn recompute(&mut self) {
        self.session.recompute();
        let count = self.session.regions.len();
        self.current = self
            .current
            .map(|at| at.min(count.saturating_sub(1)))
            .filter(|_| count > 0);
        self.notice = Some(Notice::info(format!("{count} regions differ")).unlogged());
    }

    /// Swap A and B.
    fn swap(&mut self) {
        if self.session.kind == Kind::Merge {
            self.notice = Some(Notice::info("a merge keeps its sides where they are").unlogged());
            return;
        }
        self.session.sides.swap(A, B);
        self.colors.swap(A, B);
        self.recompute();
    }

    /// Write text `side` back to its file.
    fn write(&mut self, side: usize) {
        let Some(text) = self.session.sides.get_mut(side) else {
            return;
        };
        let Some(path) = text.path.clone() else {
            self.notice = Some(Notice::info(format!(
                "{} is not a file to write to",
                letter_of(side)
            )));
            return;
        };
        // Through the buffer a tab has the file open in, where one does: that
        // tab then shows what was written, and holds no older copy a save of
        // its own could put back over it.
        let written = crate::save_through(&path, text.text.lines())
            .unwrap_or_else(|| text_file::save(&path, text.text.lines(), text.shape));
        match written {
            Ok(_) => {
                text.text.mark_saved();
                self.notice = Some(Notice::info(format!("wrote {}", path.display())));
            }
            Err(e) => self.notice = Some(Notice::error(format!("{}: {e}", path.display()))),
        }
    }

    /// Start editing text `side` in place.
    fn edit(&mut self, side: usize) {
        if self.session.kind == Kind::Merge && side != C {
            self.notice = Some(Notice::info("a merge edits its result, C").unlogged());
            return;
        }
        // The edit starts at the current region's first line in that text.
        let line = self
            .current
            .map_or(0, |index| self.session.regions[index].lines[side].start);
        let text = self.session.lines(side).join("\n");
        let opened = session::with(|session| {
            let document = session.editor.open_document(&text);
            let view = session.editor.new_view(document)?;
            let at = session.offset_of(document, line, 0);
            session.editor.set_cursor(view, at);
            Some(Editing {
                document,
                side,
                view,
            })
        });
        let Some(editing) = opened else {
            return;
        };
        self.editing = Some(editing);
        self.notice = Some(
            Notice::info(format!(
                "editing {}: C-c C-c or C-S-c goes back to the regions",
                letter_of(side)
            ))
            .unlogged(),
        );
    }

    /// Stop editing in place, back to the regions, compared afresh.
    fn stop_editing(&mut self) -> EditorOutcome {
        if let Some(editing) = self.editing.take() {
            editing.close();
        }
        self.caret = None;
        if self.session.kind != Kind::Merge {
            self.recompute();
        }
        EditorOutcome::Consumed
    }

    /// One key while a text is edited in place: `C-c C-c`, or `C-S-c` as in
    /// every edit mode, goes back to the regions; every other key is
    /// Cherry's.
    fn edit_key(&mut self, editing: Editing, key: &Key) -> EditorOutcome {
        self.edit_pending.push(*key);
        match table::lookup(&EDIT_KEYS, &self.edit_pending) {
            TableMatch::Exact(act) => {
                self.edit_pending.clear();
                return self.run(act);
            }
            TableMatch::Prefix => return EditorOutcome::Consumed,
            // A key after a prefix that ends nothing is the editor's own.
            TableMatch::Miss => self.edit_pending.clear(),
        }
        let (handled, requests) = session::with(|session| {
            let handled =
                session
                    .editor
                    .handle_key(editing.view, keys::to_cherry(key), clock::now());
            (handled, session.editor.take_requests())
        });
        self.take_edit(editing);
        for request in requests {
            match request {
                HostRequest::Notice(notice) => {
                    self.notice = Some(match notice.level {
                        NoticeLevel::Error => Notice::error(notice.message),
                        NoticeLevel::Warning => Notice::warning(notice.message),
                        _ => Notice::info(notice.message),
                    });
                }
                HostRequest::Save(_) => self.write(editing.side),
                HostRequest::Quit(_) => {
                    self.stop_editing();
                }
                _ => {}
            }
        }
        match handled {
            Handled::Ignored => EditorOutcome::Ignored,
            _ => EditorOutcome::Consumed,
        }
    }

    /// Takes what Cherry's document of the text edited holds back into the
    /// text, where it changed, the regions following.
    fn take_edit(&mut self, editing: Editing) {
        let lines = session::with(|session| session.lines(editing.document));
        let before = self.session.lines(editing.side);
        if lines == before {
            return;
        }
        let first = before
            .iter()
            .zip(&lines)
            .position(|(was, now)| was != now)
            .unwrap_or(before.len().min(lines.len()));
        let delta = lines.len() as isize - before.len() as isize;
        self.session.sides[editing.side].text.set_lines(lines);
        match self.session.kind {
            Kind::Merge => self.session.merge_edited(first, delta),
            Kind::Compare | Kind::Compare3 => self.session.recompute(),
        }
    }

    /// The text edited in place, if one is.
    fn editing_side(&self) -> Option<usize> {
        self.editing.map(|editing| editing.side)
    }

    /// Where the cursor of the text edited in place is: its line, and its
    /// column in chars.
    fn edit_cursor(&self) -> Option<(usize, usize)> {
        let editing = self.editing?;
        session::with(|session| {
            let cursor = session.editor.view(editing.view)?.cursor();
            let at = session
                .editor
                .document(editing.document)?
                .char_to_position(cursor)?;
            Some((at.line, at.column))
        })
    }

    /// The mode of the text edited in place, if one is.
    fn edit_mode(&self) -> Option<Mode> {
        let editing = self.editing?;
        session::with(|session| session.editor.view(editing.view).map(cherry::View::mode))
    }

    /// Show the differences between A and B as a diff, in a view beside.
    fn diff_output(&mut self) -> EditorOutcome {
        let dir = std::env::temp_dir().join(format!("winter-diff-{}", std::process::id()));
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.notice = Some(Notice::error(e.to_string()));
            return EditorOutcome::Consumed;
        }
        let sequence = COPY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut copies = Vec::new();
        for side in [A, B] {
            let text = &self.session.sides[side];
            let name = text
                .path
                .as_ref()
                .and_then(|path| path.file_name())
                .map_or_else(
                    || letter_of(side).to_string(),
                    |name| name.to_string_lossy().into_owned(),
                );
            let copy = dir.join(format!("{sequence}-{}-{name}", letter_of(side)));
            let mut body = text.text.lines().join("\n");
            body.push('\n');
            if let Err(e) = std::fs::write(&copy, body) {
                self.notice = Some(Notice::error(e.to_string()));
                return EditorOutcome::Consumed;
            }
            copies.push((copy, text.path.clone()));
        }
        let (old, new) = (copies[0].0.clone(), copies[1].0.clone());
        EditorOutcome::Compare(CompareRequest {
            copies,
            new,
            old,
            title: format!(
                "{} vs {}",
                self.session.sides[A].label, self.session.sides[B].label
            ),
        })
    }

    /// Say where the session stands: the region, how many differ, what a
    /// merge has resolved, and what is left out of the comparison.
    fn say_status(&mut self) {
        let count = self.session.regions.len();
        let at = self
            .current
            .map_or("none".to_string(), |at| (at + 1).to_string());
        let mut said = format!("region {at} of {count}");
        if self.session.kind == Kind::Merge {
            let open = self
                .session
                .regions
                .iter()
                .filter(|region| region.conflict && region.variant == Some(Variant::Combined))
                .count();
            said.push_str(&format!(", {open} conflicts left"));
        }
        if self.session.compare.ignore_whitespace {
            said.push_str(", whitespace left out");
        }
        if self.session.compare.ignore_case {
            said.push_str(", case left out");
        }
        self.notice = Some(Notice::info(said));
    }

    /// Quit, asking first where a text has edits not written.
    fn quit(&mut self) -> EditorOutcome {
        let dirty: Vec<&str> = (0..self.session.sides.len())
            .filter(|side| self.session.is_dirty(*side) && self.session.sides[*side].path.is_some())
            .map(letter_of)
            .collect();
        if dirty.is_empty() {
            return EditorOutcome::Close;
        }
        let question = format!(
            "Not written: side {}. Quitting loses those edits.",
            dirty.join(", ")
        );
        EditorOutcome::Prompt(
            PromptRequest::confirm(ASK_QUIT, &question)
                .titled("Discard Unsaved Changes?", "Discard")
                .destructive(),
        )
    }
}

// ========================================================================
// DiffEditor: painting
// ========================================================================

impl DiffEditor {
    /// Work each text's colors out again where it has changed.
    fn color_texts(&mut self) {
        for side in [A, B, C, ANCESTOR] {
            let (revision, grammar) = match side {
                ANCESTOR => match &self.session.ancestor {
                    Some(text) => (
                        text.text.revision(),
                        text.path.as_deref().and_then(winter_syntax::of_path),
                    ),
                    None => continue,
                },
                side => match self.session.sides.get(side) {
                    Some(text) => (
                        text.text.revision(),
                        text.path
                            .as_deref()
                            .or_else(|| self.session.sides[A].path.as_deref())
                            .and_then(winter_syntax::of_path),
                    ),
                    None => continue,
                },
            };
            if self.colors[side].0 == Some(revision) {
                continue;
            }
            let colors = match grammar {
                Some(grammar) => {
                    let mut state = LineState::start(grammar);
                    self.session
                        .lines(side)
                        .iter()
                        .map(|line| {
                            let (styles, next) = winter_syntax::highlight(line, &state);
                            state = next;
                            styles
                        })
                        .collect()
                }
                None => Vec::new(),
            };
            self.colors[side] = (Some(revision), colors);
        }
    }

    /// The rows of the texts in columns, `cols` wide and `rows` tall, a
    /// header over each.
    fn paint_columns(&mut self, rows: usize, cols: usize, aligned: &[Aligned]) -> Vec<Row> {
        let shown = self.shown();
        let count = shown.len().max(1);
        let width = cols.saturating_sub(count - 1) / count;
        let body = rows.saturating_sub(1);
        self.view.set_rows(body);
        let refined = self.refined();
        let mut out = Vec::with_capacity(rows);
        let mut header: Row = Vec::new();
        for (at, &side) in shown.iter().enumerate() {
            if at > 0 {
                header.push(Span::new(Style::Dim, DIVIDER));
            }
            header.extend(paint::header(
                &self.session,
                side,
                width,
                self.editing_side() == Some(side),
            ));
        }
        out.push(header);
        for row in aligned.iter().skip(self.view.scroll()).take(body) {
            let mut painted: Row = Vec::new();
            for (at, &side) in shown.iter().enumerate() {
                if at > 0 {
                    painted.push(Span::new(Style::Dim, DIVIDER));
                }
                let column = Column {
                    colors: &self.colors[side].1,
                    side,
                };
                let widths = self.widths(width);
                painted.extend(paint::cell(
                    &self.session,
                    row,
                    &column,
                    widths,
                    self.hscroll,
                    self.current,
                    refined.as_deref(),
                ));
            }
            out.push(painted);
        }
        out
    }

    /// The rows of the texts in bands one above another, each with its
    /// header and the same window of rows.
    fn paint_bands(&mut self, rows: usize, cols: usize, aligned: &[Aligned]) -> Vec<Row> {
        let shown = self.shown();
        let count = shown.len().max(1);
        let band = rows / count;
        let body = band.saturating_sub(1);
        self.view.set_rows(body);
        let refined = self.refined();
        let widths = self.widths(cols);
        let mut out = Vec::with_capacity(rows);
        for &side in &shown {
            out.push(paint::header(
                &self.session,
                side,
                cols,
                self.editing_side() == Some(side),
            ));
            let column = Column {
                colors: &self.colors[side].1,
                side,
            };
            for at in 0..body {
                match aligned.get(self.view.scroll() + at) {
                    Some(row) => out.push(paint::cell(
                        &self.session,
                        row,
                        &column,
                        widths,
                        self.hscroll,
                        self.current,
                        refined.as_deref(),
                    )),
                    None => out.push(Vec::new()),
                }
            }
        }
        out
    }

    /// The widths a column `total` wide splits into.
    fn widths(&self, total: usize) -> Widths {
        let longest = [A, B, C, ANCESTOR]
            .iter()
            .map(|side| self.session.lines(*side).len())
            .max()
            .unwrap_or(1);
        let digits = longest.to_string().len().max(3);
        Widths {
            gutter: digits + 2,
            total,
        }
    }

    /// The current region's changed words, while they are shown.
    fn refined(&self) -> Option<Vec<Vec<winter_model::word_diff::Segment>>> {
        let index = self.current.filter(|_| self.auto_refine)?;
        Some(paint::refine(&self.session, index))
    }

    /// Keep the line being edited in view, and note where its caret is
    /// painted.
    fn place_caret(&mut self, aligned: &[Aligned], cols: usize) {
        let (Some(side), Some((row, column))) = (self.editing_side(), self.edit_cursor()) else {
            self.caret = None;
            return;
        };
        let Some(index) = aligned
            .iter()
            .position(|aligned| aligned.lines[side] == Some(row))
        else {
            self.caret = None;
            return;
        };
        let body = self.view.rows().max(1);
        let scroll = self.view.scroll();
        if index < scroll {
            self.view.set_scroll(index);
        } else if index >= scroll + body {
            self.view.set_scroll(index + 1 - body);
        }
        let shown = self.shown();
        let position = shown.iter().position(|shown| *shown == side).unwrap_or(0);
        let (top, left, width) = match self.layout {
            Layout::Columns => {
                let width = cols.saturating_sub(shown.len() - 1) / shown.len().max(1);
                (1, position * (width + 1), width)
            }
            Layout::Bands => (position * (body + 1) + 1, 0, cols),
        };
        let widths = self.widths(width);
        let line = self
            .session
            .lines(side)
            .get(row)
            .cloned()
            .unwrap_or_default();
        let display: usize = line
            .chars()
            .take(column)
            .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0))
            .sum();
        if display < self.hscroll {
            self.hscroll = display;
        } else if display >= self.hscroll + width.saturating_sub(widths.gutter) {
            self.hscroll = display + 1 - width.saturating_sub(widths.gutter);
        }
        self.caret = Some((
            top + index - self.view.scroll(),
            left + widths.gutter + display - self.hscroll,
        ));
    }
}

impl Page for DiffEditor {
    /// What the session can do with the region under the pointer, as its
    /// kind allows, then moving between regions, the view, and writing the
    /// texts back. A text being edited in place takes its own keys, so its
    /// menu only stops the edit.
    fn context_items(&self) -> Vec<EditorMenuItem> {
        if self.editing.is_some() {
            return present([EditorMenuItem::bound_as(
                &EDIT_KEYS,
                Act::StopEditing,
                "Stop Editing",
            )]);
        }
        let own = self.own_keys();
        let key = |act: Act, label: &str| {
            [own, &KEYS]
                .into_iter()
                .find_map(|keys| EditorMenuItem::bound_as(keys, act, label))
        };
        let ancestor = self.session.ancestor.is_some();
        let region = match self.session.kind {
            Kind::Compare => present([
                key(Act::Copy(A, B), "Copy Region to B"),
                key(Act::Copy(B, A), "Copy Region to A"),
                key(Act::Restore(A), "Restore Region in A"),
                key(Act::Restore(B), "Restore Region in B"),
            ]),
            Kind::Compare3 => present([
                key(Act::Copy(A, B), "Copy Region from A to B"),
                key(Act::Copy(C, B), "Copy Region from C to B"),
                key(Act::Restore(B), "Restore Region in B"),
            ]),
            Kind::Merge => present([
                key(Act::Keep(Variant::A), "Keep A's"),
                key(Act::Keep(Variant::B), "Keep B's"),
                key(Act::Keep(Variant::Combined), "Keep Both"),
                key(Act::Keep(Variant::Ancestor), "Keep the Ancestor's").filter(|_| ancestor),
                key(Act::RestoreMerge, "Restore Region"),
            ]),
        };
        let steps = present([
            key(Act::Next, "Next Region"),
            key(Act::Previous, "Previous Region"),
            key(Act::Jump, "Go to Region\u{2026}"),
        ]);
        let merge = self.session.kind == Kind::Merge;
        let view = present([
            key(Act::ToggleRefine, "Show Changed Words"),
            key(Act::ToggleLayout, "Change Layout"),
            key(Act::Swap, "Swap A and B"),
            key(Act::ToggleAncestor, "Show the Ancestor").filter(|_| merge && ancestor),
            key(Act::DiffOutput, "Show as a Diff"),
        ]);
        // Each text with edits not written, that has a file to write to.
        let writes = (0..self.session.sides.len())
            .filter(|side| self.session.is_dirty(*side) && self.session.sides[*side].path.is_some())
            .filter_map(|side| key(Act::Write(side), &format!("Write {}", letter_of(side))));
        let end = writes.chain(key(Act::Quit, "Quit")).collect();
        menu_groups([region, steps, view, end])
    }

    fn title(&self) -> String {
        let name = |side: usize| {
            self.session
                .sides
                .get(side)
                .map_or_else(String::new, |text| {
                    text.path
                        .as_ref()
                        .and_then(|path| path.file_name())
                        .map_or_else(
                            || text.label.clone(),
                            |name| name.to_string_lossy().into_owned(),
                        )
                })
        };
        match self.session.kind {
            Kind::Compare => format!("Diff {} {}", name(A), name(B)),
            Kind::Compare3 => format!("Diff {} {} {}", name(A), name(B), name(C)),
            Kind::Merge => format!("Merge {}", name(C)),
        }
    }

    fn icon(&self) -> Option<IconKind> {
        Some(IconKind::Editor { kind: "diff" })
    }

    fn content(&mut self, rows: usize, cols: usize, _wrap: bool) -> Frame {
        self.color_texts();
        let aligned = self.aligned();
        // The caret's line is brought into view before the rows are cut.
        self.view.set_rows(match self.layout {
            Layout::Columns => rows.saturating_sub(1),
            Layout::Bands => (rows / self.shown().len().max(1)).saturating_sub(1),
        });
        self.place_caret(&aligned, cols);
        let painted = match self.layout {
            Layout::Columns => self.paint_columns(rows, cols, &aligned),
            Layout::Bands => self.paint_bands(rows, cols, &aligned),
        };
        self.place_caret(&aligned, cols);
        let frame = Frame::new(painted);
        match self.caret {
            Some((row, _)) => frame.with_cursor_line(row),
            None => frame,
        }
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        match self.editing {
            Some(editing) => self.edit_key(editing, &key.typed()),
            None => self.control_key(key),
        }
    }

    fn caret(&self) -> Option<Caret> {
        let (_, col) = self.caret?;
        Some(Caret {
            col,
            insert: self.edit_mode() == Some(Mode::Insert),
        })
    }

    fn keys(&self) -> Vec<KeyHelp> {
        if self.editing.is_some() {
            return table::help(&EDIT_KEYS);
        }
        let mut rows = table::help(self.own_keys());
        rows.extend(table::help(&KEYS));
        rows
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        self.scroll_by(-lines);
        EditorOutcome::Consumed
    }

    fn scroll_position(&self) -> Option<ScrollPosition> {
        self.view.scroll_position(self.aligned().len())
    }

    fn scroll_to(&mut self, offset: usize) -> EditorOutcome {
        self.view.scroll_to(offset, self.aligned().len());
        EditorOutcome::Consumed
    }

    /// A press on a region's row makes it the current one.
    fn on_mouse(&mut self, at: Point) -> EditorOutcome {
        if at.drag || self.editing.is_some() {
            return EditorOutcome::Ignored;
        }
        let body = self.view.rows().max(1);
        let index = match self.layout {
            Layout::Columns => at.row.checked_sub(1),
            Layout::Bands => (at.row % (body + 1)).checked_sub(1),
        };
        let region = index.and_then(|index| {
            self.aligned()
                .get(self.view.scroll() + index)
                .and_then(|row| row.region)
        });
        if let Some(region) = region {
            self.current = Some(region);
        }
        EditorOutcome::Consumed
    }

    fn on_paste(&mut self, text: String) -> EditorOutcome {
        let Some(editing) = self.editing else {
            return EditorOutcome::Consumed;
        };
        session::with(|session| {
            let Some(cursor) = session.editor.view(editing.view).map(cherry::View::cursor) else {
                return;
            };
            let paste = Edit::new(cursor..cursor, text);
            session.editor.apply_edits(editing.view, vec![paste]);
        });
        self.take_edit(editing);
        EditorOutcome::Consumed
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        match (reply.tag, reply.answer) {
            (ASK_JUMP, Some(answer)) => {
                match answer
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .filter(|n| (1..=self.session.regions.len()).contains(n))
                {
                    Some(number) => {
                        self.current = Some(number - 1);
                        self.bring_into_view(number - 1);
                    }
                    None => self.notice = Some(Notice::info("no such region").unlogged()),
                }
                EditorOutcome::Consumed
            }
            (ASK_QUIT, Some(_)) => EditorOutcome::Close,
            _ => EditorOutcome::Consumed,
        }
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }

    fn status(&self) -> EditorStatus {
        let count = self.session.regions.len();
        let at = self
            .current
            .map_or("-".to_string(), |at| (at + 1).to_string());
        let mut segments = vec![format!("region {at} of {count}")];
        if let Some(index) = self.current {
            if let Some(variant) = self.session.regions[index].variant {
                let kept = match variant {
                    Variant::A => "=A",
                    Variant::B => "=B",
                    Variant::Ancestor => "=ancestor",
                    Variant::Combined => "=A+B",
                    Variant::Edited => "edited",
                };
                segments.push(kept.to_string());
            }
        }
        if !self.pending.is_empty() {
            segments.push(format!("{}-", format_sequence(&self.pending)));
        }
        EditorStatus {
            mode: self.edit_mode().map(edit_mode_of),
            segments,
        }
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.session
            .sides
            .iter()
            .find_map(|text| text.path.as_ref()?.parent().map(PathBuf::from))
    }
}

/// A tab closed while a text is edited in place closes Cherry's document
/// of it too.
impl Drop for DiffEditor {
    fn drop(&mut self) {
        if let Some(editing) = self.editing.take() {
            editing.close();
        }
    }
}

impl Editor for DiffEditor {
    fn is_dirty(&self) -> bool {
        (0..self.session.sides.len())
            .any(|side| self.session.is_dirty(side) && self.session.sides[side].path.is_some())
    }
}

/// The letter text `side` is called by.
fn letter_of(side: usize) -> &'static str {
    match side {
        A => "A",
        B => "B",
        C => "C",
        _ => "the ancestor",
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests;
