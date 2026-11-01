//! Inline suggestions: the text an AI expects next, offered at the cursor
//! as you type (ghost text, see [`ghost`]) or, after a change, as an edit
//! somewhere else (a next edit, see [`next_edit`]).
//!
//! Where they come from is a source's business (see [`InlineSource`]):
//! [`extension`] asks an extension's source, one for each the window says
//! there is, as the Copilot extension's. Everything else is the same for
//! every source, and lives here: when to ask (once typing
//! pauses), which source to ask (the first one offering what is wanted),
//! answers that no longer fit the cursor put away, the keys that take a
//! suggestion, and how it is drawn.
//!
//! A new kind of source is one file implementing [`InlineSource`]: it says
//! what it offers, asks in its own way, and hands back an [`Answer`] in the
//! editor's own terms (characters on a row), at once or when its reply
//! comes in. An assistant is an extension's source, not a new kind.

mod extension;
mod ghost;
mod next_edit;

pub use self::extension::{InlineAsk, InlineCall, InlineTold};

use std::fmt::Debug;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::settings::EditorSettings;
use winter_ext_api::InlineAnswer;
use winter_lsp::Incoming;
use winter_model::editor::{EditorOutcome, InfoCard, Notice};
use winter_model::input::{Key, KeyCode, Mods};

use self::extension::ExtensionSource;
use self::ghost::Ghost;
use self::next_edit::Pending;
use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// How long after an edit a source is asked, so a burst of typing is one
/// question: Zed's 75ms.
const DEBOUNCE: Duration = Duration::from_millis(75);

// ========================================================================
// Data Structures
// ========================================================================

/// The two kinds of suggestion a source may offer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OfferKind {
    /// An edit somewhere else, expected after a change.
    Edit,
    /// Text at the cursor, drawn dim as you type.
    Ghost,
}

/// What to ask a source for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Want {
    Edit,
    /// Ghost text; `invoked` where `C-\` asked for it, rather than typing.
    Ghost {
        invoked: bool,
    },
}

/// One ghost text suggestion for the cursor's row: the characters from
/// column `start` up to column `end` replaced with `insert`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Suggestion {
    pub(super) end: usize,
    pub(super) insert: String,
    /// The source's own note of it, handed back when it shows and when it
    /// is taken; `Null` where the source keeps none.
    pub(super) receipt: Value,
    pub(super) start: usize,
}

/// The ghost text a source offers for row `row`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct GhostAnswer {
    /// Whether `C-\` asked for it: an empty answer then says so.
    pub(super) invoked: bool,
    pub(super) row: usize,
    pub(super) suggestions: Vec<Suggestion>,
}

/// An edit a source expects next: the text from (row, column) `start` up
/// to `end`, in characters, replaced with `text`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Edit {
    pub(super) end: (usize, usize),
    /// As a [`Suggestion`]'s.
    pub(super) receipt: Value,
    pub(super) start: (usize, usize),
    pub(super) text: String,
}

/// What a source answers.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Answer {
    Edit(Edit),
    Ghost(GhostAnswer),
    /// The source's, with nothing to show: an error, an acknowledgment.
    Nothing,
}

/// One editor's inline suggestions: where they may come from, what is on
/// screen, and when to ask next.
#[derive(Debug, Default)]
pub(super) struct Inline {
    /// What the extensions' sources are to be asked and told, for the
    /// window to hand on.
    calls: Vec<InlineCall>,
    /// When to ask next, and for what.
    due: Option<(Instant, Want)>,
    edit: Option<Pending>,
    ghost: Option<Ghost>,
    /// In the order they are asked: the first offering what is wanted is.
    sources: Vec<Box<dyn InlineSource>>,
}

// ========================================================================
// Traits
// ========================================================================

/// Somewhere inline suggestions come from. Each editor has its own of each,
/// so a source keeps what it has asked for in itself.
///
/// While one of these methods runs, the editor it is handed has no sources
/// of its own: a source reads the editor and its documents, and never
/// reaches for the other sources.
pub(super) trait InlineSource: Debug {
    /// What the status bar and notices call it.
    fn name(&self) -> &str;

    /// The full id of the extension's source it is, if it is one.
    fn extension_id(&self) -> Option<&str> {
        None
    }

