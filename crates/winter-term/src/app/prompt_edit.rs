//! Translating Vim delete operators on the last prompt into the byte edits the
//! shell's line editor understands.
//!
//! Normal mode never forwards keys to the PTY, so a delete is realized by
//! sending the shell the equivalent readline keystrokes: first arrow keys to
//! move the line-editor cursor under the Vim cursor, then a kill. This assumes
//! the default emacs-mode readline bindings (`Ctrl-K`, `Ctrl-U`, ...).

use crate::model::history::EditHistory;
use crate::model::input::{EditAction, Key, KeyCode};

// ========================================================================
// Constants
// ========================================================================

/// `Ctrl-A`: move to start of line (readline `beginning-of-line`).
const CTRL_A: u8 = 0x01;
/// `Ctrl-K`: kill from cursor to end of line.
const CTRL_K: u8 = 0x0b;
/// `Ctrl-U`: kill from cursor to start of line.
const CTRL_U: u8 = 0x15;
/// `Ctrl-W`: kill the word before the cursor.
const CTRL_W: u8 = 0x17;
/// `Ctrl-_` (0x1f): the readline / ZLE `undo` command. Undo delegates to this so
/// the shell performs a single edit operation: plugins that re-render the line on
/// every keystroke (syntax highlighting, autosuggestions) then repaint once
/// instead of flashing through Winter's multi-keystroke rebuild.
pub(crate) const READLINE_UNDO: u8 = 0x1f;
/// `Alt-d`: kill the word after the cursor.
const ALT_D: &[u8] = b"\x1bd";
/// The Delete key (CSI 3~): forward-delete one character.
const KEY_DELETE: &[u8] = b"\x1b[3~";
const ARROW_LEFT: &[u8] = b"\x1b[D";
const ARROW_RIGHT: &[u8] = b"\x1b[C";
/// Bracketed-paste markers: text between them is inserted as a single edit.
const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

// ========================================================================
// Data Structures
// ========================================================================

/// A Vim delete operator targeting the last prompt line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PromptDelete {
    /// `x`: the character under the cursor.
    CharForward,
    /// `dd`: the whole line.
    Line,
    /// `diw`, `da(`, etc.: delete an inclusive column range on the prompt line.
    Range { start_col: usize, end_col: usize },
    /// `D` / `d$`: from the cursor to the end of the line.
    ToLineEnd,
    /// `d0`: from the cursor to the start of the line.
    ToLineStart,
    /// `db`: the word before the cursor.
    WordBack,
    /// `dw`: the word after the cursor.
    WordForward,
}

// ========================================================================
// Translation
// ========================================================================

/// The bytes that perform `op` when the line-editor cursor sits at `pty_col`
/// and the Vim cursor sits at `nav_col` on the same prompt row. Column-relative
/// operators are prefixed with arrow keys that align the editor cursor first;
/// `Line` ignores the columns and clears the whole line.
pub(crate) fn prompt_delete_bytes(op: PromptDelete, pty_col: usize, nav_col: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    if op != PromptDelete::Line {
        align_cursor(&mut bytes, pty_col, nav_col);
    }
    match op {
        PromptDelete::CharForward => bytes.extend_from_slice(KEY_DELETE),
        PromptDelete::Line => {
            bytes.push(CTRL_A);
            bytes.push(CTRL_K);
        }
        PromptDelete::Range { start_col, end_col } => {
            let count = if end_col >= start_col {
                end_col - start_col + 1
            } else {
                0
            };
            for _ in 0..count {
                bytes.extend_from_slice(KEY_DELETE);
            }
        }
        PromptDelete::ToLineEnd => bytes.push(CTRL_K),
        PromptDelete::ToLineStart => bytes.push(CTRL_U),
        PromptDelete::WordBack => bytes.push(CTRL_W),
        PromptDelete::WordForward => bytes.extend_from_slice(ALT_D),
    }
    bytes
}

/// Append the arrow keys that walk the editor cursor from `from` to `to`.
pub(crate) fn align_cursor(bytes: &mut Vec<u8>, from: usize, to: usize) {
    let (seq, steps) = if to >= from {
        (ARROW_RIGHT, to - from)
    } else {
        (ARROW_LEFT, from - to)
    };
    for _ in 0..steps {
        bytes.extend_from_slice(seq);
    }
}

/// The bytes that replace the character at `nav_col` with `ch` and restore the cursor to `nav_col`.
pub(crate) fn prompt_replace_char_bytes(pty_col: usize, nav_col: usize, ch: char) -> Vec<u8> {
    let mut bytes = Vec::new();
    align_cursor(&mut bytes, pty_col, nav_col);
    bytes.extend_from_slice(KEY_DELETE);
    let mut buf = [0u8; 4];
    bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    bytes.extend_from_slice(ARROW_LEFT);
    bytes
}

