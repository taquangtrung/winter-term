//! The names as editable text: inline renaming in the manner of Emacs'
//! `wdired`, toggled by `Ctrl-X Ctrl-Q` and driven by a full Vim mode pair.
//! Normal mode carries the motion and operator vocabulary (`h`/`l`, `w`/`b`/`e`,
//! `f`/`t`/`;`, `d`/`c`/`y` with counts and doublings, `x`, `r`, `~`, `p`, `u`,
//! `i`/`a`/`I`/`A`, `ZZ`/`ZQ`); Insert mode is an Emacs-style field with the
//! readline chords (`Ctrl-A`/`E`/`B`/`F`/`N`/`P`/`K`/`U`/`W`/`T`/`Y`, the
//! case commands `Alt-U`/`L`/`C`, and `Alt-B`/`F`/`D`/`Backspace`).
//! `ZZ`, or `Enter` from Insert, applies the differences as renames; `q`,
//! `ZQ`, or a second `Ctrl-X Ctrl-Q` throws them away.

use crate::model::input::{FindChar, Key, KeyCode};
use crate::model::vim::words::{first_non_blank, next_word_start, prev_word_start, word_end};

use super::tree::Row;

// ========================================================================
// Constants
// ========================================================================

/// How many undo steps an edit session keeps: deep enough for a renaming
/// spree, bounded so a session's snapshots cannot grow without limit.
const MAX_UNDO: usize = 100;

// ========================================================================
// Data Structures
// ========================================================================

/// Which mode the editor is in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditMode {
    /// Motions and operators: a key is a command, not a character.
    Normal,
    /// Characters land in the name; the readline chords move and edit.
    Insert,
}

/// What a key in the editor asked the listing to do beyond the names
/// themselves. Row movement and the session's end are the listing's to carry
/// out; everything else happens inside [`EditState`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditAction {
    /// The key was spent on the names or the caret.
    Consumed,
    /// The editor has no binding for it, so the window may have one.
    Ignored,
    /// Move the listing's cursor by `delta` rows.
    MoveRows(isize),
    /// Move the listing's cursor to an absolute row, clamped by the listing.
    MoveToRow(usize),
    /// Apply the edited names as renames (`ZZ`, or `Enter` from Insert).
    Apply,
    /// Leave the edit session, keeping the disk as it is (`q`, `ZQ`).
    Leave,
}

/// A Vim operator awaiting its motion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OpKind {
    /// `d`: delete the range.
    Delete,
    /// `c`: delete the range and enter Insert.
    Change,
    /// `y`: copy the range into the register.
    Yank,
}

/// What Normal mode is waiting for, if anything.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pending {
    /// Nothing: a fresh keystroke starts a fresh command.
    None,
    /// The digits of a count, awaiting the command they scale.
    Count(usize),
    /// `d`/`c`/`y` with its leading count, awaiting a motion or its doubling.
    Operator { op: OpKind, count: usize },
    /// Digits typed between the operator and its motion, which multiply the
    /// leading count (`2d3w` deletes six words).
    OperatorCount {
        op: OpKind,
        count: usize,
        more: usize,
    },
    /// `f`/`F`/`t`/`T` with its count, awaiting the character to search for.
    Find { base: FindChar, count: usize },
    /// An operator whose motion is a char search (`df-`), awaiting the char.
    OperatorFind {
        op: OpKind,
        count: usize,
        base: FindChar,
    },
    /// `r`, awaiting the replacement character.
    Replace(usize),
    /// `g`, awaiting its second key.
    G,
    /// `Z`, awaiting `Z` or `Q`.
    Z,
}

/// Where a motion lands, in the terms an operator needs: the column to move
/// to, which side of the start it went, and whether an operator acting on it
/// consumes the landing character (Vim's inclusive/exclusive distinction:
/// `de` takes the word's last character, `dw` stops before the next one).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Landing {
    col: usize,
    forward: bool,
    inclusive: bool,
}

/// A Vim motion, resolved against one name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Motion {
    /// `h`/`Backspace`/`Left`.
    Left,
    /// `l`/`Space`/`Right`.
    Right,
    /// `0`.
    Start,
    /// `^`.
    FirstNonBlank,
    /// `$`, which lands on the name's last character and is inclusive.
    End,
    /// `w`/`W`, exclusive.
    WordFwd { big: bool },
    /// `b`/`B`, backward.
    WordBack { big: bool },
    /// `e`/`E`, inclusive.
    WordEnd { big: bool },
    /// A char search: `f`/`F`/`t`/`T`, or `;`/`,` repeating the last one.
    Find(FindChar),
}

/// One state of the edit session, kept so `u` and `Ctrl-R` step back and
/// forth through it. The row and column ride along, so an undo lands the
/// caret where the change happened.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot {
    col: usize,
    names: Vec<String>,
    row: usize,
}

/// Which undo stack a restore pulls from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Direction {
    Undo,
    Redo,
}

/// What one of the Emacs case commands does to a word.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Case {
    /// `M-u`: upcase the word.
    Upper,
    /// `M-l`: downcase the word.
    Lower,
    /// `M-c`: capitalize it — the first character up, the rest down, the way
    /// Emacs's `capitalize-word` leaves `README` as `Readme`.
    Capital,
}

/// The editable names of a listing, one per row, the caret in the name under
/// the listing's cursor, and the Vim/Emacs state driving both.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditState {
    /// The name of row `i` as edited so far, aligned with the listing's rows.
    names: Vec<String>,
    /// How many characters of the edited name sit before the caret.
    col: usize,
    mode: EditMode,
    pending: Pending,
    /// The text of the last delete or yank, for `p`/`P` and `Ctrl-Y`.
    register: String,
    /// The last `f`/`F`/`t`/`T`, for `;` and `,`.
    last_find: Option<FindChar>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Whether nothing has been typed since entering Insert, so a whole
    /// typing run undoes as one step, the way Vim groups it.
    insert_fresh: bool,
}

// ========================================================================
// EditState: construction and inspection
// ========================================================================

impl EditState {
    /// A snapshot of the rows' names in Normal mode, the caret parked at the
    /// end of row `cursor`'s name: a rename usually rewrites a name's tail,
    /// and `A` is where one starts.
    pub fn new(rows: &[Row], cursor: usize) -> Self {
        let names: Vec<String> = rows.iter().map(|row| row.entry.name.clone()).collect();
        let col = names
            .get(cursor)
            .map(|name| name.chars().count())
            .unwrap_or(0);
        Self {
            names,
            col,
            mode: EditMode::Normal,
            pending: Pending::None,
            register: String::new(),
            last_find: None,
            undo: Vec::new(),
            redo: Vec::new(),
            insert_fresh: true,
        }
    }

    /// The edited name of row `index`, if the listing still has that row.
    pub fn name(&self, index: usize) -> Option<&str> {
        self.names.get(index).map(String::as_str)
    }

    /// Where the caret sits, in characters.
    pub fn col(&self) -> usize {
        self.col
    }

    /// Which mode the editor is in.
    pub fn mode(&self) -> EditMode {
        self.mode
    }

    /// The text of the last delete or yank: what `p`/`P` and `Ctrl-Y` put back.
    pub fn register(&self) -> &str {
        &self.register
    }

    /// Clamp the caret into the name of the row the listing's cursor moved to.
    pub fn move_to_row(&mut self, index: usize) {
        if let Some(name) = self.names.get(index) {
            self.col = self.col.min(name.chars().count());
        }
    }
}

// ========================================================================
// EditState: key dispatch
// ========================================================================

