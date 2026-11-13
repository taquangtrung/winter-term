//! Inline suggestions: the text an extension's source (the Copilot
//! extension's, see `winter_ext_api::InlineProvider`) expects next, offered
//! at the cursor as you type (ghost text) or, after a change, as an edit
//! somewhere else (a next edit).
//!
//! A tab asks once typing pauses, of the first source offering what is
//! wanted, through the window ([`InlineCall`]), and takes the answer when
//! the window hands it back under its ticket. Ghost text is laid in as
//! Cherry's inlay at the cursor, its later lines as virtual lines under the
//! cursor's, and goes as soon as typing no longer fits it; a next edit is a
//! card at the cursor until the text changes.

use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use cherry::{Edit, Inlay, Mode, Utf16Position, VirtualLine};
use misty::theme::Theme;
use misty::{Frame, Rect};
use winter_ext_api::{InlineAnswer, InlineEdit, InlineKind, InlineRequest, TextPosition};
use winter_lsp::protocol;
use winter_model::editor::{EditorOutcome, Notice};
use winter_model::input::table;
use winter_model::input::Key;

use super::keys::{InlineAct, INLINE_KEYS};
use super::requests::draw_info_card;
use super::settings::{EditorSettings, InlineOffered};
use super::{session, TextEditor};

// ========================================================================
// Constants
// ========================================================================

/// How long after an edit a source is asked, so a burst of typing is one
/// question: Zed's 75ms.
const DEBOUNCE: Duration = Duration::from_millis(75);

/// The owner of the inlays and virtual lines ghost text is laid in as.
const GHOST: &str = "ghost";

/// The most lines of each side a next edit's card shows.
const CARD_LINES: usize = 8;

/// What is said when `C-\` asked and the source had nothing.
const NOTHING_TO_SUGGEST: &str = "nothing to suggest here";

/// What a next edit's card says to do with it.
const NEXT_EDIT_KEYS: &str = "Tab to make it, Esc to leave it";

/// The ticket the next ask goes under, unique in the process, as answers
/// come back to the window from every tab.
static NEXT_TICKET: AtomicU64 = AtomicU64::new(1);

// ========================================================================
// Data Structures
// ========================================================================

/// What a tab asks or tells an extension's inline source, for the window
/// to hand on.
#[derive(Clone, Debug, PartialEq)]
pub enum InlineCall {
    /// A suggestion is wanted.
    Ask(InlineAsk),
    /// A suggestion shows.
    Shown(InlineTold),
    /// A suggestion was taken, all of it.
    Taken(InlineTold),
}

/// A suggestion asked of a source.
#[derive(Clone, Debug, PartialEq)]
pub struct InlineAsk {
    /// What is asked.
    pub request: InlineRequest,
    /// The source's full id.
    pub source: String,
}

/// What a source is told of a suggestion of its own.
#[derive(Clone, Debug, PartialEq)]
pub struct InlineTold {
    /// Which kind it was.
    pub kind: InlineKind,
    /// The source's own note of it.
    pub receipt: String,
    /// The source's full id.
    pub source: String,
}

/// One tab's inline suggestions: the sources, what is on screen, and when
/// to ask next.
#[derive(Debug, Default)]
pub(super) struct Inline {
    /// The question waiting on an answer.
    asked: Option<Asked>,
    /// What the sources are to be asked and told, for the window.
    calls: Vec<InlineCall>,
    /// When to ask next, and for which kind.
    due: Option<(Instant, InlineKind)>,
    /// The next edit shown in a card.
    edit: Option<NextEdit>,
    /// The ghost text laid in at the cursor.
    ghost: Option<Ghost>,
    /// The sources, in the order they are asked, as the settings last had
    /// them.
    sources: Vec<InlineOffered>,
}

/// A question sent to a source.
#[derive(Clone, Debug)]
struct Asked {
    /// Whether `C-\` asked, rather than typing.
    invoked: bool,
    /// What was asked for.
    kind: InlineKind,
    /// The cursor's line when asked.
    line: usize,
    /// The source's full id.
    source: String,
    /// What the answer comes back under.
    ticket: u64,
    /// The text's version asked about.
    version: u64,
}

/// The ghost text a source offered for a line.
#[derive(Clone, Debug, PartialEq)]
struct Ghost {
    /// Which of the suggestions shows.
    at: usize,
    /// Each suggestion, the first shown first.
    items: Vec<Suggestion>,
    /// The line they are for.
    line: usize,
    /// The source's full id.
    source: String,
    /// The suggestions the source was told show, by place.
    told: Vec<usize>,
}

