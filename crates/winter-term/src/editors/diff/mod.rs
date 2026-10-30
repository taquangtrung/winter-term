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

mod lines;
mod model;
mod paint;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::editors::list::Viewport;
use crate::editors::text_file::{self, FileShape};
use crate::editors::highlight::{self, LineState};
use crate::model::editor::{
    CompareRequest, Editor, EditorOutcome, EditorStatus, DiffRequest, Notice, PromptMode,
    PromptReply, PromptRequest,
};
use crate::model::input::table::KeyHelp;
use crate::model::input::{Key, KeyCode, Mods};
use crate::model::paint::{Caret, Frame, IconKind, Point, Row, Shown, Span, Style};
use crate::model::textedit::buffer::TextBuffer;
use crate::model::textedit::modal::{EditOutcome, EditView, Marks, ModalEditor};

use model::{Aligned, Kind, Session, Side, Variant, A, ANCESTOR, B, C};
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

/// The keys of a session, for the help the Keys view shows.
const HELP: &[(&str, &str)] = &[
    ("n  SPC", "Next region"),
    ("p  DEL", "Previous region"),
    ("J", "Jump to region number..."),
    ("j  k  C-d  C-u  v  V  gg  G", "Scroll"),
    ("<  >", "Scroll sideways"),
    ("a  b", "Copy the region A to B, or B to A; in a merge, keep A's or B's"),
    ("ab ac ba bc ca cb", "With three texts: copy the region from one to another"),
    ("+", "Merge: keep both, A's first, between markers"),
    ("o", "Merge: keep the ancestor's"),
    ("r  ra rb rc", "Put the region back as it was"),
    ("$$", "Merge: step only through regions still in conflict"),
    ("!", "Work the regions out again"),
    ("##  #c", "Leave whitespace, or case, out of the comparison"),
    ("@", "Show the changed words of the current region, or not"),
    ("|", "Columns or stacked bands"),
    ("~", "Swap A and B"),
    ("/", "Merge: show the ancestor too, or not"),
    ("ea eb ec", "Edit the text in place; C-c C-c or C-S-c stops"),
    ("wa wb wc", "Write the text back to its file"),
    ("D", "Show the differences as a diff"),
    ("i", "Say where the session stands"),
    ("q", "Quit"),
];

// ========================================================================
// Data Structures
// ========================================================================

/// How the texts are laid out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Layout {
    /// Side by side.
    Columns,
    /// One above another.
    Bands,
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
    /// The text being edited in place, and whether `C-c` waits for its
    /// second key.
    editing: Option<usize>,
    edit_chord: bool,
    hscroll: usize,
    layout: Layout,
    marks: Marks,
    modal: ModalEditor,
    notice: Option<Notice>,
    /// The first key of a two-key command, waiting for its second.
    pending: Option<char>,
    /// The window over the aligned rows: the first in view, and how many
    /// rows each text's window showed at the last paint.
    view: Viewport,
    session: Session,
    /// Whether a merge shows its ancestor as a text of its own.
    with_ancestor: bool,
}

// ========================================================================
// DiffEditor: making one
// ========================================================================

