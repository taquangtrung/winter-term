//! A vim cursor in a document's text: where it is, what it has selected, and
//! the motions, finds, and searches that move it, each taken from the text
//! the pages were rebuilt into.
//!
//! Everything here takes an already-resolved motion and count; the grammar
//! that produces them is the viewer's.

use super::document::Document;

// ========================================================================
// Constants
// ========================================================================

/// What ends a sentence, for `(` and `)`, when a space or the line's end
/// follows it.
const SENTENCE_ENDS: [char; 3] = ['.', '!', '?'];

/// Said when there is no text to put a cursor in or to search.
pub const NO_TEXT: &str = "this document has no text";

// ========================================================================
// Data Structures
// ========================================================================

/// A character of the document: which page, which of its lines, which
/// character along it, all from zero.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Pos {
    pub char: usize,
    pub line: usize,
    pub page: usize,
}

/// What a search landed on: where it starts, and how long it runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Found {
    pub at: Pos,
    pub length: usize,
}

/// The cursor, once there is one, and everything it carries between keys.
#[derive(Clone, Debug, Default)]
pub struct Caret {
    /// The other end of a selection.
    pub anchor: Option<Pos>,
    /// The column a run of `j` and `k` is holding, so a short line passed
    /// through does not narrow it for good.
    column: Option<usize>,
    /// The last `f`, `F`, `t`, or `T`, for `;` and `,`.
    find: Option<(FindKind, char)>,
    /// What a search last landed on, drawn until the cursor moves off it.
    pub found: Option<Found>,
    /// What `/` or `?` last looked for, and whether down the document.
    search: Option<(String, bool)>,
    /// Whether `pos` was only put there by the pointer, a click or a drag
    /// made with no cursor in the text: it holds a selection's end, but the
    /// cursor is not drawn and the keys stay the ones that scroll, until
    /// `i` asks for the cursor.
    pub pointed: bool,
    /// Where the cursor is, or `None` with no cursor in the text.
    pub pos: Option<Pos>,
    pub visual: Visual,
}

/// What a command did, for the viewer to follow up.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// The cursor moved: bring it into view.
    Moved,
    /// Nothing changed.
    Nothing,
    /// It could not be done, and this says why.
    Refused(String),
    /// Text was copied.
    Yanked(String),
}

/// A motion the cursor makes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Motion {
    DocEnd,
    DocStart,
    Down,
    Left,
    LineEnd,
    LineFirstNonBlank,
    LineStart,
    /// The first line of the page a count names.
    Page,
    ParagraphBackward,
    ParagraphForward,
    Right,
    SentenceBackward,
    SentenceForward,
    Up,
    ViewBottom,
    ViewMiddle,
    ViewTop,
    WordBackward,
    WordEnd,
    WordForward,
}

/// Which way `f`, `F`, `t`, and `T` look, and whether they stop on the
/// character or just short of it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FindKind {
    /// `F`: back to the character.
    Backward,
    /// `T`: back to just after it.
    BackwardTill,
    /// `f`: on to the character.
    Forward,
    /// `t`: on to just before it.
    ForwardTill,
}

/// What is being done with what a motion passes over.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Operator {
    /// No operator: the motion moves the cursor.
    #[default]
    Move,
    /// `y`: the motion says what to copy.
    Yank,
}

/// What the cursor has selected.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Visual {
    /// Up to the character the cursor is on.
    Char,
    /// Whole lines, however far into them the ends sit.
    Line,
    /// Nothing is selected.
    #[default]
    None,
}

/// What a search looks for: lower case matching either case, a capital
/// anywhere making it exact, the rule the editor searches by.
struct Needle {
    chars: Vec<char>,
    folded: bool,
}

/// What kind of character a word motion sees.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Class {
    Punct,
    Space,
    Word,
}

// ========================================================================
// Caret
// ========================================================================

impl Caret {
    // --------------------------------------------------------------------
    // Into and out of the text
    // --------------------------------------------------------------------

    /// Put the cursor at `at`, or at the start of the text when `at` is
    /// `None`.
    pub fn enter(&mut self, doc: &mut Document, at: Option<Pos>) -> Effect {
        // Where the pointer left it is where the cursor starts.
        if self.pointed {
            self.pointed = false;
            return Effect::Moved;
        }
        if self.pos.is_some() {
            return Effect::Nothing;
        }
        let Some(at) = at.or_else(|| document_start(doc)) else {
            return Effect::Refused(NO_TEXT.to_string());
        };
        self.pos = Some(at);
        self.column = None;
        Effect::Moved
    }