/// One ghost text suggestion: the characters of its line from column
/// `start` up to column `end` replaced with `insert`.
#[derive(Clone, Debug, PartialEq)]
struct Suggestion {
    end: usize,
    insert: String,
    /// The source's own note of it.
    receipt: String,
    start: usize,
}

/// An edit a source expects next: the chars in `range` replaced with
/// `text`, worked out from the text at `version`.
#[derive(Clone, Debug, PartialEq)]
struct NextEdit {
    range: Range<usize>,
    /// The source's own note of it.
    receipt: String,
    /// The source's full id.
    source: String,
    text: String,
    version: u64,
}

/// Where the cursor is.
#[derive(Clone, Copy, Debug)]
struct Place {
    /// Its column, in chars.
    column: usize,
    /// Its line, from 0.
    line: usize,
    /// Its char offset.
    offset: usize,
}

// ========================================================================
// TextEditor: inline suggestions
// ========================================================================

impl TextEditor {
    /// What the sources are to be asked and told since last taken, for the
    /// window to hand on to them.
    pub fn take_inline_calls(&mut self) -> Vec<InlineCall> {
        std::mem::take(&mut self.inline.calls)
    }

    /// Takes the answer a source gave under `ticket`, where this tab asked
    /// for it. Whether it had.
    pub fn take_inline_answer(&mut self, ticket: u64, answer: &InlineAnswer) -> bool {
        let Some(asked) = self.inline.asked.take_if(|asked| asked.ticket == ticket) else {
            return false;
        };
        match (asked.kind, answer) {
            (InlineKind::Ghost, InlineAnswer::Ghost(edits)) => self.offer_ghost(&asked, edits),
            (InlineKind::Ghost, _) => self.offer_ghost(&asked, &[]),
            (InlineKind::Edit, InlineAnswer::Edit(edit)) if self.version() == asked.version => {
                self.offer_edit(&asked, edit);
            }
            _ => {}
        }
        true
    }

    /// Takes the sources the settings name, putting away what one no
    /// longer offering it showed.
    pub(super) fn configure_inline(&mut self, settings: &EditorSettings) {
        self.inline.sources.clone_from(&settings.inline);
        let offers = |source: &str, kind| {
            settings
                .inline
                .iter()
                .any(|offered| offered.id == source && offered.offer.offers(kind))
        };
        if let Some(ghost) = &self.inline.ghost {
            if !offers(&ghost.source, InlineKind::Ghost) {
                self.put_away_ghost();
            }
        }
        if let Some(edit) = &self.inline.edit {
            if !offers(&edit.source, InlineKind::Edit) {
                self.inline.edit = None;
            }
        }
    }

    /// A key the suggestions give a meaning of their own: what it did, or
    /// `None` for a key left to Cherry. `C-\` asks for ghost text while
    /// typing.
    pub(super) fn inline_key(&mut self, key: &Key) -> Option<EditorOutcome> {
        let act = table::find(&INLINE_KEYS, key)?;
        let typing = self.is_typing();
        if act == InlineAct::Ask {
            if !typing || self.inline_source(InlineKind::Ghost).is_none() {
                return None;
            }
            self.ask_inline(InlineKind::Ghost, true);
            return Some(EditorOutcome::Consumed);
        }
        if typing && self.ghost_text().is_some() {
            return self.ghost_key(act);
        }
        if self.inline.edit.is_some() {
            return self.next_edit_key(act);
        }
        None
    }

    /// After a key or the pointer Cherry handled: the ghost text follows
    /// the typing or goes; an edit (the text no longer at `before`) puts
    /// away a next edit and asks again once typing pauses, for ghost text
    /// while typing and a next edit otherwise.
    pub(super) fn inline_after_key(&mut self, before: u64) {
        self.lay_ghost();
        let version = self.version();
        if version == before {
            return;
        }
        if self
            .inline
            .edit
            .as_ref()
            .is_some_and(|edit| edit.version != version)
        {
            self.inline.edit = None;
        }
        let kind = match self.is_typing() && self.inline_source(InlineKind::Ghost).is_some() {
            true => InlineKind::Ghost,
            false => InlineKind::Edit,
        };
        if self.inline_source(kind).is_some() {
            self.inline.due = Some((Instant::now() + DEBOUNCE, kind));
        }
    }

    /// When a source is next to be asked.
    pub(super) fn inline_next_tick(&self) -> Option<Instant> {
        self.inline.due.map(|(at, _)| at)
    }