/// The bytes that replace the character at `nav_col` with `toggled` and leave the cursor advanced to `nav_col + 1`.
pub(crate) fn prompt_toggle_case_bytes(pty_col: usize, nav_col: usize, toggled: char) -> Vec<u8> {
    let mut bytes = Vec::new();
    align_cursor(&mut bytes, pty_col, nav_col);
    bytes.extend_from_slice(KEY_DELETE);
    let mut buf = [0u8; 4];
    bytes.extend_from_slice(toggled.encode_utf8(&mut buf).as_bytes());
    bytes
}

/// The bytes that delete delimiters at `c1` and `c2` on the prompt row.
pub(crate) fn prompt_delete_surround_bytes(
    pty_col: usize,
    c1: usize,
    c2: usize,
    nav_col: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    let (first, second) = if c1 <= c2 { (c1, c2) } else { (c2, c1) };
    align_cursor(&mut bytes, pty_col, second);
    bytes.extend_from_slice(KEY_DELETE);
    align_cursor(&mut bytes, second, first);
    bytes.extend_from_slice(KEY_DELETE);
    align_cursor(&mut bytes, first, nav_col.min(second.saturating_sub(2)));
    bytes
}

/// The bytes that replace delimiters at `c1` with `open` and `c2` with `close`.
pub(crate) fn prompt_change_surround_bytes(
    pty_col: usize,
    c1: usize,
    open: char,
    c2: usize,
    close: char,
    nav_col: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    let (first, open_ch, second, close_ch) = if c1 <= c2 {
        (c1, open, c2, close)
    } else {
        (c2, close, c1, open)
    };
    align_cursor(&mut bytes, pty_col, second);
    bytes.extend_from_slice(KEY_DELETE);
    let mut buf = [0u8; 4];
    bytes.extend_from_slice(close_ch.encode_utf8(&mut buf).as_bytes());
    align_cursor(&mut bytes, second + 1, first);
    bytes.extend_from_slice(KEY_DELETE);
    let mut buf = [0u8; 4];
    bytes.extend_from_slice(open_ch.encode_utf8(&mut buf).as_bytes());
    align_cursor(&mut bytes, first + 1, nav_col);
    bytes
}

/// The bytes that wrap column span `c1..=c2` with `open` and `close`.
pub(crate) fn prompt_add_surround_bytes(
    pty_col: usize,
    c1: usize,
    c2: usize,
    open: char,
    close: char,
    nav_col: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    align_cursor(&mut bytes, pty_col, c2 + 1);
    let mut buf = [0u8; 4];
    bytes.extend_from_slice(close.encode_utf8(&mut buf).as_bytes());
    align_cursor(&mut bytes, c2 + 2, c1);
    let mut buf = [0u8; 4];
    bytes.extend_from_slice(open.encode_utf8(&mut buf).as_bytes());
    align_cursor(&mut bytes, c1 + 1, nav_col);
    bytes
}

// ========================================================================
// Shadow buffer
// ========================================================================

/// Winter's model of the editable prompt line, in character space. `cursor` is a
/// char index in `[0, text.chars().count()]`. This is a best-effort shadow of the
/// shell's readline buffer: it tracks keys Winter forwards while at the prompt,
/// and is restored onto the shell by [`restore_prompt_bytes`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct PromptLine {
    pub cursor: usize,
    pub text: String,
}

/// Whether a tracked edit added or removed text. Consecutive inserts coalesce
/// into one undo step; every removal is its own step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EditKind {
    Delete,
    Insert,
}

/// Per-pane undo/redo state over [`PromptLine`].
///
/// `active` gates tracking to spans where the shadow is trusted to match the
/// shell: it starts true, is re-armed from an empty line on `Enter` (a fresh
/// prompt follows), and drops to false on anything Winter cannot model (Tab
/// completion, history recall, a paste, leaving the prompt). While inactive,
/// undo/redo are no-ops, so a stale model never rewrites the real line.
pub(crate) struct PromptShadow {
    active: bool,
    history: EditHistory<PromptLine>,
    last_kind: Option<EditKind>,
}

impl Default for PromptShadow {
    fn default() -> Self {
        Self {
            active: true,
            history: EditHistory::new(PromptLine::default()),
            last_kind: None,
        }
    }
}