    /// Forget where the cursor and its selection are, for a document read
    /// again whose lines may have moved, keeping the last find and search
    /// for `;` and `n`.
    pub fn forget_places(&mut self) {
        *self = Self {
            find: self.find,
            search: self.search.take(),
            ..Self::default()
        };
    }

    /// Take the cursor out of the text, its selection with it.
    pub fn leave(&mut self) {
        self.pointed = false;
        self.pos = None;
        self.anchor = None;
        self.found = None;
        self.visual = Visual::None;
        self.column = None;
    }

    // --------------------------------------------------------------------
    // Motions
    // --------------------------------------------------------------------

    /// Run `motion` `count` times, or yank what it passes over. `screen` is
    /// where a `H`, `M`, or `L` lands, worked out from the view.
    pub fn motion(
        &mut self,
        doc: &mut Document,
        motion: Motion,
        count: usize,
        operator: Operator,
        screen: Option<Pos>,
    ) -> Effect {
        let Some(from) = self.pos else {
            return Effect::Nothing;
        };
        self.found = None;
        let column = self.column.unwrap_or(from.char);
        let mut at = from;
        for _ in 0..count {
            let next = match motion {
                Motion::DocStart => document_start(doc),
                Motion::DocEnd => document_end(doc),
                Motion::Page => page_start(doc, count.saturating_sub(1)),
                Motion::ViewTop | Motion::ViewMiddle | Motion::ViewBottom => screen,
                _ => step(doc, at, motion, column),
            };
            let Some(next) = next else {
                break;
            };
            at = next;
            // An absolute motion means the same however often it is asked.
            if matches!(
                motion,
                Motion::DocStart
                    | Motion::DocEnd
                    | Motion::Page
                    | Motion::ViewTop
                    | Motion::ViewMiddle
                    | Motion::ViewBottom
            ) {
                break;
            }
        }
        self.column = matches!(motion, Motion::Down | Motion::Up).then_some(column);
        if operator == Operator::Yank {
            let inclusive = matches!(motion, Motion::DocEnd | Motion::LineEnd | Motion::WordEnd);
            return self.yank_range(doc, from, at, false, inclusive);
        }
        self.pos = Some(at);
        Effect::Moved
    }

    /// `f`, `F`, `t`, `T`: to the `count`th `ch` on the cursor's own line.
    pub fn find(
        &mut self,
        doc: &mut Document,
        kind: FindKind,
        ch: char,
        count: usize,
        operator: Operator,
    ) -> Effect {
        let Some(from) = self.pos else {
            return Effect::Nothing;
        };
        self.found = None;
        self.find = Some((kind, ch));
        let Some(at) = find_on_line(doc, from, kind, ch, count) else {
            return Effect::Nothing;
        };
        if operator == Operator::Yank {
            let inclusive = matches!(kind, FindKind::Forward | FindKind::ForwardTill);
            return self.yank_range(doc, from, at, false, inclusive);
        }
        self.pos = Some(at);
        self.column = None;
        Effect::Moved
    }

    /// `;` and `,`: the last find again, the same way or the other.
    pub fn repeat_find(
        &mut self,
        doc: &mut Document,
        reverse: bool,
        count: usize,
        operator: Operator,
    ) -> Effect {
        let Some((kind, ch)) = self.find else {
            return Effect::Nothing;
        };
        let kind = match reverse {
            true => kind.opposite(),
            false => kind,
        };
        let typed = self.find;
        let effect = self.find(doc, kind, ch, count, operator);
        // What `;` repeats stays the find that was typed, not its reversal.
        self.find = typed;
        effect
    }

    // --------------------------------------------------------------------
    // Searching
    // --------------------------------------------------------------------

    /// `/` and `?`: look for `pattern` from `from`, `count` matches on, and
    /// remember it for `n` and `N`. `from` is where a search with no cursor
    /// sets off: the first line in view.
    pub fn search(
        &mut self,
        doc: &mut Document,
        pattern: &str,
        forward: bool,
        count: usize,
        operator: Operator,
        from: Option<Pos>,
    ) -> Effect {
        if pattern.is_empty() {
            return Effect::Nothing;
        }
        self.search = Some((pattern.to_string(), forward));
        self.run_search(doc, forward, count, operator, from)
    }

