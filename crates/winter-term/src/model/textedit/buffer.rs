//! The text under the cursor: lines, the point moving over them, and every
//! edit that changes either. Pure text, no keys and no files.
//!
//! A buffer may hold its line count fixed, for text whose lines stand for
//! something outside it (one name per listed entry, a one-line answer): the
//! edits that would split or join lines then do nothing, and the ones that
//! would remove whole lines empty them instead.

use crate::model::vim::motion::FindChar;
use crate::model::vim::objects::{matching_bracket, paragraph_edge, TextRows};
use crate::model::vim::words;
use crate::model::vim::words::{
    first_non_blank, last_non_blank, next_word_start, prev_word_end, prev_word_start, word_end,
};

// ========================================================================
// Constants
// ========================================================================

/// How many undo steps a buffer keeps: deep enough to walk back out of a bad
/// run of edits, bounded so a long session's snapshots cannot grow without
/// limit over a file already capped at a few megabytes.
const MAX_UNDO: usize = 200;

/// How many spaces one level of indentation is worth, for a file that indents
/// with them rather than with tabs.
const INDENT_SPACES: usize = 4;

// ========================================================================
// Data Structures
// ========================================================================

/// One file's text and the cursor over it, with the edits that can be undone.
#[derive(Clone, Debug)]
pub struct TextBuffer {
    /// The first line changed since anything last asked, for a reader that
    /// keeps something per line and has to know how much of it still holds.
    changed_from: Option<usize>,
    col: usize,
    /// Whether the number of lines is held where it started.
    fixed: bool,
    /// The column a vertical move aims for, so stepping through a short line
    /// returns to where the cursor was rather than where the short line ended.
    goal: usize,
    lines: Vec<String>,
    /// The name the next undo step takes. Names are never reused, so a state
    /// of the text is known by the step that produced it.
    next_step: u64,
    redo: Vec<Step>,
    /// How many changes the text has been through, so a caller can tell one
    /// command's worth of them from none at all.
    revision: usize,
    register: Register,
    row: usize,
    /// The state the text was in when it was last read or written, so an
    /// undo back to it leaves nothing to save.
    saved: StateId,
    undo: Vec<Step>,
}

/// What a delete or a yank left behind. The buffer holds one at a time, the
/// unnamed one; which register that came from or goes to is the caller's.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Register {
    /// Whether it holds whole lines, which paste above or below rather than
    /// inside the line the cursor is on.
    pub linewise: bool,
    /// The characters taken, newline-separated when it holds whole lines.
    pub text: String,
}

/// One undoable command: where the cursor was when it started, and every
/// change it made, in the order it made them.
#[derive(Clone, Debug, Default)]
struct Step {
    col: usize,
    edits: Vec<Edit>,
    /// Which step this is, kept across undo and redo.
    id: u64,
    row: usize,
}

/// A state of the text, named by the last step that changed it and how many
/// of that step's changes were in by then: a run of typing is one step, and a
/// save part-way through it is a state of its own.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct StateId {
    changes: usize,
    step: u64,
}

/// One change to the lines, as the little that has to be kept to put it back:
/// where it started, what was there, and how many lines stand there now. The
/// cost of an edit is the size of the edit, never the size of the file.
#[derive(Clone, Debug)]
struct Edit {
    from: usize,
    /// How many lines the change left in place of `removed`.
    inserted: usize,
    /// The lines that were there before it.
    removed: Vec<String>,
}

// ========================================================================
// TextBuffer: state
// ========================================================================

impl TextBuffer {
    /// A buffer over `lines`, with the cursor at the start.
    pub fn new(lines: Vec<String>) -> Self {
        Self {
            changed_from: None,
            col: 0,
            fixed: false,
            goal: 0,
            lines: if lines.is_empty() {
                vec![String::new()]
            } else {
                lines
            },
            next_step: 1,
            redo: Vec::new(),
            register: Register::default(),
            revision: 0,
            row: 0,
            saved: StateId::default(),
            undo: Vec::new(),
        }
    }

    /// The same buffer with its line count held fixed: nothing typed or
    /// deleted adds a line or takes one away.
    pub fn with_fixed_lines(mut self) -> Self {
        self.fixed = true;
        self
    }

    /// Whether the line count is held fixed, which is what makes `Enter` an
    /// answer rather than a line break.
    pub fn is_fixed(&self) -> bool {
        self.fixed
    }

    /// The text, one string per line, with no terminators.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// The line the cursor is on, counting from zero.
    pub fn row(&self) -> usize {
        self.row
    }

    /// The character the cursor is on within its line, counting from zero.
    pub fn col(&self) -> usize {
        self.col
    }

    /// Whether the text has changed since it was last read or written, which
    /// is the whole of what closing has to ask about.
    pub fn is_dirty(&self) -> bool {
        self.state() != self.saved
    }

    /// The state the text is in: the last step that changed anything, since
    /// a step that changed nothing leaves the text as it found it.
    fn state(&self) -> StateId {
        self.undo
            .iter()
            .rev()
            .find(|step| !step.edits.is_empty())
            .map_or(StateId::default(), |step| StateId {
                changes: step.edits.len(),
                step: step.id,
            })
    }

    /// A number that moves whenever the text does, so a caller can tell a
    /// command that changed something from one that only moved the cursor.
    pub fn revision(&self) -> usize {
        self.revision
    }

    /// Take the current text as the saved state, after it has been written.
    pub fn mark_saved(&mut self) {
        self.saved = self.state();
    }

    /// The first line changed since this was last asked, and forget it: a
    /// reader keeping something per line holds everything above that line and
    /// has to work the rest out again.
    pub fn take_change(&mut self) -> Option<usize> {
        self.changed_from.take()
    }

    /// Note that `row` and everything under it may have moved or changed.
    fn touch(&mut self, row: usize) {
        self.changed_from = Some(match self.changed_from {
            Some(first) => first.min(row),
            None => row,
        });
        self.revision += 1;
    }

    /// The line the cursor is on, as characters: every column in the buffer
    /// counts characters, never bytes, so a line of CJK or accents edits the
    /// same way an ASCII one does.
    pub fn chars(&self) -> Vec<char> {
        self.line_chars(self.row)
    }

    fn line_chars(&self, row: usize) -> Vec<char> {
        self.lines
            .get(row)
            .map(|line| line.chars().collect())
            .unwrap_or_default()
    }

    /// How many characters row `row` holds.
    pub fn line_len_of(&self, row: usize) -> usize {
        self.lines.get(row).map_or(0, |line| line.chars().count())
    }

    /// How many characters the cursor's line holds.
    pub fn line_len(&self) -> usize {
        self.line_len_of(self.row)
    }

    /// Put the cursor at `row` and `col`, held inside the text.
    pub fn move_to(&mut self, row: usize, col: usize) {
        self.row = row.min(self.lines.len().saturating_sub(1));
        self.col = col;
        self.goal = col;
        self.clamp(false);
    }