impl PromptShadow {
    /// Re-arm tracking from an empty line, the state of the next prompt after a
    /// command is submitted.
    fn rearm_empty(&mut self) {
        self.history.reset(PromptLine::default());
        self.active = true;
        self.last_kind = None;
    }

    /// Stop trusting the shadow until the next fresh prompt. Keeps the recorded
    /// history but makes undo/redo no-ops.
    pub(crate) fn desync(&mut self) {
        self.active = false;
        self.last_kind = None;
    }

    /// Fold one Insert-mode key into the shadow. `at_prompt` is the pane's live
    /// shell-integration state; off the prompt the shadow cannot be trusted.
    pub(crate) fn apply_insert_key(&mut self, key: &Key, at_prompt: bool) {
        if !at_prompt || key.alt {
            self.desync();
            return;
        }
        if key.ctrl {
            match key.code {
                KeyCode::Char('a') => self.cursor_op(PromptLine::move_home),
                KeyCode::Char('e') => self.cursor_op(PromptLine::move_end),
                KeyCode::Char('u') => self.edit_op(PromptLine::kill_to_start, EditKind::Delete),
                KeyCode::Char('k') => self.edit_op(PromptLine::kill_to_end, EditKind::Delete),
                KeyCode::Char('w') => self.edit_op(PromptLine::kill_word_back, EditKind::Delete),
                // Readline's `Ctrl-D` on a non-empty line is delete-char-forward, the
                // same edit as the Delete key. Without modeling it, `Ctrl-D` would hit
                // the `_ => self.desync()` fallback and break undo/redo tracking.
                KeyCode::Char('d') => self.edit_op(PromptLine::delete_forward, EditKind::Delete),
                // SIGINT: readline-based shells abort whatever is on the line and
                // redraw a fresh, empty prompt, so undo/redo tracking should reset
                // the same way it does on `Enter`.
                KeyCode::Char('c') => self.rearm_empty(),
                // Undo/redo themselves arrive as actions, not raw keys.
                KeyCode::Char('/') | KeyCode::Char('\\') => {}
                _ => self.desync(),
            }
            return;
        }
        match key.code {
            KeyCode::Char(c) => self.edit_op(|l| l.insert_char(c), EditKind::Insert),
            KeyCode::Space => self.edit_op(|l| l.insert_char(' '), EditKind::Insert),
            KeyCode::Backspace => self.edit_op(PromptLine::backspace, EditKind::Delete),
            KeyCode::Delete => self.edit_op(PromptLine::delete_forward, EditKind::Delete),
            KeyCode::Left => self.cursor_op(PromptLine::move_left),
            KeyCode::Right => self.cursor_op(PromptLine::move_right),
            KeyCode::Home => self.cursor_op(PromptLine::move_home),
            KeyCode::End => self.cursor_op(PromptLine::move_end),
            KeyCode::Enter => self.rearm_empty(),
            _ => self.desync(),
        }
    }

    /// Fold a configurable Insert-mode line edit into the shadow. The matching
    /// readline bytes are sent separately via [`edit_action_bytes`].
    pub(crate) fn apply_edit_action(&mut self, action: EditAction, at_prompt: bool) {
        if !at_prompt {
            self.desync();
            return;
        }
        match action {
            EditAction::DeleteToLineEnd => self.edit_op(PromptLine::kill_to_end, EditKind::Delete),
            EditAction::DeleteToLineStart => {
                self.edit_op(PromptLine::kill_to_start, EditKind::Delete)
            }
            EditAction::DeleteWordBackward => {
                self.edit_op(PromptLine::kill_word_back, EditKind::Delete)
            }
            EditAction::DeleteWordForward => {
                self.edit_op(PromptLine::kill_word_forward, EditKind::Delete)
            }
        }
    }

    /// Mirror a Normal-mode delete operator into the shadow. `nav_col`/`pty_col`
    /// are the grid columns of the Vim cursor and the shell cursor; their gap maps
    /// the operator's target into character space.
    pub(crate) fn record_normal_delete(
        &mut self,
        op: PromptDelete,
        nav_col: usize,
        pty_col: usize,
    ) {
        if !self.active {
            return;
        }
        let line = self.history.current();
        let len = line.text.chars().count() as isize;
        let nav_index =
            (line.cursor as isize + nav_col as isize - pty_col as isize).clamp(0, len) as usize;
        let mut next = line.clone();
        next.apply_delete(op, nav_index);
        self.push(next, EditKind::Delete);
    }

