//! Several cursors editing at once, as VS Code's and vim-visual-multi's do.
//!
//! The buffer keeps one cursor, the main one; the others are the file's, beside
//! it, each with modal state of its own: its mode, what it waits on, where its
//! selection is anchored, and its register. A key that edits or moves is
//! carried out at the main cursor as always, then again at each of the others
//! in turn from that cursor's own state, so `d` waits for its motion at every
//! cursor, `v` and a motion select at every cursor, and `yw` then `p` puts back
//! at each cursor the word it took. After each, the cursors not yet moved, and
//! their selections' anchors, follow the edit it made: the lines it touched are
//! compared before and after, the change read off them anchored at the cursor
//! that made it. Everything one cursor's keys already do, Insert's typing,
//! `ciw`, `>>`, `.`, works at all of them without a command of its own, and
//! the round is one undo step.
//!
//! An undo or a redo walks the history once, and the cursors follow it. The
//! clipboard is one: a paste of what the cursors last copied each puts back its
//! own, one of as many lines as there are cursors puts a line at each, and any
//! other puts the whole at every cursor.

use winter_model::editor::EditorOutcome;
use winter_model::input::Key;
use winter_model::textedit::buffer::{HistoryMark, Register};
use winter_model::textedit::modal::{shared, EditOutcome, Marks, ModalEditor, Selection};
use winter_model::vim::words::char_class;

use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// The most cursors one file takes: every occurrence of a short word in a
/// long file would otherwise cost a replay of each key thousands of times.
const MAX_CURSORS: usize = 1000;

/// What a command adding at an occurrence says with no word to look for.
const NO_WORD: &str = "no word under the cursor";

/// What a command adding a cursor says when there is nowhere left to add one.
const NO_MORE: &str = "every occurrence has a cursor";

// ========================================================================
// Data Structures
// ========================================================================

/// A file's cursors besides the main one.
#[derive(Clone, Debug, Default)]
pub(super) struct Cursors {
    /// The cursors, in the order they were added.
    pub(super) extra: Vec<Extra>,
    /// What the main cursor last put on the clipboard while the others
    /// were there, to tell a paste of what they copied from another.
    main_clip: Option<String>,
    /// What the occurrences they were added at are of, kept from the first
    /// so the later ones look for the same text.
    needle: Option<Needle>,
}

/// One cursor besides the main one.
#[derive(Clone, Debug)]
pub(super) struct Extra {
    /// Where it is, as a row and a column.
    pub(super) at: (usize, usize),
    /// What it last put on the clipboard, for a paste of what the cursors
    /// copied to put back its own.
    clip: Option<String>,
    /// Its modal state: its mode, what it waits on, where its selection is
    /// anchored.
    modal: ModalEditor,
    /// What it last yanked or deleted, for its own `p`.
    register: Register,
}

/// The text cursors are added at the occurrences of.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Needle {
    /// How far into an occurrence its cursor goes, as the main one was into
    /// the word it was on.
    offset: usize,
    text: Vec<char>,
    /// Whether only whole words count, as for the word under the cursor;
    /// a selection's text counts wherever it is.
    whole: bool,
}

/// The editor as it stood before a key, for carrying the key out at the
/// other cursors from the state the main one had.
pub(super) struct Round {
    depth: usize,
    history: HistoryMark,
    lines: Vec<String>,
    marks: Marks,
    modal: ModalEditor,
    pos: (usize, usize),
    shared: shared::Saved,
}

// ========================================================================
// Free functions
// ========================================================================