    /// Take the settings, the source reading its own; says whether they
    /// changed, after which what it offered is put away unless it still
    /// [`offers`](Self::offers) that kind.
    fn configure(&mut self, settings: &EditorSettings) -> bool;

    /// Whether it would answer for `kind` in the document in front now:
    /// switched on, running, and willing to read this file.
    fn offers(&self, editor: &TextEditor, kind: OfferKind) -> bool;

    /// Ask for `want` at the cursor: the answer, where the source has it at
    /// once, or `None` for one that comes later, through [`hear`](Self::hear).
    fn ask(&mut self, editor: &mut TextEditor, want: Want) -> Option<Answer>;

    /// A language server's message: the answer to what was asked, or `None`
    /// where it is nothing of this source's.
    fn hear(&mut self, _editor: &mut TextEditor, _incoming: &Incoming) -> Option<Answer> {
        None
    }

    /// The answer an extension gave under `ticket`, where it is to what
    /// this source asked.
    fn answer(
        &mut self,
        _editor: &mut TextEditor,
        _ticket: u64,
        _answer: &InlineAnswer,
    ) -> Option<Answer> {
        None
    }

    /// The suggestion `receipt` is the note of now shows.
    fn shown(&mut self, _editor: &mut TextEditor, _kind: OfferKind, _receipt: &Value) {}

    /// The suggestion `receipt` is the note of was taken, all of it.
    fn taken(&mut self, _editor: &mut TextEditor, _kind: OfferKind, _receipt: &Value) {}
}

// ========================================================================
// TextEditor: inline suggestions
// ========================================================================

impl TextEditor {
    /// Hand every source the settings, and put away what a source switched
    /// off offered.
    pub(super) fn configure_inline(&mut self, settings: &EditorSettings) {
        for offered in &settings.inline {
            let known = self
                .inline
                .sources
                .iter()
                .any(|source| source.extension_id() == Some(offered.id.as_str()));
            if !known {
                let source = ExtensionSource::new(offered.clone());
                self.inline.sources.push(Box::new(source));
            }
        }
        let mut changed = Vec::new();
        for (at, source) in self.inline.sources.iter_mut().enumerate() {
            if source.configure(settings) {
                changed.push(at);
            }
        }
        for at in changed {
            let offers = |kind| self.inline.sources[at].offers(self, kind);
            let (ghost, edit) = (offers(OfferKind::Ghost), offers(OfferKind::Edit));
            if !ghost
                && self
                    .inline
                    .ghost
                    .as_ref()
                    .is_some_and(|shown| shown.source == at)
            {
                self.inline.ghost = None;
            }
            if !edit
                && self
                    .inline
                    .edit
                    .as_ref()
                    .is_some_and(|shown| shown.source == at)
            {
                self.inline.edit = None;
            }
        }
    }

    /// The first source offering `kind` here, by its place in the list.
    fn inline_source(&self, kind: OfferKind) -> Option<usize> {
        self.inline
            .sources
            .iter()
            .position(|source| source.offers(self, kind))
    }

    /// Run `act` on the source at `at`, handed the editor too: the source
    /// is taken out of the editor while it runs, and put back after.
    fn with_source<T>(
        &mut self,
        at: usize,
        act: impl FnOnce(&mut dyn InlineSource, &mut TextEditor) -> T,
    ) -> Option<T> {
        let mut sources = std::mem::take(&mut self.inline.sources);
        let outcome = sources.get_mut(at).map(|source| act(source.as_mut(), self));
        self.inline.sources = sources;
        outcome
    }

    /// What the source at `at` is called.
    fn source_name(&self, at: usize) -> String {
        self.inline
            .sources
            .get(at)
            .map_or_else(String::new, |source| source.name().to_string())
    }

    /// What the extensions' sources are to be asked and told since last
    /// taken, for the window to hand on to them.
    pub fn take_inline_calls(&mut self) -> Vec<InlineCall> {
        std::mem::take(&mut self.inline.calls)
    }