    /// Realign the shadow cursor with the shell cursor when re-entering Insert
    /// mode after Normal-mode navigation, so later inserts land at the right spot.
    pub(crate) fn sync_cursor(&mut self, nav_col: usize, pty_col: usize) {
        if !self.active {
            return;
        }
        let mut line = self.history.current().clone();
        let len = line.text.chars().count() as isize;
        line.cursor =
            (line.cursor as isize + nav_col as isize - pty_col as isize).clamp(0, len) as usize;
        self.history.amend(line);
        self.last_kind = None;
    }

    /// Advance the undo pointer one step, keeping `current` aligned for a later
    /// redo. Undo itself is delegated to the shell ([`READLINE_UNDO`]); this only
    /// mirrors that move in the shadow. Returns whether a step was available.
    pub(crate) fn step_back(&mut self) -> bool {
        if !self.active {
            return false;
        }
        let stepped = self.history.undo().is_some();
        if stepped {
            self.last_kind = None;
        }
        stepped
    }

    /// The next redo state, or `None` when there is nothing to redo. The caller
    /// rebuilds the line from it ([`rebuild_line_bytes`]); rebuilding the whole
    /// target keeps redo correct even after the shell's own undo left the line in
    /// a state our shadow does not exactly model.
    pub(crate) fn redo_target(&mut self) -> Option<PromptLine> {
        if !self.active {
            return None;
        }
        let target = self.history.redo()?.clone();
        self.last_kind = None;
        Some(target)
    }

    fn edit_op(&mut self, f: impl FnOnce(&mut PromptLine), kind: EditKind) {
        if !self.active {
            return;
        }
        let mut line = self.history.current().clone();
        f(&mut line);
        self.push(line, kind);
    }

    fn cursor_op(&mut self, f: impl FnOnce(&mut PromptLine)) {
        if !self.active {
            return;
        }
        let mut line = self.history.current().clone();
        f(&mut line);
        // Pure motion is not an undo point, but it ends the current insert run.
        self.history.amend(line);
        self.last_kind = None;
    }

    /// Record `next`, coalescing a run of inserts into a single undo step.
    fn push(&mut self, next: PromptLine, kind: EditKind) {
        if self.last_kind == Some(EditKind::Insert) && kind == EditKind::Insert {
            self.history.amend(next);
        } else {
            self.history.record(next);
        }
        self.last_kind = Some(kind);
    }
}

impl PromptLine {
    fn len(&self) -> usize {
        self.text.chars().count()
    }

    fn set(&mut self, chars: Vec<char>, cursor: usize) {
        let len = chars.len();
        self.text = chars.into_iter().collect();
        self.cursor = cursor.min(len);
    }

    fn insert_char(&mut self, c: char) {
        let mut chars: Vec<char> = self.text.chars().collect();
        let at = self.cursor.min(chars.len());
        chars.insert(at, c);
        self.set(chars, at + 1);
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let mut chars: Vec<char> = self.text.chars().collect();
        chars.remove(self.cursor - 1);
        let cursor = self.cursor - 1;
        self.set(chars, cursor);
    }

    fn delete_forward(&mut self) {
        let mut chars: Vec<char> = self.text.chars().collect();
        if self.cursor < chars.len() {
            chars.remove(self.cursor);
            let cursor = self.cursor;
            self.set(chars, cursor);
        }
    }

    fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.len());
    }

    fn move_home(&mut self) {
        self.cursor = 0;
    }

    fn move_end(&mut self) {
        self.cursor = self.len();
    }

    fn kill_to_start(&mut self) {
        let chars: Vec<char> = self.text.chars().skip(self.cursor).collect();
        self.set(chars, 0);
    }

    fn kill_to_end(&mut self) {
        let chars: Vec<char> = self.text.chars().take(self.cursor).collect();
        let cursor = self.cursor;
        self.set(chars, cursor);
    }

    fn kill_word_back(&mut self) {
        let start = word_start_before(&self.text, self.cursor);
        let chars: Vec<char> = self.text.chars().collect();
        let kept: Vec<char> = chars[..start]
            .iter()
            .chain(chars[self.cursor..].iter())
            .copied()
            .collect();
        self.set(kept, start);
    }

    fn kill_word_forward(&mut self) {
        let end = word_end_after(&self.text, self.cursor);
        let chars: Vec<char> = self.text.chars().collect();
        let cursor = self.cursor;
        let kept: Vec<char> = chars[..cursor]
            .iter()
            .chain(chars[end..].iter())
            .copied()
            .collect();
        self.set(kept, cursor);
    }

    /// Apply a Vim delete operator with its target at char index `at`, mirroring
    /// the readline edit [`prompt_delete_bytes`] sends for the same operator.
    fn apply_delete(&mut self, op: PromptDelete, at: usize) {
        let chars: Vec<char> = self.text.chars().collect();
        match op {
            PromptDelete::CharForward => {
                if at < chars.len() {
                    let kept: Vec<char> = chars[..at]
                        .iter()
                        .chain(chars[at + 1..].iter())
                        .copied()
                        .collect();
                    self.set(kept, at);
                }
            }
            PromptDelete::Line => self.set(Vec::new(), 0),
            PromptDelete::Range { start_col, end_col } => {
                let start = start_col.min(chars.len());
                let end = (end_col + 1).min(chars.len());
                if start < end {
                    let kept: Vec<char> = chars[..start]
                        .iter()
                        .chain(chars[end..].iter())
                        .copied()
                        .collect();
                    self.set(kept, start);
                }
            }
            PromptDelete::ToLineEnd => self.set(chars[..at].to_vec(), at),
            PromptDelete::ToLineStart => self.set(chars[at..].to_vec(), 0),
            PromptDelete::WordBack => {
                let start = word_start_before(&self.text, at);
                let kept: Vec<char> = chars[..start]
                    .iter()
                    .chain(chars[at..].iter())
                    .copied()
                    .collect();
                self.set(kept, start);
            }
            PromptDelete::WordForward => {
                let end = word_end_after(&self.text, at);
                let kept: Vec<char> = chars[..at]
                    .iter()
                    .chain(chars[end..].iter())
                    .copied()
                    .collect();
                self.set(kept, at);
            }
        }
    }
}