    /// Asks what is due to be asked by `now`.
    pub(super) fn inline_tick(&mut self, now: Instant) {
        let Some((at, kind)) = self.inline.due else {
            return;
        };
        if at <= now {
            self.inline.due = None;
            self.ask_inline(kind, false);
        }
    }

    /// Draws the card of the next edit offered, hung from `cursor` within
    /// `bounds`.
    pub(super) fn draw_next_edit(
        &self,
        frame: &mut Frame,
        cursor: Rect,
        bounds: Rect,
        theme: &Theme,
    ) {
        if let Some(text) = self.next_edit_card() {
            draw_info_card(frame, &text, None, cursor, bounds, theme);
        }
    }

    /// What the status bar says of the ghost text on screen: its source,
    /// and which of the suggestions offered shows.
    pub(super) fn inline_status(&self) -> Option<String> {
        let ghost = self.inline.ghost.as_ref()?;
        let name = self.source_title(&ghost.source);
        Some(match ghost.items.len() {
            1 => name,
            n => format!("{name} {}/{n}", ghost.at + 1),
        })
    }

    // --------------------------------------------------------------------
    // Asking
    // --------------------------------------------------------------------

    /// The first source offering `kind` for this file.
    fn inline_source(&self, kind: InlineKind) -> Option<&InlineOffered> {
        if winter_base::remote::parse(&self.path).is_some() {
            return None;
        }
        self.inline
            .sources
            .iter()
            .find(|source| source.offer.offers(kind))
    }

    /// What the status bar and notices call the source `id`.
    fn source_title(&self, id: &str) -> String {
        self.inline
            .sources
            .iter()
            .find(|source| source.id == id)
            .map_or_else(|| id.to_string(), |source| source.title.clone())
    }

    /// Asks the first source offering `kind` for it at the cursor, its
    /// extension's servers handed the text first; `invoked` where `C-\`
    /// asked.
    fn ask_inline(&mut self, kind: InlineKind, invoked: bool) {
        let Some(source) = self.inline_source(kind).cloned() else {
            return;
        };
        let Some(place) = self.place() else {
            return;
        };
        let language = self.language();
        let (version, at, tab_size, insert_spaces) = session::with(|session| {
            let version = session.sync_extension(self.document, &source.servers, &language);
            let text = session.editor.document(self.document)?;
            let at = text.char_to_utf16_position(place.offset)?;
            let tab_size = session.editor.options().tab_width;
            let insert_spaces = !(0..text.len_lines())
                .filter_map(|line| text.line_text(line))
                .any(|line| line.starts_with('\t'));
            Some((version, at, tab_size, insert_spaces))
        })
        .unwrap_or_default();
        let ticket = NEXT_TICKET.fetch_add(1, Ordering::Relaxed);
        let at = TextPosition::new(at.line as u32, at.column as u32);
        let mut request = InlineRequest::new(ticket, kind, self.path.clone(), at);
        request.insert_spaces = insert_spaces;
        request.invoked = invoked;
        request.tab_size = tab_size as u32;
        request.uri = protocol::uri_of(&self.path);
        request.version = version.map(i64::from);
        self.inline.calls.push(InlineCall::Ask(InlineAsk {
            request,
            source: source.id.clone(),
        }));
        self.inline.asked = Some(Asked {
            invoked,
            kind,
            line: place.line,
            source: source.id,
            ticket,
            version: self.version(),
        });
    }

    /// Tells `source` its suggestion `receipt` is the note of now shows, or
    /// was taken, as `call` says.
    fn tell_inline(
        &mut self,
        source: &str,
        kind: InlineKind,
        receipt: &str,
        call: fn(InlineTold) -> InlineCall,
    ) {
        self.inline.calls.push(call(InlineTold {
            kind,
            receipt: receipt.to_string(),
            source: source.to_string(),
        }));
    }

    // --------------------------------------------------------------------
    // Ghost text
    // --------------------------------------------------------------------