    /// `n` and `N`: the last search again, the same way or the other.
    pub fn search_again(
        &mut self,
        doc: &mut Document,
        reverse: bool,
        count: usize,
        operator: Operator,
        from: Option<Pos>,
    ) -> Effect {
        let Some((_, forward)) = self.search else {
            return Effect::Refused("nothing has been searched for".to_string());
        };
        self.run_search(doc, forward != reverse, count, operator, from)
    }

    /// Walk `count` matches from the cursor, or from `from` with none, and
    /// land on the last: a search with no cursor leaves one, as `i` would.
    fn run_search(
        &mut self,
        doc: &mut Document,
        forward: bool,
        count: usize,
        operator: Operator,
        from: Option<Pos>,
    ) -> Effect {
        let Some((pattern, _)) = self.search.clone() else {
            return Effect::Nothing;
        };
        let Some(start) = self.pos.or(from).or_else(|| document_start(doc)) else {
            return Effect::Refused(NO_TEXT.to_string());
        };
        let mut at = Found {
            at: start,
            length: 0,
        };
        for _ in 0..count {
            match next_match(doc, at.at, forward, &pattern) {
                Some(found) => at = found,
                None => {
                    return Effect::Refused(format!("`{pattern}` is not in this document"));
                }
            }
        }
        if operator == Operator::Yank {
            // Vim's own `/` under an operator: up to the match, not over it.
            return self.yank_range(doc, start, at.at, false, false);
        }
        self.found = Some(at);
        self.pos = Some(at.at);
        self.column = None;
        Effect::Moved
    }

    // --------------------------------------------------------------------
    // Selecting and copying
    // --------------------------------------------------------------------

    /// `v` and `V`, and the same key again to drop the selection.
    pub fn set_visual(&mut self, kind: Visual) {
        let Some(pos) = self.pos else {
            return;
        };
        match kind {
            Visual::None => self.anchor = None,
            Visual::Char | Visual::Line => self.anchor = self.anchor.or(Some(pos)),
        }
        self.visual = kind;
    }

    /// `o`: the cursor to the other end of the selection.
    pub fn swap_ends(&mut self) -> Effect {
        let (Some(anchor), Some(pos)) = (self.anchor, self.pos) else {
            return Effect::Nothing;
        };
        self.anchor = Some(pos);
        self.pos = Some(anchor);
        Effect::Moved
    }

    /// `y` with a selection: copy it, both ends included, and drop it.
    pub fn yank_selection(&mut self, doc: &mut Document) -> Effect {
        let (Some(anchor), Some(pos)) = (self.anchor, self.pos) else {
            return Effect::Nothing;
        };
        let linewise = self.visual == Visual::Line;
        self.yank_range(doc, anchor, pos, linewise, true)
    }

    /// What is selected, the selection left as it is: `None` with nothing
    /// selected.
    pub fn selected_text(&self, doc: &mut Document) -> Option<String> {
        let (start, end, linewise) = self.selection()?;
        Some(text_between(doc, start, end, linewise, true))
    }

    /// `yy` and `Y`: `count` whole lines from the cursor's.
    pub fn yank_lines(&mut self, doc: &mut Document, count: usize) -> Effect {
        let Some(pos) = self.pos else {
            return Effect::Nothing;
        };
        let mut end = pos;
        for _ in 1..count {
            match next_line(doc, end) {
                Some(next) => end = next,
                None => break,
            }
        }
        self.yank_range(doc, pos, end, true, true)
    }

    /// Point at `at` with the pointer: the cursor goes there, and a drag
    /// from `origin` selects up to it.
    pub fn point(&mut self, at: Pos, origin: Option<Pos>) {
        if self.pos.is_none() {
            self.pointed = true;
        }
        self.found = None;
        self.column = None;
        match origin.filter(|origin| *origin != at) {
            Some(origin) => {
                self.anchor = Some(origin);
                if self.visual == Visual::None {
                    self.visual = Visual::Char;
                }
            }
            None => {
                self.anchor = None;
                self.visual = Visual::None;
            }
        }
        self.pos = Some(at);
    }

    /// Where the cursor is drawn: its place, unless only the pointer put it
    /// there.
    pub fn shown(&self) -> Option<Pos> {
        self.pos.filter(|_| !self.pointed)
    }

    /// The selection's two ends in reading order, and whether it takes whole
    /// lines, while there is one.
    pub fn selection(&self) -> Option<(Pos, Pos, bool)> {
        let (anchor, pos) = (self.anchor?, self.pos?);
        if self.visual == Visual::None {
            return None;
        }
        let (start, end) = ordered(anchor, pos);
        Some((start, end, self.visual == Visual::Line))
    }