    /// Put the cursor at `row` and `col` the way typing may: one past the
    /// line's last character is a place to type.
    pub fn move_to_typing(&mut self, row: usize, col: usize) {
        self.row = row;
        self.col = col;
        self.goal = col;
        self.clamp(true);
    }

    /// Put the cursor on `row`, counting from zero, at its first non-blank.
    pub fn move_to_line(&mut self, row: usize) {
        self.row = row.min(self.lines.len().saturating_sub(1));
        self.col = first_non_blank(&self.chars());
        self.goal = self.col;
    }

    /// Hold the cursor inside the text. Insert mode may sit one past the last
    /// character, where the next typed one goes; Normal mode may not, since
    /// there is nothing under the cursor there to act on.
    pub fn clamp(&mut self, insert: bool) {
        self.row = self.row.min(self.lines.len().saturating_sub(1));
        let len = self.line_len_of(self.row);
        let last = if insert { len } else { len.saturating_sub(1) };
        self.col = self.col.min(last);
    }
}

impl TextRows for TextBuffer {
    fn row_count(&self) -> usize {
        self.lines.len()
    }

    fn row_chars(&self, row: usize) -> Vec<char> {
        self.line_chars(row)
    }
}

// ========================================================================
// TextBuffer: motions
// ========================================================================

impl TextBuffer {
    /// Back `count` characters, stopping at the start of the line.
    pub fn move_left(&mut self, count: usize) {
        self.col = self.col.saturating_sub(count);
        self.goal = self.col;
    }

    /// On `count` characters, stopping at the end of the line.
    pub fn move_right(&mut self, count: usize, insert: bool) {
        self.col += count;
        self.clamp(insert);
        self.goal = self.col;
    }

    /// Down `count` lines, keeping the column the cursor was aiming for rather
    /// than the one a shorter line on the way clipped it to.
    pub fn move_down(&mut self, count: usize, insert: bool) {
        self.row = (self.row + count).min(self.lines.len().saturating_sub(1));
        self.land_on_goal(insert);
    }

    /// Up `count` lines, keeping the column being aimed for.
    pub fn move_up(&mut self, count: usize, insert: bool) {
        self.row = self.row.saturating_sub(count);
        self.land_on_goal(insert);
    }

    /// To column zero (`0`).
    pub fn move_line_start(&mut self) {
        self.col = 0;
        self.goal = 0;
    }

    /// To the end of the line (`$`).
    pub fn move_line_end(&mut self, insert: bool) {
        self.col = self.line_len_of(self.row);
        self.clamp(insert);
        // `$` is sticky in Vim: stepping down from it lands on the next line's
        // end however long that line is.
        self.goal = usize::MAX;
    }

    /// To the line's first non-blank character (`^`).
    pub fn move_first_non_blank(&mut self) {
        self.col = first_non_blank(&self.chars());
        self.goal = self.col;
    }

    /// To the line's last non-blank character (`g_`).
    pub fn move_last_non_blank(&mut self) {
        self.col = last_non_blank(&self.chars());
        self.goal = self.col;
    }

    /// To the first line (`gg`).
    pub fn move_top(&mut self) {
        self.move_to_line(0);
    }

    /// To the last line (`G`).
    pub fn move_bottom(&mut self) {
        self.move_to_line(self.lines.len().saturating_sub(1));
    }

    /// Forward to the next word start, crossing into the following line when
    /// the rest of this one holds no further word.
    pub fn move_word_forward(&mut self, count: usize, big: bool) {
        for _ in 0..count {
            match next_word_start(&self.chars(), self.col, big) {
                Some(col) => self.col = col,
                None if self.row + 1 < self.lines.len() => {
                    self.row += 1;
                    self.col = first_non_blank(&self.chars());
                }
                None => self.col = self.line_len_of(self.row).saturating_sub(1),
            }
        }
        self.goal = self.col;
    }

    /// Back to the previous word start, crossing into the line above when
    /// nothing precedes the cursor on this one.
    pub fn move_word_back(&mut self, count: usize, big: bool) {
        for _ in 0..count {
            match prev_word_start(&self.chars(), self.col, big) {
                Some(col) => self.col = col,
                None if self.row > 0 => {
                    self.row -= 1;
                    let chars = self.chars();
                    self.col = prev_word_start(&chars, chars.len(), big).unwrap_or(0);
                }
                None => self.col = 0,
            }
        }
        self.goal = self.col;
    }

    /// On to the end of the next word, crossing into the line below when this
    /// one holds no further word.
    pub fn move_word_end(&mut self, count: usize, big: bool) {
        for _ in 0..count {
            match word_end(&self.chars(), self.col, big) {
                Some(col) => self.col = col,
                None if self.row + 1 < self.lines.len() => {
                    self.row += 1;
                    let chars = self.chars();
                    self.col = word_end(&chars, 0, big).unwrap_or(0);
                }
                None => self.col = self.line_len_of(self.row).saturating_sub(1),
            }
        }
        self.goal = self.col;
    }

    /// Back to the end of the previous word (`ge`/`gE`), crossing into the
    /// line above when nothing precedes the cursor on this one.
    pub fn move_word_end_back(&mut self, count: usize, big: bool) {
        for _ in 0..count {
            match prev_word_end(&self.chars(), self.col, big) {
                Some(col) => self.col = col,
                None if self.row > 0 => {
                    self.row -= 1;
                    let chars = self.chars();
                    self.col = prev_word_end(&chars, chars.len(), big).unwrap_or(0);
                }
                None => self.col = 0,
            }
        }
        self.goal = self.col;
    }

    /// To the next or previous paragraph boundary (`{`/`}`), which is the
    /// blank line past the run of text the cursor is in.
    pub fn move_paragraph(&mut self, count: usize, forward: bool) {
        for _ in 0..count {
            self.row = paragraph_edge(self, self.row, forward);
        }
        self.col = 0;
        self.goal = 0;
        self.clamp(false);
    }

    /// To the bracket matching the first one at or right of the cursor (`%`),
    /// which may be lines away. Nothing happens where nothing matches.
    pub fn move_matching_bracket(&mut self) {
        if let Some((row, col)) = matching_bracket(self, (self.row, self.col)) {
            self.row = row;
            self.col = col;
            self.goal = col;
        }
    }

    /// To the `count`th match of a char search on this line (`f`/`F`/`t`/`T`),
    /// reporting whether one was found: a search that fails moves nothing, so
    /// an operator waiting on it has nothing to reach over.
    pub fn move_to_char(&mut self, find: FindChar, count: usize) -> bool {
        let chars = self.chars();
        let mut at = self.col;
        for _ in 0..count {
            match words::find_char(&chars, at, find) {
                Some(col) => at = col,
                None => return false,
            }
        }
        self.col = at;
        self.goal = at;
        true
    }

    /// The column a vertical move should land on: the one being aimed for,
    /// clipped to the line actually arrived at.
    fn land_on_goal(&mut self, insert: bool) {
        self.col = self.goal;
        let goal = self.goal;
        self.clamp(insert);
        self.goal = goal;
    }
}