    /// Shows the ghost text `edits` offer for the line `asked` was about,
    /// where the cursor still types on it; where none fits, says so if
    /// `C-\` asked, or asks for a next edit if not.
    fn offer_ghost(&mut self, asked: &Asked, edits: &[InlineEdit]) {
        let Some(place) = self.place().filter(|place| place.line == asked.line) else {
            return;
        };
        if !self.is_typing() {
            return;
        }
        let items: Vec<Suggestion> = session::with(|session| {
            let text = session.editor.document(self.document)?;
            let line_start = place.offset - place.column;
            let column = |at: TextPosition| {
                let position = Utf16Position {
                    column: at.column as usize,
                    line: at.line as usize,
                };
                text.utf16_position_to_char(position)
                    .map(|offset| offset - line_start)
            };
            let on_line = |edit: &&InlineEdit| {
                let line = asked.line as u32;
                edit.range.start.line == line && edit.range.end.line == line
            };
            Some(
                edits
                    .iter()
                    .filter(on_line)
                    .filter_map(|edit| {
                        Some(Suggestion {
                            end: column(edit.range.end)?,
                            insert: edit.text.clone(),
                            receipt: edit.receipt.clone(),
                            start: column(edit.range.start)?,
                        })
                    })
                    .filter(|item| item.start <= item.end)
                    .collect(),
            )
        })
        .unwrap_or_default();
        self.inline.ghost = (!items.is_empty()).then(|| Ghost {
            at: 0,
            items,
            line: asked.line,
            source: asked.source.clone(),
            told: Vec::new(),
        });
        if self.ghost_text().is_some() {
            self.tell_shown();
            self.lay_ghost();
            return;
        }
        self.inline.ghost = None;
        match asked.invoked {
            true => {
                let said = format!("{}: {NOTHING_TO_SUGGEST}", self.source_title(&asked.source));
                self.notice = Some(Notice::info(said).unlogged());
            }
            false => self.ask_inline(InlineKind::Edit, false),
        }
    }

    /// The ghost text to show: what the suggestion shown adds past what is
    /// typed on its line, leaving out the line's own text after the cursor
    /// it ends with. `None` where typing went another way or left the line.
    fn ghost_text(&self) -> Option<String> {
        let ghost = self.inline.ghost.as_ref()?;
        let item = ghost.items.get(ghost.at)?;
        let place = self.place().filter(|place| place.line == ghost.line)?;
        let line: Vec<char> = session::with(|session| {
            session
                .editor
                .document(self.document)?
                .line_text(ghost.line)
        })?
        .chars()
        .collect();
        let column = place.column.min(line.len());
        if item.start > column {
            return None;
        }
        let typed: String = line[item.start..column].iter().collect();
        let rest = item.insert.strip_prefix(&typed)?;
        let tail: String = line[column..item.end.max(column).min(line.len())]
            .iter()
            .collect();
        let rest = rest.strip_suffix(&tail).unwrap_or(rest);
        (!rest.is_empty()).then(|| rest.to_string())
    }

    /// Lays the ghost text in at the cursor while typing fits it: its first
    /// line as an inlay, the rest as lines of their own under the cursor's.
    /// Puts it away where it no longer fits.
    fn lay_ghost(&mut self) {
        let shown = self.ghost_text().filter(|_| self.is_typing());
        let (Some(text), Some(place)) = (shown, self.place()) else {
            if self.inline.ghost.is_some() {
                self.put_away_ghost();
            }
            return;
        };
        session::with(|session| {
            let spaces = " ".repeat(session.editor.options().tab_width);
            let mut lines = text.split('\n').map(|line| line.replace('\t', &spaces));
            let first = lines.next().unwrap_or_default();
            let later = lines
                .map(|line| VirtualLine::new(place.line, line))
                .collect();
            let inlay = Inlay::new(place.offset, first);
            session.editor.set_inlays(self.document, GHOST, vec![inlay]);
            session
                .editor
                .set_virtual_lines(self.document, GHOST, later);
        });
    }

    /// Takes the ghost text off the screen, and forgets it.
    fn put_away_ghost(&mut self) {
        self.inline.ghost = None;
        session::with(|session| {
            session.editor.set_inlays(self.document, GHOST, Vec::new());
            session
                .editor
                .set_virtual_lines(self.document, GHOST, Vec::new());
        });
    }

    /// What `act` does over ghost text: takes all of it, its next word,
    /// or its line; steps through the suggestions; or puts it away, before
    /// the key goes on to Cherry where it leaves.
    fn ghost_key(&mut self, act: InlineAct) -> Option<EditorOutcome> {
        match act {
            InlineAct::Ask => return None,
            InlineAct::Leave => {
                self.put_away_ghost();
                return None;
            }
            InlineAct::Next => self.step_ghost(true),
            InlineAct::Previous => self.step_ghost(false),
            InlineAct::PutAway => self.put_away_ghost(),
            InlineAct::TakeAll => self.take_whole_ghost(),
            InlineAct::TakeLine => self.take_ghost_part(true),
            InlineAct::TakeWord => self.take_ghost_part(false),
        }
        Some(EditorOutcome::Consumed)
    }