/// Move `points` past the edit that made `after` of `before`, made by the
/// cursor at `anchor`. The lines the edit touched are those between the
/// unchanged ones at both ends; within them the change is the run between
/// the characters alike at both ends, its start kept no later than the anchor
/// so that typing into a run of one letter is read where it was typed. A
/// point before the change stays, one after it moves by what it added or
/// took, and one inside text it replaced goes to where the change starts.
pub(super) fn follow(
    before: &[String],
    after: &[String],
    anchor: (usize, usize),
    points: &mut [(usize, usize)],
) {
    let top = before
        .iter()
        .zip(after)
        .take_while(|(old, new)| old == new)
        .count();
    if top == before.len() && top == after.len() {
        return;
    }
    let room = before.len().min(after.len()) - top;
    let bottom = before
        .iter()
        .rev()
        .zip(after.iter().rev())
        .take(room)
        .take_while(|(old, new)| old == new)
        .count();
    let (old_lines, new_lines) = (
        &before[top..before.len() - bottom],
        &after[top..after.len() - bottom],
    );
    let (old, new) = (joined(old_lines), joined(new_lines));
    let mut start = old
        .iter()
        .zip(&new)
        .take_while(|(old, new)| old == new)
        .count();
    if (top..top + old_lines.len()).contains(&anchor.0) {
        start = start.min(offset_of(old_lines, (anchor.0 - top, anchor.1)));
    }
    let tail_room = old.len().min(new.len()) - start;
    let tail = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(tail_room)
        .take_while(|(old, new)| old == new)
        .count();
    let (old_end, new_end) = (old.len() - tail, new.len() - tail);
    let first_below = before.len() - bottom;
    for point in points.iter_mut() {
        if point.0 < top {
            continue;
        }
        if point.0 >= first_below {
            point.0 = point.0 - first_below + (after.len() - bottom);
            continue;
        }
        let at = offset_of(old_lines, (point.0 - top, point.1));
        let moved = match at {
            at if at < start => at,
            at if at >= old_end => at - old_end + new_end,
            _ => start,
        };
        let (row, col) = position_of(new_lines, moved);
        *point = (top + row, col);
    }
}

/// `lines` as one run of characters, a newline between each two.
fn joined(lines: &[String]) -> Vec<char> {
    let mut chars = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        if at > 0 {
            chars.push('\n');
        }
        chars.extend(line.chars());
    }
    chars
}

/// Where `point` is in [`joined`] `lines`.
fn offset_of(lines: &[String], point: (usize, usize)) -> usize {
    let above: usize = lines[..point.0.min(lines.len())]
        .iter()
        .map(|line| line.chars().count() + 1)
        .sum();
    above + point.1
}

/// The row and column of `offset` in [`joined`] `lines`, held to the text.
fn position_of(lines: &[String], offset: usize) -> (usize, usize) {
    let mut left = offset;
    for (row, line) in lines.iter().enumerate() {
        let len = line.chars().count();
        if left <= len {
            return (row, left);
        }
        left -= len + 1;
    }
    let last = lines.len().saturating_sub(1);
    (last, lines.get(last).map_or(0, |line| line.chars().count()))
}

/// Where in `lines` `needle` occurs: every start of its text, a whole word
/// where it must be one.
fn occurrences(lines: &[String], needle: &Needle) -> Vec<(usize, usize)> {
    let keyword = char_class('a', false);
    let is_word = |c: Option<&char>| c.is_some_and(|c| char_class(*c, false) == keyword);
    let mut found = Vec::new();
    let len = needle.text.len();
    for (row, line) in lines.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        if len == 0 || chars.len() < len {
            continue;
        }
        for col in 0..=chars.len() - len {
            if chars[col..col + len] != needle.text[..] {
                continue;
            }
            let bounded = !is_word(col.checked_sub(1).and_then(|at| chars.get(at)))
                && !is_word(chars.get(col + len));
            if !needle.whole || bounded {
                found.push((row, col));
            }
        }
    }
    found
}

/// The word around `col` of `line`, as its first column and its characters.
fn word_around(line: &[char], col: usize) -> Option<(usize, Vec<char>)> {
    let keyword = char_class('a', false);
    let is_word = |at: usize| {
        line.get(at)
            .is_some_and(|c| char_class(*c, false) == keyword)
    };
    if !is_word(col) {
        return None;
    }
    let start = (0..=col).rev().take_while(|at| is_word(*at)).last()?;
    let end = (col..line.len()).take_while(|at| is_word(*at)).last()?;
    Some((start, line[start..=end].to_vec()))
}