// ========================================================================
// TextBuffer: edits
// ========================================================================

impl TextBuffer {
    /// Open an undo step, so everything changed from here until the next one
    /// is put back together. Every command opens one first; an insert-mode
    /// run opens one on entry, so undo steps back over the typing rather than
    /// over each character.
    pub fn snapshot(&mut self) {
        // A command that changed nothing leaves an empty step, which would
        // otherwise cost a press of `u` to walk back over.
        if self.undo.last().is_some_and(|step| step.edits.is_empty()) {
            self.undo.pop();
        }
        let id = self.take_step_id();
        self.undo.push(Step {
            col: self.col,
            edits: Vec::new(),
            id,
            row: self.row,
        });
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        // A new edit is a new branch: what was undone is no longer ahead.
        self.redo.clear();
    }

    /// Replace `remove` lines at `from` with `insert`, recording what it took
    /// out so the step being built can put it back. Every change to the text
    /// goes through here, which is what keeps undo exact and cheap.
    fn splice(&mut self, from: usize, remove: usize, insert: Vec<String>) {
        // The edits that reshape the text check for a fixed count before
        // they move the cursor; this is the backstop for any that do not.
        let removed = (from + remove).min(self.lines.len()) - from.min(self.lines.len());
        if self.fixed && insert.len() != removed {
            return;
        }
        if self.undo.is_empty() {
            let id = self.take_step_id();
            self.undo.push(Step {
                id,
                ..Step::default()
            });
        }
        let edit = self.apply(from, remove, insert);
        if let Some(step) = self.undo.last_mut() {
            step.edits.push(edit);
        }
    }

    /// Carry out one splice and report its inverse, without recording it.
    fn apply(&mut self, from: usize, remove: usize, insert: Vec<String>) -> Edit {
        let from = from.min(self.lines.len());
        let end = (from + remove).min(self.lines.len());
        let inserted = insert.len();
        let removed: Vec<String> = self.lines.splice(from..end, insert).collect();
        // Nothing indexes into an empty buffer, so a line always remains.
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.touch(from);
        Edit {
            from,
            inserted,
            removed,
        }
    }

    /// Type `c` where the cursor is, and step past it.
    pub fn insert_char(&mut self, c: char) {
        let mut chars = self.chars();
        let at = self.col.min(chars.len());
        chars.insert(at, c);
        self.set_line(self.row, chars.into_iter().collect());
        self.col = at + 1;
        self.goal = self.col;
    }

    /// Split the line at the cursor, carrying what follows onto a new one and
    /// the indent with it: a line that starts back at column zero is a retype
    /// of the indentation every time, in code that is nothing but indented.
    pub fn insert_newline(&mut self) {
        if self.fixed {
            return;
        }
        let chars = self.chars();
        let at = self.col.min(chars.len());
        let head: String = chars[..at].iter().collect();
        let indent: String = chars
            .iter()
            .take_while(|c| c.is_whitespace())
            .take(at)
            .collect();
        let tail: String = indent.chars().chain(chars[at..].iter().copied()).collect();
        self.splice(self.row, 1, vec![head, tail]);
        self.row += 1;
        self.col = indent.chars().count();
        self.goal = self.col;
    }

    /// Delete the character before the cursor, joining onto the line above
    /// when there is nothing before it on this one.
    pub fn backspace(&mut self) {
        if self.col > 0 {
            let mut chars = self.chars();
            chars.remove(self.col - 1);
            self.set_line(self.row, chars.into_iter().collect());
            self.col -= 1;
            self.goal = self.col;
            return;
        }
        if self.row == 0 || self.fixed {
            return;
        }
        let joined = format!("{}{}", self.lines[self.row - 1], self.lines[self.row]);
        self.col = self.line_len_of(self.row - 1);
        self.goal = self.col;
        self.splice(self.row - 1, 2, vec![joined]);
        self.row -= 1;
    }

    /// Delete `count` characters at the cursor (`x`), keeping what they were.
    pub fn delete_under(&mut self, count: usize) {
        let chars = self.chars();
        if chars.is_empty() {
            return;
        }
        let end = (self.col + count).min(chars.len());
        self.take_range(self.col, end, false);
    }

    /// Open a blank line below or above, with the cursor on it, indented to
    /// match the line it was opened from: an editor that drops the indent
    /// makes every new line in indented code a retype.
    pub fn open_line(&mut self, below: bool) {
        if self.fixed {
            return;
        }
        let indent: String = self
            .chars()
            .iter()
            .take_while(|c| c.is_whitespace())
            .collect();
        let at = if below { self.row + 1 } else { self.row };
        self.col = indent.chars().count();
        self.goal = self.col;
        self.splice(at, 0, vec![indent]);
        self.row = at;
    }

    /// Replace the character under the cursor, leaving it where it is (`r`).
    pub fn replace_char(&mut self, c: char) {
        let mut chars = self.chars();
        if self.col >= chars.len() {
            return;
        }
        chars[self.col] = c;
        self.set_line(self.row, chars.into_iter().collect());
    }

    /// Write `c` over the character under the cursor and step past it, which
    /// is what Replace mode does to every key. Past the line's end there is
    /// nothing to write over, so the character is added.
    pub fn replace_char_over(&mut self, c: char) {
        let mut chars = self.chars();
        match self.col < chars.len() {
            true => chars[self.col] = c,
            false => chars.push(c),
        }
        self.set_line(self.row, chars.into_iter().collect());
        self.col += 1;
        self.goal = self.col;
    }

    /// Flip the case of `count` characters and step past them (`~`).
    pub fn toggle_case(&mut self, count: usize) {
        let mut chars = self.chars();
        if chars.is_empty() {
            return;
        }
        let end = (self.col + count).min(chars.len());
        for c in chars.iter_mut().take(end).skip(self.col) {
            *c = flipped(*c);
        }
        self.set_line(self.row, chars.into_iter().collect());
        self.col = end.min(self.line_len_of(self.row).saturating_sub(1));
        self.goal = self.col;
    }

    /// Pull the next line onto this one with a single space between (`J`).
    pub fn join_lines(&mut self, count: usize) {
        if self.fixed {
            return;
        }
        for _ in 0..count.max(1) {
            if self.row + 1 >= self.lines.len() {
                return;
            }
            let head = self.lines[self.row].trim_end().to_string();
            let tail = self.lines[self.row + 1].trim_start().to_string();
            self.col = head.chars().count();
            self.goal = self.col;
            let spacer = if head.is_empty() || tail.is_empty() {
                ""
            } else {
                " "
            };
            let joined = format!("{head}{spacer}{tail}");
            self.splice(self.row, 2, vec![joined]);
        }
    }