/// The char index where the whitespace-delimited word ending at `cursor` begins:
/// skip trailing spaces, then the word's non-space run.
fn word_start_before(text: &str, cursor: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut i = cursor.min(chars.len());
    while i > 0 && chars[i - 1].is_whitespace() {
        i -= 1;
    }
    while i > 0 && !chars[i - 1].is_whitespace() {
        i -= 1;
    }
    i
}

/// The char index where the whitespace-delimited word starting at `cursor` ends:
/// skip the non-space run, then following spaces.
fn word_end_after(text: &str, cursor: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut i = cursor.min(chars.len());
    while i < chars.len() && !chars[i].is_whitespace() {
        i += 1;
    }
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    i
}

/// The readline keystrokes that make the live line become `target`, used by redo.
///
/// Redo cannot trust the line to match our shadow (the shell's own undo, which
/// powers undo, may leave it in a state we do not exactly model), so it rebuilds
/// the whole line: `Ctrl-A` `Ctrl-K` clears whatever is there, then the text is
/// inserted in a **single** operation. When the terminal has bracketed paste on
/// (`bracketed`), the text rides inside paste markers so the shell treats it as
/// one edit and plugins (syntax highlighting, autosuggestions) repaint once
/// instead of flashing per character. Finally the cursor walks left to the target.
pub(crate) fn rebuild_line_bytes(target: &PromptLine, bracketed: bool) -> Vec<u8> {
    let mut bytes = vec![CTRL_A, CTRL_K];
    if bracketed {
        bytes.extend_from_slice(BRACKETED_PASTE_START);
        bytes.extend_from_slice(target.text.as_bytes());
        bytes.extend_from_slice(BRACKETED_PASTE_END);
    } else {
        bytes.extend_from_slice(target.text.as_bytes());
    }
    for _ in 0..target.len().saturating_sub(target.cursor) {
        bytes.extend_from_slice(ARROW_LEFT);
    }
    bytes
}

