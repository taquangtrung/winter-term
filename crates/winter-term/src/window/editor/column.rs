//! The column cursor every editor drawing rows gets on its cursor row, for
//! the Vim motions along a line: `h` `l`, `0` `^` `$`, `w` `b` `e` and their
//! big forms, and `f` `F` `t` `T` `;` `,`.
//!
//! An editor whose cursor is a whole row (Git, Dir, Grep, the process list)
//! keeps the rows: `j`, `k`, and the paging keys move its cursor through
//! what it lists. The column along the row is the window's, kept here once
//! for all of them and read off the row as it was painted, so a motion means
//! the same over every editor as it does over a terminal's Normal mode,
//! with the same word rules beneath. The editor's own bindings come first:
//! Dir's `h` and `l` still fold, and a motion reaches the column only when
//! the editor has declined the key. An editor with a caret of its own (the
//! text editor) moves it itself.

use crate::model::input::{Key, KeyCode, Mods};
use crate::model::layout::TabId;
use crate::model::vim::motion::FindChar;
use crate::model::vim::words::{
    char_class, find_char, first_non_blank, next_word_start, prev_word_start, word_end,
};

use crate::window::terminal::navigation::vim::line_chars;
use crate::window::App;

// ========================================================================
// Data Structures
// ========================================================================

/// The column cursor on an editor's cursor row, and the char search it
/// carries between keys.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::window) struct RowCursor {
    /// Where along the row: kept as rows change, the way Vim keeps the
    /// column a `j` or `k` leaves.
    col: Col,
    /// `f`, `F`, `t`, or `T`, waiting for the character to search for.
    finding: Option<Finding>,
    /// The last char search, for `;` and `,`.
    last_find: Option<FindChar>,
}

/// Where along a row the column cursor is, resolved against the row it
/// lands on: a place that depends on the row's text is kept as that place,
/// not as the column it was on the row before.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Col {
    /// A column, cut back to the row's end on a shorter row.
    At(usize),
    /// The row's last character (`$`).
    End,
    /// The row's first non-blank character (`^`): where a row cursor is
    /// read from, until a motion along the row moves it.
    #[default]
    FirstNonBlank,
    /// The end of the row's first word, for `e` carried down from a row
    /// with no word left.
    FirstWordEnd,
    /// The start of the row's last word, for `b` carried up from a row's
    /// first word.
    LastWordStart,
}

/// A char search waiting for its character.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Finding {
    forward: bool,
    till: bool,
}

/// What a key did to the column cursor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) enum Step {
    /// The key is no motion along a row.
    Unhandled,
    /// The cursor moved, or the key started a char search.
    Moved,
    /// The motion ran off the row: the editor's cursor steps down (`true`)
    /// or up, and the column is where the motion lands there.
    Cross(bool),
}

// ========================================================================
// RowCursor
// ========================================================================

impl RowCursor {
    /// A cursor parked at the leftmost column (0).
    pub(in crate::window) fn leftmost() -> Self {
        Self {
            col: Col::At(0),
            finding: None,
            last_find: None,
        }
    }

    /// The column the cursor is on along `line`.
    pub(in crate::window) fn col_on(&self, line: &[char]) -> usize {
        let last = line.len().saturating_sub(1);
        match self.col {
            Col::At(col) => col.min(last),
            Col::End => last,
            Col::FirstNonBlank => first_non_blank(line),
            Col::FirstWordEnd => first_word_end(line),
            Col::LastWordStart => prev_word_start(line, line.len(), false).unwrap_or(0),
        }
    }