    /// Puts all of the ghost text in.
    fn take_whole_ghost(&mut self) {
        if let Some(rest) = self.ghost_text() {
            self.put_in(&rest);
            self.finish_ghost();
        }
    }

    /// Puts the ghost text's next word in, or the rest of its line where
    /// `line`; taking a line break starts the rest on the next line.
    fn take_ghost_part(&mut self, line: bool) {
        let Some(ghost) = self.ghost_text() else {
            return;
        };
        let chars: Vec<char> = ghost.chars().collect();
        let end = match line {
            true => match chars.first() {
                Some('\n') => 1 + chars[1..].iter().take_while(|c| **c != '\n').count(),
                _ => chars.iter().take_while(|c| **c != '\n').count(),
            },
            false => {
                let spaces = chars.iter().take_while(|c| c.is_whitespace()).count();
                let word = chars[spaces..]
                    .iter()
                    .take_while(|c| !c.is_whitespace())
                    .count();
                (spaces + word).max(1)
            }
        };
        let end = end.min(chars.len());
        let piece: String = chars[..end].iter().collect();
        let left: String = chars[end..].iter().collect();
        let tail = self.ghost_tail();
        self.put_in(&piece);
        if piece.contains('\n') {
            self.rebase_ghost(&left, tail);
        }
        match self.ghost_text() {
            Some(_) => self.lay_ghost(),
            None => self.finish_ghost(),
        }
    }

    /// How many chars of the line after the cursor the suggestion shown
    /// ends with, which taking a line break carries to the next line.
    fn ghost_tail(&self) -> usize {
        let Some(item) = self
            .inline
            .ghost
            .as_ref()
            .and_then(|ghost| ghost.items.get(ghost.at))
        else {
            return 0;
        };
        let Some(place) = self.place() else {
            return 0;
        };
        let length = self.line_chars(place.line).len();
        item.end.max(place.column).min(length) - place.column
    }

    /// Makes `left`, what is left of the suggestion shown after a line
    /// break was taken, the one suggestion for the cursor's new line, over
    /// the `tail` chars after the cursor it ends with.
    fn rebase_ghost(&mut self, left: &str, tail: usize) {
        let Some(place) = self.place() else {
            return;
        };
        let line = self.line_chars(place.line);
        let Some(ghost) = self.inline.ghost.as_mut() else {
            return;
        };
        let Some(shown) = ghost.items.get(ghost.at) else {
            return;
        };
        let column = place.column.min(line.len());
        let end = (column + tail).min(line.len());
        let typed: String = line[..column].iter().collect();
        let after: String = line[column..end].iter().collect();
        ghost.items = vec![Suggestion {
            end,
            insert: format!("{typed}{left}{after}"),
            receipt: shown.receipt.clone(),
            start: 0,
        }];
        ghost.at = 0;
        ghost.line = place.line;
        ghost.told = vec![0];
    }

    /// Shows the next suggestion, or the one before, round the list.
    fn step_ghost(&mut self, forward: bool) {
        if let Some(ghost) = self.inline.ghost.as_mut() {
            let count = ghost.items.len().max(1);
            ghost.at = match forward {
                true => (ghost.at + 1) % count,
                false => (ghost.at + count - 1) % count,
            };
        }
        self.tell_shown();
        self.lay_ghost();
    }

    /// Tells the source of the ghost text on screen that it shows, once for
    /// each of its suggestions.
    fn tell_shown(&mut self) {
        let Some(ghost) = self.inline.ghost.as_mut() else {
            return;
        };
        if ghost.told.contains(&ghost.at) {
            return;
        }
        ghost.told.push(ghost.at);
        let Some(receipt) = ghost.items.get(ghost.at).map(|item| item.receipt.clone()) else {
            return;
        };
        let source = ghost.source.clone();
        self.tell_inline(&source, InlineKind::Ghost, &receipt, InlineCall::Shown);
    }

    /// Puts the ghost text away, all of it taken, its source told.
    fn finish_ghost(&mut self) {
        let Some(ghost) = self.inline.ghost.clone() else {
            return;
        };
        self.put_away_ghost();
        if let Some(item) = ghost.items.get(ghost.at) {
            self.tell_inline(
                &ghost.source,
                InlineKind::Ghost,
                &item.receipt,
                InlineCall::Taken,
            );
        }
    }

    // --------------------------------------------------------------------
    // Next edits
    // --------------------------------------------------------------------