/// What a paste of `text` puts at each cursor: the main one's, then each of
/// `extra`'s, with `order` the cursors' places in the text (the main one's
/// first) and `copied` what the main cursor last put on the clipboard.
fn pasted_at_each(
    text: &str,
    copied: Option<&str>,
    extra: &[Extra],
    order: &[(usize, usize)],
) -> (String, Vec<String>) {
    // What the cursors copied, each its own.
    let own: Option<Vec<String>> = extra.iter().map(|cursor| cursor.clip.clone()).collect();
    if let (Some(own), Some(copied)) = (own, copied) {
        if copied == text {
            return (text.to_string(), own);
        }
    }
    // As many lines as cursors, one each, in the order the cursors stand.
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() == order.len() && order.len() > 1 {
        let mut ranked: Vec<usize> = (0..order.len()).collect();
        ranked.sort_by_key(|at| order[*at]);
        let mut each = vec![String::new(); order.len()];
        for (line, at) in lines.iter().zip(ranked) {
            each[at] = line.to_string();
        }
        let main = each.remove(0);
        return (main, each);
    }
    (text.to_string(), vec![text.to_string(); extra.len()])
}

// ========================================================================
// TextEditor: carrying a key out at every cursor
// ========================================================================

impl TextEditor {
    /// The editor as it stands before a key, where the file has cursors
    /// besides the main one; `None` where it has only that one.
    pub(super) fn begin_round(&self) -> Option<Round> {
        if self.doc().cursors.extra.is_empty() {
            return None;
        }
        let buffer = self.buffer();
        Some(Round {
            depth: buffer.undo_depth(),
            history: buffer.history_mark(),
            lines: buffer.lines().to_vec(),
            marks: self.doc().marks.clone(),
            modal: self.modal.clone(),
            pos: (buffer.row(), buffer.col()),
            shared: shared::save(),
        })
    }

    /// Carry `edit` out at every other cursor, now the main one has, each
    /// from its own state, and leave the editor as the main one's edit left
    /// it. `edit` is handed the cursor's place in the list, and gives back
    /// what it put on the clipboard.
    pub(super) fn end_round(
        &mut self,
        round: Option<Round>,
        edit: impl Fn(&mut Self, usize) -> Option<String>,
    ) {
        let Some(round) = round else {
            return;
        };
        let mut extra = std::mem::take(&mut self.doc_mut().cursors.extra);
        let mut points = points_of(&extra);
        follow(&round.lines, self.buffer().lines(), round.pos, &mut points);
        set_points(&mut extra, &points);
        if round.history.walked_to(&self.buffer().history_mark()) {
            self.doc_mut().cursors.extra = extra;
            self.tidy_cursors();
            return;
        }
        let main_modal = self.modal.clone();
        let main_marks = self.doc().marks.clone();
        let main_shared = shared::save();
        let main_register = self.buffer().register().clone();
        let mut main = (self.buffer().row(), self.buffer().col());
        let mut main_anchor = main_modal.anchor_point();
        for at in 0..extra.len() {
            let before = self.buffer().lines().to_vec();
            let start = extra[at].at;
            // A cursor whose mode fell out of step with the main one's (a
            // command that is no key changed the main one's) takes the main
            // one's state, selecting from where it stands.
            self.modal = match extra[at].modal.mode() == round.modal.mode() {
                true => extra[at].modal.clone(),
                false => {
                    let mut modal = round.modal.clone();
                    modal.set_anchor_point(start);
                    modal
                }
            };
            self.doc_mut().marks = round.marks.clone();
            shared::restore(round.shared.clone());
            self.buffer_mut().set_register(extra[at].register.clone());
            self.place_cursor_at(start);
            let clip = edit(self, at);
            let mut others: Vec<(usize, usize)> = Vec::new();
            for (other, cursor) in extra.iter().enumerate() {
                if other != at {
                    others.push(cursor.at);
                    others.push(cursor.modal.anchor_point().unwrap_or(cursor.at));
                }
            }
            others.push(main);
            others.push(main_anchor.unwrap_or(main));
            follow(&before, self.buffer().lines(), start, &mut others);
            main_anchor = main_anchor.map(|_| others[others.len() - 1]);
            main = others[others.len() - 2];
            let mut moved = others.chunks(2);
            for (other, cursor) in extra.iter_mut().enumerate() {
                if other == at {
                    continue;
                }
                if let Some([to, anchor]) = moved.next() {
                    cursor.at = *to;
                    cursor.modal.set_anchor_point(*anchor);
                }
            }
            let cursor = &mut extra[at];
            cursor.at = (self.buffer().row(), self.buffer().col());
            cursor.modal = self.modal.clone();
            cursor.register = self.buffer().register().clone();
            if clip.is_some() {
                cursor.clip = clip;
            }
        }
        self.modal = main_modal;
        if let Some(anchor) = main_anchor {
            self.modal.set_anchor_point(anchor);
        }
        self.doc_mut().marks = main_marks;
        shared::restore(main_shared);
        self.buffer_mut().set_register(main_register);
        self.place_cursor_at(main);
        self.buffer_mut().join_steps(round.depth);
        self.doc_mut().cursors.extra = extra;
        self.tidy_cursors();
    }