/// The readline keystrokes that perform `action` from the live cursor. Cursor is
/// the shell's own, so no alignment is needed (unlike the Normal-mode operators).
pub(crate) fn edit_action_bytes(action: EditAction) -> Vec<u8> {
    match action {
        EditAction::DeleteToLineEnd => vec![CTRL_K],
        EditAction::DeleteToLineStart => vec![CTRL_U],
        EditAction::DeleteWordBackward => vec![CTRL_W],
        EditAction::DeleteWordForward => ALT_D.to_vec(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_clears_without_alignment() {
        assert_eq!(
            prompt_delete_bytes(PromptDelete::Line, 7, 3),
            vec![CTRL_A, CTRL_K]
        );
    }

    #[test]
    fn test_char_forward_aligns_left_then_deletes() {
        // Cursor at col 7, Vim cursor at col 5: two left arrows, then Delete.
        let bytes = prompt_delete_bytes(PromptDelete::CharForward, 7, 5);
        assert_eq!(bytes, [ARROW_LEFT, ARROW_LEFT, KEY_DELETE].concat());
    }

    #[test]
    fn test_to_line_end_aligns_right_then_kills() {
        // Cursor at col 2, Vim cursor at col 4: two right arrows, then Ctrl-K.
        let bytes = prompt_delete_bytes(PromptDelete::ToLineEnd, 2, 4);
        assert_eq!(bytes, [ARROW_RIGHT, ARROW_RIGHT, &[CTRL_K][..]].concat());
    }

    #[test]
    fn test_to_line_start_kills_with_ctrl_u() {
        let bytes = prompt_delete_bytes(PromptDelete::ToLineStart, 4, 4);
        assert_eq!(bytes, vec![CTRL_U]);
    }

    #[test]
    fn test_word_back_and_forward() {
        assert_eq!(
            prompt_delete_bytes(PromptDelete::WordBack, 3, 3),
            vec![CTRL_W]
        );
        assert_eq!(
            prompt_delete_bytes(PromptDelete::WordForward, 3, 3),
            ALT_D.to_vec()
        );
    }

    // -- Shadow buffer ----------------------------------------------------

    fn ch(c: char) -> Key {
        Key {
            alt: false,
            code: KeyCode::Char(c),
            ctrl: false,
            shift: false,
        }
    }

    fn typed(shadow: &mut PromptShadow, text: &str) {
        for c in text.chars() {
            let key = if c == ' ' {
                Key {
                    code: KeyCode::Space,
                    ..ch(' ')
                }
            } else {
                ch(c)
            };
            shadow.apply_insert_key(&key, true);
        }
    }

    fn current(shadow: &PromptShadow) -> PromptLine {
        shadow.history.current().clone()
    }

    #[test]
    fn test_insert_tracks_text_and_cursor() {
        let mut s = PromptShadow::default();
        typed(&mut s, "echo");
        assert_eq!(current(&s).text, "echo");
        assert_eq!(current(&s).cursor, 4);
    }

    #[test]
    fn test_inserts_coalesce_into_one_undo_step() {
        let mut s = PromptShadow::default();
        typed(&mut s, "echo");
        // One step back wipes the whole coalesced run to empty (the shell's own
        // undo realizes the edit; the shadow just tracks the move).
        assert!(s.step_back());
        assert_eq!(current(&s).text, "");
        assert!(!s.step_back(), "nothing left to undo");
    }

    #[test]
    fn test_undo_then_redo_restores_text() {
        let mut s = PromptShadow::default();
        typed(&mut s, "ls");
        s.step_back();
        let target = s.redo_target().unwrap();
        assert_eq!(target.text, "ls");
        assert_eq!(current(&s).text, "ls");
        // Rebuild clears the line then types the target (no bracketed paste here).
        let mut expected = vec![CTRL_A, CTRL_K];
        expected.extend_from_slice(b"ls");
        assert_eq!(rebuild_line_bytes(&target, false), expected);
    }

    #[test]
    fn test_rebuild_uses_bracketed_paste_when_available() {
        let target = PromptLine {
            text: String::from("echo hi"),
            cursor: 7,
        };
        let mut expected = vec![CTRL_A, CTRL_K];
        expected.extend_from_slice(BRACKETED_PASTE_START);
        expected.extend_from_slice(b"echo hi");
        expected.extend_from_slice(BRACKETED_PASTE_END);
        assert_eq!(rebuild_line_bytes(&target, true), expected);
    }

    #[test]
    fn test_rebuild_positions_cursor_left_of_end() {
        // Target cursor before the end: clear, type, then walk left.
        let target = PromptLine {
            text: String::from("abcd"),
            cursor: 1,
        };
        let mut expected = vec![CTRL_A, CTRL_K];
        expected.extend_from_slice(b"abcd");
        for _ in 0..3 {
            expected.extend_from_slice(ARROW_LEFT); // 4 - 1 = 3 lefts
        }
        assert_eq!(rebuild_line_bytes(&target, false), expected);
    }

    #[test]
    fn test_backspace_is_its_own_undo_step() {
        let mut s = PromptShadow::default();
        typed(&mut s, "abc");
        let bsp = Key {
            code: KeyCode::Backspace,
            ..ch('x')
        };
        s.apply_insert_key(&bsp, true);
        assert_eq!(current(&s).text, "ab");
        // Stepping back over the backspace restores "abc" (its own undo step).
        s.step_back();
        assert_eq!(current(&s).text, "abc");
    }

    /// A minimal emacs-readline buffer that interprets the byte streams undo/redo
    /// emit, so a test can confirm the bytes actually reproduce the target line.
    struct Readline {
        chars: Vec<char>,
        cursor: usize,
    }

    impl Readline {
        fn new(line: &PromptLine) -> Self {
            Self {
                chars: line.text.chars().collect(),
                cursor: line.cursor,
            }
        }

        fn apply(&mut self, bytes: &[u8]) {
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i..].starts_with(BRACKETED_PASTE_START) {
                    i += BRACKETED_PASTE_START.len();
                } else if bytes[i..].starts_with(BRACKETED_PASTE_END) {
                    i += BRACKETED_PASTE_END.len();
                } else if bytes[i..].starts_with(ARROW_LEFT) {
                    self.cursor = self.cursor.saturating_sub(1);
                    i += ARROW_LEFT.len();
                } else if bytes[i..].starts_with(ARROW_RIGHT) {
                    self.cursor = (self.cursor + 1).min(self.chars.len());
                    i += ARROW_RIGHT.len();
                } else if bytes[i] == CTRL_A {
                    self.cursor = 0;
                    i += 1;
                } else if bytes[i] == CTRL_K {
                    self.chars.truncate(self.cursor);
                    i += 1;
                } else {
                    // Printable ASCII (sufficient for these tests).
                    self.chars.insert(self.cursor, bytes[i] as char);
                    self.cursor += 1;
                    i += 1;
                }
            }
        }

        fn matches(&self, line: &PromptLine) -> bool {
            self.chars.iter().collect::<String>() == line.text && self.cursor == line.cursor
        }
    }

    /// Type `keys`, rewind to the start (as the shell's native undo would), then
    /// redo forward, asserting at every step that the emitted redo bytes drive a
    /// real readline buffer to the shadow's state. The buffer starts from an
    /// arbitrary "diverged" state each step to prove the rebuild does not depend on
    /// the prior line matching our shadow. (Undo itself is the shell's own
    /// single-op `Ctrl-_`, so only the redo rebuild is byte-tested here.)
    fn check_roundtrip(keys: &[Key]) {
        let mut shadow = PromptShadow::default();
        for k in keys {
            shadow.apply_insert_key(k, true);
        }
        while shadow.step_back() {}

        let mut steps = 0;
        while let Some(target) = shadow.redo_target() {
            // Start from junk to model the shell having left the line elsewhere.
            let mut real = Readline::new(&PromptLine {
                text: String::from("ZZZ"),
                cursor: 1,
            });
            real.apply(&rebuild_line_bytes(&target, steps % 2 == 0));
            assert!(
                real.matches(shadow.history.current()),
                "redo step {steps} diverged: real={:?}@{} shadow={:?}@{}",
                real.chars.iter().collect::<String>(),
                real.cursor,
                shadow.history.current().text,
                shadow.history.current().cursor,
            );
            steps += 1;
        }
    }

    fn keys(spec: &str) -> Vec<Key> {
        spec.chars()
            .map(|c| match c {
                '<' => Key {
                    code: KeyCode::Left,
                    ..ch('x')
                },
                '>' => Key {
                    code: KeyCode::Right,
                    ..ch('x')
                },
                '_' => Key {
                    code: KeyCode::Backspace,
                    ..ch('x')
                },
                ' ' => Key {
                    code: KeyCode::Space,
                    ..ch(' ')
                },
                other => ch(other),
            })
            .collect()
    }

    #[test]
    fn test_roundtrip_simple_typing() {
        check_roundtrip(&keys("echo hello"));
    }

    #[test]
    fn test_roundtrip_typing_with_backspaces() {
        check_roundtrip(&keys("helo__llo"));
    }

    #[test]
    fn test_roundtrip_typing_with_cursor_moves() {
        // Type "abc", go left twice, insert "XY", end with a left.
        check_roundtrip(&keys("abc<<XY<"));
    }

    #[test]
    fn test_roundtrip_insert_in_middle() {
        check_roundtrip(&keys("worldhello<<<<<"));
    }

    #[test]
    fn test_ctrl_c_rearms_tracking_after_a_desync() {
        let mut s = PromptShadow::default();
        // History recall (an unmodeled key) desyncs the shadow, mirroring a
        // user pressing Up to browse a previous command.
        let up = Key {
            code: KeyCode::Up,
            ..ch('x')
        };
        s.apply_insert_key(&up, true);
        assert!(!s.step_back(), "desynced: undo is a no-op");

        // Ctrl-C then clears the recalled line back to a fresh, empty prompt,
        // so subsequent typing builds on a fresh base rather than stale history.
        let ctrl_c = Key {
            code: KeyCode::Char('c'),
            ctrl: true,
            ..ch('x')
        };
        s.apply_insert_key(&ctrl_c, true);
        typed(&mut s, "next");
        assert_eq!(current(&s).text, "next");
    }

    #[test]
    fn test_repeated_ctrl_d_deletes_from_front() {
        let mut s = PromptShadow::default();
        typed(&mut s, "hi");
        let home = Key {
            code: KeyCode::Home,
            ..ch('x')
        };
        s.apply_insert_key(&home, true);

        let ctrl_d = Key {
            code: KeyCode::Char('d'),
            ctrl: true,
            ..ch('x')
        };
        s.apply_insert_key(&ctrl_d, true);
        assert_eq!(current(&s).text, "i");

        s.apply_insert_key(&ctrl_d, true);
        assert_eq!(current(&s).text, "");
    }

    #[test]
    fn test_unmodeled_key_desyncs_and_disables_undo() {
        let mut s = PromptShadow::default();
        typed(&mut s, "hi");
        let tab = Key {
            code: KeyCode::Tab,
            ..ch('x')
        };
        s.apply_insert_key(&tab, true);
        assert!(!s.step_back(), "undo is a no-op after a desync");
    }

    #[test]
    fn test_off_prompt_desyncs() {
        let mut s = PromptShadow::default();
        typed(&mut s, "hi");
        s.apply_insert_key(&ch('x'), false); // not at prompt
        assert!(!s.step_back());
    }

    #[test]
    fn test_enter_rearms_from_empty() {
        let mut s = PromptShadow::default();
        typed(&mut s, "first");
        let enter = Key {
            code: KeyCode::Enter,
            ..ch('x')
        };
        s.apply_insert_key(&enter, true);
        assert_eq!(current(&s).text, "");
        assert!(!s.step_back(), "history reset after submit");
        typed(&mut s, "next");
        assert_eq!(current(&s).text, "next");
    }

    #[test]
    fn test_normal_delete_word_back_is_undoable() {
        let mut s = PromptShadow::default();
        typed(&mut s, "echo hello");
        // Vim cursor and shell cursor both at end (col gap zero): db removes "hello".
        s.record_normal_delete(PromptDelete::WordBack, 10, 10);
        assert_eq!(current(&s).text, "echo ");
        s.step_back();
        assert_eq!(current(&s).text, "echo hello");
    }

    #[test]
    fn test_edit_action_bytes_match_readline() {
        assert_eq!(
            edit_action_bytes(EditAction::DeleteWordBackward),
            vec![CTRL_W]
        );
        assert_eq!(
            edit_action_bytes(EditAction::DeleteWordForward),
            ALT_D.to_vec()
        );
        assert_eq!(
            edit_action_bytes(EditAction::DeleteToLineStart),
            vec![CTRL_U]
        );
        assert_eq!(edit_action_bytes(EditAction::DeleteToLineEnd), vec![CTRL_K]);
    }

    #[test]
    fn test_apply_edit_action_delete_word_backward_is_undoable() {
        let mut s = PromptShadow::default();
        typed(&mut s, "git commit");
        s.apply_edit_action(EditAction::DeleteWordBackward, true);
        assert_eq!(current(&s).text, "git ");
        s.step_back();
        assert_eq!(current(&s).text, "git commit");
    }

    #[test]
    fn test_apply_edit_action_off_prompt_desyncs() {
        let mut s = PromptShadow::default();
        typed(&mut s, "abc");
        s.apply_edit_action(EditAction::DeleteWordBackward, false);
        assert!(!s.step_back());
    }

    #[test]
    fn test_word_start_before_skips_trailing_space() {
        assert_eq!(word_start_before("echo hello", 10), 5);
        assert_eq!(word_start_before("echo ", 5), 0);
    }

    #[test]
    fn test_word_end_after_includes_trailing_space() {
        assert_eq!(word_end_after("echo hello", 0), 5);
        assert_eq!(word_end_after("echo hello", 5), 10);
    }

    #[test]
    fn test_prompt_replace_char_bytes_aligns_deletes_and_steps_back() {
        // From pty_col 5 to nav_col 2: 3 left arrows, delete 1 char, 'x', 1 left arrow
        let bytes = prompt_replace_char_bytes(5, 2, 'x');
        let expected = b"\x1b[D\x1b[D\x1b[D\x1b[3~x\x1b[D";
        assert_eq!(bytes, expected);
    }

    #[test]
    fn test_prompt_toggle_case_bytes_aligns_deletes_and_inserts() {
        // From pty_col 2 to nav_col 4: 2 right arrows, delete 1 char, 'A'
        let bytes = prompt_toggle_case_bytes(2, 4, 'A');
        let expected = b"\x1b[C\x1b[C\x1b[3~A";
        assert_eq!(bytes, expected);
    }
}