impl EditState {
    /// One key, in whichever mode the editor is in. `row` is the listing's
    /// cursor row; effects beyond the names come back as an [`EditAction`].
    pub fn on_key(&mut self, row: usize, key: &Key) -> EditAction {
        match self.mode {
            EditMode::Normal => self.on_normal_key(row, key),
            EditMode::Insert => self.on_insert_key(row, key),
        }
    }

    /// Normal mode: the Vim motion and operator vocabulary.
    fn on_normal_key(&mut self, row: usize, key: &Key) -> EditAction {
        if key.alt {
            return EditAction::Ignored;
        }
        if key.ctrl {
            return match key.code {
                KeyCode::Char('r') => self.restore(row, Direction::Redo),
                _ => EditAction::Ignored,
            };
        }
        // The special keys run bare, cancelling anything pending rather than
        // trying to serve as its motion: an arrow mid-`d` means the operator
        // was abandoned, and Vim agrees.
        match key.code {
            KeyCode::Escape => {
                self.pending = Pending::None;
                return EditAction::Consumed;
            }
            KeyCode::Left | KeyCode::Backspace => {
                self.pending = Pending::None;
                return self.motion_key(row, Motion::Left, 1);
            }
            KeyCode::Right | KeyCode::Space => {
                self.pending = Pending::None;
                return self.motion_key(row, Motion::Right, 1);
            }
            KeyCode::Home => {
                self.pending = Pending::None;
                return self.motion_key(row, Motion::Start, 1);
            }
            KeyCode::End => {
                self.pending = Pending::None;
                return self.motion_key(row, Motion::End, 1);
            }
            KeyCode::Up => {
                self.pending = Pending::None;
                return EditAction::MoveRows(-1);
            }
            KeyCode::Down | KeyCode::Enter => {
                self.pending = Pending::None;
                return EditAction::MoveRows(1);
            }
            _ => {}
        }
        let KeyCode::Char(ch) = key.code else {
            return EditAction::Ignored;
        };
        // A pending state consumes the key itself.
        let pending = std::mem::replace(&mut self.pending, Pending::None);
        match pending {
            Pending::Count(count) => self.counted_key(row, count, ch),
            Pending::Operator { op, count } => self.operator_key(row, op, count, 0, ch),
            Pending::OperatorCount { op, count, more } => {
                self.operator_key(row, op, count, more, ch)
            }
            Pending::Find { base, count } => {
                let find = FindChar { ch, ..base };
                self.last_find = Some(find);
                self.motion_key(row, Motion::Find(find), count)
            }
            Pending::OperatorFind { op, count, base } => {
                let find = FindChar { ch, ..base };
                self.last_find = Some(find);
                self.operate(row, op, count, Motion::Find(find))
            }
            Pending::Replace(count) => {
                self.replace_chars(row, count, ch);
                EditAction::Consumed
            }
            Pending::G => {
                if ch == 'g' {
                    EditAction::MoveToRow(0)
                } else {
                    EditAction::Consumed
                }
            }
            Pending::Z => match ch {
                'Z' => EditAction::Apply,
                'Q' => EditAction::Leave,
                _ => EditAction::Consumed,
            },
            Pending::None => self.bare_key(row, ch),
        }
    }

    /// A key with nothing pending behind it.
    fn bare_key(&mut self, row: usize, ch: char) -> EditAction {
        match ch {
            '1'..='9' => {
                self.pending = Pending::Count(ch.to_digit(10).unwrap_or(1) as usize);
            }
            '0' => return self.motion_key(row, Motion::Start, 1),
            '^' => return self.motion_key(row, Motion::FirstNonBlank, 1),
            'h' => return self.motion_key(row, Motion::Left, 1),
            'l' => return self.motion_key(row, Motion::Right, 1),
            'w' => return self.motion_key(row, Motion::WordFwd { big: false }, 1),
            'W' => return self.motion_key(row, Motion::WordFwd { big: true }, 1),
            'b' => return self.motion_key(row, Motion::WordBack { big: false }, 1),
            'B' => return self.motion_key(row, Motion::WordBack { big: true }, 1),
            'e' => return self.motion_key(row, Motion::WordEnd { big: false }, 1),
            'E' => return self.motion_key(row, Motion::WordEnd { big: true }, 1),
            '$' => return self.motion_key(row, Motion::End, 1),
            'f' | 'F' | 't' | 'T' => {
                self.pending = Pending::Find {
                    base: FindChar {
                        ch: '\0',
                        forward: ch == 'f' || ch == 't',
                        till: ch == 't' || ch == 'T',
                    },
                    count: 1,
                };
            }
            ';' | ',' => {
                if let Some(find) = self.last_find {
                    let find = if ch == ',' { find.reversed() } else { find };
                    return self.motion_key(row, Motion::Find(find), 1);
                }
            }
            'g' => self.pending = Pending::G,
            'G' => return EditAction::MoveToRow(usize::MAX),
            'j' => return EditAction::MoveRows(1),
            'k' => return EditAction::MoveRows(-1),
            'x' => {
                self.snapshot(row);
                self.take_forward(row, 1);
            }
            'X' => {
                self.snapshot(row);
                self.take_back(row, 1);
            }
            'D' => {
                self.snapshot(row);
                self.cut_range(row, self.col, self.len(row));
            }
            'C' => {
                self.snapshot(row);
                self.cut_range(row, self.col, self.len(row));
                self.enter_insert();
            }
            's' => {
                self.snapshot(row);
                self.take_forward(row, 1);
                self.enter_insert();
            }
            'S' => {
                self.snapshot(row);
                let name = self.name(row).unwrap_or_default().to_string();
                self.register = name;
                self.set_name(row, "");
                self.enter_insert();
            }
            'r' => self.pending = Pending::Replace(1),
            '~' => {
                self.snapshot(row);
                self.toggle_case(row, 1);
            }
            'p' => {
                self.snapshot(row);
                self.paste(row, true);
            }
            'P' => {
                self.snapshot(row);
                self.paste(row, false);
            }
            'u' => return self.restore(row, Direction::Undo),
            'd' => {
                self.pending = Pending::Operator {
                    op: OpKind::Delete,
                    count: 1,
                }
            }
            'c' => {
                self.pending = Pending::Operator {
                    op: OpKind::Change,
                    count: 1,
                }
            }
            'y' => {
                self.pending = Pending::Operator {
                    op: OpKind::Yank,
                    count: 1,
                }
            }
            'i' => self.enter_insert(),
            'a' => {
                self.col = (self.col + 1).min(self.len(row));
                self.enter_insert();
            }
            'I' => {
                self.col = self.first_non_blank(row);
                self.enter_insert();
            }
            'A' => {
                self.col = self.len(row);
                self.enter_insert();
            }
            'Z' => self.pending = Pending::Z,
            'q' => return EditAction::Leave,
            _ => {}
        }
        EditAction::Consumed
    }

    /// A key behind a count: another digit extends it, an operator seeds its
    /// own count, and anything else runs scaled.
    fn counted_key(&mut self, row: usize, count: usize, ch: char) -> EditAction {
        if let Some(digit) = ch.to_digit(10) {
            self.pending = Pending::Count(count * 10 + digit as usize);
            return EditAction::Consumed;
        }
        match ch {
            'd' => {
                self.pending = Pending::Operator {
                    op: OpKind::Delete,
                    count,
                }
            }
            'c' => {
                self.pending = Pending::Operator {
                    op: OpKind::Change,
                    count,
                }
            }
            'y' => {
                self.pending = Pending::Operator {
                    op: OpKind::Yank,
                    count,
                }
            }
            'f' | 'F' | 't' | 'T' => {
                self.pending = Pending::Find {
                    base: FindChar {
                        ch: '\0',
                        forward: ch == 'f' || ch == 't',
                        till: ch == 't' || ch == 'T',
                    },
                    count,
                };
            }
            'r' => self.pending = Pending::Replace(count),
            'g' => self.pending = Pending::G,
            'G' => return EditAction::MoveToRow(count.saturating_sub(1)),
            'x' => {
                self.snapshot(row);
                self.take_forward(row, count);
            }
            '~' => {
                self.snapshot(row);
                self.toggle_case(row, count);
            }
            'j' => return EditAction::MoveRows(count.min(isize::MAX as usize) as isize),
            'k' => return EditAction::MoveRows(-(count.min(isize::MAX as usize) as isize)),
            _ => return self.motion_char(row, ch, count),
        }
        EditAction::Consumed
    }