    /// Move along `line` by `key`, which has no modifier but Shift.
    pub(in crate::window) fn key(&mut self, key: &Key, line: &[char]) -> Step {
        let col = self.col_on(line);
        let last = line.len().saturating_sub(1);
        if let Some(finding) = self.finding.take() {
            if let (Mods::NONE | Mods::SHIFT, KeyCode::Char(ch)) = key.chord() {
                let find = FindChar {
                    ch,
                    forward: finding.forward,
                    till: finding.till,
                };
                self.last_find = Some(find);
                self.find(line, col, find);
            }
            return Step::Moved;
        }
        let word = |big: bool| next_word_start(line, col, big);
        match key.chord() {
            (Mods::NONE, KeyCode::Char('h') | KeyCode::Left) => {
                self.col = Col::At(col.saturating_sub(1))
            }
            (Mods::NONE, KeyCode::Char('l') | KeyCode::Right) => {
                self.col = Col::At((col + 1).min(last))
            }
            (Mods::NONE, KeyCode::Char('0') | KeyCode::Home) => self.col = Col::At(0),
            (Mods::NONE, KeyCode::Char('^' | '_')) => self.col = Col::FirstNonBlank,
            (Mods::NONE, KeyCode::Char('$') | KeyCode::End) => self.col = Col::End,
            (Mods::NONE, KeyCode::Char(c @ ('w' | 'W'))) => match word(c == 'W') {
                Some(at) => self.col = Col::At(at),
                None => return self.cross(true, Col::FirstNonBlank),
            },
            (Mods::NONE, KeyCode::Char(c @ ('b' | 'B'))) => {
                match prev_word_start(line, col, c == 'B') {
                    Some(at) => self.col = Col::At(at),
                    None => return self.cross(false, Col::LastWordStart),
                }
            }
            (Mods::NONE, KeyCode::Char(c @ ('e' | 'E'))) => match word_end(line, col, c == 'E') {
                Some(at) => self.col = Col::At(at),
                None => return self.cross(true, Col::FirstWordEnd),
            },
            (Mods::NONE, KeyCode::Char(c @ ('f' | 'F' | 't' | 'T'))) => {
                self.finding = Some(Finding {
                    forward: c.is_ascii_lowercase(),
                    till: c.eq_ignore_ascii_case(&'t'),
                });
            }
            (Mods::NONE, KeyCode::Char(c @ (';' | ','))) => {
                let Some(find) = self.last_find else {
                    return Step::Moved;
                };
                let find = match c == ',' {
                    true => find.reversed(),
                    false => find,
                };
                self.find(line, col, find);
            }
            _ => return Step::Unhandled,
        }
        Step::Moved
    }

    /// Put the cursor on column `col`, cut back to the row's end wherever it
    /// lands, and drop a char search waiting for its character.
    pub(in crate::window) fn place(&mut self, col: usize) {
        self.col = Col::At(col);
        self.finding = None;
    }

    /// Land a char search from `col`, staying put where it finds nothing.
    fn find(&mut self, line: &[char], col: usize, find: FindChar) {
        if let Some(at) = find_char(line, col, find) {
            self.col = Col::At(at);
        }
    }

    /// Run off the row to the next (`down`) or the one before, landing at
    /// `col` there.
    fn cross(&mut self, down: bool, col: Col) -> Step {
        self.col = col;
        Step::Cross(down)
    }
}

// ========================================================================
// App: the column cursor
// ========================================================================

impl App {
    /// Move the column cursor of the editor in `tab` by `key`, which the
    /// editor declined. Returns whether it was a motion along a row; one run
    /// off the row steps the editor's own cursor down or up.
    pub(in crate::window) fn editor_column_key(&mut self, tab: TabId, key: &Key) -> bool {
        if !matches!(key.chord().0, Mods::NONE | Mods::SHIFT) {
            return false;
        }
        let Some(slot) = self.open_tabs.editor_mut(&tab) else {
            return false;
        };
        if slot.editor.caret().is_some() {
            return false;
        }
        let (Some(row), Some(grid)) = (slot.view.cursor_line, slot.view.painted.as_ref()) else {
            return false;
        };
        let line = line_chars(grid, row);
        match slot.view.column.key(key, &line) {
            Step::Unhandled => return false,
            Step::Moved => {}
            Step::Cross(down) => {
                let code = match down {
                    true => KeyCode::Down,
                    false => KeyCode::Up,
                };
                let outcome = slot.editor.on_key(&Key::plain(code));
                self.act_on_editor_outcome(tab, outcome);
            }
        }
        self.dirty = true;
        true
    }