    /// Take `count` whole lines into the register (`dd`), or copy them (`yy`).
    pub fn take_lines(&mut self, count: usize, keep: bool) {
        let end = (self.row + count).min(self.lines.len());
        let taken: Vec<String> = self.lines[self.row..end].to_vec();
        self.register = Register {
            linewise: true,
            text: taken.join("\n"),
        };
        if keep {
            return;
        }
        self.splice(self.row, end - self.row, self.emptied(end - self.row));
        self.row = self.row.min(self.lines.len() - 1);
        self.col = first_non_blank(&self.chars());
        self.goal = self.col;
    }

    /// Empty `count` lines without removing them, for `cc`: the line stays so
    /// that insert mode has somewhere to type.
    pub fn clear_lines(&mut self, count: usize) {
        let end = (self.row + count).min(self.lines.len());
        let taken: Vec<String> = self.lines[self.row..end].to_vec();
        self.register = Register {
            linewise: true,
            text: taken.join("\n"),
        };
        let cleared = match self.fixed {
            true => vec![String::new(); end - self.row],
            false => vec![String::new()],
        };
        self.splice(self.row, end - self.row, cleared);
        self.col = 0;
        self.goal = 0;
    }

    /// Replace `count` rows from `first` with `lines` as one undo step, leaving
    /// the cursor at the start of the new text.
    pub fn replace_rows(&mut self, first: usize, count: usize, lines: Vec<String>) {
        self.snapshot();
        self.splice(first, count, lines);
        self.row = first.min(self.lines.len() - 1);
        self.col = 0;
        self.goal = 0;
    }

    /// Take rows `first` through `last` into the register as whole lines,
    /// removing them unless `keep`.
    pub fn take_rows(&mut self, first: usize, last: usize, keep: bool) {
        let first = first.min(self.lines.len().saturating_sub(1));
        let end = (last + 1).min(self.lines.len());
        self.register = Register {
            linewise: true,
            text: self.lines[first..end].join("\n"),
        };
        if keep {
            self.row = first;
            self.col = first_non_blank(&self.chars());
            self.goal = self.col;
            return;
        }
        self.splice(first, end - first, self.emptied(end - first));
        self.row = first.min(self.lines.len() - 1);
        self.col = first_non_blank(&self.chars());
        self.goal = self.col;
    }

    /// Take the text between two points into the register, removing it unless
    /// `keep`. Both ends are a row and a column on it, `end` exclusive, which
    /// is what an operator over a motion or an object reaches across.
    pub fn take_between(&mut self, start: (usize, usize), end: (usize, usize), keep: bool) {
        let ((first, from), (last, to)) = match start <= end {
            true => (start, end),
            false => (end, start),
        };
        if first == last {
            self.row = first;
            self.take_range(from, to, keep);
            return;
        }
        // Taking text across lines joins what is left of them, which a fixed
        // count has no room for.
        if self.fixed && !keep {
            return;
        }
        let head = self.line_chars(first);
        let tail = self.line_chars(last);
        let from = from.min(head.len());
        let to = to.min(tail.len());
        let mut taken: Vec<String> = vec![head[from..].iter().collect()];
        taken.extend(self.lines[first + 1..last].iter().cloned());
        taken.push(tail[..to].iter().collect());
        self.register = Register {
            linewise: false,
            text: taken.join("\n"),
        };
        self.row = first;
        self.col = from;
        self.goal = from;
        if keep {
            return;
        }
        let joined: String = head[..from]
            .iter()
            .chain(tail[to..].iter())
            .collect::<String>();
        self.splice(first, last - first + 1, vec![joined]);
        self.clamp(false);
    }

    /// Indent `count` lines by one level, or take a level off them (`>>`/`<<`).
    pub fn shift_lines(&mut self, count: usize, right: bool) {
        let unit = self.indent_unit();
        let end = (self.row + count).min(self.lines.len());
        let shifted: Vec<String> = (self.row..end)
            .map(|row| {
                let line = &self.lines[row];
                if right {
                    // A line with nothing on it gains no indent, as Vim does:
                    // trailing blanks on an empty line are not indentation.
                    return match line.trim().is_empty() {
                        true => line.clone(),
                        false => format!("{unit}{line}"),
                    };
                }
                if let Some(rest) = line.strip_prefix('\t') {
                    return rest.to_string();
                }
                let spaces = line
                    .chars()
                    .take(INDENT_SPACES)
                    .take_while(|c| *c == ' ')
                    .count();
                line[spaces..].to_string()
            })
            .collect();
        self.splice(self.row, end - self.row, shifted);
        self.col = first_non_blank(&self.chars());
        self.goal = self.col;
    }

    /// Add `by` to the number at or right of the cursor (`Ctrl-A`/`Ctrl-X`),
    /// leaving the cursor on its last digit. Reports whether there was one.
    pub fn add_to_number(&mut self, by: i64) -> bool {
        let chars = self.chars();
        let Some(start) = (self.col..chars.len()).find(|&i| chars[i].is_ascii_digit()) else {
            return false;
        };
        let mut first = start;
        while first > 0 && chars[first - 1].is_ascii_digit() {
            first -= 1;
        }
        let mut end = start;
        while end + 1 < chars.len() && chars[end + 1].is_ascii_digit() {
            end += 1;
        }
        // A `-` against the digits is part of the number, so stepping `-1`
        // down reaches `-2` rather than `-0`.
        let negative = first > 0 && chars[first - 1] == '-';
        let digits: String = chars[first..=end].iter().collect();
        let Ok(value) = digits.parse::<i64>() else {
            return false;
        };
        let value = if negative { -value } else { value };
        let text = (value + by).to_string();
        let at = first - usize::from(negative);
        let mut replaced: String = chars[..at].iter().collect();
        replaced.push_str(&text);
        replaced.extend(chars[end + 1..].iter());
        self.set_line(self.row, replaced);
        self.col = (at + text.chars().count()).saturating_sub(1);
        self.goal = self.col;
        true
    }

    /// What the register holds, for a caller keeping registers of its own.
    pub fn register(&self) -> &Register {
        &self.register
    }

    /// Put `register` in as the one a paste reads and a take overwrites.
    pub fn set_register(&mut self, register: Register) {
        self.register = register;
    }

    /// Take the characters between the cursor and `col` into the register,
    /// removing them unless `keep`. The half-open range runs whichever way the
    /// motion went, and the cursor ends at its start, as Vim's operators do.
    pub fn take_to(&mut self, col: usize, keep: bool) {
        let (start, end) = if col < self.col {
            (col, self.col)
        } else {
            (self.col, col)
        };
        self.take_range(start, end, keep);
    }