impl DiffEditor {
    /// A session over what `request` names.
    pub fn new(request: DiffRequest) -> Self {
        let side = |text: crate::model::editor::DiffText| {
            // The shape comes with the text, from where it was read: the
            // file is not read a second time to learn it.
            let shape = text.shape.unwrap_or(FileShape { trailing_newline: true, ..FileShape::default() });
            let lines = match text.lines.is_empty() {
                true => vec![String::new()],
                false => text.lines,
            };
            Side {
                buffer: TextBuffer::new(lines),
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
            edit_chord: false,
            hscroll: 0,
            layout: Layout::Columns,
            marks: Marks::default(),
            modal: ModalEditor::new(),
            notice: None,
            pending: None,
            view: Viewport::new(),
            session,
            with_ancestor: false,
        };
        editor.step(true);
        let count = editor.session.regions.len();
        editor.notice = Some(Notice::info(match count {
            0 => "no differences".to_string(),
            1 => "1 region differs".to_string(),
            n => format!("{n} regions differ"),
        }).unlogged());
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
        let Some(row) = self.aligned().iter().position(|row| row.region == Some(index)) else {
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
    /// One key in control mode.
    fn control_key(&mut self, key: &Key) -> EditorOutcome {
        if let Some(first) = self.pending.take() {
            return self.second_key(first, key);
        }
        let page = self.view.rows().max(1) as isize;
        match key.chord() {
            (Mods::CTRL, KeyCode::Char('d')) => self.scroll_by(page / 2),
            (Mods::CTRL, KeyCode::Char('u')) => self.scroll_by(-page / 2),
            (Mods::CTRL, KeyCode::Char('v') | KeyCode::Char('f')) => self.scroll_by(page),
            (Mods::CTRL, KeyCode::Char('b')) | (Mods::ALT, KeyCode::Char('v')) => self.scroll_by(-page),
            (Mods::CTRL, KeyCode::Char('e')) => self.scroll_by(1),
            (Mods::CTRL, KeyCode::Char('y')) => self.scroll_by(-1),
            (Mods::NONE, code) => return self.plain_key(code, page),
            _ => return EditorOutcome::Ignored,
        }
        EditorOutcome::Consumed
    }

    /// One key with no modifier in control mode, `page` rows a page.
    fn plain_key(&mut self, code: KeyCode, page: isize) -> EditorOutcome {
        match code {
            KeyCode::Char('n') | KeyCode::Space => self.step(true),
            KeyCode::Char('p') | KeyCode::Backspace => self.step(false),
            KeyCode::Char('J') => {
                return EditorOutcome::Prompt(PromptRequest {
                    initial: String::new(),
                    label: format!("Region (1-{}): ", self.session.regions.len()),
                    mode: PromptMode::Text,
                    tag: ASK_JUMP,
                })
            }
            KeyCode::Char('j') | KeyCode::Down => self.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_by(-1),
            KeyCode::Char('v') | KeyCode::PageDown => self.scroll_by(page),
            KeyCode::Char('V') | KeyCode::PageUp => self.scroll_by(-page),
            KeyCode::Char('G') | KeyCode::End => self.scroll_by(isize::MAX / 2),
            KeyCode::Home => self.view.set_scroll(0),
            KeyCode::Char('<') => self.hscroll += HSCROLL_STEP,
            KeyCode::Char('>') => self.hscroll = self.hscroll.saturating_sub(HSCROLL_STEP),
            KeyCode::Right => self.hscroll += HSCROLL_STEP,
            KeyCode::Left => self.hscroll = self.hscroll.saturating_sub(HSCROLL_STEP),
            KeyCode::Char('|') => {
                self.layout = match self.layout {
                    Layout::Columns => Layout::Bands,
                    Layout::Bands => Layout::Columns,
                };
            }
            KeyCode::Char('~') => self.swap(),
            KeyCode::Char('/') => {
                if self.session.ancestor.is_some() {
                    self.with_ancestor = !self.with_ancestor;
                }
            }
            KeyCode::Char(letter @ ('a' | 'b' | 'c')) => return self.letter(letter),
            KeyCode::Char('+') if self.session.kind == Kind::Merge => {
                if let Some(index) = self.region() {
                    self.session.choose(index, Variant::Combined);
                    self.step(true);
                }
            }
            KeyCode::Char('o') if self.session.kind == Kind::Merge && self.session.ancestor.is_some() => {
                self.keep(Variant::Ancestor);
            }
            KeyCode::Char('r') if self.session.kind == Kind::Merge => {
                if let Some(index) = self.region() {
                    self.session.restore(index, C);
                }
            }
            KeyCode::Char(first @ ('r' | 'w' | 'e' | '#' | '$' | 'g')) => self.pending = Some(first),
            KeyCode::Char('!') => self.recompute(),
            KeyCode::Char('@') => {
                self.auto_refine = !self.auto_refine;
                let state = if self.auto_refine { "on" } else { "off" };
                self.notice = Some(Notice::info(format!("changed words shown {state}")).unlogged());
            }
            KeyCode::Char('D') => return self.diff_output(),
            KeyCode::Char('i') => self.say_status(),
            KeyCode::Char('?') => return EditorOutcome::RunActionHere("tab_keys".to_string()),
            KeyCode::Char('q') => return self.quit(),
            _ => return EditorOutcome::Ignored,
        }
        EditorOutcome::Consumed
    }

    /// The second key of a two-key command started by `first`.
    fn second_key(&mut self, first: char, key: &Key) -> EditorOutcome {
        let (Mods::NONE, KeyCode::Char(second)) = key.chord() else {
            return EditorOutcome::Consumed;
        };
        let side = match second {
            'a' => Some(A),
            'b' => Some(B),
            'c' => Some(C),
            _ => None,
        }
        .filter(|side| *side < self.session.sides.len());
        match (first, second, side) {
            ('g', 'g', _) => self.view.set_scroll(0),
            ('#', '#', _) => {
                self.session.compare.ignore_whitespace = !self.session.compare.ignore_whitespace;
                let state = if self.session.compare.ignore_whitespace { "left out" } else { "counted" };
                self.notice = Some(Notice::info(format!("whitespace {state}")).unlogged());
                self.recompute();
            }
            ('#', 'c', _) => {
                self.session.compare.ignore_case = !self.session.compare.ignore_case;
                let state = if self.session.compare.ignore_case { "left out" } else { "counted" };
                self.notice = Some(Notice::info(format!("case {state}")).unlogged());
                self.recompute();
            }
            ('$', '$', _) if self.session.kind == Kind::Merge => {
                self.conflicts_only = !self.conflicts_only;
                let state = if self.conflicts_only { "only conflicts" } else { "every region" };
                self.notice = Some(Notice::info(format!("stepping through {state}")).unlogged());
            }
            ('w', _, Some(side)) => self.write(side),
            ('r', _, Some(side)) => {
                if let Some(index) = self.region() {
                    self.session.restore(index, side);
                }
            }
            ('e', _, Some(side)) => self.edit(side),
            (from @ ('a' | 'b' | 'c'), _, Some(to)) if self.session.kind == Kind::Compare3 => {
                let from = (from as u8 - b'a') as usize;
                if from != to {
                    self.copy(from, to);
                }
            }
            _ => {}
        }
        EditorOutcome::Consumed
    }

    /// `a`, `b`, or `c` on its own: copy a region to the other text, keep a
    /// side in a merge, or, with three texts, start a copy from that text.
    fn letter(&mut self, letter: char) -> EditorOutcome {
        match (self.session.kind, letter) {
            (Kind::Compare, 'a') => self.copy(A, B),
            (Kind::Compare, 'b') => self.copy(B, A),
            (Kind::Merge, 'a') => self.keep(Variant::A),
            (Kind::Merge, 'b') => self.keep(Variant::B),
            (Kind::Compare3, letter) => self.pending = Some(letter),
            _ => {}
        }
        EditorOutcome::Consumed
    }

    /// Copy the current region from text `from` over text `to`.
    fn copy(&mut self, from: usize, to: usize) {
        let Some(index) = self.region() else {
            return;
        };
        self.session.copy_region(index, from, to);
        self.notice = Some(Notice::info(format!("region {} copied {} to {}", index + 1, letter_of(from), letter_of(to))).unlogged());
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
        self.current = self.current.map(|at| at.min(count.saturating_sub(1))).filter(|_| count > 0);
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
            self.notice = Some(Notice::info(format!("{} is not a file to write to", letter_of(side))));
            return;
        };
        // Through the buffer a tab has the file open in, where one does: that
        // tab then shows what was written, and holds no older copy a save of
        // its own could put back over it.
        let written = crate::editors::text::save_through(&path, text.buffer.lines())
            .unwrap_or_else(|| text_file::save(&path, text.buffer.lines(), text.shape));
        match written {
            Ok(_) => {
                text.buffer.mark_saved();
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
            .map(|index| self.session.regions[index].lines[side].start)
            .unwrap_or_else(|| self.session.sides[side].buffer.row());
        let buffer = &mut self.session.sides[side].buffer;
        buffer.move_to(line.min(buffer.lines().len() - 1), 0);
        self.modal.reset(buffer);
        self.editing = Some(side);
        self.notice = Some(Notice::info(format!("editing {}: C-c C-c or C-S-c goes back to the regions", letter_of(side))).unlogged());
    }

    /// Stop editing in place, back to the regions, compared afresh.
    fn stop_editing(&mut self) -> EditorOutcome {
        self.editing = None;
        self.caret = None;
        if self.session.kind != Kind::Merge {
            self.recompute();
        }
        EditorOutcome::Consumed
    }

    /// One key while a text is edited in place: `C-c C-c`, or `C-S-c` as in
    /// every edit mode, goes back to the regions.
    fn edit_key(&mut self, side: usize, key: &Key) -> EditorOutcome {
        if key.chord() == (Mods::CTRL_SHIFT, KeyCode::Char('c')) {
            self.edit_chord = false;
            return self.stop_editing();
        }
        if std::mem::take(&mut self.edit_chord) {
            if key.chord() == (Mods::CTRL, KeyCode::Char('c')) {
                return self.stop_editing();
            }
        } else if key.chord() == (Mods::CTRL, KeyCode::Char('c')) {
            self.edit_chord = true;
            return EditorOutcome::Consumed;
        }
        let rows = self.view.rows();
        let others: Vec<Vec<String>> = (0..self.session.sides.len())
            .filter(|other| *other != side)
            .map(|other| self.session.lines(other).to_vec())
            .collect();
        let others: Vec<&[String]> = others.iter().map(Vec::as_slice).collect();
        let before = self.session.sides[side].buffer.lines().len();
        let revision = self.session.sides[side].buffer.revision();
        let buffer = &mut self.session.sides[side].buffer;
        let outcome = self.modal.on_key(
            EditView {
                buffer,
                marks: &mut self.marks,
                others: &others,
                shown: Shown::default(),
                viewport: rows,
            },
            key,
        );
        let changed_from = self.session.sides[side].buffer.take_change();
        if self.session.sides[side].buffer.revision() != revision {
            let delta = self.session.sides[side].buffer.lines().len() as isize - before as isize;
            match self.session.kind {
                Kind::Merge => self.session.merge_edited(changed_from.unwrap_or(0), delta),
                Kind::Compare | Kind::Compare3 => self.session.recompute(),
            }
        }
        match outcome {
            EditOutcome::Save => self.write(side),
            EditOutcome::Quit { save } => {
                if save {
                    self.write(side);
                }
                self.editing = None;
                self.caret = None;
            }
            EditOutcome::Yank(text) => return EditorOutcome::Yank(text),
            EditOutcome::Paste => return EditorOutcome::Paste,
            EditOutcome::Ignored => return EditorOutcome::Ignored,
            _ => {}
        }
        EditorOutcome::Consumed
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
                .map_or_else(|| letter_of(side).to_string(), |name| name.to_string_lossy().into_owned());
            let copy = dir.join(format!("{sequence}-{}-{name}", letter_of(side)));
            let mut body = text.buffer.lines().join("\n");
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
            title: format!("{} vs {}", self.session.sides[A].label, self.session.sides[B].label),
        })
    }

    /// Say where the session stands: the region, how many differ, what a
    /// merge has resolved, and what is left out of the comparison.
    fn say_status(&mut self) {
        let count = self.session.regions.len();
        let at = self.current.map_or("none".to_string(), |at| (at + 1).to_string());
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
        let question = format!("{} not written. Quit anyway?", dirty.join(", "));
        EditorOutcome::Prompt(PromptRequest::confirm(ASK_QUIT, &question))
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
                    Some(text) => (text.buffer.revision(), text.path.as_deref().and_then(highlight::of_path)),
                    None => continue,
                },
                side => match self.session.sides.get(side) {
                    Some(text) => (
                        text.buffer.revision(),
                        text.path
                            .as_deref()
                            .or_else(|| self.session.sides[A].path.as_deref())
                            .and_then(highlight::of_path),
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
                            let (styles, next) = highlight::highlight(line, &state);
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
            header.extend(paint::header(&self.session, side, width, self.editing == Some(side)));
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
                painted.extend(paint::cell(&self.session, row, &column, widths, self.hscroll, self.current, refined.as_deref()));
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
            out.push(paint::header(&self.session, side, cols, self.editing == Some(side)));
            let column = Column {
                colors: &self.colors[side].1,
                side,
            };
            for at in 0..body {
                match aligned.get(self.view.scroll() + at) {
                    Some(row) => out.push(paint::cell(&self.session, row, &column, widths, self.hscroll, self.current, refined.as_deref())),
                    None => out.push(Vec::new()),
                }
            }
        }
        out
    }

    /// The widths a column `total` wide splits into.
    fn widths(&self, total: usize) -> Widths {
        let longest = [A, B, C, ANCESTOR].iter().map(|side| self.session.lines(*side).len()).max().unwrap_or(1);
        let digits = longest.to_string().len().max(3);
        Widths {
            gutter: digits + 2,
            total,
        }
    }

    /// The current region's changed words, while they are shown.
    fn refined(&self) -> Option<Vec<Vec<crate::editors::word_diff::Segment>>> {
        let index = self.current.filter(|_| self.auto_refine)?;
        Some(paint::refine(&self.session, index))
    }

    /// Keep the line being edited in view, and note where its caret is
    /// painted.
    fn place_caret(&mut self, aligned: &[Aligned], cols: usize) {
        let Some(side) = self.editing else {
            self.caret = None;
            return;
        };
        let row = self.session.sides[side].buffer.row();
        let Some(index) = aligned.iter().position(|aligned| aligned.lines[side] == Some(row)) else {
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
        let line = self.session.lines(side).get(row).cloned().unwrap_or_default();
        let display: usize = line
            .chars()
            .take(self.session.sides[side].buffer.col())
            .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0))
            .sum();
        if display < self.hscroll {
            self.hscroll = display;
        } else if display >= self.hscroll + width.saturating_sub(widths.gutter) {
            self.hscroll = display + 1 - width.saturating_sub(widths.gutter);
        }
        self.caret = Some((top + index - self.view.scroll(), left + widths.gutter + display - self.hscroll));
    }
}

impl Editor for DiffEditor {
    fn title(&self) -> String {
        let name = |side: usize| {
            self.session.sides.get(side).map_or_else(String::new, |text| {
                text.path
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .map_or_else(|| text.label.clone(), |name| name.to_string_lossy().into_owned())
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
            Some(side) => self.edit_key(side, &key.typed()),
            None => self.control_key(key),
        }
    }

    fn caret(&self) -> Option<Caret> {
        let (_, col) = self.caret?;
        Some(Caret {
            col,
            insert: self.modal.is_typing(),
        })
    }

    fn keys(&self) -> Vec<KeyHelp> {
        HELP.iter()
            .map(|(keys, label)| KeyHelp {
                keys: keys.to_string(),
                label: label.to_string(),
            })
            .collect()
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        self.scroll_by(-lines);
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
        let region = index.and_then(|index| self.aligned().get(self.view.scroll() + index).and_then(|row| row.region));
        if let Some(region) = region {
            self.current = Some(region);
        }
        EditorOutcome::Consumed
    }

    fn on_paste(&mut self, text: String) -> EditorOutcome {
        let Some(side) = self.editing else {
            return EditorOutcome::Consumed;
        };
        let rows = self.view.rows();
        let buffer = &mut self.session.sides[side].buffer;
        self.modal.paste_text(
            EditView {
                buffer,
                marks: &mut self.marks,
                others: &[],
                shown: Shown::default(),
                viewport: rows,
            },
            text,
        );
        if self.session.kind != Kind::Merge {
            self.session.recompute();
        }
        EditorOutcome::Consumed
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        match (reply.tag, reply.answer) {
            (ASK_JUMP, Some(answer)) => {
                match answer.trim().parse::<usize>().ok().filter(|n| (1..=self.session.regions.len()).contains(n)) {
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
        self.notice.take().or_else(|| self.modal.take_notice())
    }

    fn status(&self) -> EditorStatus {
        let count = self.session.regions.len();
        let at = self.current.map_or("-".to_string(), |at| (at + 1).to_string());
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
        if let Some(first) = self.pending {
            segments.push(format!("{first}-"));
        }
        EditorStatus {
            mode: self.editing.map(|_| self.modal.mode()),
            segments,
        }
    }

    fn is_dirty(&self) -> bool {
        (0..self.session.sides.len()).any(|side| self.session.is_dirty(side) && self.session.sides[side].path.is_some())
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.session.sides.iter().find_map(|text| text.path.as_ref()?.parent().map(PathBuf::from))
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