    /// Shows the next edit `edit` a source offered, its source told.
    fn offer_edit(&mut self, asked: &Asked, edit: &InlineEdit) {
        let range = session::with(|session| {
            let text = session.editor.document(self.document)?;
            let offset = |at: TextPosition| {
                text.utf16_position_to_char(Utf16Position {
                    column: at.column as usize,
                    line: at.line as usize,
                })
            };
            Some(offset(edit.range.start)?..offset(edit.range.end)?)
        });
        let Some(range) = range.filter(|range| range.start <= range.end) else {
            return;
        };
        self.inline.edit = Some(NextEdit {
            range,
            receipt: edit.receipt.clone(),
            source: asked.source.clone(),
            text: edit.text.clone(),
            version: asked.version,
        });
        self.tell_inline(
            &asked.source,
            InlineKind::Edit,
            &edit.receipt,
            InlineCall::Shown,
        );
    }

    /// What `act` does over a next edit: makes it, or leaves it, before
    /// the key goes on to Cherry where it leaves; the ghost text's own
    /// keys go on to Cherry.
    fn next_edit_key(&mut self, act: InlineAct) -> Option<EditorOutcome> {
        match act {
            InlineAct::Ask
            | InlineAct::Next
            | InlineAct::Previous
            | InlineAct::TakeLine
            | InlineAct::TakeWord => return None,
            InlineAct::Leave => {
                self.inline.edit = None;
                return None;
            }
            InlineAct::PutAway => self.inline.edit = None,
            InlineAct::TakeAll => self.take_next_edit(),
        }
        Some(EditorOutcome::Consumed)
    }

    /// Makes the next edit offered, where the text is still what it was
    /// worked out from, landing after it; its source told.
    fn take_next_edit(&mut self) {
        let Some(edit) = self.inline.edit.take() else {
            return;
        };
        if edit.version != self.version() {
            return;
        }
        let view = self.view_id();
        let typing = self.is_typing();
        let length = edit.text.chars().count();
        session::with(|session| {
            let change = Edit::new(edit.range.clone(), &edit.text);
            session.editor.apply_edits(view, vec![change]);
            let end = edit.range.start + length;
            let cursor = match typing {
                true => end,
                false => end.saturating_sub(1).max(edit.range.start),
            };
            session.editor.set_cursor(view, cursor);
        });
        self.tell_inline(
            &edit.source,
            InlineKind::Edit,
            &edit.receipt,
            InlineCall::Taken,
        );
    }

    /// What the card of the next edit offered says: the source, the line,
    /// the text it takes out and what it puts in.
    fn next_edit_card(&self) -> Option<String> {
        let edit = self.inline.edit.as_ref()?;
        let (line, taken) = session::with(|session| {
            let text = session.editor.document(self.document)?;
            let start = text.char_to_position(edit.range.start)?;
            let taken = text.slice(edit.range.clone());
            Some((start.line, taken))
        })?;
        let name = self.source_title(&edit.source);
        let mut card = format!("{name}: line {} ({NEXT_EDIT_KEYS})", line + 1);
        for removed in taken
            .lines()
            .filter(|line| !line.is_empty())
            .take(CARD_LINES)
        {
            card.push_str(&format!("\n- {removed}"));
        }
        for added in edit.text.lines().take(CARD_LINES) {
            card.push_str(&format!("\n+ {added}"));
        }
        Some(card)
    }

    // --------------------------------------------------------------------
    // The text
    // --------------------------------------------------------------------

    /// Where the cursor is.
    fn place(&self) -> Option<Place> {
        let view = self.view_id();
        session::with(|session| {
            let offset = session.editor.view(view)?.cursor();
            let at = session
                .editor
                .document(self.document)?
                .char_to_position(offset)?;
            Some(Place {
                column: at.column,
                line: at.line,
                offset,
            })
        })
    }

    /// Whether keys type text: the view is in Insert mode.
    fn is_typing(&self) -> bool {
        let view = self.view_id();
        session::with(|session| {
            session
                .editor
                .view(view)
                .is_some_and(|view| view.mode() == Mode::Insert)
        })
    }

    /// The chars of `line`.
    fn line_chars(&self, line: usize) -> Vec<char> {
        session::with(|session| {
            session
                .editor
                .document(self.document)
                .and_then(|text| text.line_text(line))
        })
        .map(|text| text.chars().collect())
        .unwrap_or_default()
    }