    /// Put the register back, after the cursor or before it (`p` / `P`).
    pub fn paste(&mut self, after: bool) {
        if self.register.text.is_empty() {
            return;
        }
        if self.fixed && self.register.text.contains('\n') {
            return;
        }
        // One whole line put back where lines cannot be added goes in as
        // text, which is what a yanked name pasted into another one means.
        if self.register.linewise && !self.fixed {
            let at = if after { self.row + 1 } else { self.row };
            let pasted: Vec<String> = self.register.text.split('\n').map(str::to_string).collect();
            let landing = at.min(self.lines.len());
            self.splice(landing, 0, pasted);
            self.row = landing;
            self.col = first_non_blank(&self.chars());
            self.goal = self.col;
            return;
        }
        let mut chars = self.chars();
        let at = if after && !chars.is_empty() {
            (self.col + 1).min(chars.len())
        } else {
            self.col.min(chars.len())
        };
        let text: Vec<char> = self.register.text.chars().collect();
        let landed = at + text.len();
        for (offset, c) in text.into_iter().enumerate() {
            chars.insert(at + offset, c);
        }
        self.set_line(self.row, chars.into_iter().collect());
        self.col = landed.saturating_sub(1);
        self.goal = self.col;
    }

    /// Step back over the last command, and leave the way forward.
    pub fn undo(&mut self) -> bool {
        // A command that changed nothing is nothing to step back over.
        while self.undo.last().is_some_and(|step| step.edits.is_empty()) {
            self.undo.pop();
        }
        let Some(step) = self.undo.pop() else {
            return false;
        };
        let undone = self.invert(&step);
        self.redo.push(undone);
        // Back where the command was typed, which is where the eye is.
        self.row = step.row.min(self.lines.len().saturating_sub(1));
        self.col = step.col;
        self.goal = self.col;
        self.clamp(false);
        true
    }

    /// Step forward again over what undo walked back.
    pub fn redo(&mut self) -> bool {
        let Some(step) = self.redo.pop() else {
            return false;
        };
        let redone = self.invert(&step);
        self.undo.push(redone);
        self.row = step.row.min(self.lines.len().saturating_sub(1));
        self.col = step.col;
        self.goal = self.col;
        self.clamp(false);
        true
    }

    /// Put `step`'s changes back the way they were, and report the step that
    /// would undo *that*: the two stacks are each other's inverse, so one
    /// walk back and one walk forward cost the same.
    ///
    /// Later changes are undone first, since an earlier one's line numbers
    /// only mean what they meant once everything after it is back in place.
    fn invert(&mut self, step: &Step) -> Step {
        let mut edits = Vec::with_capacity(step.edits.len());
        for edit in step.edits.iter().rev() {
            edits.push(self.apply(edit.from, edit.inserted, edit.removed.clone()));
        }
        // Left in the order they were produced, which is the reverse of the
        // order they were made in: inverting this step walks them back again,
        // and two reversals put the command back the way it was typed.
        Step {
            col: self.col,
            edits,
            id: step.id,
            row: self.row,
        }
    }

    /// A name for a new undo step, never handed out before.
    fn take_step_id(&mut self) -> u64 {
        let id = self.next_step;
        self.next_step += 1;
        id
    }

    /// Cut or copy a character range on the cursor's line into the register.
    fn take_range(&mut self, start: usize, end: usize, keep: bool) {
        let mut chars = self.chars();
        let start = start.min(chars.len());
        let end = end.min(chars.len());
        if start >= end {
            return;
        }
        self.register = Register {
            linewise: false,
            text: chars[start..end].iter().collect(),
        };
        if keep {
            self.col = start;
            self.goal = self.col;
            return;
        }
        chars.drain(start..end);
        self.set_line(self.row, chars.into_iter().collect());
        self.col = start;
        self.goal = self.col;
    }

    /// What `count` removed lines leave behind: nothing, or as many empty
    /// lines where the count is fixed, so a deleted name reads as a blank one.
    fn emptied(&self, count: usize) -> Vec<String> {
        match self.fixed {
            true => vec![String::new(); count],
            false => Vec::new(),
        }
    }

    fn set_line(&mut self, row: usize, text: String) {
        if row < self.lines.len() {
            self.splice(row, 1, vec![text]);
        }
    }
}

impl TextBuffer {
    /// One level of indentation, in what the file already indents with: a
    /// tab where the first indented line uses one, spaces otherwise.
    fn indent_unit(&self) -> String {
        let tabbed = self
            .lines
            .iter()
            .filter(|line| line.starts_with(char::is_whitespace))
            .map(|line| line.starts_with('\t'))
            .next()
            .unwrap_or(false);
        match tabbed {
            true => "\t".to_string(),
            false => " ".repeat(INDENT_SPACES),
        }
    }
}

// ========================================================================
// TextBuffer: Emacs editing
// ========================================================================

/// How a word is recased in place.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WordCase {
    /// First letter upper, the rest lower (`Alt-c`).
    Capitalize,
    /// Every letter upper (`Alt-u`).
    Upper,
}

impl TextBuffer {
    /// Where an Emacs word motion from the cursor lands, as a row and column.
    /// A word is a run of letters and digits, so `foo_bar` is two, and the
    /// motion runs over whatever lies between words, crossing lines when the
    /// rest of this one holds none.
    fn emacs_word_point(&self, forward: bool) -> (usize, usize) {
        let (mut row, mut col) = (self.row, self.col);
        loop {
            let chars = self.line_chars(row);
            let mut at = col.min(chars.len());
            if forward {
                while at < chars.len() && !chars[at].is_alphanumeric() {
                    at += 1;
                }
                if at < chars.len() {
                    while at < chars.len() && chars[at].is_alphanumeric() {
                        at += 1;
                    }
                    return (row, at);
                }
                if row + 1 >= self.lines.len() {
                    return (row, chars.len());
                }
                row += 1;
                col = 0;
            } else {
                while at > 0 && !chars[at - 1].is_alphanumeric() {
                    at -= 1;
                }
                if at > 0 {
                    while at > 0 && chars[at - 1].is_alphanumeric() {
                        at -= 1;
                    }
                    return (row, at);
                }
                if row == 0 {
                    return (0, 0);
                }
                row -= 1;
                col = self.line_len_of(row);
            }
        }
    }

    /// Move over one Emacs word, forward to its end or back to its start
    /// (`Alt-f` / `Alt-b`).
    pub fn move_emacs_word(&mut self, forward: bool) {
        (self.row, self.col) = self.emacs_word_point(forward);
        self.goal = self.col;
    }

    /// Kill one Emacs word into the register, the one ahead of the cursor or
    /// the one behind it (`Alt-d` / `Alt-Backspace`).
    pub fn kill_emacs_word(&mut self, forward: bool) {
        let here = (self.row, self.col);
        let there = self.emacs_word_point(forward);
        let start = here.min(there);
        self.take_between(start, here.max(there), false);
        // Insert mode may rest one past the last character, which the
        // clamp a multi-line take ends with does not allow.
        (self.row, self.col) = start;
        self.goal = self.col;
    }

    /// Kill from the cursor to the end of the line into the register; at the
    /// end of the line, the line break itself, so the next line joins this
    /// one (`Ctrl-k`).
    pub fn kill_to_line_end(&mut self) {
        let len = self.line_len();
        if self.col < len {
            self.take_range(self.col, len, false);
            return;
        }
        if self.row + 1 < self.lines.len() && !self.fixed {
            self.register = Register {
                linewise: false,
                text: "\n".to_string(),
            };
            let joined = format!("{}{}", self.lines[self.row], self.lines[self.row + 1]);
            self.splice(self.row, 2, vec![joined]);
        }
    }