    /// Put the column cursor of the editor in `tab` on column `col`, where a
    /// click landed. An editor with a caret of its own placed it itself.
    pub(in crate::window) fn place_editor_column(&mut self, tab: TabId, col: usize) {
        if let Some(slot) = self.open_tabs.editor_mut(&tab) {
            if slot.editor.caret().is_none() {
                slot.view.column.place(col);
            }
        }
    }

    /// The column the cursor of the editor in `tab` is on along its painted
    /// row `row`.
    pub(in crate::window) fn editor_column(&self, tab: TabId, row: usize) -> Option<usize> {
        let slot = self.open_tabs.editor(&tab)?;
        let grid = slot.view.painted.as_ref()?;
        Some(slot.view.column.col_on(&line_chars(grid, row)))
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// The column the row's first word ends on: where `e` lands coming onto it.
fn first_word_end(line: &[char]) -> usize {
    let start = first_non_blank(line);
    let Some(&first) = line.get(start) else {
        return 0;
    };
    let class = char_class(first, false);
    let mut end = start;
    while line
        .get(end + 1)
        .is_some_and(|c| char_class(*c, false) == class)
    {
        end += 1;
    }
    end
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    fn keys(cursor: &mut RowCursor, line: &[char], typed: &str) -> Step {
        let mut last = Step::Unhandled;
        for c in typed.chars() {
            last = cursor.key(&Key::plain(KeyCode::Char(c)), line);
        }
        last
    }

    #[test]
    fn test_a_row_cursor_starts_on_the_first_non_blank_and_moves_along_the_row() {
        let line = chars("  M  src/main.rs  2 hours");
        let mut cursor = RowCursor::default();
        assert_eq!(cursor.col_on(&line), 2);
        keys(&mut cursor, &line, "l");
        assert_eq!(cursor.col_on(&line), 3);
        keys(&mut cursor, &line, "w");
        assert_eq!(cursor.col_on(&line), 5, "on to src");
        keys(&mut cursor, &line, "e");
        assert_eq!(cursor.col_on(&line), 7, "the end of src");
        keys(&mut cursor, &line, "$");
        assert_eq!(cursor.col_on(&line), line.len() - 1);
        keys(&mut cursor, &line, "0");
        assert_eq!(cursor.col_on(&line), 0);
        keys(&mut cursor, &line, "^");
        assert_eq!(cursor.col_on(&line), 2);
    }

    #[test]
    fn test_the_column_is_kept_and_cut_back_on_a_shorter_row() {
        let long = chars("abcdefghij");
        let short = chars("abc");
        let mut cursor = RowCursor::default();
        keys(&mut cursor, &long, "$");
        assert_eq!(cursor.col_on(&short), 2, "`$` holds to each row's end");
        keys(&mut cursor, &long, "0lllll");
        assert_eq!(cursor.col_on(&short), 2);
        assert_eq!(cursor.col_on(&long), 5, "and the column comes back");
    }

    #[test]
    fn test_word_motions_run_off_the_row_to_the_next_and_the_one_before() {
        let line = chars("one two");
        let mut cursor = RowCursor::default();
        assert_eq!(keys(&mut cursor, &line, "w"), Step::Moved);
        assert_eq!(keys(&mut cursor, &line, "w"), Step::Cross(true));
        assert_eq!(cursor.col_on(&chars("  next row")), 2, "its first word");
        let mut back = RowCursor::default();
        assert_eq!(keys(&mut back, &line, "b"), Step::Cross(false));
        assert_eq!(back.col_on(&chars("prev row here")), 9, "its last word");
        let mut end = RowCursor::default();
        keys(&mut end, &line, "$");
        assert_eq!(keys(&mut end, &line, "e"), Step::Cross(true));
        assert_eq!(end.col_on(&chars(" abc def")), 3);
    }

    #[test]
    fn test_char_searches_land_and_repeat_both_ways() {
        let line = chars("a-b-c-d");
        let mut cursor = RowCursor::default();
        assert_eq!(keys(&mut cursor, &line, "f-"), Step::Moved);
        assert_eq!(cursor.col_on(&line), 1);
        keys(&mut cursor, &line, ";");
        assert_eq!(cursor.col_on(&line), 3);
        keys(&mut cursor, &line, ",");
        assert_eq!(cursor.col_on(&line), 1);
        keys(&mut cursor, &line, "tc");
        assert_eq!(cursor.col_on(&line), 3, "till stops short");
    }

    /// An app whose focused tab is an editor whose cursor is a whole row
    /// (the notifications list), painted with `lines` and its cursor on the
    /// first.
    fn app_with_rows(lines: &[&str]) -> (App, TabId) {
        use crate::editors::notifications::NotificationsEditor;
        use crate::window::editor::EditorKind;
        let mut app = App::new();
        let tab = app.show_editor(
            EditorKind::Notifications,
            Box::new(NotificationsEditor::new(Vec::new(), 0)),
        );
        let mut grid = winter_core::Grid::new(40, lines.len());
        for (row, line) in lines.iter().enumerate() {
            grid.move_to(row, 0);
            line.chars().for_each(|ch| grid.print(ch));
        }
        if let Some(slot) = app.open_tabs.editor_mut(&tab) {
            slot.view.painted = Some(grid);
            slot.view.cursor_line = Some(0);
        }
        (app, tab)
    }

    /// Type `c` through the window's whole key routing.
    fn type_char(app: &mut App, c: char) {
        use winit::event::ElementState;
        use winit::keyboard::{Key as WinitKey, NativeKeyCode, PhysicalKey};
        let physical = PhysicalKey::Unidentified(NativeKeyCode::Unidentified);
        app.handle_key(
            ElementState::Pressed,
            WinitKey::Character(c.to_string().into()),
            physical,
        );
        app.handle_key(
            ElementState::Released,
            WinitKey::Character(c.to_string().into()),
            physical,
        );
    }

    #[test]
    fn test_an_editor_s_row_takes_the_motions_along_it_and_v_selects_from_there() {
        let (mut app, tab) = app_with_rows(&["  alpha beta gamma", "next"]);
        assert_eq!(app.editor_column(tab, 0), Some(2), "on the first non-blank");
        type_char(&mut app, 'w');
        assert_eq!(app.editor_column(tab, 0), Some(8), "on to beta");
        type_char(&mut app, 'l');
        assert_eq!(app.editor_column(tab, 0), Some(9));
        type_char(&mut app, '$');
        assert_eq!(app.editor_column(tab, 0), Some(17));
        type_char(&mut app, 'b');
        assert_eq!(app.editor_column(tab, 0), Some(13), "back to gamma");
        app.start_editor_cursor(tab);
        let cursor = app.editor_cursor.as_ref().expect("a selection cursor");
        assert_eq!(
            (cursor.row, cursor.col),
            (0, 13),
            "v starts where the column is"
        );
    }

    #[test]
    fn test_a_click_puts_the_column_cursor_on_the_cell_clicked() {
        let (mut app, tab) = app_with_rows(&["  alpha beta", "x"]);
        app.place_editor_column(tab, 9);
        assert_eq!(app.editor_column(tab, 0), Some(9));
        assert_eq!(
            app.editor_column(tab, 1),
            Some(0),
            "cut back on a short row"
        );
        app.place_editor_column(tab, 30);
        assert_eq!(
            app.editor_column(tab, 0),
            Some(11),
            "past the end, on the last"
        );
    }

    #[test]
    fn test_leftmost_cursor_stays_at_column_zero() {
        let line = chars("   indented text");
        let cursor = RowCursor::leftmost();
        assert_eq!(cursor.col_on(&line), 0);
    }

    #[test]
    fn test_keys_that_are_no_motion_along_a_row_are_left_alone() {
        let line = chars("text");
        let mut cursor = RowCursor::default();
        assert_eq!(keys(&mut cursor, &line, "j"), Step::Unhandled);
        assert_eq!(keys(&mut cursor, &line, "s"), Step::Unhandled);
        assert_eq!(cursor, RowCursor::default());
    }
}