    /// Puts `text` in at the cursor, landing after it.
    fn put_in(&mut self, text: &str) {
        let Some(place) = self.place() else {
            return;
        };
        let view = self.view_id();
        session::with(|session| {
            let at = place.offset;
            session
                .editor
                .apply_edits(view, vec![Edit::new(at..at, text)]);
            session.editor.set_cursor(view, at + text.chars().count());
        });
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;
    use winter_ext_api::{InlineOffer, TextRange};
    use winter_lsp::{Port, ServerId};
    use winter_model::editor::Page;
    use winter_model::input::{KeyCode, Mods};

    use super::*;

    /// The source every test's tab asks.
    const SOURCE: &str = "pilot.ghost";

    /// `code` pressed with `mods`.
    fn press(code: KeyCode, mods: Mods) -> Key {
        Key {
            alt: mods.alt,
            code,
            ctrl: mods.ctrl,
            shift: false,
            sup: false,
        }
    }

    /// A tab on a file of its own (named for `tag`) holding `text`, typing
    /// at the end of its first line, with one source offering both kinds.
    fn typing(tag: &str, text: &str) -> (TextEditor, PathBuf) {
        typing_served(tag, text, Vec::new())
    }

    /// [`typing`], the source's extension running `servers`.
    fn typing_served(tag: &str, text: &str, servers: Vec<ServerId>) -> (TextEditor, PathBuf) {
        let path =
            std::env::temp_dir().join(format!("winter-inline-{tag}-{}.txt", std::process::id()));
        std::fs::write(&path, text).expect("written");
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        let settings = EditorSettings {
            inline: vec![InlineOffered {
                id: SOURCE.to_string(),
                offer: InlineOffer::new(true, true),
                servers,
                title: "Pilot".to_string(),
            }],
            ..EditorSettings::default()
        };
        editor.configure(&settings);
        editor.on_key(&press(KeyCode::Char('A'), Mods::NONE));
        (editor, path)
    }

    /// The one ask the tab made since its calls were last taken.
    fn asked(editor: &mut TextEditor) -> InlineAsk {
        let calls = editor.take_inline_calls();
        let [InlineCall::Ask(ask)] = calls.as_slice() else {
            panic!("one ask: {calls:?}");
        };
        ask.clone()
    }

    /// Ghost text replacing line 0 from column `start` to `end` with
    /// `text`, under the receipt `r1`.
    fn ghost(start: u32, end: u32, text: &str) -> InlineAnswer {
        let range = TextRange::new(TextPosition::new(0, start), TextPosition::new(0, end));
        InlineAnswer::Ghost(vec![InlineEdit::new(range, text, "r1")])
    }

    /// The source told of `kind` under the receipt `r1`.
    fn told(kind: InlineKind, call: fn(InlineTold) -> InlineCall) -> InlineCall {
        call(InlineTold {
            kind,
            receipt: "r1".to_string(),
            source: SOURCE.to_string(),
        })
    }

    #[test]
    fn test_asked_ghost_text_shows_past_what_is_typed_and_tab_takes_it() {
        let (mut editor, path) = typing("take", "fo");
        editor.on_key(&press(KeyCode::Char('\\'), Mods::CTRL));
        let ask = asked(&mut editor);
        assert_eq!(ask.source, SOURCE);
        assert!(ask.request.invoked);
        assert_eq!(ask.request.at, TextPosition::new(0, 2));
        let answer = ghost(0, 2, "foo(bar)");
        assert!(
            !editor.take_inline_answer(ask.request.ticket + 1, &answer),
            "not its ticket"
        );
        assert!(editor.take_inline_answer(ask.request.ticket, &answer));
        assert_eq!(editor.ghost_text().as_deref(), Some("o(bar)"));
        assert_eq!(editor.inline_status().as_deref(), Some("Pilot"));
        editor.on_key(&press(KeyCode::Char('o'), Mods::NONE));
        assert_eq!(
            editor.ghost_text().as_deref(),
            Some("(bar)"),
            "typing what it shows keeps it"
        );
        editor.on_key(&press(KeyCode::Tab, Mods::NONE));
        assert_eq!(session::with(|s| s.lines(editor.document))[0], "foo(bar)");
        assert_eq!(
            editor.take_inline_calls(),
            [
                told(InlineKind::Ghost, InlineCall::Shown),
                told(InlineKind::Ghost, InlineCall::Taken)
            ]
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_typing_past_the_ghost_text_puts_it_away() {
        let (mut editor, path) = typing("past", "fo");
        editor.on_key(&press(KeyCode::Char('\\'), Mods::CTRL));
        let ticket = asked(&mut editor).request.ticket;
        editor.take_inline_answer(ticket, &ghost(0, 2, "foo"));
        editor.on_key(&press(KeyCode::Char('x'), Mods::NONE));
        assert_eq!(editor.ghost_text(), None);
        assert!(editor.inline.ghost.is_none(), "forgotten");
        assert!(editor.inline_status().is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_the_ghost_text_leaves_out_the_line_s_own_text_after_the_cursor() {
        let (mut editor, path) = typing("tail", "foo()");
        editor.on_key(&press(KeyCode::Left, Mods::NONE));
        editor.on_key(&press(KeyCode::Char('\\'), Mods::CTRL));
        let ticket = asked(&mut editor).request.ticket;
        editor.take_inline_answer(ticket, &ghost(0, 5, "foo(x, y)"));
        assert_eq!(editor.ghost_text().as_deref(), Some("x, y"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_a_word_then_each_line_of_a_suggestion_are_taken() {
        let (mut editor, path) = typing("parts", "let");
        editor.on_key(&press(KeyCode::Char('\\'), Mods::CTRL));
        let ticket = asked(&mut editor).request.ticket;
        let answer = ghost(0, 3, "let total = sum(xs);\nprint(total)");
        editor.take_inline_answer(ticket, &answer);
        let lines = |editor: &TextEditor| session::with(|s| s.lines(editor.document));
        editor.on_key(&press(KeyCode::Char('f'), Mods::ALT));
        assert_eq!(lines(&editor), ["let total"]);
        editor.on_key(&press(KeyCode::Char('.'), Mods::CTRL));
        assert_eq!(lines(&editor), ["let total = sum(xs);"]);
        editor.on_key(&press(KeyCode::Char('.'), Mods::CTRL));
        assert_eq!(lines(&editor), ["let total = sum(xs);", "print(total)"]);
        assert!(editor.inline.ghost.is_none(), "all of it taken");
        let calls = editor.take_inline_calls();
        assert_eq!(
            calls.last(),
            Some(&told(InlineKind::Ghost, InlineCall::Taken))
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_a_next_edit_is_made_by_tab_only_while_the_text_is_unchanged() {
        let (mut editor, path) = typing("edit", "foo baz");
        let range = TextRange::new(TextPosition::new(0, 0), TextPosition::new(0, 3));
        let answer = InlineAnswer::Edit(InlineEdit::new(range, "bar", "r1"));
        let lines = |editor: &TextEditor| session::with(|s| s.lines(editor.document));
        editor.ask_inline(InlineKind::Edit, false);
        let ticket = asked(&mut editor).request.ticket;
        editor.take_inline_answer(ticket, &answer);
        let card = editor.next_edit_card().expect("a card");
        assert!(card.contains("\n- foo\n+ bar"), "{card}");
        editor.on_key(&press(KeyCode::Tab, Mods::NONE));
        assert_eq!(lines(&editor), ["bar baz"]);
        assert_eq!(
            editor.take_inline_calls(),
            [
                told(InlineKind::Edit, InlineCall::Shown),
                told(InlineKind::Edit, InlineCall::Taken)
            ]
        );
        editor.ask_inline(InlineKind::Edit, false);
        let ticket = asked(&mut editor).request.ticket;
        editor.take_inline_answer(ticket, &answer);
        editor.on_key(&press(KeyCode::Char('!'), Mods::NONE));
        assert!(
            editor.next_edit_card().is_none(),
            "stale once the text changed"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_an_ask_hands_the_text_to_the_source_s_servers_first() {
        let dir = std::env::temp_dir().join(format!("winter-inline-served-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a folder");
        let (port, sent) = Port::fake("copilot", &dir, json!({}));
        let (mut editor, path) = typing_served("served", "fo", vec![port.id()]);
        editor.connect_lsp(1, &[port]);
        let methods = || -> Vec<String> {
            sent.try_iter()
                .filter_map(|message| message["method"].as_str().map(str::to_string))
                .collect()
        };
        editor.on_key(&press(KeyCode::Char('\\'), Mods::CTRL));
        assert_eq!(asked(&mut editor).request.version, Some(1));
        assert_eq!(methods(), ["textDocument/didOpen"]);
        editor.on_key(&press(KeyCode::Char('o'), Mods::NONE));
        editor.on_key(&press(KeyCode::Char('\\'), Mods::CTRL));
        assert_eq!(asked(&mut editor).request.version, Some(2));
        assert_eq!(methods(), ["textDocument/didChange"]);
        let _ = std::fs::remove_file(path);
    }
}