    /// `key` at the cursor the buffer has now, as typing it there edits: the
    /// file's own reading of it first (a list going on past `Enter`), else
    /// the modal keys'. What it asks of the window is the main cursor's to
    /// ask, so it is dropped here, all but what it copied.
    pub(super) fn edit_here(&mut self, key: &Key) -> Option<String> {
        if self.context_key(key).is_some() {
            return None;
        }
        match self.with_modal(|modal, view| modal.on_key(view, key)) {
            EditOutcome::Yank(text) | EditOutcome::Cut(text) => Some(text),
            _ => None,
        }
    }

    /// Remember what the main cursor's key put on the clipboard, while
    /// there are others, to know a paste of it from a paste of another.
    pub(super) fn note_main_copy(&mut self, outcome: &EditOutcome) {
        if self.doc().cursors.extra.is_empty() {
            return;
        }
        if let EditOutcome::Yank(text) | EditOutcome::Cut(text) = outcome {
            self.doc_mut().cursors.main_clip = Some(text.clone());
        }
    }

    /// Paste `text` at every cursor, each its own share of it (see
    /// [`pasted_at_each`]), and say what the main cursor's paste does.
    pub(super) fn paste_at_every_cursor(&mut self, text: String) -> EditorOutcome {
        let round = self.begin_round();
        let order: Vec<(usize, usize)> =
            std::iter::once((self.buffer().row(), self.buffer().col()))
                .chain(self.doc().cursors.extra.iter().map(|cursor| cursor.at))
                .collect();
        let cursors = &self.doc().cursors;
        let (main, each) =
            pasted_at_each(&text, cursors.main_clip.as_deref(), &cursors.extra, &order);
        let outcome = self.paste_text(main);
        self.end_round(round, |this, at| {
            let _ = this.paste_text(each[at].clone());
            None
        });
        outcome
    }

    /// Put the buffer's cursor at `point`, past the line's end where the
    /// cursor is typing.
    fn place_cursor_at(&mut self, (row, col): (usize, usize)) {
        match self.modal.is_typing() {
            true => self.buffer_mut().move_to_typing(row, col),
            false => self.buffer_mut().move_to(row, col),
        }
    }

    /// Keep each cursor inside the text and on a place no other one holds.
    fn tidy_cursors(&mut self) {
        let typing = self.modal.is_typing();
        let main = (self.buffer().row(), self.buffer().col());
        let lines = self.buffer().lines().to_vec();
        let held = |(row, col): (usize, usize)| {
            let row = row.min(lines.len().saturating_sub(1));
            let len = lines.get(row).map_or(0, |line| line.chars().count());
            match typing {
                true => (row, col.min(len)),
                false => (row, col.min(len.saturating_sub(1))),
            }
        };
        let mut kept: Vec<Extra> = Vec::new();
        for mut cursor in std::mem::take(&mut self.doc_mut().cursors.extra) {
            cursor.at = held(cursor.at);
            if let Some(anchor) = cursor.modal.anchor_point() {
                cursor.modal.set_anchor_point(held(anchor));
            }
            if cursor.at != main && !kept.iter().any(|other| other.at == cursor.at) {
                kept.push(cursor);
            }
        }
        if kept.is_empty() {
            self.doc_mut().cursors = Cursors::default();
            return;
        }
        self.doc_mut().cursors.extra = kept;
    }
}