    /// Take the answer an extension gave under `ticket`, where one of this
    /// editor's sources asked for it. Says whether one had.
    pub fn take_inline_answer(&mut self, ticket: u64, answer: &InlineAnswer) -> bool {
        for at in 0..self.inline.sources.len() {
            let taken =
                self.with_source(at, |source, editor| source.answer(editor, ticket, answer));
            if let Some(Some(answer)) = taken {
                self.take_answer(at, answer);
                return true;
            }
        }
        false
    }

    /// After a key: an edit asks again once typing pauses, ghost text while
    /// typing and a next edit otherwise; moving off the ghost text's row
    /// puts it away, and an edit makes a next edit stale.
    pub(super) fn inline_after_key(&mut self, revision: usize) {
        if let Some(ghost) = &self.inline.ghost {
            if !self.modal.is_typing()
                || ghost.row != self.buffer().row()
                || self.ghost_text().is_none()
            {
                self.inline.ghost = None;
            }
        }
        if self.buffer().revision() == revision {
            return;
        }
        if self
            .inline
            .edit
            .as_ref()
            .is_some_and(|edit| edit.revision != self.buffer().revision())
        {
            self.inline.edit = None;
        }
        let want = match self.modal.is_typing() && self.inline_source(OfferKind::Ghost).is_some() {
            true => Want::Ghost { invoked: false },
            false => Want::Edit,
        };
        if want == Want::Edit && self.inline_source(OfferKind::Edit).is_none() {
            return;
        }
        self.inline.due = Some((Instant::now() + DEBOUNCE, want));
    }

    /// When a source is next to be asked.
    pub(super) fn inline_next_tick(&self) -> Option<Instant> {
        self.inline.due.map(|(at, _)| at)
    }

    /// Ask what is due to be asked.
    pub(super) fn inline_tick(&mut self, now: Instant) {
        let Some((at, want)) = self.inline.due else {
            return;
        };
        if at > now {
            return;
        }
        self.inline.due = None;
        self.ask_inline(want);
    }

    /// Ask the first source offering it for `want` at the cursor.
    fn ask_inline(&mut self, want: Want) {
        let kind = match want {
            Want::Edit => OfferKind::Edit,
            Want::Ghost { .. } => OfferKind::Ghost,
        };
        let Some(at) = self.inline_source(kind) else {
            return;
        };
        if let Some(Some(answer)) = self.with_source(at, |source, editor| source.ask(editor, want))
        {
            self.take_answer(at, answer);
        }
    }

    /// A language server's message, where it answers a source.
    pub(super) fn inline_hear(&mut self, incoming: &Incoming) -> Option<EditorOutcome> {
        for at in 0..self.inline.sources.len() {
            if let Some(Some(answer)) =
                self.with_source(at, |source, editor| source.hear(editor, incoming))
            {
                self.take_answer(at, answer);
                return Some(EditorOutcome::Consumed);
            }
        }
        None
    }

    /// Take what the source at `at` answered, where it still fits.
    fn take_answer(&mut self, at: usize, answer: Answer) {
        match answer {
            Answer::Edit(edit) => self.offer_edit(at, edit),
            Answer::Ghost(ghost) => self.offer_ghost(at, ghost),
            Answer::Nothing => {}
        }
    }

    /// Show the ghost text the source at `at` offered, where the cursor is
    /// still typing on its row; where none fits, say so if `C-\` asked, or
    /// ask for a next edit if not.
    fn offer_ghost(&mut self, at: usize, answer: GhostAnswer) {
        if !self.modal.is_typing() || self.buffer().row() != answer.row {
            return;
        }
        let suggestions: Vec<Suggestion> = answer
            .suggestions
            .into_iter()
            .filter(|suggestion| suggestion.start <= suggestion.end)
            .collect();
        self.inline.ghost = (!suggestions.is_empty()).then(|| Ghost {
            at: 0,
            items: suggestions,
            row: answer.row,
            source: at,
            told: Vec::new(),
        });
        if self.ghost_text().is_some() {
            self.tell_shown();
            return;
        }
        self.inline.ghost = None;
        match answer.invoked {
            true => {
                let notice = format!("{}: nothing to suggest here", self.source_name(at));
                self.modal.report(Notice::info(notice).unlogged());
            }
            false => self.ask_inline(Want::Edit),
        }
    }