    fn yank_range(
        &mut self,
        doc: &mut Document,
        from: Pos,
        to: Pos,
        linewise: bool,
        inclusive: bool,
    ) -> Effect {
        let text = text_between(doc, from, to, linewise, inclusive);
        self.visual = Visual::None;
        self.anchor = None;
        Effect::Yanked(text)
    }
}

// ========================================================================
// FindKind
// ========================================================================

impl FindKind {
    /// The same find the other way round.
    fn opposite(self) -> Self {
        match self {
            Self::Backward => Self::Forward,
            Self::BackwardTill => Self::ForwardTill,
            Self::Forward => Self::Backward,
            Self::ForwardTill => Self::BackwardTill,
        }
    }
}

// ========================================================================
// Needle
// ========================================================================

impl Needle {
    /// The needle `pattern` is, or `None` for an empty one.
    fn new(pattern: &str) -> Option<Self> {
        let folded = !pattern.chars().any(char::is_uppercase);
        let chars: Vec<char> = pattern.chars().map(|ch| fold(ch, folded)).collect();
        (!chars.is_empty()).then_some(Self { chars, folded })
    }

    /// Where on `text` the needle starts, overlapping matches included.
    fn starts(&self, text: &[char]) -> Vec<usize> {
        let text: Vec<char> = text.iter().map(|ch| fold(*ch, self.folded)).collect();
        let last = text.len().checked_sub(self.chars.len());
        last.map_or_else(Vec::new, |last| {
            (0..=last)
                .filter(|start| text[*start..].starts_with(&self.chars))
                .collect()
        })
    }
}

// ========================================================================
// Pos
// ========================================================================

impl Pos {
    /// `char` of `line` on `page`.
    pub fn new(page: usize, line: usize, char: usize) -> Self {
        Self { char, line, page }
    }

    /// Where it falls in reading order.
    fn order(self) -> (usize, usize, usize) {
        (self.page, self.line, self.char)
    }
}

// ========================================================================
// Free functions: the text
// ========================================================================

/// The text of the line `pos` is on.
pub fn line_text(doc: &mut Document, pos: Pos) -> Option<Vec<char>> {
    Some(doc.text(pos.page)?.lines.get(pos.line)?.text.clone())
}

/// `a` and `b` in reading order.
pub fn ordered(a: Pos, b: Pos) -> (Pos, Pos) {
    match a.order() <= b.order() {
        true => (a, b),
        false => (b, a),
    }
}

/// The start of the line after `pos`'s, over pages with no text on them.
pub fn next_line(doc: &mut Document, pos: Pos) -> Option<Pos> {
    let lines = doc.text(pos.page)?.lines.len();
    if pos.line + 1 < lines {
        return Some(Pos::new(pos.page, pos.line + 1, 0));
    }
    (pos.page + 1..doc.page_count())
        .find(|page| doc.text(*page).is_some_and(|text| !text.lines.is_empty()))
        .map(|page| Pos::new(page, 0, 0))
}

/// The start of the line before `pos`'s, over pages with no text on them.
pub fn previous_line(doc: &mut Document, pos: Pos) -> Option<Pos> {
    if pos.line > 0 {
        return Some(Pos::new(pos.page, pos.line - 1, 0));
    }
    (0..pos.page).rev().find_map(|page| {
        let lines = doc.text(page)?.lines.len();
        lines.checked_sub(1).map(|last| Pos::new(page, last, 0))
    })
}

/// The first character with any text.
pub fn document_start(doc: &mut Document) -> Option<Pos> {
    (0..doc.page_count())
        .find(|page| doc.text(*page).is_some_and(|text| !text.lines.is_empty()))
        .map(|page| Pos::new(page, 0, 0))
}

/// The last character with any text.
fn document_end(doc: &mut Document) -> Option<Pos> {
    (0..doc.page_count()).rev().find_map(|page| {
        let text = doc.text(page)?;
        let line = text.lines.len().checked_sub(1)?;
        let last = text.lines[line].text.len().saturating_sub(1);
        Some(Pos::new(page, line, last))
    })
}

/// The first character of page `page`, or of the nearest page after it with
/// text.
fn page_start(doc: &mut Document, page: usize) -> Option<Pos> {
    let page = page.min(doc.page_count().saturating_sub(1));
    (page..doc.page_count())
        .find(|page| doc.text(*page).is_some_and(|text| !text.lines.is_empty()))
        .map(|page| Pos::new(page, 0, 0))
}