/// Each cursor's place and its selection's anchor (its place where it
/// selects nothing), two to a cursor.
fn points_of(extra: &[Extra]) -> Vec<(usize, usize)> {
    extra
        .iter()
        .flat_map(|cursor| [cursor.at, cursor.modal.anchor_point().unwrap_or(cursor.at)])
        .collect()
}

/// Put back what [`points_of`] took, each moved as it now is.
fn set_points(extra: &mut [Extra], points: &[(usize, usize)]) {
    for (cursor, pair) in extra.iter_mut().zip(points.chunks(2)) {
        if let [at, anchor] = pair {
            cursor.at = *at;
            cursor.modal.set_anchor_point(*anchor);
        }
    }
}

// ========================================================================
// TextEditor: adding and taking away cursors
// ========================================================================

impl TextEditor {
    /// Add a cursor at the next occurrence, after the cursor added last:
    /// selecting it, where the main cursor selects.
    pub(super) fn add_cursor_at_next(&mut self) -> EditorOutcome {
        let Some(needle) = self.needle() else {
            return self.say(NO_WORD);
        };
        let after = self.last_cursor();
        self.add_occurrence_after(&needle, after)
    }

    /// Add a cursor at every occurrence.
    pub(super) fn add_cursor_at_every(&mut self) -> EditorOutcome {
        let Some(needle) = self.needle() else {
            return self.say(NO_WORD);
        };
        let found = occurrences(self.buffer().lines(), &needle);
        for start in found {
            if self.doc().cursors.extra.len() + 1 >= MAX_CURSORS {
                break;
            }
            let cursor = self.cursor_at_occurrence(&needle, start);
            if self.is_free(cursor.at) {
                self.doc_mut().cursors.extra.push(cursor);
            }
        }
        self.tidy_cursors();
        EditorOutcome::Consumed
    }

    /// Add a cursor on the line below the lowest cursor (`down`), or above
    /// the highest, in its column, selecting from the same column as the
    /// main cursor's selection does.
    pub(super) fn add_cursor_vertically(&mut self, down: bool) -> EditorOutcome {
        let main = (self.buffer().row(), self.buffer().col());
        let all = self
            .doc()
            .cursors
            .extra
            .iter()
            .map(|cursor| cursor.at)
            .chain([main]);
        let edge = match down {
            true => all.max_by_key(|point| point.0),
            false => all.min_by_key(|point| point.0),
        };
        let Some((row, col)) = edge else {
            return EditorOutcome::Consumed;
        };
        let last = self.buffer().lines().len().saturating_sub(1);
        let row = match down {
            true if row < last => row + 1,
            false if row > 0 => row - 1,
            _ => return self.say("no line to add a cursor on"),
        };
        let mut cursor = self.new_cursor((row, col));
        if let Some((_, anchor_col)) = self.modal.anchor_point() {
            cursor.modal.set_anchor_point((row, anchor_col));
        }
        if self.doc().cursors.extra.len() + 1 < MAX_CURSORS {
            self.doc_mut().cursors.extra.push(cursor);
        }
        self.tidy_cursors();
        EditorOutcome::Consumed
    }

    /// Move the cursor added last on to the occurrence after it.
    pub(super) fn skip_cursor(&mut self) -> EditorOutcome {
        let Some(needle) = self.needle() else {
            return self.say(NO_WORD);
        };
        let after = self.last_cursor();
        self.doc_mut().cursors.extra.pop();
        self.add_occurrence_after(&needle, after)
    }

    /// Take away the cursor added last.
    pub(super) fn remove_last_cursor(&mut self) -> EditorOutcome {
        self.doc_mut().cursors.extra.pop();
        self.tidy_cursors();
        EditorOutcome::Consumed
    }

    /// Take away every cursor but the main one.
    pub(super) fn clear_cursors(&mut self) {
        self.doc_mut().cursors = Cursors::default();
    }