    /// The key an operator was waiting for: its doubling works the whole
    /// name, a digit extends the motion's count, a char search becomes the
    /// motion, and anything else resolves as one.
    fn operator_key(
        &mut self,
        row: usize,
        op: OpKind,
        count: usize,
        more: usize,
        ch: char,
    ) -> EditAction {
        let doubled = match op {
            OpKind::Delete => ch == 'd',
            OpKind::Change => ch == 'c',
            OpKind::Yank => ch == 'y',
        };
        if doubled {
            self.operate_whole_name(row, op);
            return EditAction::Consumed;
        }
        if ch.is_ascii_digit() && (ch != '0' || more > 0) {
            self.pending = Pending::OperatorCount {
                op,
                count,
                more: more * 10 + ch.to_digit(10).unwrap_or(0) as usize,
            };
            return EditAction::Consumed;
        }
        if matches!(ch, 'f' | 'F' | 't' | 'T') {
            self.pending = Pending::OperatorFind {
                op,
                count: count * more.max(1),
                base: FindChar {
                    ch: '\0',
                    forward: ch == 'f' || ch == 't',
                    till: ch == 't' || ch == 'T',
                },
            };
            return EditAction::Consumed;
        }
        if matches!(ch, ';' | ',') {
            if let Some(mut find) = self.last_find {
                if ch == ',' {
                    find = find.reversed();
                }
                return self.operate(row, op, count * more.max(1), Motion::Find(find));
            }
            return EditAction::Consumed;
        }
        self.operate(row, op, count * more.max(1), motion_of(ch))
    }

    /// Insert mode: an Emacs-style field. Every printable character lands in
    /// the name; the readline chords move, delete, kill, yank, and case words;
    /// `Enter` accepts the whole edit and `Escape` returns to Normal.
    fn on_insert_key(&mut self, row: usize, key: &Key) -> EditAction {
        if key.alt {
            return match key.code {
                KeyCode::Char('b') => {
                    self.word_step(row, false);
                    EditAction::Consumed
                }
                KeyCode::Char('f') => {
                    self.word_step(row, true);
                    EditAction::Consumed
                }
                KeyCode::Char('d') => {
                    self.snapshot_typed(row);
                    self.kill_word(row, true);
                    EditAction::Consumed
                }
                // The case commands, over the word after the point.
                KeyCode::Char('u') => {
                    self.snapshot_typed(row);
                    self.case_word(row, Case::Upper);
                    EditAction::Consumed
                }
                KeyCode::Char('l') => {
                    self.snapshot_typed(row);
                    self.case_word(row, Case::Lower);
                    EditAction::Consumed
                }
                KeyCode::Char('c') => {
                    self.snapshot_typed(row);
                    self.case_word(row, Case::Capital);
                    EditAction::Consumed
                }
                KeyCode::Backspace => {
                    self.snapshot_typed(row);
                    self.kill_word(row, false);
                    EditAction::Consumed
                }
                _ => EditAction::Ignored,
            };
        }
        if key.ctrl {
            return match key.code {
                KeyCode::Char('a') => {
                    self.col = 0;
                    EditAction::Consumed
                }
                KeyCode::Char('e') => {
                    self.col = self.len(row);
                    EditAction::Consumed
                }
                KeyCode::Char('b') => {
                    self.col = self.col.saturating_sub(1);
                    EditAction::Consumed
                }
                KeyCode::Char('f') => {
                    self.col = (self.col + 1).min(self.len(row));
                    EditAction::Consumed
                }
                // Previous and next line, the way the arrow keys spell them.
                KeyCode::Char('p') => EditAction::MoveRows(-1),
                KeyCode::Char('n') => EditAction::MoveRows(1),
                KeyCode::Char('d') => {
                    self.snapshot_typed(row);
                    self.delete_at(row);
                    EditAction::Consumed
                }
                KeyCode::Char('h') => {
                    self.snapshot_typed(row);
                    self.backspace(row);
                    EditAction::Consumed
                }
                KeyCode::Char('k') => {
                    self.snapshot_typed(row);
                    let end = self.len(row);
                    self.cut_range(row, self.col, end);
                    EditAction::Consumed
                }
                KeyCode::Char('u') => {
                    self.snapshot_typed(row);
                    let col = self.col;
                    self.cut_range(row, 0, col);
                    EditAction::Consumed
                }
                KeyCode::Char('w') => {
                    self.snapshot_typed(row);
                    self.kill_word(row, false);
                    EditAction::Consumed
                }
                KeyCode::Char('t') => {
                    self.snapshot_typed(row);
                    self.transpose(row);
                    EditAction::Consumed
                }
                KeyCode::Char('y') => {
                    self.snapshot_typed(row);
                    self.insert_str(row, self.register.clone());
                    EditAction::Consumed
                }
                KeyCode::Char('m') | KeyCode::Char('j') => EditAction::Apply,
                KeyCode::Char('_') => self.restore(row, Direction::Undo),
                _ => EditAction::Ignored,
            };
        }
        match key.code {
            KeyCode::Enter => EditAction::Apply,
            KeyCode::Escape => {
                self.mode = EditMode::Normal;
                EditAction::Consumed
            }
            KeyCode::Backspace => {
                self.snapshot_typed(row);
                self.backspace(row);
                EditAction::Consumed
            }
            KeyCode::Delete => {
                self.snapshot_typed(row);
                self.delete_at(row);
                EditAction::Consumed
            }
            KeyCode::Left => {
                self.col = self.col.saturating_sub(1);
                EditAction::Consumed
            }
            KeyCode::Right => {
                self.col = (self.col + 1).min(self.len(row));
                EditAction::Consumed
            }
            KeyCode::Home => {
                self.col = 0;
                EditAction::Consumed
            }
            KeyCode::End => {
                self.col = self.len(row);
                EditAction::Consumed
            }
            KeyCode::Up => EditAction::MoveRows(-1),
            KeyCode::Down => EditAction::MoveRows(1),
            KeyCode::Space => {
                self.snapshot_typed(row);
                self.insert_char(row, ' ');
                EditAction::Consumed
            }
            KeyCode::Tab => {
                self.snapshot_typed(row);
                self.insert_char(row, '\t');
                EditAction::Consumed
            }
            KeyCode::Char(c) => {
                self.snapshot_typed(row);
                self.insert_char(row, c);
                EditAction::Consumed
            }
            _ => EditAction::Ignored,
        }
    }
}

// ========================================================================
// EditState: motions and operators
// ========================================================================

impl EditState {
    /// Run `motion` `count` times as a movement of the caret.
    fn motion_key(&mut self, row: usize, motion: Motion, count: usize) -> EditAction {
        if let Some(landing) = self.resolve(row, motion, count) {
            self.col = landing.col;
        }
        EditAction::Consumed
    }

    /// Map a bare character to its motion, then move `count` of them.
    fn motion_char(&mut self, row: usize, ch: char, count: usize) -> EditAction {
        self.motion_key(row, motion_of(ch), count)
    }