/// The document's text from `from` to `to` either way round, lines joined
/// by newlines: whole lines when `linewise`, and the later end's character
/// included when `inclusive`.
pub fn text_between(
    doc: &mut Document,
    from: Pos,
    to: Pos,
    linewise: bool,
    inclusive: bool,
) -> String {
    let (start, end) = ordered(from, to);
    let mut parts = Vec::new();
    let mut at = Pos {
        char: if linewise { 0 } else { start.char },
        ..start
    };
    while let Some(text) = line_text(doc, at) {
        let last = at.page == end.page && at.line == end.line;
        let stop = if inclusive { end.char + 1 } else { end.char };
        let until = match linewise || !last {
            true => text.len(),
            false => stop.min(text.len()),
        };
        let from = at.char.min(until);
        parts.push(text[from..until].iter().collect::<String>());
        if last {
            break;
        }
        match next_line(doc, at) {
            Some(next) => at = next,
            None => break,
        }
    }
    parts.join("\n")
}

// ========================================================================
// Free functions: motions
// ========================================================================

fn class_of(ch: Option<char>) -> Class {
    match ch {
        None => Class::Space,
        Some(ch) if ch.is_whitespace() => Class::Space,
        Some(ch) if ch.is_alphanumeric() || ch == '_' => Class::Word,
        Some(_) => Class::Punct,
    }
}

/// One step of `motion` from `pos`, holding `column` across `j` and `k`.
/// `None` where the document ends first.
fn step(doc: &mut Document, pos: Pos, motion: Motion, column: usize) -> Option<Pos> {
    let text = line_text(doc, pos)?;
    let last = text.len().saturating_sub(1);
    match motion {
        Motion::Left => pos.char.checked_sub(1).map(|char| Pos { char, ..pos }),
        Motion::Right => (pos.char < last).then_some(Pos {
            char: pos.char + 1,
            ..pos
        }),
        Motion::Down | Motion::Up => {
            let target = match motion {
                Motion::Down => next_line(doc, pos)?,
                _ => previous_line(doc, pos)?,
            };
            let width = line_text(doc, target)?.len().saturating_sub(1);
            Some(Pos {
                char: column.min(width),
                ..target
            })
        }
        Motion::LineStart => Some(Pos { char: 0, ..pos }),
        Motion::LineEnd => Some(Pos { char: last, ..pos }),
        Motion::LineFirstNonBlank => Some(Pos {
            char: text.iter().position(|ch| !ch.is_whitespace()).unwrap_or(0),
            ..pos
        }),
        Motion::WordForward => word_forward(doc, pos),
        Motion::WordBackward => word_backward(doc, pos),
        Motion::WordEnd => word_end(doc, pos),
        Motion::ParagraphForward => paragraph(doc, pos, true),
        Motion::ParagraphBackward => paragraph(doc, pos, false),
        Motion::SentenceForward => sentence(doc, pos, &text, true, column),
        Motion::SentenceBackward => sentence(doc, pos, &text, false, column),
        Motion::DocEnd
        | Motion::DocStart
        | Motion::Page
        | Motion::ViewBottom
        | Motion::ViewMiddle
        | Motion::ViewTop => None,
    }
}

/// Vim's `w`: the start of the next run of like characters, across lines.
fn word_forward(doc: &mut Document, pos: Pos) -> Option<Pos> {
    let mut at = pos;
    let mut text = line_text(doc, at)?;
    let started = class_of(text.get(at.char).copied());
    if started != Class::Space {
        while at.char < text.len() && class_of(text.get(at.char).copied()) == started {
            at.char += 1;
        }
    }
    loop {
        if at.char >= text.len() {
            at = next_line(doc, at)?;
            text = line_text(doc, at)?;
            if text.first().is_some_and(|ch| !ch.is_whitespace()) {
                return Some(at);
            }
            continue;
        }
        if class_of(text.get(at.char).copied()) != Class::Space {
            return Some(at);
        }
        at.char += 1;
    }
}

/// Vim's `b`: back to the start of this run, or of the one before it.
fn word_backward(doc: &mut Document, pos: Pos) -> Option<Pos> {
    let mut at = pos;
    let mut text = line_text(doc, at)?;
    loop {
        if at.char == 0 {
            let previous = previous_line(doc, at)?;
            text = line_text(doc, previous)?;
            at = Pos {
                char: text.len(),
                ..previous
            };
            continue;
        }
        at.char -= 1;
        let kind = class_of(text.get(at.char).copied());
        if kind == Class::Space {
            continue;
        }
        while at.char > 0 && class_of(text.get(at.char - 1).copied()) == kind {
            at.char -= 1;
        }
        return Some(at);
    }
}