    /// How many cursors there are, for the status line, where there are more
    /// than one.
    pub(super) fn cursors_status(&self) -> Option<String> {
        let count = self.doc().cursors.extra.len();
        (count > 0).then(|| format!("{} cursors", count + 1))
    }

    /// The cursors besides the main one on line `row`, by column.
    pub(super) fn cursors_on(&self, row: usize) -> Vec<usize> {
        self.doc()
            .cursors
            .extra
            .iter()
            .filter(|cursor| cursor.at.0 == row)
            .map(|cursor| cursor.at.1)
            .collect()
    }

    /// What the cursors besides the main one select.
    pub(super) fn cursor_selections(&self) -> Vec<Selection> {
        self.doc()
            .cursors
            .extra
            .iter()
            .filter_map(|cursor| cursor.modal.selection_at(cursor.at))
            .collect()
    }

    /// What occurrences are looked for: the one the first cursor added was
    /// at, else the selection's text on one line, else the word under the
    /// main cursor.
    fn needle(&mut self) -> Option<Needle> {
        if let Some(needle) = self.doc().cursors.needle.clone() {
            return Some(needle);
        }
        let selected = self
            .selection()
            .filter(|chosen| !chosen.linewise && !chosen.block && chosen.start.0 == chosen.end.0);
        let needle = match selected {
            Some(chosen) => {
                let line: Vec<char> = self.buffer().lines()[chosen.start.0].chars().collect();
                let end = (chosen.end.1 + 1).min(line.len());
                Needle {
                    offset: 0,
                    text: line[chosen.start.1.min(end)..end].to_vec(),
                    whole: false,
                }
            }
            None => {
                let line: Vec<char> = self.buffer().lines()[self.buffer().row()].chars().collect();
                let col = self.buffer().col();
                let (start, text) = word_around(&line, col)?;
                Needle {
                    offset: col - start,
                    text,
                    whole: true,
                }
            }
        };
        if needle.text.is_empty() {
            return None;
        }
        self.doc_mut().cursors.needle = Some(needle.clone());
        Some(needle)
    }

    /// A cursor for the occurrence of `needle` starting at `start`: over it,
    /// where the main cursor selects, else as far into it as the main one is
    /// into its word.
    fn cursor_at_occurrence(&self, needle: &Needle, start: (usize, usize)) -> Extra {
        match self.modal.is_selecting() {
            true => {
                let end = (start.0, start.1 + needle.text.len().saturating_sub(1));
                let mut cursor = self.new_cursor(end);
                cursor.modal.set_anchor_point(start);
                cursor
            }
            false => self.new_cursor((start.0, start.1 + needle.offset)),
        }
    }

    /// A cursor at `at`, in the main one's state: its mode, its register.
    fn new_cursor(&self, at: (usize, usize)) -> Extra {
        Extra {
            at,
            clip: None,
            modal: self.modal.clone(),
            register: self.buffer().register().clone(),
        }
    }

    /// Whether no cursor stands at `point`.
    fn is_free(&self, point: (usize, usize)) -> bool {
        let main = (self.buffer().row(), self.buffer().col());
        point != main
            && !self
                .doc()
                .cursors
                .extra
                .iter()
                .any(|cursor| cursor.at == point)
    }

    /// Add a cursor at the first occurrence of `needle` after `after` that
    /// holds none, going round past the end.
    fn add_occurrence_after(&mut self, needle: &Needle, after: (usize, usize)) -> EditorOutcome {
        let candidates: Vec<Extra> = occurrences(self.buffer().lines(), needle)
            .into_iter()
            .map(|start| self.cursor_at_occurrence(needle, start))
            .collect();
        let next = candidates
            .iter()
            .filter(|cursor| cursor.at > after)
            .chain(candidates.iter())
            .find(|cursor| self.is_free(cursor.at))
            .cloned();
        let Some(cursor) = next else {
            return self.say(NO_MORE);
        };
        if self.doc().cursors.extra.len() + 1 < MAX_CURSORS {
            self.doc_mut().cursors.extra.push(cursor);
        }
        EditorOutcome::Consumed
    }