    /// Apply `op` over the range `motion` lands on, `count` times over.
    fn operate(&mut self, row: usize, op: OpKind, count: usize, motion: Motion) -> EditAction {
        // Vim's `cw` quirk: started on a word it changes to that word's end
        // like `ce` — keeping the space after the word — instead of eating
        // into the next one the way `dw` does.
        let motion = if matches!(op, OpKind::Change)
            && matches!(motion, Motion::WordFwd { big: false })
            && self
                .char_at(row, self.col)
                .is_some_and(|c| !c.is_whitespace())
        {
            Motion::WordEnd { big: false }
        } else {
            motion
        };
        let Some(landing) = self.resolve(row, motion, count) else {
            return EditAction::Consumed;
        };
        if matches!(op, OpKind::Delete | OpKind::Change) {
            self.snapshot(row);
        }
        let len = self.len(row);
        let (start, end) = if landing.forward {
            (
                self.col,
                (landing.col + usize::from(landing.inclusive)).min(len),
            )
        } else {
            (landing.col, self.col)
        };
        match op {
            OpKind::Delete => self.cut_range(row, start, end),
            OpKind::Change => {
                self.cut_range(row, start, end);
                self.enter_insert();
            }
            OpKind::Yank => self.yank_range(row, start, end),
        }
        EditAction::Consumed
    }

    /// The doubled operators: `dd` empties the name into the register, `cc`
    /// empties it and enters Insert, `yy` copies it.
    fn operate_whole_name(&mut self, row: usize, op: OpKind) {
        match op {
            OpKind::Delete => {
                self.snapshot(row);
                let name = self.name(row).unwrap_or_default().to_string();
                self.register = name;
                self.set_name(row, "");
                self.col = 0;
            }
            OpKind::Change => {
                self.snapshot(row);
                let name = self.name(row).unwrap_or_default().to_string();
                self.register = name;
                self.set_name(row, "");
                self.col = 0;
                self.enter_insert();
            }
            OpKind::Yank => {
                self.register = self.name(row).unwrap_or_default().to_string();
            }
        }
    }

    /// Resolve `motion` against the current name, repeating it `count` times.
    /// Each repetition starts from where the last landed, so `3w` crosses
    /// three words; a step with nowhere to go stops the run where it stands.
    fn resolve(&self, row: usize, motion: Motion, count: usize) -> Option<Landing> {
        let chars: Vec<char> = self.name(row)?.chars().collect();
        let mut col = self.col.min(chars.len());
        let mut landing = None;
        for _ in 0..count {
            let step = resolve_once(&chars, col, motion)?;
            // A step that moves nowhere and is exclusive offers an operator
            // nothing (a `w` already at the last word). An inclusive one
            // still does: `d$` with the caret on the last character takes it.
            if step.col == col && !step.inclusive {
                break;
            }
            col = step.col;
            landing = Some(step);
        }
        landing
    }

    /// The character at `col` of `row`'s name, if it has one there.
    fn char_at(&self, row: usize, col: usize) -> Option<char> {
        self.names.get(row)?.chars().nth(col)
    }

    /// The name's length in characters.
    fn len(&self, row: usize) -> usize {
        self.names.get(row).map(|n| n.chars().count()).unwrap_or(0)
    }

    /// The column of the name's first non-blank character.
    fn first_non_blank(&self, row: usize) -> usize {
        self.names
            .get(row)
            .map(|n| first_non_blank(&n.chars().collect::<Vec<_>>()))
            .unwrap_or(0)
    }

    /// Enter Insert mode, marking the typing run as not yet undoable on its
    /// own: the first typed change takes the snapshot for the whole run.
    fn enter_insert(&mut self) {
        self.mode = EditMode::Insert;
        self.insert_fresh = true;
    }

    /// Take the undo snapshot for the first change of a typing run.
    fn snapshot_typed(&mut self, row: usize) {
        if self.insert_fresh {
            self.snapshot(row);
            self.insert_fresh = false;
        }
    }

    /// Record the current state for `u`, dropping the redo branch: a new
    /// change always cuts the timeline forward.
    fn snapshot(&mut self, row: usize) {
        self.redo.clear();
        self.undo.push(Snapshot {
            names: self.names.clone(),
            row,
            col: self.col,
        });
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
    }

    /// Step backward or forward through the edit history.
    fn restore(&mut self, row: usize, direction: Direction) -> EditAction {
        let from = match direction {
            Direction::Undo => &mut self.undo,
            Direction::Redo => &mut self.redo,
        };
        let Some(snapshot) = from.pop() else {
            return EditAction::Consumed;
        };
        let into = match direction {
            Direction::Undo => &mut self.redo,
            Direction::Redo => &mut self.undo,
        };
        into.push(Snapshot {
            names: std::mem::replace(&mut self.names, snapshot.names),
            row,
            col: std::mem::replace(&mut self.col, snapshot.col),
        });
        self.mode = EditMode::Normal;
        self.pending = Pending::None;
        EditAction::MoveToRow(snapshot.row)
    }
}

// ========================================================================
// EditState: text edits
// ========================================================================

impl EditState {
    /// Replace the name of `row`.
    fn set_name(&mut self, row: usize, name: &str) {
        if let Some(current) = self.names.get_mut(row) {
            *current = name.to_string();
            self.col = self.col.min(current.chars().count());
        }
    }

    /// Type `c` at the caret. A path separator is refused on the spot: a name
    /// is a name, and letting one in would only fail later at the rename.
    fn insert_char(&mut self, row: usize, c: char) {
        if c == '/' || c == '\\' {
            return;
        }
        if let Some(name) = self.names.get_mut(row) {
            self.col = self.col.min(name.chars().count());
            name.insert(char_boundary(name, self.col), c);
            self.col += 1;
        }
    }

    /// Type `text` at the caret, separators refused as in [`Self::insert_char`].
    fn insert_str(&mut self, row: usize, text: String) {
        for c in text.chars() {
            self.insert_char(row, c);
        }
    }

    /// Delete the character before the caret. [`Self::cut_range`] parks the
    /// caret at the removal's start, which is exactly where it belongs.
    fn backspace(&mut self, row: usize) {
        if self.col == 0 || self.names.get(row).is_none() {
            return;
        }
        self.delete_range(row, self.col - 1, self.col);
    }

    /// Delete the character at the caret.
    fn delete_at(&mut self, row: usize) {
        self.delete_range(row, self.col, self.col + 1);
    }

    /// `x`: remove `count` characters at the caret into the register.
    fn take_forward(&mut self, row: usize, count: usize) {
        let end = (self.col + count).min(self.len(row));
        self.cut_range(row, self.col, end);
    }

    /// `X`: remove `count` characters before the caret into the register.
    fn take_back(&mut self, row: usize, count: usize) {
        let start = self.col.saturating_sub(count);
        self.cut_range(row, start, self.col);
        self.col = start;
    }

    /// Remove `[start, end)` into the register, leaving the caret at `start`.
    fn cut_range(&mut self, row: usize, start: usize, end: usize) {
        let Some(name) = self.names.get_mut(row) else {
            return;
        };
        let chars: Vec<char> = name.chars().collect();
        let (start, end) = (start.min(chars.len()), end.min(chars.len()));
        if start >= end {
            return;
        }
        self.register = chars[start..end].iter().collect();
        *name = chars[..start].iter().chain(chars[end..].iter()).collect();
        self.col = start;
    }