/// Vim's `e`: on to the last character of this run, or of the next.
fn word_end(doc: &mut Document, pos: Pos) -> Option<Pos> {
    let mut at = pos;
    let mut text = line_text(doc, at)?;
    let mut char = at.char as isize;
    loop {
        char += 1;
        if char as usize >= text.len() {
            at = next_line(doc, at)?;
            text = line_text(doc, at)?;
            char = -1;
            continue;
        }
        let kind = class_of(text.get(char as usize).copied());
        if kind == Class::Space {
            continue;
        }
        let mut end = char as usize;
        while end + 1 < text.len() && class_of(text.get(end + 1).copied()) == kind {
            end += 1;
        }
        return Some(Pos { char: end, ..at });
    }
}

/// `{` and `}`: the next line that starts a paragraph.
fn paragraph(doc: &mut Document, pos: Pos, forward: bool) -> Option<Pos> {
    let mut at = pos;
    loop {
        at = match forward {
            true => next_line(doc, at)?,
            false => previous_line(doc, at)?,
        };
        let starts = doc.text(at.page)?.breaks.contains(&at.line);
        if at.line == 0 || starts {
            return Some(at);
        }
    }
}

/// `(` and `)`: the start of the next sentence on the line, or the next line
/// when none is left on it.
fn sentence(
    doc: &mut Document,
    pos: Pos,
    text: &[char],
    forward: bool,
    column: usize,
) -> Option<Pos> {
    let starts: Vec<usize> = (0..text.len())
        .filter(|at| {
            SENTENCE_ENDS.contains(&text[*at])
                && text.get(at + 1).is_none_or(|ch| ch.is_whitespace())
        })
        .filter_map(|at| (at + 1..text.len()).find(|next| !text[*next].is_whitespace()))
        .collect();
    let target = match forward {
        true => starts.iter().find(|start| **start > pos.char),
        false => starts.iter().rev().find(|start| **start < pos.char),
    };
    match target {
        Some(char) => Some(Pos { char: *char, ..pos }),
        None => step(
            doc,
            pos,
            if forward { Motion::Down } else { Motion::Up },
            column,
        ),
    }
}

/// `f`, `F`, `t`, `T` on the cursor's own line.
fn find_on_line(
    doc: &mut Document,
    pos: Pos,
    kind: FindKind,
    ch: char,
    count: usize,
) -> Option<Pos> {
    let text = line_text(doc, pos)?;
    let forward = matches!(kind, FindKind::Forward | FindKind::ForwardTill);
    let mut at = pos.char;
    for _ in 0..count {
        at = match forward {
            true => (at + 1..text.len()).find(|i| text[*i] == ch)?,
            false => (0..at).rev().find(|i| text[*i] == ch)?,
        };
    }
    let char = match kind {
        FindKind::ForwardTill => at.saturating_sub(1),
        FindKind::BackwardTill => (at + 1).min(text.len().saturating_sub(1)),
        FindKind::Forward | FindKind::Backward => at,
    };
    Some(Pos { char, ..pos })
}

/// The next match of `pattern` from `from`, going `forward` or back and
/// wrapping round the document once.
fn next_match(doc: &mut Document, from: Pos, forward: bool, pattern: &str) -> Option<Found> {
    let needle = Needle::new(pattern)?;
    let mut at = from;
    // The line set off from counts only past the cursor at first, and whole
    // when the wrap comes back round to it.
    let mut bounded = true;
    loop {
        if let Some(text) = line_text(doc, at) {
            let starts = needle.starts(&text);
            let hit = match (forward, bounded) {
                (true, true) => starts.into_iter().find(|start| *start > from.char),
                (true, false) => starts.first().copied(),
                (false, true) => starts.into_iter().rev().find(|start| *start < from.char),
                (false, false) => starts.last().copied(),
            };
            if let Some(char) = hit {
                return Some(Found {
                    at: Pos { char, ..at },
                    length: needle.chars.len(),
                });
            }
        }
        if !bounded && at.page == from.page && at.line == from.line {
            return None;
        }
        let next = match forward {
            true => next_line(doc, at),
            false => previous_line(doc, at),
        };
        at = match (next, forward) {
            (Some(next), _) => next,
            (None, true) => document_start(doc)?,
            (None, false) => document_end(doc)?,
        };
        bounded = false;
    }
}