    /// The cursor added last, else the main one.
    fn last_cursor(&self) -> (usize, usize) {
        self.doc()
            .cursors
            .extra
            .last()
            .map(|cursor| cursor.at)
            .unwrap_or((self.buffer().row(), self.buffer().col()))
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_string).collect()
    }

    fn followed(
        before: &str,
        after: &str,
        anchor: (usize, usize),
        points: &[(usize, usize)],
    ) -> Vec<(usize, usize)> {
        let mut points = points.to_vec();
        follow(&lines(before), &lines(after), anchor, &mut points);
        points
    }

    fn extra(clip: Option<&str>) -> Extra {
        Extra {
            at: (0, 0),
            clip: clip.map(str::to_string),
            modal: ModalEditor::new(),
            register: Register::default(),
        }
    }

    #[test]
    fn test_a_point_after_typing_on_its_line_moves_on_and_one_before_stays() {
        assert_eq!(
            followed("ab cd", "abX cd", (0, 2), &[(0, 0), (0, 3), (0, 4)]),
            vec![(0, 0), (0, 4), (0, 5)]
        );
    }

    #[test]
    fn test_typing_into_a_run_of_one_letter_is_read_where_it_was_typed() {
        // `aa` typed into at the start reads alike as typed at the end; the
        // anchor says it was the start, so the point at the end moves on.
        assert_eq!(followed("aa", "aaa", (0, 0), &[(0, 2)]), vec![(0, 3)]);
    }

    #[test]
    fn test_points_on_lines_below_a_new_line_move_down_and_those_above_stay() {
        assert_eq!(
            followed(
                "one\ntwo\nthree",
                "one\nt\nwo\nthree",
                (1, 1),
                &[(0, 1), (1, 2), (2, 3)]
            ),
            vec![(0, 1), (2, 1), (3, 3)]
        );
    }

    #[test]
    fn test_a_point_on_a_deleted_line_goes_where_the_deletion_was() {
        assert_eq!(
            followed("one\ntwo\nthree", "one\nthree", (1, 0), &[(1, 2), (2, 1)]),
            vec![(1, 0), (1, 1)]
        );
    }

    #[test]
    fn test_joining_a_line_up_moves_its_points_onto_the_line_above() {
        assert_eq!(followed("ab\ncd", "abcd", (1, 0), &[(1, 1)]), vec![(0, 3)]);
    }

    #[test]
    fn test_occurrences_of_a_word_are_whole_words_unless_selected() {
        let text = lines("cat concat cat_x cat");
        let word = Needle {
            offset: 1,
            text: "cat".chars().collect(),
            whole: true,
        };
        assert_eq!(occurrences(&text, &word), vec![(0, 0), (0, 17)]);
        let selected = Needle {
            offset: 0,
            whole: false,
            ..word
        };
        assert_eq!(
            occurrences(&text, &selected),
            vec![(0, 0), (0, 7), (0, 11), (0, 17)]
        );
    }

    #[test]
    fn test_a_paste_of_what_the_cursors_copied_puts_back_each_its_own() {
        let cursors = [extra(Some("two")), extra(Some("three"))];
        let order = [(0, 0), (1, 0), (2, 0)];
        assert_eq!(
            pasted_at_each("one", Some("one"), &cursors, &order),
            (
                "one".to_string(),
                vec!["two".to_string(), "three".to_string()]
            )
        );
    }

    #[test]
    fn test_a_paste_of_one_line_a_cursor_goes_a_line_to_each_in_text_order() {
        // The main cursor stands last in the text, so it takes the last line.
        let cursors = [extra(None), extra(None)];
        let order = [(5, 0), (1, 0), (3, 0)];
        assert_eq!(
            pasted_at_each("a\nb\nc", None, &cursors, &order),
            ("c".to_string(), vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn test_any_other_paste_puts_the_whole_at_every_cursor() {
        let cursors = [extra(Some("x"))];
        let order = [(0, 0), (1, 0)];
        assert_eq!(
            pasted_at_each("other", Some("mine"), &cursors, &order),
            ("other".to_string(), vec!["other".to_string()])
        );
    }
}