    /// Copy `[start, end)` into the register without changing the name.
    fn yank_range(&mut self, row: usize, start: usize, end: usize) {
        let Some(name) = self.names.get(row) else {
            return;
        };
        let chars: Vec<char> = name.chars().collect();
        let (start, end) = (start.min(chars.len()), end.min(chars.len()));
        if start < end {
            self.register = chars[start..end].iter().collect();
        }
    }

    /// Delete the characters from `from` to `to`, both counted in characters,
    /// when the range names anything.
    fn delete_range(&mut self, row: usize, from: usize, to: usize) {
        let saved = self.register.clone();
        self.cut_range(row, from, to);
        self.register = saved;
    }

    /// `r`: replace `count` characters at the caret with `ch`. Refused whole,
    /// the way Vim refuses, when the name holds fewer than `count` characters
    /// from the caret: half a replacement helps nobody.
    fn replace_chars(&mut self, row: usize, count: usize, ch: char) {
        let Some(name) = self.names.get_mut(row) else {
            return;
        };
        let mut chars: Vec<char> = name.chars().collect();
        let start = self.col.min(chars.len());
        if start + count > chars.len() {
            return;
        }
        for i in 0..count {
            chars[start + i] = ch;
        }
        *name = chars.into_iter().collect();
        self.col = (start + count).saturating_sub(1);
    }

    /// `~`: flip the case of `count` characters from the caret, advancing
    /// past the last one flipped.
    fn toggle_case(&mut self, row: usize, count: usize) {
        let Some(name) = self.names.get_mut(row) else {
            return;
        };
        let mut chars: Vec<char> = name.chars().collect();
        let start = self.col.min(chars.len());
        for ch in chars.iter_mut().skip(start).take(count) {
            *ch = match ch.to_uppercase().next() {
                Some(upper) if upper != *ch => upper,
                _ => ch.to_lowercase().next().unwrap_or(*ch),
            };
        }
        self.col = (start + count).min(chars.len());
        *name = chars.into_iter().collect();
    }

    /// `p`/`P`: insert the register after or at the caret, leaving the caret
    /// on the last pasted character the way Vim does.
    fn paste(&mut self, row: usize, after: bool) {
        let len = self.len(row);
        self.col = if after {
            (self.col + 1).min(len)
        } else {
            self.col
        };
        let at = self.col;
        let text = self.register.clone();
        self.insert_str(row, text);
        let pasted = self.len(row).saturating_sub(len);
        self.col = at + pasted.saturating_sub(1);
    }

    /// `Ctrl-T`: swap the characters around the caret, or the last two when
    /// the caret sits at the name's end, advancing the way readline does.
    /// At the start there is no character before the caret to drag, so — as
    /// in Emacs — nothing happens.
    fn transpose(&mut self, row: usize) {
        let Some(name) = self.names.get_mut(row) else {
            return;
        };
        let mut chars: Vec<char> = name.chars().collect();
        if chars.len() < 2 || self.col == 0 {
            return;
        }
        let (a, b, next) = if self.col >= chars.len() {
            (chars.len() - 2, chars.len() - 1, self.col)
        } else {
            (self.col - 1, self.col, self.col + 1)
        };
        chars.swap(a, b);
        self.col = next.min(chars.len());
        *name = chars.into_iter().collect();
    }

    /// `M-u`/`M-l`/`M-c`: re-case the word after the caret, leaving the caret
    /// past it the way the Emacs case commands move point. A caret sitting
    /// on blanks first steps forward to the word, as they do; a caret at the
    /// end of the name takes the word it sits at the end of, which is where
    /// an append leaves it and what the command is wanted for.
    fn case_word(&mut self, row: usize, case: Case) {
        let Some(name) = self.names.get_mut(row) else {
            return;
        };
        let mut chars: Vec<char> = name.chars().collect();
        let end = chars.len();
        let start = {
            let mut i = self.col.min(end);
            while i < end && chars[i].is_whitespace() {
                i += 1;
            }
            if i < end {
                i
            } else {
                // Nothing ahead: the word ending at the caret, if any.
                let mut j = self.col.min(end);
                while j > 0 && chars[j - 1].is_whitespace() {
                    j -= 1;
                }
                while j > 0 && !chars[j - 1].is_whitespace() {
                    j -= 1;
                }
                j
            }
        };
        let word_end = {
            let mut i = start;
            while i < end && !chars[i].is_whitespace() {
                i += 1;
            }
            i
        };
        if start == word_end {
            return;
        }
        for (i, ch) in chars.iter_mut().enumerate().take(word_end).skip(start) {
            *ch = match case {
                Case::Upper => ch.to_uppercase().next().unwrap_or(*ch),
                Case::Lower => ch.to_lowercase().next().unwrap_or(*ch),
                Case::Capital if i == start => ch.to_uppercase().next().unwrap_or(*ch),
                Case::Capital => ch.to_lowercase().next().unwrap_or(*ch),
            };
        }
        self.col = word_end;
        *name = chars.into_iter().collect();
    }

    /// Move the caret one word, the way `Alt-B`/`Alt-F` do.
    fn word_step(&mut self, row: usize, forward: bool) {
        let Some(name) = self.names.get(row) else {
            return;
        };
        let chars: Vec<char> = name.chars().collect();
        let col = if forward {
            next_word_start(&chars, self.col, true).unwrap_or(chars.len())
        } else {
            prev_word_start(&chars, self.col, true).unwrap_or(0)
        };
        self.col = col;
    }

    /// Kill the word before or after the caret into the register.
    fn kill_word(&mut self, row: usize, forward: bool) {
        let len = self.len(row);
        if forward {
            let end = next_word_start(&self.chars_of(row), self.col, true).unwrap_or(len);
            self.cut_range(row, self.col, end);
        } else {
            let start = prev_word_start(&self.chars_of(row), self.col, true).unwrap_or(0);
            self.cut_range(row, start, self.col);
        }
    }