/// How many matches of `pattern` the whole document holds, and which of
/// them, counting from one, starts at `at`: zero when none does.
pub fn count_matches(doc: &mut Document, pattern: &str, at: Option<Pos>) -> (usize, usize) {
    let Some(needle) = Needle::new(pattern) else {
        return (0, 0);
    };
    let mut index = 0;
    let mut total = 0;
    for page in 0..doc.page_count() {
        let Some(text) = doc.text(page) else {
            continue;
        };
        for (line, text) in text.lines.iter().enumerate() {
            for char in needle.starts(&text.text) {
                total += 1;
                if at == Some(Pos::new(page, line, char)) {
                    index = total;
                }
            }
        }
    }
    (index, total)
}

/// `ch` as a search compares it.
fn fold(ch: char, folded: bool) -> char {
    match folded {
        true => ch.to_lowercase().next().unwrap_or(ch),
        false => ch,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::text::tests::words;
    use super::super::text::PageText;
    use super::*;

    const PAGE: (f32, f32) = (600.0, 800.0);

    /// A document of `pages`, each a list of lines set down the page.
    fn doc(pages: &[&[&str]]) -> Document {
        let texts = pages
            .iter()
            .map(|lines| {
                let glyphs = lines
                    .iter()
                    .enumerate()
                    .flat_map(|(row, line)| words(line, 20.0, 40.0 + row as f32 * 14.0, 10.0))
                    .collect();
                PageText::new(glyphs)
            })
            .collect();
        Document::of_text(vec![PAGE; pages.len()], texts)
    }

    fn caret_at(doc: &mut Document, at: Pos) -> Caret {
        let mut caret = Caret::default();
        caret.enter(doc, Some(at));
        caret
    }

    #[test]
    fn test_w_crosses_punctuation_and_lines_the_way_vim_does() {
        let mut doc = doc(&[&["one, two", "three"]]);
        let mut caret = caret_at(&mut doc, Pos::new(0, 0, 0));
        let mut stops = Vec::new();
        for _ in 0..3 {
            caret.motion(&mut doc, Motion::WordForward, 1, Operator::Move, None);
            stops.push(caret.pos.expect("a cursor"));
        }
        assert_eq!(
            stops,
            [Pos::new(0, 0, 3), Pos::new(0, 0, 5), Pos::new(0, 1, 0)],
            "the comma is a word of its own, and the next line's start one more"
        );
    }

    #[test]
    fn test_b_and_e_find_the_edges_of_words() {
        let mut doc = doc(&[&["alpha beta"]]);
        let mut caret = caret_at(&mut doc, Pos::new(0, 0, 7));
        caret.motion(&mut doc, Motion::WordBackward, 1, Operator::Move, None);
        assert_eq!(caret.pos, Some(Pos::new(0, 0, 6)));
        caret.motion(&mut doc, Motion::WordEnd, 1, Operator::Move, None);
        assert_eq!(caret.pos, Some(Pos::new(0, 0, 9)));
    }

    #[test]
    fn test_j_holds_its_column_through_a_short_line() {
        let mut doc = doc(&[&["a long line", "ab", "another long one"]]);
        let mut caret = caret_at(&mut doc, Pos::new(0, 0, 8));
        caret.motion(&mut doc, Motion::Down, 1, Operator::Move, None);
        assert_eq!(
            caret.pos,
            Some(Pos::new(0, 1, 1)),
            "as far as the line goes"
        );
        caret.motion(&mut doc, Motion::Down, 1, Operator::Move, None);
        assert_eq!(caret.pos, Some(Pos::new(0, 2, 8)), "and back out again");
    }

    #[test]
    fn test_motions_step_over_a_page_with_no_text() {
        let mut doc = doc(&[&["first"], &[], &["third"]]);
        let mut caret = caret_at(&mut doc, Pos::new(0, 0, 0));
        caret.motion(&mut doc, Motion::Down, 1, Operator::Move, None);
        assert_eq!(caret.pos, Some(Pos::new(2, 0, 0)));
    }

    #[test]
    fn test_yw_copies_up_to_the_next_word_and_ye_through_the_end_of_this_one() {
        let mut doc = doc(&[&["copy these words"]]);
        let mut caret = caret_at(&mut doc, Pos::new(0, 0, 5));
        let yanked = caret.motion(&mut doc, Motion::WordForward, 1, Operator::Yank, None);
        assert_eq!(yanked, Effect::Yanked("these ".to_string()));
        assert_eq!(
            caret.pos,
            Some(Pos::new(0, 0, 5)),
            "a yank leaves the cursor"
        );
        let yanked = caret.motion(&mut doc, Motion::WordEnd, 1, Operator::Yank, None);
        assert_eq!(yanked, Effect::Yanked("these".to_string()));
    }

    #[test]
    fn test_a_line_selection_copies_whole_lines_across_pages() {
        let mut doc = doc(&[&["page one"], &["page two", "last"]]);
        let mut caret = caret_at(&mut doc, Pos::new(0, 0, 3));
        caret.set_visual(Visual::Line);
        caret.motion(&mut doc, Motion::Down, 1, Operator::Move, None);
        assert_eq!(
            caret.yank_selection(&mut doc),
            Effect::Yanked("page one\npage two".to_string())
        );
        assert_eq!(caret.selection(), None, "a yank ends the selection");
    }

    #[test]
    fn test_search_wraps_and_matches_case_only_when_asked() {
        let mut doc = doc(&[&["Figure one", "a figure"], &["figure Two"]]);
        let mut caret = caret_at(&mut doc, Pos::new(0, 1, 2));
        caret.search(&mut doc, "figure", true, 1, Operator::Move, None);
        assert_eq!(caret.pos, Some(Pos::new(1, 0, 0)), "past the cursor first");
        caret.search_again(&mut doc, false, 1, Operator::Move, None);
        assert_eq!(
            caret.pos,
            Some(Pos::new(0, 0, 0)),
            "then round the end, any case"
        );
        assert_eq!(caret.found.map(|found| found.length), Some(6));
        let missing = caret.search(&mut doc, "FIGURE", true, 1, Operator::Move, None);
        assert!(matches!(missing, Effect::Refused(_)), "a capital is exact");
    }

    #[test]
    fn test_matches_are_counted_through_the_whole_document() {
        let mut doc = doc(&[&["a figure, a Figure"], &["no match"], &["figure"]]);
        assert_eq!(
            count_matches(&mut doc, "figure", Some(Pos::new(2, 0, 0))),
            (3, 3)
        );
        assert_eq!(
            count_matches(&mut doc, "Figure", None),
            (0, 1),
            "a capital is exact"
        );
        assert_eq!(count_matches(&mut doc, "absent", None), (0, 0));
    }

    #[test]
    fn test_n_says_so_when_nothing_was_searched_for() {
        let mut doc = doc(&[&["text"]]);
        let mut caret = caret_at(&mut doc, Pos::new(0, 0, 0));
        assert!(matches!(
            caret.search_again(&mut doc, false, 1, Operator::Move, None),
            Effect::Refused(_)
        ));
    }

    #[test]
    fn test_f_and_t_stop_on_and_short_of_the_character_and_repeat() {
        let mut doc = doc(&[&["a-b-c-d"]]);
        let mut caret = caret_at(&mut doc, Pos::new(0, 0, 0));
        caret.find(&mut doc, FindKind::Forward, '-', 1, Operator::Move);
        assert_eq!(caret.pos, Some(Pos::new(0, 0, 1)));
        caret.repeat_find(&mut doc, false, 1, Operator::Move);
        assert_eq!(caret.pos, Some(Pos::new(0, 0, 3)));
        caret.repeat_find(&mut doc, true, 1, Operator::Move);
        assert_eq!(caret.pos, Some(Pos::new(0, 0, 1)), "`,` goes back");
        caret.find(&mut doc, FindKind::ForwardTill, 'd', 1, Operator::Move);
        assert_eq!(caret.pos, Some(Pos::new(0, 0, 5)), "just short of the d");
    }

    #[test]
    fn test_a_document_with_no_text_refuses_a_cursor() {
        let mut doc = doc(&[&[]]);
        assert!(matches!(
            Caret::default().enter(&mut doc, None),
            Effect::Refused(_)
        ));
    }

    #[test]
    fn test_paragraph_motions_stop_where_a_paragraph_starts() {
        let mut doc = Document::of_text(
            vec![PAGE],
            vec![PageText::new(
                [20.0, 34.0, 48.0, 72.0, 86.0]
                    .into_iter()
                    .enumerate()
                    .flat_map(|(row, y)| words(&format!("line {row}"), 20.0, y, 10.0))
                    .collect(),
            )],
        );
        let mut caret = caret_at(&mut doc, Pos::new(0, 0, 0));
        caret.motion(&mut doc, Motion::ParagraphForward, 1, Operator::Move, None);
        assert_eq!(caret.pos, Some(Pos::new(0, 3, 0)));
    }
}