    /// Delete the character under the cursor, or at the end of a line the
    /// line break, without touching the register (`Ctrl-d`).
    pub fn delete_forward(&mut self) {
        let mut chars = self.chars();
        if self.col < chars.len() {
            chars.remove(self.col);
            self.set_line(self.row, chars.into_iter().collect());
        } else if self.row + 1 < self.lines.len() && !self.fixed {
            let joined = format!("{}{}", self.lines[self.row], self.lines[self.row + 1]);
            self.splice(self.row, 2, vec![joined]);
        }
        self.goal = self.col;
    }

    /// Break the line at the cursor and leave the cursor where it was, ahead
    /// of the break (`Ctrl-o`).
    pub fn open_line_here(&mut self) {
        if self.fixed {
            return;
        }
        let chars = self.chars();
        let at = self.col.min(chars.len());
        let head: String = chars[..at].iter().collect();
        let tail: String = chars[at..].iter().collect();
        self.splice(self.row, 1, vec![head, tail]);
    }

    /// Swap the character before the cursor with the one under it, and step
    /// over both; at the end of a line, the last two (`Ctrl-t`).
    pub fn transpose_chars(&mut self) {
        let mut chars = self.chars();
        if chars.len() < 2 || self.col == 0 {
            return;
        }
        let at = self.col.min(chars.len() - 1);
        chars.swap(at - 1, at);
        self.set_line(self.row, chars.into_iter().collect());
        self.col = at + 1;
        self.goal = self.col;
    }

    /// Drag the word at or before the cursor past the next one on its line,
    /// leaving the cursor after both (`Alt-t`).
    pub fn transpose_words(&mut self) {
        let chars = self.chars();
        let mut words: Vec<(usize, usize)> = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            if chars[i].is_alphanumeric() {
                let start = i;
                while i < chars.len() && chars[i].is_alphanumeric() {
                    i += 1;
                }
                words.push((start, i));
            } else {
                i += 1;
            }
        }
        let first = words
            .iter()
            .rposition(|(start, _)| *start < self.col)
            .unwrap_or(0);
        let (Some(&(a, b)), Some(&(c, d))) = (words.get(first), words.get(first + 1)) else {
            return;
        };
        let swapped: Vec<char> = chars[c..d]
            .iter()
            .chain(&chars[b..c])
            .chain(&chars[a..b])
            .copied()
            .collect();
        let mut out = chars.clone();
        out.splice(a..d, swapped);
        self.set_line(self.row, out.into_iter().collect());
        self.col = d;
        self.goal = self.col;
    }

    /// Recase the word ahead of the cursor on this line and step past it
    /// (`Alt-u` / `Alt-c`).
    pub fn change_word_case(&mut self, case: WordCase) {
        let mut chars = self.chars();
        let mut start = self.col.min(chars.len());
        while start < chars.len() && !chars[start].is_alphanumeric() {
            start += 1;
        }
        let mut end = start;
        while end < chars.len() && chars[end].is_alphanumeric() {
            end += 1;
        }
        if start == end {
            return;
        }
        let recased: Vec<char> = chars[start..end]
            .iter()
            .enumerate()
            .flat_map(|(index, c)| match (case, index) {
                (WordCase::Upper, _) | (WordCase::Capitalize, 0) => {
                    c.to_uppercase().collect::<Vec<_>>()
                }
                (WordCase::Capitalize, _) => c.to_lowercase().collect(),
            })
            .collect();
        chars.splice(start..end, recased.iter().copied());
        self.set_line(self.row, chars.into_iter().collect());
        self.col = start + recased.len();
        self.goal = self.col;
    }

    /// Put what was last killed back in at the cursor, and step past it
    /// (`Ctrl-y`). Whole lines come in as lines.
    pub fn yank(&mut self) {
        if self.register.text.is_empty() {
            return;
        }
        let mut text = self.register.text.clone();
        if self.register.linewise && !self.fixed {
            text.push('\n');
        }
        if self.fixed && text.contains('\n') {
            return;
        }
        let chars = self.chars();
        let at = self.col.min(chars.len());
        let head: String = chars[..at].iter().collect();
        let tail: String = chars[at..].iter().collect();
        let parts: Vec<&str> = text.split('\n').collect();
        let mut lines: Vec<String> = parts.iter().map(|part| part.to_string()).collect();
        lines[0] = format!("{head}{}", lines[0]);
        let last = lines.len() - 1;
        let landed = match last {
            0 => at + parts[0].chars().count(),
            _ => parts[last].chars().count(),
        };
        lines[last].push_str(&tail);
        self.splice(self.row, 1, lines);
        self.row += last;
        self.col = landed;
        self.goal = self.col;
    }

    /// To the very start of the text (`Alt-<`).
    pub fn move_text_start(&mut self) {
        self.row = 0;
        self.col = 0;
        self.goal = 0;
    }

    /// To the very end of the text (`Alt->`).
    pub fn move_text_end(&mut self) {
        self.row = self.lines.len().saturating_sub(1);
        self.col = self.line_len_of(self.row);
        self.goal = self.col;
    }
}

// ========================================================================
// Helpers
// ========================================================================