    /// The name of `row` as characters.
    fn chars_of(&self, row: usize) -> Vec<char> {
        self.names
            .get(row)
            .map(|n| n.chars().collect())
            .unwrap_or_default()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Map a bare motion character to its motion.
fn motion_of(ch: char) -> Motion {
    match ch {
        'h' => Motion::Left,
        'l' => Motion::Right,
        '0' => Motion::Start,
        '^' => Motion::FirstNonBlank,
        '$' => Motion::End,
        'w' => Motion::WordFwd { big: false },
        'W' => Motion::WordFwd { big: true },
        'b' => Motion::WordBack { big: false },
        'B' => Motion::WordBack { big: true },
        'e' => Motion::WordEnd { big: false },
        'E' => Motion::WordEnd { big: true },
        _ => Motion::Right,
    }
}

/// Resolve one step of `motion` from `col` over `chars`.
fn resolve_once(chars: &[char], col: usize, motion: Motion) -> Option<Landing> {
    let col = col.min(chars.len());
    let landing = match motion {
        Motion::Left => Landing {
            col: col.saturating_sub(1),
            forward: false,
            inclusive: false,
        },
        Motion::Right => Landing {
            col: (col + 1).min(chars.len()),
            forward: true,
            inclusive: false,
        },
        Motion::Start => Landing {
            col: 0,
            forward: false,
            inclusive: false,
        },
        Motion::FirstNonBlank => Landing {
            col: first_non_blank(chars),
            forward: false,
            inclusive: false,
        },
        Motion::End => Landing {
            col: chars.len().saturating_sub(1),
            forward: true,
            inclusive: true,
        },
        Motion::WordFwd { big } => {
            // On the line's last word there is no next start to stop at, and
            // Vim's `dw` deletes to the end of the line rather than nothing.
            let start = next_word_start(chars, col, big).or_else(|| {
                chars
                    .get(col)
                    .is_some_and(|c| !c.is_whitespace())
                    .then_some(chars.len())
            })?;
            Landing {
                col: start,
                forward: true,
                inclusive: false,
            }
        }
        Motion::WordBack { big } => Landing {
            col: prev_word_start(chars, col, big)?,
            forward: false,
            inclusive: false,
        },
        Motion::WordEnd { big } => {
            // On a word's last character with no further word after it, `e`
            // has nowhere to go — but `de` still takes that character.
            let end = word_end(chars, col, big).or_else(|| {
                chars
                    .get(col)
                    .is_some_and(|c| !c.is_whitespace())
                    .then_some(col)
            })?;
            Landing {
                col: end,
                forward: true,
                inclusive: true,
            }
        }
        Motion::Find(find) => {
            let index = if find.forward {
                (col + 1..chars.len()).find(|&i| chars[i] == find.ch)?
            } else {
                (0..col).rev().find(|&i| chars[i] == find.ch)?
            };
            // A char search always hands the operator the target's column as
            // its edge (`df-` through the `-`, `dt-` up to it), which is why
            // `t` lands one short and `T` one past, forward and back.
            if find.forward {
                // `t` with its target on the very next character has nothing
                // between to move to or delete, so the motion fails outright.
                if find.till && index <= col + 1 {
                    return None;
                }
                Landing {
                    col: if find.till { index - 1 } else { index },
                    forward: true,
                    inclusive: true,
                }
            } else {
                Landing {
                    col: if find.till { index + 1 } else { index },
                    forward: false,
                    inclusive: false,
                }
            }
        }
    };
    Some(landing)
}

/// The byte offset of character `at` in `name`, or the name's end.
fn char_boundary(name: &str, at: usize) -> usize {
    name.char_indices()
        .nth(at)
        .map(|(i, _)| i)
        .unwrap_or(name.len())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::dir::entry::{Entry, EntryKind, Meta};
    use std::path::PathBuf;

    fn row(name: &str) -> Row {
        Row::new(
            0,
            Entry::new(
                EntryKind::File,
                Meta::default(),
                name.to_string(),
                PathBuf::from(name),
            ),
            false,
        )
    }

    fn state(names: &[&str], cursor: usize) -> EditState {
        let rows: Vec<Row> = names.iter().map(|n| row(n)).collect();
        EditState::new(&rows, cursor)
    }

    fn key(code: KeyCode) -> Key {
        Key {
            alt: false,
            code,
            ctrl: false,
            shift: false,
        }
    }

    fn char(c: char) -> Key {
        key(KeyCode::Char(c))
    }

    fn ctrl(c: char) -> Key {
        Key {
            code: KeyCode::Char(c),
            ctrl: true,
            ..key(KeyCode::Space)
        }
    }

    fn alt(c: char) -> Key {
        Key {
            alt: true,
            code: KeyCode::Char(c),
            ctrl: false,
            shift: false,
        }
    }

    /// Press a sequence of characters into `edit` at `row`.
    fn press(edit: &mut EditState, row: usize, keys: &str) {
        for c in keys.chars() {
            edit.on_key(row, &char(c));
        }
    }

    #[test]
    fn test_the_editor_opens_in_normal_mode_at_the_append_position() {
        let edit = state(&["alpha.txt", "b.txt"], 0);
        assert_eq!(edit.mode(), EditMode::Normal);
        assert_eq!(edit.col(), 9);
        assert_eq!(edit.name(0), Some("alpha.txt"));
    }

    #[test]
    fn test_the_insert_entry_points_place_the_caret() {
        // `i` at the caret, `a` after its character, `I` at the first
        // non-blank, `A` at the end.
        let mut edit = state(&["  alpha.txt"], 0);
        press(&mut edit, 0, "$i");
        assert_eq!((edit.mode(), edit.col()), (EditMode::Insert, 10));
        edit.on_key(0, &key(KeyCode::Escape));

        press(&mut edit, 0, "a");
        assert_eq!((edit.mode(), edit.col()), (EditMode::Insert, 11));
        edit.on_key(0, &key(KeyCode::Escape));

        press(&mut edit, 0, "I");
        assert_eq!((edit.mode(), edit.col()), (EditMode::Insert, 2));
        edit.on_key(0, &key(KeyCode::Escape));

        press(&mut edit, 0, "A");
        assert_eq!((edit.mode(), edit.col()), (EditMode::Insert, 11));
    }

    #[test]
    fn test_the_line_motions_stop_where_vim_does() {
        let mut edit = state(&["notes.txt"], 0);
        press(&mut edit, 0, "$");
        assert_eq!(edit.col(), 8, "on the last character");
        press(&mut edit, 0, "0");
        assert_eq!(edit.col(), 0);
        press(&mut edit, 0, "3l");
        assert_eq!(edit.col(), 3);
        press(&mut edit, 0, "9h");
        assert_eq!(edit.col(), 0, "and never past the start");
        press(&mut edit, 0, "99l");
        assert_eq!(edit.col(), 9, "or past the append position");
    }

    #[test]
    fn test_the_word_motions_cross_words_and_counts() {
        let mut edit = state(&["notes.txt"], 0);
        press(&mut edit, 0, "0w");
        assert_eq!(edit.col(), 6, "over the `.`, onto `txt`");
        press(&mut edit, 0, "b");
        assert_eq!(edit.col(), 0, "and back over it, to the start of `notes`");
        press(&mut edit, 0, "e");
        assert_eq!(edit.col(), 4, "the end of `notes`");
        press(&mut edit, 0, "e");
        assert_eq!(edit.col(), 8, "then the end of `txt`");
        press(&mut edit, 0, "0W");
        assert_eq!(
            edit.col(),
            9,
            "the big jump has the whole name as one WORD, with none after it"
        );
    }

    #[test]
    fn test_char_searches_and_their_repeats() {
        let mut edit = state(&["notes.txt"], 0);
        press(&mut edit, 0, "0ft");
        assert_eq!(edit.col(), 2, "the first `t` is in `notes`");
        press(&mut edit, 0, ";");
        assert_eq!(edit.col(), 6, "the next one, in `txt`");
        press(&mut edit, 0, ";");
        assert_eq!(edit.col(), 8, "and the last");
        press(&mut edit, 0, ",");
        assert_eq!(edit.col(), 6, "and back");
        press(&mut edit, 0, "Tt");
        assert_eq!(edit.col(), 3, "one short of the `t` in `notes`");
        press(&mut edit, 0, "$Fo");
        assert_eq!(edit.col(), 1, "back onto the `o`");
        press(&mut edit, 0, "$2Ft");
        assert_eq!(edit.col(), 2, "a counted search takes the second one");
        press(&mut edit, 0, "2fn");
        assert_eq!(edit.col(), 2, "no occurrence ahead, so it stays put");
    }

    #[test]
    fn test_x_and_x_delete_with_counts() {
        let mut edit = state(&["abcdef"], 0);
        press(&mut edit, 0, "03x");
        assert_eq!(edit.name(0), Some("def"));
        press(&mut edit, 0, "2X");
        // Col 0 after the 3x: nothing before it, so X does nothing.
        assert_eq!(edit.name(0), Some("def"));
        press(&mut edit, 0, "$X");
        assert_eq!(edit.name(0), Some("df"), "the character before the end");
    }

    #[test]
    fn test_r_replaces_whole_or_not_at_all() {
        let mut edit = state(&["notes"], 0);
        press(&mut edit, 0, "0rZ");
        assert_eq!(edit.name(0), Some("Zotes"));
        press(&mut edit, 0, "$3r-");
        // Only two characters sit from the caret: Vim refuses the whole
        // replacement rather than doing half of it.
        assert_eq!(edit.name(0), Some("Zotes"));
        press(&mut edit, 0, "h2r-");
        assert_eq!(edit.name(0), Some("Zot--"));
        assert_eq!(edit.col(), 4, "on the last replaced character");
    }

    #[test]
    fn test_tilde_flips_case_and_advances() {
        let mut edit = state(&["aBc"], 0);
        press(&mut edit, 0, "0~");
        assert_eq!(edit.name(0), Some("ABc"));
        assert_eq!(edit.col(), 1);
        press(&mut edit, 0, "~");
        assert_eq!(edit.name(0), Some("Abc"));
        press(&mut edit, 0, "5~");
        assert_eq!(edit.name(0), Some("AbC"));
        assert_eq!(edit.col(), 3, "stopped at the end");
    }

    #[test]
    fn test_operators_over_motions() {
        let mut edit = state(&["notes.txt"], 0);
        press(&mut edit, 0, "0dw");
        assert_eq!(edit.name(0), Some("txt"), "`dw` stops at the next start");
        press(&mut edit, 0, "u0db");
        // `db` from the very start has no word behind it to take.
        assert_eq!(edit.name(0), Some("notes.txt"));
        assert_eq!(edit.col(), 0);

        press(&mut edit, 0, "0de");
        assert_eq!(edit.name(0), Some(".txt"), "`de` takes the word whole");
        press(&mut edit, 0, "u0d$");
        assert_eq!(edit.name(0), Some(""), "`d$` empties to the end");
        press(&mut edit, 0, "u0df.");
        assert_eq!(edit.name(0), Some("txt"), "`df` goes through the target");
        press(&mut edit, 0, "udt.");
        assert_eq!(edit.name(0), Some(".txt"), "`dt` stops before it");
        press(&mut edit, 0, "uD");
        assert_eq!(edit.name(0), Some(""), "`D` takes the rest of the name");
    }

    #[test]
    fn test_dw_on_the_last_word_deletes_to_the_end() {
        // Vim's `dw` on the line's last word has no next start to stop at, so
        // it deletes to the end of the line instead of leaving it stranded.
        let mut edit = state(&["notes.txt"], 0);
        press(&mut edit, 0, "0wdw");
        assert_eq!(edit.name(0), Some("notes."));
    }

    #[test]
    fn test_cw_keeps_the_space_after_the_word() {
        // Vim's `cw` quirk: on a word it changes to the word's end like `ce`,
        // so the following separator survives for the replacement.
        let mut edit = state(&["old name"], 0);
        press(&mut edit, 0, "0cw");
        assert_eq!(edit.mode(), EditMode::Insert);
        assert_eq!(edit.name(0), Some(" name"));
        press(&mut edit, 0, "new");
        assert_eq!(edit.name(0), Some("new name"));
    }

    #[test]
    fn test_the_doubled_operators_work_the_whole_name() {
        let mut edit = state(&["notes.txt"], 0);
        press(&mut edit, 0, "yy");
        assert_eq!(edit.register(), "notes.txt");
        press(&mut edit, 0, "dd");
        assert_eq!(edit.name(0), Some(""));
        press(&mut edit, 0, "P");
        assert_eq!(edit.name(0), Some("notes.txt"));
        press(&mut edit, 0, "cc");
        assert_eq!(edit.mode(), EditMode::Insert);
        assert_eq!(edit.name(0), Some(""));
    }

    #[test]
    fn test_paste_after_and_before() {
        let mut edit = state(&["ab"], 0);
        press(&mut edit, 0, "0x");
        assert_eq!(edit.register(), "a");
        press(&mut edit, 0, "p");
        assert_eq!(edit.name(0), Some("ba"), "after the character");
        press(&mut edit, 0, "$P");
        assert_eq!(edit.name(0), Some("baa"), "before the character");
    }

    #[test]
    fn test_counts_compose_with_operators() {
        let mut edit = state(&["one two three"], 0);
        press(&mut edit, 0, "02dw");
        assert_eq!(edit.name(0), Some("three"));
        press(&mut edit, 0, "u0d2w");
        // The same two words, the count arriving after the operator.
        assert_eq!(edit.name(0), Some("three"));
    }

    #[test]
    fn test_undo_groups_an_insert_run_and_redo_replays_it() {
        let mut edit = state(&["x"], 0);
        press(&mut edit, 0, "$a");
        press(&mut edit, 0, "yz");
        assert_eq!(edit.name(0), Some("xyz"));
        edit.on_key(0, &key(KeyCode::Escape));
        press(&mut edit, 0, "u");
        assert_eq!(edit.name(0), Some("x"), "the whole typing run, one step");
        edit.on_key(0, &ctrl('r'));
        assert_eq!(edit.name(0), Some("xyz"));
    }

    #[test]
    fn test_the_session_keys_ask_the_listing_to_move_or_finish() {
        let mut edit = state(&["a", "b", "c"], 0);
        assert_eq!(edit.on_key(0, &char('j')), EditAction::MoveRows(1));
        assert_eq!(edit.on_key(0, &char('k')), EditAction::MoveRows(-1));
        press(&mut edit, 0, "3");
        assert_eq!(edit.on_key(0, &char('j')), EditAction::MoveRows(3));
        assert_eq!(
            edit.on_key(0, &char('G')),
            EditAction::MoveToRow(usize::MAX)
        );
        press(&mut edit, 0, "2");
        assert_eq!(edit.on_key(0, &char('G')), EditAction::MoveToRow(1));
        press(&mut edit, 0, "g");
        assert_eq!(edit.on_key(0, &char('g')), EditAction::MoveToRow(0));
        press(&mut edit, 0, "Z");
        assert_eq!(edit.on_key(0, &char('Z')), EditAction::Apply);
        press(&mut edit, 0, "Z");
        assert_eq!(edit.on_key(0, &char('Q')), EditAction::Leave);
        assert_eq!(edit.on_key(0, &char('q')), EditAction::Leave);
    }

    #[test]
    fn test_escape_cancels_a_pending_operator_instead_of_leaving() {
        let mut edit = state(&["notes"], 0);
        press(&mut edit, 0, "d");
        edit.on_key(0, &key(KeyCode::Escape));
        press(&mut edit, 0, "w");
        assert_eq!(edit.name(0), Some("notes"), "the `d` was abandoned");
    }

    #[test]
    fn test_special_keys_move_without_a_pending_command() {
        let mut edit = state(&["notes"], 0);
        press(&mut edit, 0, "0");
        edit.on_key(0, &key(KeyCode::Right));
        assert_eq!(edit.col(), 1);
        edit.on_key(0, &key(KeyCode::End));
        assert_eq!(edit.col(), 4);
        edit.on_key(0, &key(KeyCode::Home));
        assert_eq!(edit.col(), 0);
        assert_eq!(edit.on_key(0, &key(KeyCode::Up)), EditAction::MoveRows(-1));
        assert_eq!(edit.on_key(0, &key(KeyCode::Down)), EditAction::MoveRows(1));
        assert_eq!(
            edit.on_key(0, &key(KeyCode::Enter)),
            EditAction::MoveRows(1)
        );
    }

    #[test]
    fn test_insert_mode_types_and_refuses_separators() {
        let mut edit = state(&["plain"], 0);
        press(&mut edit, 0, "A");
        press(&mut edit, 0, "2j/q\\");
        // j and q are letters in a name; the separators are refused.
        assert_eq!(edit.name(0), Some("plain2jq"));
    }

    #[test]
    fn test_insert_mode_carries_the_readline_chords() {
        let mut edit = state(&["hello world"], 0);
        press(&mut edit, 0, "A");
        edit.on_key(0, &ctrl('a'));
        assert_eq!(edit.col(), 0);
        edit.on_key(0, &ctrl('e'));
        assert_eq!(edit.col(), 11);
        edit.on_key(0, &ctrl('b'));
        edit.on_key(0, &ctrl('b'));
        assert_eq!(edit.col(), 9);
        edit.on_key(0, &ctrl('f'));
        assert_eq!(edit.col(), 10);
        edit.on_key(0, &alt('b'));
        assert_eq!(edit.col(), 6, "back to the start of `world`");
        edit.on_key(0, &alt('f'));
        assert_eq!(edit.col(), 11);
        edit.on_key(0, &ctrl('u'));
        assert_eq!(
            edit.name(0),
            Some(""),
            "killed everything back to the start"
        );
        edit.on_key(0, &ctrl('y'));
        assert_eq!(edit.name(0), Some("hello world"), "the kill came back");
        edit.on_key(0, &ctrl('b'));
        edit.on_key(0, &ctrl('h'));
        assert_eq!(edit.name(0), Some("hello word"), "backspace takes the `l`");
        edit.on_key(0, &ctrl('d'));
        assert_eq!(edit.name(0), Some("hello wor"), "delete takes the `d`");
    }

    #[test]
    fn test_insert_mode_kills_words_both_ways() {
        let mut edit = state(&["one two three"], 0);
        press(&mut edit, 0, "A");
        edit.on_key(0, &alt('b'));
        edit.on_key(0, &ctrl('w'));
        // The point sat at the start of `three`; the word before it goes.
        assert_eq!(edit.name(0), Some("one three"));
        edit.on_key(0, &alt('d'));
        assert_eq!(edit.name(0), Some("one "), "and the word after");
        edit.on_key(0, &alt('d'));
        assert_eq!(edit.name(0), Some("one "), "nothing after to kill");
    }

    #[test]
    fn test_ctrl_t_transposes_around_the_point() {
        let mut edit = state(&["abc"], 0);
        press(&mut edit, 0, "0li");
        edit.on_key(0, &ctrl('t'));
        assert_eq!(edit.name(0), Some("bac"), "the characters around the point");
        assert_eq!(edit.col(), 2);
        edit.on_key(0, &key(KeyCode::Escape));
        press(&mut edit, 0, "uA");
        edit.on_key(0, &ctrl('t'));
        assert_eq!(edit.name(0), Some("acb"), "at the end, the last two");
    }

    #[test]
    fn test_insert_mode_enter_applies_and_escape_returns_to_normal() {
        let mut edit = state(&["notes.txt"], 0);
        press(&mut edit, 0, "A");
        assert_eq!(
            edit.on_key(0, &key(KeyCode::Enter)),
            EditAction::Apply,
            "Enter accepts the whole edit"
        );
        assert_eq!(
            edit.on_key(0, &ctrl('m')),
            EditAction::Apply,
            "as does the readline accept-line chord"
        );
        assert_eq!(
            edit.on_key(0, &key(KeyCode::Escape)),
            EditAction::Consumed,
            "Escape only returns to Normal"
        );
        assert_eq!(edit.mode(), EditMode::Normal);
    }

    #[test]
    fn test_ctrl_n_and_ctrl_p_move_between_names() {
        // The Emacs spellings of the arrow keys, previous and next line.
        let mut edit = state(&["a", "b"], 0);
        press(&mut edit, 0, "i");
        assert_eq!(edit.on_key(0, &ctrl('n')), EditAction::MoveRows(1));
        assert_eq!(edit.on_key(0, &ctrl('p')), EditAction::MoveRows(-1));
    }

    #[test]
    fn test_the_case_commands_recase_the_word_after_the_point() {
        let mut edit = state(&["read me now"], 0);
        press(&mut edit, 0, "A");

        edit.on_key(0, &alt('u'));
        assert_eq!(
            edit.name(0),
            Some("read me NOW"),
            "M-u takes the word after"
        );
        assert_eq!(edit.col(), 11, "the point moves past it");

        edit.on_key(0, &alt('b'));
        edit.on_key(0, &alt('b'));
        edit.on_key(0, &alt('c'));
        assert_eq!(edit.name(0), Some("read Me NOW"), "M-c capitalizes");

        let mut fresh = state(&["README"], 0);
        press(&mut fresh, 0, "0i");
        fresh.on_key(0, &alt('c'));
        assert_eq!(fresh.name(0), Some("Readme"), "M-c downs the rest");
        fresh.on_key(0, &alt('u'));
        assert_eq!(fresh.name(0), Some("README"), "and M-u brings it back");
    }

    #[test]
    fn test_the_kill_word_chords_agree() {
        // `C-w` and `M-Backspace` both kill the word before the point; only
        // `M-d` kills forward. The point sits at the start of `three`.
        let mut edit = state(&["one two three"], 0);
        press(&mut edit, 0, "A");
        edit.on_key(0, &alt('b'));
        edit.on_key(
            0,
            &Key {
                alt: true,
                code: KeyCode::Backspace,
                ctrl: false,
                shift: false,
            },
        );
        assert_eq!(
            edit.name(0),
            Some("one three"),
            "M-Backspace kills the word"
        );
        edit.on_key(0, &ctrl('w'));
        assert_eq!(edit.name(0), Some("three"), "as does C-w");
    }

    #[test]
    fn test_ctrl_t_at_the_start_does_nothing() {
        // There is no character before the point to drag forward, and Emacs
        // agrees by doing nothing rather than swapping the first two.
        let mut edit = state(&["abc"], 0);
        press(&mut edit, 0, "0i");
        edit.on_key(0, &ctrl('t'));
        assert_eq!(edit.name(0), Some("abc"));
        assert_eq!(edit.col(), 0);
    }

    #[test]
    fn test_ctrl_underscore_undoes_from_insert_mode() {
        // The readline undo chord works where the typing happens.
        let mut edit = state(&["base"], 0);
        press(&mut edit, 0, "A");
        press(&mut edit, 0, "!");
        assert_eq!(edit.name(0), Some("base!"));
        edit.on_key(0, &ctrl('_'));
        assert_eq!(edit.name(0), Some("base"), "the typing run undid");
        assert_eq!(edit.mode(), EditMode::Normal, "undo lands in Normal");
    }

    #[test]
    fn test_insert_mode_moves_rows_with_the_arrows() {
        let mut edit = state(&["a", "b"], 0);
        press(&mut edit, 0, "i");
        assert_eq!(edit.on_key(0, &key(KeyCode::Up)), EditAction::MoveRows(-1));
        assert_eq!(edit.on_key(0, &key(KeyCode::Down)), EditAction::MoveRows(1));
        // Window chords the editor does not bind fall through.
        assert_eq!(edit.on_key(0, &alt('h')), EditAction::Ignored);
    }

    #[test]
    fn test_clamping_between_rows_keeps_the_column_vim_keeps() {
        let mut edit = state(&["longer-name.txt", "b"], 0);
        assert_eq!(edit.col(), 15);
        edit.move_to_row(1);
        assert_eq!(edit.col(), 1, "the shorter name clamps it");
    }

    #[test]
    fn test_multibyte_names_edit_by_character() {
        let mut edit = state(&["h\u{e9}llo.txt"], 0);
        press(&mut edit, 0, "0ex");
        // `e` lands on the whole `o`, and `x` takes it whole, never half of é.
        assert_eq!(edit.name(0), Some("h\u{e9}ll.txt"));
        press(&mut edit, 0, "u0lx");
        assert_eq!(edit.name(0), Some("hllo.txt"), "\u{e9} removed whole");
    }
}