    /// Tell the source of the ghost text on screen that it shows, once for
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
        let source = ghost.source;
        self.with_source(source, |source, editor| {
            source.shown(editor, OfferKind::Ghost, &receipt)
        });
    }

    /// Tell the source at `at` its suggestion was taken.
    fn tell_taken(&mut self, at: usize, kind: OfferKind, receipt: &Value) {
        self.with_source(at, |source, editor| source.taken(editor, kind, receipt));
    }

    /// A key the suggestions on screen give a meaning of their own: what it
    /// did, or `None` for a key left to the rest. `C-\` asks for ghost text
    /// where none came by itself.
    pub(super) fn inline_key(&mut self, key: &Key) -> Option<EditorOutcome> {
        if key.chord() == (Mods::CTRL, KeyCode::Char('\\'))
            && self.inline_source(OfferKind::Ghost).is_some()
        {
            self.ask_inline(Want::Ghost { invoked: true });
            return Some(EditorOutcome::Consumed);
        }
        if self.ghost_text().is_some() && self.modal.is_typing() {
            return self.ghost_key(key);
        }
        if self.inline.edit.is_some() {
            return self.next_edit_key(key);
        }
        None
    }

    /// The card under the cursor that shows a next edit. Ghost text never
    /// shows in one: its later lines are rows of their own under the
    /// cursor's (see `ghost_rows`), so while it shows there is no card.
    pub(super) fn inline_card(&self) -> Option<InfoCard> {
        if self.modal.is_typing() && self.ghost_text().is_some() {
            return None;
        }
        self.next_edit_card()
    }

    /// What the status bar says of the ghost text on screen: its source,
    /// and which of the suggestions offered shows.
    pub(super) fn inline_status(&self) -> Option<String> {
        let ghost = self
            .inline
            .ghost
            .as_ref()
            .filter(|_| self.ghost_text().is_some())?;
        let name = self.source_name(ghost.source);
        Some(match ghost.items.len() {
            1 => name.to_string(),
            n => format!("{name} {}/{n}", ghost.at + 1),
        })
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Where text put in at `row`, `col` ends.
fn end_of(row: usize, col: usize, text: &str) -> (usize, usize) {
    let pieces: Vec<&str> = text.split('\n').collect();
    let last = pieces.last().map_or(0, |piece| piece.chars().count());
    match pieces.len() {
        1 => (row, col + last),
        n => (row + n - 1, last),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;

    use serde_json::json;
    use winter_ext_api::{InlineEdit, InlineKind, InlineOffer, TextPosition, TextRange};
    use winter_model::editor::Page;

    use super::super::settings::InlineOffered;

    /// A source answering at once with `insert` over the cursor's line up
    /// to the cursor, noting what it is told in `log`.
    #[derive(Debug)]
    struct Fake {
        insert: String,
        log: Rc<RefCell<Vec<String>>>,
    }

    impl InlineSource for Fake {
        fn name(&self) -> &str {
            "Fake"
        }

        fn configure(&mut self, _settings: &EditorSettings) -> bool {
            false
        }

        fn offers(&self, _editor: &TextEditor, kind: OfferKind) -> bool {
            kind == OfferKind::Ghost
        }

        fn ask(&mut self, editor: &mut TextEditor, want: Want) -> Option<Answer> {
            let Want::Ghost { invoked } = want else {
                return None;
            };
            Some(Answer::Ghost(GhostAnswer {
                invoked,
                row: editor.buffer().row(),
                suggestions: vec![Suggestion {
                    end: editor.buffer().col(),
                    insert: self.insert.clone(),
                    receipt: json!("r1"),
                    start: 0,
                }],
            }))
        }

        fn shown(&mut self, _editor: &mut TextEditor, _kind: OfferKind, receipt: &Value) {
            self.log.borrow_mut().push(format!("shown {receipt}"));
        }

        fn taken(&mut self, _editor: &mut TextEditor, _kind: OfferKind, receipt: &Value) {
            self.log.borrow_mut().push(format!("taken {receipt}"));
        }
    }

    /// An editor over `text` typing at the end of its first line, asking
    /// only a [`Fake`] offering `insert`. `tag` names the file, so tests
    /// run side by side.
    fn with_fake(
        tag: &str,
        text: &str,
        insert: &str,
    ) -> (TextEditor, PathBuf, Rc<RefCell<Vec<String>>>) {
        let path =
            std::env::temp_dir().join(format!("winter-inline-{tag}-{}.txt", std::process::id()));
        std::fs::write(&path, text).expect("temp file");
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        editor.on_key(&key(KeyCode::Char('A'), false));
        let log = Rc::new(RefCell::new(Vec::new()));
        editor.inline.sources = vec![Box::new(Fake {
            insert: insert.to_string(),
            log: Rc::clone(&log),
        })];
        (editor, path, log)
    }

    fn key(code: KeyCode, ctrl: bool) -> Key {
        Key {
            alt: false,
            code,
            ctrl,
            shift: false,
            sup: false,
        }
    }

    #[test]
    fn test_any_source_s_ghost_text_shows_and_is_taken_with_the_source_told() {
        let (mut editor, path, log) = with_fake("fake", "fo", "foo(bar)");
        editor.on_key(&key(KeyCode::Char('\\'), true));
        assert_eq!(editor.ghost_text().as_deref(), Some("o(bar)"));
        assert_eq!(
            editor.inline_status().as_deref(),
            Some("Fake"),
            "named after its source"
        );
        editor.on_key(&key(KeyCode::Tab, false));
        assert_eq!(editor.buffer().lines()[0], "foo(bar)");
        assert_eq!(*log.borrow(), ["shown \"r1\"", "taken \"r1\""]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_an_extension_s_source_is_asked_through_the_window_and_its_answer_shows() {
        let (mut editor, path, _) = with_fake("extension", "fo", "unused");
        editor.inline.sources.clear();
        let offered = InlineOffered {
            id: "pilot.ghost".to_string(),
            offer: InlineOffer::new(true, false),
            servers: Vec::new(),
            title: "Pilot".to_string(),
        };
        let settings = EditorSettings {
            inline: vec![offered],
            ..EditorSettings::default()
        };
        editor.configure_inline(&settings);
        editor.on_key(&key(KeyCode::Char('\\'), true));
        let calls = editor.take_inline_calls();
        let [InlineCall::Ask(asked)] = calls.as_slice() else {
            panic!("one ask: {calls:?}");
        };
        let asked = asked.clone();
        assert_eq!(asked.source, "pilot.ghost");
        assert!(asked.request.invoked);
        assert_eq!(asked.request.at, TextPosition::new(0, 2));
        let at = asked.request.at;
        let ghost = InlineEdit::new(TextRange::new(TextPosition::new(0, 0), at), "foo()", "r9");
        let answer = InlineAnswer::Ghost(vec![ghost]);
        assert!(
            !editor.take_inline_answer(asked.request.ticket + 1, &answer),
            "not its ticket"
        );
        assert!(editor.take_inline_answer(asked.request.ticket, &answer));
        assert_eq!(editor.ghost_text().as_deref(), Some("o()"));
        assert_eq!(editor.inline_status().as_deref(), Some("Pilot"));
        editor.on_key(&key(KeyCode::Tab, false));
        assert_eq!(editor.buffer().lines()[0], "foo()");
        let told: Vec<InlineCall> = editor.take_inline_calls();
        let told_of = |kind: fn(InlineTold) -> InlineCall| {
            kind(InlineTold {
                kind: InlineKind::Ghost,
                receipt: "r9".to_string(),
                source: "pilot.ghost".to_string(),
            })
        };
        assert_eq!(
            told,
            [told_of(InlineCall::Shown), told_of(InlineCall::Taken)]
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_an_answer_for_a_row_the_cursor_has_left_is_put_away() {
        let (mut editor, path, log) = with_fake("left", "fo\nbar", "foo");
        let answer = GhostAnswer {
            invoked: false,
            row: 1,
            suggestions: vec![Suggestion {
                end: 0,
                insert: "barx".to_string(),
                receipt: Value::Null,
                start: 0,
            }],
        };
        editor.take_answer(0, Answer::Ghost(answer));
        assert_eq!(editor.ghost_text(), None);
        assert!(log.borrow().is_empty(), "nothing shown, nothing told");
        let _ = std::fs::remove_file(path);
    }
}