/// A character with its case flipped, left alone where it has no other case.
fn flipped(c: char) -> char {
    if c.is_lowercase() {
        return c.to_uppercase().next().unwrap_or(c);
    }
    if c.is_uppercase() {
        return c.to_lowercase().next().unwrap_or(c);
    }
    c
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(lines: &[&str]) -> TextBuffer {
        TextBuffer::new(lines.iter().map(|line| line.to_string()).collect())
    }

    #[test]
    fn test_emacs_words_are_letters_and_digits_and_cross_lines() {
        let mut buf = buffer(&["foo_bar baz", "  qux"]);
        buf.move_emacs_word(true);
        assert_eq!((buf.row(), buf.col()), (0, 3), "foo ends at the underscore");
        buf.move_emacs_word(true);
        assert_eq!((buf.row(), buf.col()), (0, 7), "bar");
        buf.move_emacs_word(true);
        buf.move_emacs_word(true);
        assert_eq!((buf.row(), buf.col()), (1, 5), "baz, then onto the next line");
        buf.move_emacs_word(false);
        assert_eq!((buf.row(), buf.col()), (1, 2), "back to qux");
        buf.move_emacs_word(false);
        assert_eq!((buf.row(), buf.col()), (0, 8), "back across the line break");
    }

    #[test]
    fn test_killing_a_word_keeps_it_for_a_yank_and_leaves_the_cursor_at_the_gap() {
        let mut buf = buffer(&["one two three"]);
        buf.move_to(0, 4);
        buf.kill_emacs_word(true);
        assert_eq!(buf.lines(), ["one  three"]);
        assert_eq!(buf.register().text, "two");
        assert_eq!(buf.col(), 4);
        buf.kill_emacs_word(false);
        assert_eq!(buf.lines(), [" three"]);
        assert_eq!(buf.col(), 0);
        assert_eq!(buf.register().text, "one ", "the latest kill is the one kept");
        buf.yank();
        assert_eq!(buf.lines(), ["one  three"]);
        assert_eq!(buf.col(), 4, "the cursor steps past what came back");
    }

    #[test]
    fn test_killing_the_last_word_of_a_line_leaves_the_cursor_at_its_end() {
        // Insert mode rests one past the last character; Normal mode's clamp
        // would pull the cursor back onto the one before.
        let mut buf = buffer(&["one two"]);
        buf.move_right(100, true);
        buf.kill_emacs_word(false);
        assert_eq!(buf.lines(), ["one "]);
        assert_eq!(buf.col(), 4);
        let mut across = buffer(&["one", "two"]);
        across.move_to(1, 0);
        across.move_right(0, true);
        across.kill_emacs_word(false);
        assert_eq!(across.lines(), ["two"], "the line break goes with the word");
        assert_eq!((across.row(), across.col()), (0, 0));
    }

    #[test]
    fn test_ctrl_k_kills_to_the_end_of_the_line_and_then_the_line_break() {
        let mut buf = buffer(&["abc def", "ghi"]);
        buf.move_to(0, 3);
        buf.kill_to_line_end();
        assert_eq!(buf.lines(), ["abc", "ghi"]);
        assert_eq!(buf.register().text, " def");
        buf.move_right(10, true);
        buf.kill_to_line_end();
        assert_eq!(buf.lines(), ["abcghi"], "the next line joins this one");
        assert_eq!(buf.col(), 3);
    }

    #[test]
    fn test_ctrl_d_deletes_forward_and_joins_at_the_end_without_touching_the_register() {
        let mut buf = buffer(&["ab", "cd"]);
        buf.set_register(Register {
            linewise: false,
            text: "kept".to_string(),
        });
        buf.delete_forward();
        assert_eq!(buf.lines(), ["b", "cd"]);
        buf.move_right(10, true);
        buf.delete_forward();
        assert_eq!(buf.lines(), ["bcd"]);
        assert_eq!(buf.register().text, "kept");
    }

    #[test]
    fn test_ctrl_o_breaks_the_line_and_stays_ahead_of_the_break() {
        let mut buf = buffer(&["abcd"]);
        buf.move_to(0, 2);
        buf.open_line_here();
        assert_eq!(buf.lines(), ["ab", "cd"]);
        assert_eq!((buf.row(), buf.col()), (0, 2));
    }

    #[test]
    fn test_ctrl_t_swaps_around_the_cursor_and_the_last_two_at_the_end() {
        let mut buf = buffer(&["abcd"]);
        buf.move_to(0, 2);
        buf.transpose_chars();
        assert_eq!(buf.lines(), ["acbd"]);
        assert_eq!(buf.col(), 3);
        buf.move_right(10, true);
        buf.transpose_chars();
        assert_eq!(buf.lines(), ["acdb"]);
        let mut start = buffer(&["ab"]);
        start.transpose_chars();
        assert_eq!(start.lines(), ["ab"], "nothing precedes the first character");
    }

    #[test]
    fn test_alt_t_drags_a_word_past_the_next_and_ends_after_both() {
        let mut buf = buffer(&["one two three"]);
        buf.move_to(0, 3);
        buf.transpose_words();
        assert_eq!(buf.lines(), ["two one three"]);
        assert_eq!(buf.col(), 7);
        buf.transpose_words();
        assert_eq!(buf.lines(), ["two three one"], "again, from just after it");
        let mut last = buffer(&["only"]);
        last.move_to(0, 2);
        last.transpose_words();
        assert_eq!(last.lines(), ["only"], "no word follows to swap with");
    }

    #[test]
    fn test_alt_u_and_alt_c_recase_the_word_ahead_and_step_past_it() {
        let mut buf = buffer(&["foo bAR baz"]);
        buf.change_word_case(WordCase::Upper);
        assert_eq!(buf.lines(), ["FOO bAR baz"]);
        assert_eq!(buf.col(), 3);
        buf.change_word_case(WordCase::Capitalize);
        assert_eq!(buf.lines(), ["FOO Bar baz"]);
        assert_eq!(buf.col(), 7);
    }

    #[test]
    fn test_yanking_a_killed_line_break_puts_it_back() {
        let mut buf = buffer(&["abc", "def"]);
        buf.move_right(10, true);
        buf.kill_to_line_end();
        buf.yank();
        assert_eq!(buf.lines(), ["abc", "def"]);
        assert_eq!((buf.row(), buf.col()), (1, 0));
    }

    #[test]
    fn test_a_short_line_on_the_way_down_does_not_clip_the_column() {
        // Vim remembers the column being aimed for. Clipping it at the short
        // line means a long file walked with `j` drifts to the left margin.
        let mut buf = buffer(&["a long first line", "short", "another long line"]);
        buf.move_right(12, false);
        assert_eq!(buf.col(), 12);

        buf.move_down(1, false);
        assert_eq!(buf.col(), 4, "clipped to what the short line holds");

        buf.move_down(1, false);
        assert_eq!(buf.col(), 12, "and back to the column being aimed for");
    }

    #[test]
    fn test_undo_steps_back_over_a_run_of_typing_rather_than_one_character() {
        // One snapshot per keystroke would make undo useless for anything but
        // a typo: walking back out of a sentence would take a sentence of
        // presses.
        let mut buf = buffer(&["hello"]);
        buf.move_line_end(true);
        buf.snapshot();
        for c in " there".chars() {
            buf.insert_char(c);
        }
        assert_eq!(buf.lines(), ["hello there"]);

        assert!(buf.undo());
        assert_eq!(buf.lines(), ["hello"]);
        assert!(buf.redo());
        assert_eq!(buf.lines(), ["hello there"]);
    }

    #[test]
    fn test_backspace_at_the_start_of_a_line_joins_onto_the_one_above() {
        let mut buf = buffer(&["one", "two"]);
        buf.move_down(1, true);
        buf.move_line_start();
        buf.backspace();
        assert_eq!(buf.lines(), ["onetwo"]);
        assert_eq!((buf.row(), buf.col()), (0, 3), "at the seam");
    }

    #[test]
    fn test_a_newline_carries_the_rest_of_the_line_down_with_it() {
        let mut buf = buffer(&["onetwo"]);
        buf.move_right(3, true);
        buf.insert_newline();
        assert_eq!(buf.lines(), ["one", "two"]);
        assert_eq!((buf.row(), buf.col()), (1, 0));
    }

    #[test]
    fn test_taken_lines_paste_back_as_lines_and_taken_text_pastes_inside_one() {
        // The two registers behave differently on purpose: `dd` then `p` puts
        // a line back below, while `x` then `p` puts a character after the
        // cursor. Losing the distinction pastes lines into the middle of one.
        let mut lines = buffer(&["one", "two", "three"]);
        lines.take_lines(1, false);
        assert_eq!(lines.lines(), ["two", "three"]);
        lines.paste(true);
        assert_eq!(lines.lines(), ["two", "one", "three"]);

        let mut chars = buffer(&["abc"]);
        chars.delete_under(1);
        assert_eq!(chars.lines(), ["bc"]);
        chars.paste(true);
        assert_eq!(chars.lines(), ["bac"]);
    }

    #[test]
    fn test_an_opened_line_keeps_the_indent_of_the_one_it_came_from() {
        // Without this every new line in indented code is a retype.
        let mut buf = buffer(&["    if x {", "    }"]);
        buf.open_line(true);
        assert_eq!(buf.lines(), ["    if x {", "    ", "    }"]);
        assert_eq!((buf.row(), buf.col()), (1, 4), "past the indent");
    }

    #[test]
    fn test_joining_lines_leaves_one_space_and_no_stray_indent() {
        let mut buf = buffer(&["one", "    two"]);
        buf.join_lines(1);
        assert_eq!(buf.lines(), ["one two"]);
    }

    #[test]
    fn test_columns_count_characters_rather_than_bytes() {
        // Indexing a line by bytes panics mid-character on any file with an
        // accent in it, and silently edits the wrong place on the rest.
        let mut buf = buffer(&["héllo wörld"]);
        buf.move_right(1, false);
        buf.delete_under(1);
        assert_eq!(buf.lines(), ["hllo wörld"]);

        buf.move_line_end(false);
        assert_eq!(buf.col(), 9, "ten characters, however many bytes");
    }

    #[test]
    fn test_normal_mode_stays_on_a_character_and_insert_mode_may_sit_past_one() {
        // Normal mode acts on the character under the cursor, so there has to
        // be one; insert mode types where the next one goes, so it may sit at
        // the end of the line.
        let mut buf = buffer(&["abc"]);
        buf.move_right(9, false);
        assert_eq!(buf.col(), 2);

        buf.move_right(9, true);
        assert_eq!(buf.col(), 3);
    }

    #[test]
    fn test_an_edit_costs_what_it_changed_rather_than_what_the_file_holds() {
        // Undo used to keep a copy of the whole file per edit: on a 2 MB file
        // that was 1.9ms and 2 MB of memory per keystroke, and it grew with
        // the file. What is kept has to be the size of the change.
        let lines: Vec<String> = (0..5_000).map(|n| format!("line number {n}")).collect();
        let mut buf = TextBuffer::new(lines);
        for n in 0..100 {
            buf.move_to_line(n * 7);
            buf.snapshot();
            buf.delete_under(1);
        }

        let kept: usize = buf
            .undo
            .iter()
            .flat_map(|step| step.edits.iter())
            .map(|edit| edit.removed.len())
            .sum();
        assert_eq!(kept, 100, "one line kept per edit, not five thousand");

        // And it still walks all the way back.
        for _ in 0..100 {
            assert!(buf.undo());
        }
        assert_eq!(buf.lines()[0], "line number 0", "the first edit came back");
        assert_eq!(buf.lines().len(), 5_000);
    }

    #[test]
    fn test_undo_and_redo_walk_a_command_that_changed_many_lines() {
        // A command is one step however many lines it touched, and the two
        // stacks have to be each other's inverse: replaying the changes in
        // the wrong order puts the lines back in the wrong places.
        let mut buf = buffer(&["one", "two", "three", "four"]);
        buf.snapshot();
        buf.take_lines(2, false);
        buf.move_to_line(1);
        buf.snapshot();
        buf.paste(true);
        assert_eq!(buf.lines(), ["three", "four", "one", "two"]);

        assert!(buf.undo());
        assert_eq!(buf.lines(), ["three", "four"]);
        assert!(buf.undo());
        assert_eq!(buf.lines(), ["one", "two", "three", "four"]);

        assert!(buf.redo());
        assert_eq!(buf.lines(), ["three", "four"]);
        assert!(buf.redo());
        assert_eq!(buf.lines(), ["three", "four", "one", "two"]);
    }

    #[test]
    fn test_a_command_that_changed_nothing_is_nothing_to_undo() {
        // Every command opens a step before it knows whether it will change
        // anything; an empty one would cost a press of `u` to walk back over.
        let mut buf = buffer(&["text"]);
        buf.snapshot();
        buf.delete_under(1);
        buf.snapshot();
        buf.move_line_end(false);
        buf.snapshot();

        assert!(buf.undo(), "the one real edit");
        assert_eq!(buf.lines(), ["text"]);
        assert!(!buf.undo(), "and nothing behind it");
    }

    #[test]
    fn test_deleting_the_last_line_leaves_a_line_to_stand_on() {
        // An empty `lines` has no row zero, and everything downstream indexes
        // one.
        let mut buf = buffer(&["only"]);
        buf.take_lines(1, false);
        assert_eq!(buf.lines(), [""]);
        assert_eq!((buf.row(), buf.col()), (0, 0));
    }

    #[test]
    fn test_undoing_back_to_the_saved_text_leaves_nothing_to_save() {
        let mut buf = buffer(&["one"]);
        buf.snapshot();
        buf.insert_char('x');
        buf.mark_saved();
        assert!(!buf.is_dirty());

        // A run of typing that goes on after a save is a change.
        buf.insert_char('y');
        assert!(buf.is_dirty(), "typed past the save");
        buf.snapshot();
        buf.insert_char('z');
        assert!(buf.undo());
        assert!(buf.is_dirty(), "still one step past the save");
        assert!(buf.undo());
        assert!(buf.is_dirty(), "this step began before the save");

        assert!(buf.redo());
        buf.mark_saved();
        assert!(buf.undo());
        assert!(buf.is_dirty(), "undone past the save");
        assert!(buf.redo());
        assert!(!buf.is_dirty(), "redone back to exactly what was written");
    }

    #[test]
    fn test_a_fixed_buffer_never_gains_or_loses_a_line() {
        let mut buf = buffer(&["alpha", "beta"]).with_fixed_lines();

        // The edits that split or join lines do nothing at all.
        buf.move_to(0, 2);
        buf.insert_newline();
        buf.open_line(true);
        buf.join_lines(1);
        buf.move_line_end(true);
        buf.kill_to_line_end();
        buf.delete_forward();
        buf.move_to(1, 0);
        buf.backspace();
        buf.take_between((0, 2), (1, 2), false);
        assert_eq!(buf.lines(), ["alpha", "beta"]);

        // A whole-line delete empties the line, and the line pastes back into
        // another one as text.
        buf.move_to(0, 0);
        buf.take_lines(1, false);
        assert_eq!(buf.lines(), ["", "beta"]);
        buf.move_to(1, 3);
        buf.paste(true);
        assert_eq!(buf.lines(), ["", "betaalpha"]);
    }
}
