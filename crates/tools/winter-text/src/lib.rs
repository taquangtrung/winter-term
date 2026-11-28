//! The text editor tab: a view of a file open in Cherry, the editor every
//! text tab shares ([`session`]), drawn by Cherry's `EditorView` in
//! its pane. Cherry owns the text, the cursor, the modes, and every editing
//! key; the tab reads the file in, writes it out, and carries what the
//! editor asks of the window (a save, a close, a notice, a file to open)
//! to the window as an [`EditorOutcome`].
//!
//! Keys come through the window's dispatch, so the window's own sequences
//! run first; the pointer and an input method's text come to the view
//! straight from misty ([`TextEditor::handle_input`]).

#![deny(unsafe_code)]
#![deny(missing_docs)]

use std::any::Any;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cherry::markup::Format;
use cherry::misty::keys::to_cherry;
use cherry::misty::{ChangeNotes, EditorResponse, EditorView, clock};
use cherry::{CommandError, DocumentId, Handled, HostRequest, Mode, ViewId};
use misty::{EventCx, Font, Frame, Input, Rect, Theme};
use winter_base::edit_wait::EditWaiter;
use winter_base::watch::FileWatch;
use winter_field::misty_key;
use winter_lsp::diagnostics;
use winter_lsp::protocol::TextEdit;
use winter_lsp::{Event, Incoming, Port};
use winter_model::editor::EditPoint;
use winter_model::editor::{
    Editor, EditorCommand, EditorMenuItem, EditorOutcome, EditorRequest, EditorStatus, FILE_REVERT,
    FileEdit, JobReply, JumpStep, JumpTarget, Notice, OpenTarget, Page, PromptReply, PromptRequest,
    SAVE_ALL, ScrollPosition, SharedText, TOGGLE_SWATCHES, TargetStyles, menu_groups,
};
use winter_model::input::{Key, KeyContext};
use winter_model::modal::EditMode;
use winter_model::paint::{Frame as PaintFrame, IconKind, Row, WrapIndent};

use session::DiskChange;

mod completing;
mod conflict;
pub mod diff;
mod git_changes;
mod git_files;
mod hints;
mod inline;
mod jumps;
mod keys;
mod language;
mod markup;
mod notice;
mod requests;
mod servers;
mod session;
pub mod settings;
mod spelling;
mod swatch;
mod symbols;
pub mod syntax;

pub use cherry::misty::HighlightStyle;
pub use inline::{InlineAsk, InlineCall, InlineTold};
pub use keys::{KEY_TABLES, is_cherry_block, set_user_keys};
pub use language::known_languages;
use notice::{FileFailure, TextNotice};
pub use session::save_through;
pub use settings::history_dir;

// ========================================================================
// Constants
// ========================================================================

/// The text editor's commands the palette runs by name: none of Winter's
/// own, as Cherry runs every editing command.
pub const COMMANDS: &[EditorCommand] = &[];

/// The question asked before a revert drops unsaved edits.
const ASK_REVERT: &str = "text-revert";

/// What a tab's title ends with while its file has edits not written.
pub const DIRTY_MARK: &str = " [+]";

/// The right-click menu's entries, as the keys Cherry binds them to and
/// their labels: over a selection, and elsewhere.
const SELECTION_MENU: [(&str, &str); 2] = [("y", "Copy"), ("d", "Cut")];
const TEXT_MENU: [(&str, &str); 3] = [("p", "Paste"), ("u", "Undo"), ("C-r", "Redo")];

/// How long typing pauses before a language server is handed the text.
const FLUSH_DELAY: Duration = Duration::from_millis(150);

/// What each level of an outline entry is indented by, below the top.
const OUTLINE_INDENT: &str = "  ";

/// How many characters of an outline entry's title its label shows.
const OUTLINE_TITLE_CHARS: usize = 100;

/// What a change bar in the gutter says under the pointer, of each kind.
const CHANGE_ADDED: &str = "Added since the last commit";
const CHANGE_DELETED: &str = "Lines deleted here since the last commit";
const CHANGE_MODIFIED: &str = "Changed since the last commit";

// ========================================================================
// Data Structures
// ========================================================================

/// A text tab: one view of a file open in the shared editor.
#[derive(Debug)]
pub struct TextEditor {
    /// Whether the tab is to look at its file on disk on its next tick: shown
    /// again, it may have changed meanwhile.
    disk_due: bool,
    /// The watch on its file while the tab is shown, which says when another
    /// program wrote it.
    disk_watch: Option<FileWatch>,
    /// The document shown, the file's.
    document: DocumentId,
    /// Whether the tab was closed by finishing what git handed it (`C-c
    /// C-c`, `ZZ`), rather than by the window: an unfinished plan aborts
    /// its rebase.
    finished: bool,
    /// Whether a save has the language server format the file first.
    format_on_save: bool,
    /// Whether the keyboard coming to the tab puts it in Normal mode: from
    /// the start for a file, and for a commit message (which opens in
    /// Insert, ready to type) once it has had the keyboard the first time.
    focused_before: bool,
    /// The language server's inlay hints and lenses in the text.
    hints: hints::Hints,
    /// What the language server says the file defines.
    symbols: symbols::Symbols,
    /// The extensions' inline suggestions at the cursor.
    inline: inline::Inline,
    /// What the tab is waiting on its language server for.
    lsp: requests::Lsp,
    /// Where the cursor was before Scout's preview moved it, to go back to
    /// should the jump be called off.
    jump_origin: Option<usize>,
    /// Where each line Scout's colors have walked to begins, and the text's
    /// version it was walked in.
    jump_walk: (Option<u64>, Vec<cherry::syntax::LineStart>),
    /// The checkers' publications last laid on the text, by their store's
    /// generation.
    problems_seen: Option<u64>,
    /// The language each extension settings add is written in.
    languages: BTreeMap<String, String>,
    /// Where recalling earlier commit messages has got to.
    recall: git_files::MessageRecall,
    /// A notice for the window, not yet taken.
    notice: Option<TextNotice>,
    /// Where the file is.
    path: PathBuf,
    /// Whether a pane shows the tab: only then does it look at the disk on
    /// a timer, and it looks at once when shown again.
    shown: bool,
    /// The view the tab in front shows, drawn in its pane; `None` while a
    /// tab on this buffer is between keeping its view and handing the
    /// next one in (see [`Editor::save_view`]).
    view: RefCell<Option<EditorView>>,
    /// A program blocked on this edit (`git commit` waiting on its
    /// message), answered when the tab goes.
    waiter: Option<EditWaiter>,
}

/// The view of a tab gone behind another on the same buffer, kept in the
/// window until the tab comes in front again; closed in the editor when
/// the tab goes for good.
#[derive(Debug)]
struct KeptView {
    /// The document it shows.
    document: DocumentId,
    /// The view, until it is shown again.
    view: Option<EditorView>,
}

/// What the status bar says of a text editor, before it is written out.
#[derive(Debug)]
struct StatusParts {
    /// The cursor's column, from 1.
    column: usize,
    /// The source of the ghost text on screen, and which of its offers.
    ghost: Option<String>,
    /// The cursor's line, from 1.
    line: usize,
    /// How many lines the file has.
    lines: usize,
    /// The file's errors and warnings, while a server serves it and it has
    /// any.
    problems: Option<(usize, usize)>,
}

// ========================================================================
// TextEditor Implementations
// ========================================================================

impl TextEditor {
    /// What the status bar says of this tab; `None` while its view is not
    /// open.
    fn status_parts(&self) -> Option<StatusParts> {
        session::with(|session| {
            let cursor = session.editor.view(self.view_id())?.cursor();
            let text = session.editor.document(self.document)?;
            let at = text.char_to_position(cursor)?;
            Some(StatusParts {
                column: at.column + 1,
                ghost: self.inline_status(),
                line: at.line + 1,
                lines: text.len_lines(),
                problems: session.problem_counts(self.document),
            })
        })
    }

    /// A tab on `path`'s contents, on the document another tab has it open
    /// in if one has, landing on `line` (from 1) when whatever asked for the
    /// file knew which one mattered.
    pub fn new(path: PathBuf, line: Option<usize>) -> io::Result<Self> {
        let (document, view) = session::with(|session| session.open_view(&path))?;
        let mut editor = Self {
            disk_due: false,
            disk_watch: None,
            document,
            format_on_save: false,
            jump_origin: None,
            hints: hints::Hints::default(),
            symbols: symbols::Symbols::default(),
            inline: inline::Inline::default(),
            lsp: requests::Lsp::default(),
            jump_walk: (None, Vec::new()),
            languages: BTreeMap::new(),
            problems_seen: None,
            notice: None,
            recall: git_files::MessageRecall::default(),
            path,
            shown: false,
            view: RefCell::new(Some(view_of(view))),
            waiter: None,
            finished: false,
            focused_before: false,
        };
        if let Some(line) = line {
            editor.move_to(line.saturating_sub(1), 0);
        }
        if editor.holds_message() {
            let view = editor.view_id();
            session::with(|session| {
                let _ = session.editor.run_command(view, "enter-insert", None);
            });
        }
        editor.focused_before = !editor.holds_message();
        Ok(editor)
    }

    /// A tab on `target`'s file at its line and column (both from 1).
    pub fn at(target: OpenTarget) -> io::Result<Self> {
        let column = target.column;
        let mut editor = Self::new(target.path, target.line)?;
        if let (Some(line), Some(column)) = (target.line, column) {
            editor.move_to(line.saturating_sub(1), column.saturating_sub(1));
        }
        Ok(editor)
    }

    /// [`Self::new`] for a saved session coming back.
    pub fn restored(path: PathBuf, line: Option<usize>) -> io::Result<Self> {
        Self::new(path, line)
    }

    /// Takes the language servers running as of `generation`, handing the
    /// file to the one serving it.
    pub fn connect_lsp(&mut self, generation: u64, ports: &[Port]) {
        let language = self.language();
        session::with(|session| {
            session.connect(generation, ports);
            session.sync(self.document, &language);
        });
    }

    /// Acts on what a language server said: new problems in this tab's
    /// file, or edits to make to it.
    pub fn on_lsp(&mut self, incoming: &Incoming) -> EditorOutcome {
        match &incoming.event {
            Event::Diagnostics(path) if self.holds_file(path) => {
                match session::with(|session| session.refresh_problems(self.document)) {
                    true => EditorOutcome::Consumed,
                    false => EditorOutcome::Ignored,
                }
            }
            Event::Answer {
                id,
                result: Ok(answer),
            } if session::with(|session| {
                session.answer_completion(incoming.server, *id, answer)
            }) =>
            {
                EditorOutcome::Consumed
            }
            Event::Answer { id, result } => self
                .on_lsp_answer(incoming.server, *id, result)
                .unwrap_or(EditorOutcome::Ignored),
            Event::ApplyEdit(edits) => {
                let ours: Vec<FileEdit> = edits
                    .iter()
                    .filter(|(path, _)| self.holds_file(path))
                    .flat_map(|(_, edits)| edits.iter().map(file_edit))
                    .collect();
                match !ours.is_empty() && self.apply_text_edits(&ours) {
                    true => EditorOutcome::Consumed,
                    false => EditorOutcome::Ignored,
                }
            }
            _ => EditorOutcome::Ignored,
        }
    }

    /// Sends the completion asks Cherry noted while a key was handled, the
    /// text handed over first.
    fn send_asks(&mut self) {
        let language = self.language();
        session::with(|session| {
            let pending = session
                .files
                .get(&self.document)
                .is_some_and(|file| !file.asks.lock().map_or(true, |asks| asks.is_empty()));
            if !pending {
                return;
            }
            session.sync(self.document, &language);
            if let Some(port) = session.serving(self.document, &language).cloned() {
                session.send_asks(self.document, &port);
            }
        });
    }

    /// The language the file is written in, as settings name it.
    fn language(&self) -> String {
        let detected = language::language_id(&self.path);
        language::language_of(&self.path, detected, &self.languages)
    }

    /// Has `waiter` answered when this tab goes, which is what lets a
    /// program that launched the editor and blocked on it carry on.
    pub fn with_waiter(mut self, waiter: EditWaiter) -> Self {
        self.waiter = Some(waiter);
        self
    }

    /// Whether this tab edits the message of a commit about to be made,
    /// the one message worth showing the staged changes beside.
    pub fn edits_commit(&self) -> bool {
        language::is_commit_message(&self.path)
    }

    /// The language the file is written in, as settings name it, and where
    /// it is: what a language server is started for.
    pub fn lsp_subject(&self) -> Option<(String, PathBuf)> {
        Some((self.language(), self.path.clone()))
    }

    /// Follows `settings`: the editor's options, and the languages settings
    /// add.
    pub fn configure(&mut self, settings: &settings::EditorSettings) {
        session::with(|session| {
            let options = session.editor.options();
            let wanted = cherry::EditorOptions {
                auto_complete: settings.completion.auto,
                conceal: markup::conceal_mode(settings.markup_markers),
                indent_guides: settings.indent_guides,
                screen_row_motions: settings.screen_row_motions,
                break_indent: break_indent(settings.wrap_indent),
                wrap_mode: match settings.wrap_words {
                    true => cherry::WrapMode::Words,
                    false => cherry::WrapMode::Chars,
                },
                ..*options
            };
            if wanted != *options {
                let options = wanted;
                session.editor.set_options(session::options(&options));
            }
        });
        syntax::set_todo_keywords(&settings.todo_keywords);
        self.languages.clone_from(&settings.languages);
        self.format_on_save = settings.format_on_save;
        self.configure_inline(settings);
    }

    /// Makes `edits` (a language server's, in its UTF-16 columns) to the
    /// file, as one change. Whether the tab took them.
    pub fn apply_text_edits(&mut self, edits: &[FileEdit]) -> bool {
        let view = self.view_id();
        session::with(|session| {
            let Some(text) = session.editor.document(self.document) else {
                return false;
            };
            let offset = |point: EditPoint| {
                let position = cherry::Utf16Position {
                    column: point.column as usize,
                    line: point.line as usize,
                };
                text.utf16_position_to_char(position)
                    .unwrap_or_else(|| text.len_chars())
            };
            let mut changes: Vec<cherry::Edit> = edits
                .iter()
                .map(|edit| cherry::Edit::new(offset(edit.start)..offset(edit.end), &edit.new_text))
                .collect();
            changes.sort_by_key(|edit| edit.range.start);
            session.editor.apply_edits(view, changes);
            true
        })
    }

    /// The file's text for a view following it (a Markdown preview) when
    /// this tab holds `path`: the lines left out where the asker knows the
    /// revision already.
    pub fn shared_text(&self, path: &Path, known: Option<usize>) -> Option<SharedText> {
        if !self.holds_file(path) {
            return None;
        }
        session::with(|session| {
            let revision = session.editor.document_version(self.document)? as usize;
            let cursor = session.editor.view(self.view_id())?.cursor();
            let line = session
                .editor
                .document(self.document)?
                .char_to_position(cursor)
                .map_or(0, |at| at.line);
            Some(SharedText {
                cursor: line,
                lines: (known != Some(revision)).then(|| session.lines(self.document)),
                revision,
            })
        })
    }

    /// Asks for the next frame of a scroll's glide still under way,
    /// returning when it is due; `None` at rest. The window ticks the
    /// editor itself, so this alone keeps the glide moving.
    pub fn glide(&self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.with_view(|view| view.glide(cx))
    }

    /// Draws the view in `area` of `frame` in `font`, the highlighter's
    /// styles drawn as `styles` gives them, by number, and the conflict
    /// markers' band and the color swatches' numbered past them; its cursor
    /// hollow unless `focused`.
    pub fn draw(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        font: &Font,
        mut styles: Vec<HighlightStyle>,
        focused: bool,
    ) {
        let band = styles.len();
        styles.push(conflict::band_style(theme.raised));
        session::with(|session| {
            session.git_shown(self.document);
            session.conceal_markup(self.document);
            session.check_spelling(self.document);
            session.mark_summary(self.document);
            session.lay_conflicts(self.document, band);
            let swatches = session.lay_swatches(self.document, styles.len());
            styles.extend(swatches);
        });
        let generation = diagnostics::generation();
        if self.problems_seen != Some(generation) {
            self.problems_seen = Some(generation);
            session::with(|session| session.refresh_problems(self.document));
        }
        self.with_view(|view| {
            view.set_font(Some(font.clone()));
            view.set_highlight_styles(styles);
            view.set_focused(focused);
            session::with(|session| {
                use misty::Widget;
                view.lend(&mut session.editor).draw(frame, area, theme);
            });
        });
        if let Some(cursor) = self.with_view(|view| view.cursor_rect())
            && !self.draw_card(frame, cursor, area, theme)
        {
            self.draw_next_edit(frame, cursor, area, theme);
        }
    }

    /// Hands the view `input` from the pointer or an input method: a press
    /// moves the cursor, a drag selects, the wheel scrolls, composed text is
    /// typed. What it asks of the window.
    pub fn handle_input(&mut self, input: &Input, cx: &mut EventCx<'_>) -> EditorOutcome {
        let before = self.version();
        let response = self.with_view(|view| {
            session::with(|session| {
                use misty::Widget;
                view.lend(&mut session.editor).handle(input, cx)
            })
        });
        self.send_asks();
        self.inline_after_key(before);
        match response {
            EditorResponse::Handled => EditorOutcome::Consumed,
            EditorResponse::Ignored => EditorOutcome::Ignored,
            EditorResponse::Requests(requests) => self.carry_out(requests),
        }
    }

    /// The input method's area: the cursor's cell while keys type text.
    pub fn ime_area(&self) -> Option<Rect> {
        self.with_view(|view| session::with(|session| view.ime_area(&session.editor)))
    }

    /// `act` on the view the tab in front shows, a new view of the file
    /// for a tab that has none of its own yet.
    fn with_view<R>(&self, act: impl FnOnce(&mut EditorView) -> R) -> R {
        let mut slot = self.view.borrow_mut();
        let view = slot.get_or_insert_with(|| {
            view_of(session::with(|session| session.add_view(self.document)))
        });
        act(view)
    }

    /// The Cherry view the tab in front shows.
    fn view_id(&self) -> ViewId {
        self.with_view(|view| view.view())
    }

    /// Puts the cursor on `line` and `column` (both from 0, the column in
    /// chars), each clamped to the text.
    fn move_to(&mut self, line: usize, column: usize) {
        let view = self.view_id();
        session::with(|session| {
            let offset = session.offset_of(self.document, line, column);
            session.editor.set_cursor(view, offset);
        });
    }

    /// The lines of the file.
    fn lines(&self) -> Vec<String> {
        session::with(|session| session.lines(self.document))
    }

    /// Replaces `count` lines from line `first` (as many as there are, for
    /// a count past the end) with `text`, as one edit.
    fn replace_lines(&mut self, first: usize, count: usize, text: &str) {
        let view = self.view_id();
        session::with(|session| {
            let Some(document) = session.editor.document(self.document) else {
                return;
            };
            let total = document.len_lines();
            let length = document.len_chars();
            let start = session.offset_of(self.document, first, 0);
            let end = match first.saturating_add(count) < total {
                true => session.offset_of(self.document, first + count, 0),
                false => length,
            };
            let text = match end < length {
                true => format!("{text}\n"),
                false => text.to_string(),
            };
            session
                .editor
                .apply_edits(view, vec![cherry::Edit::new(start..end, text)]);
        });
    }

    /// The cursor's line and column, both from 0, the column in chars.
    fn cursor_line_column(&self) -> (usize, usize) {
        let view = self.view_id();
        session::with(|session| {
            let cursor = session.editor.view(view)?.cursor();
            let at = session
                .editor
                .document(self.document)?
                .char_to_position(cursor)?;
            Some((at.line, at.column))
        })
        .unwrap_or_default()
    }

    /// Carries out what the editor asked, oldest first: what the window is
    /// to do, the last request's that asks anything of it.
    fn carry_out(&mut self, requests: Vec<HostRequest>) -> EditorOutcome {
        let mut outcome = EditorOutcome::Consumed;
        for request in requests {
            match request {
                HostRequest::Notice(notice) => {
                    self.notice = Some(TextNotice::Cherry(notice));
                }
                HostRequest::Open(open) => {
                    if let Some(path) = open.path {
                        let target = OpenTarget {
                            column: None,
                            line: None,
                            path: self.relative(&path),
                        };
                        outcome = EditorOutcome::OpenPath(target);
                    }
                }
                HostRequest::Command(command) => {
                    let id = command.command.as_str();
                    let done = self
                        .run_git_command(id)
                        .or_else(|| self.run_conflict_command(id));
                    if let Some(done) = done {
                        outcome = done;
                    }
                }
                HostRequest::Quit(quit) if quit.force && self.holds_message() => {
                    outcome = self.cancel_message().unwrap_or(EditorOutcome::Close);
                }
                HostRequest::Quit(quit) => {
                    if quit.force || !self.is_dirty() {
                        // `:q` on what was saved carries it out, as Vim
                        // leaves it to git; a forced quit does not.
                        self.finished = !quit.force;
                        outcome = EditorOutcome::Close;
                    } else {
                        self.notice = Some(TextNotice::Unsaved);
                    }
                }
                HostRequest::ReadInto(read) => self.read_into(read.line, read.path),
                HostRequest::Save(_) => {
                    self.leave_insert();
                    outcome = self.format_before_save().unwrap_or_else(|| self.write());
                }
                _ => {}
            }
        }
        outcome
    }

    /// Back to Normal mode from Insert, as a save asked for by a key or a
    /// command leaves the editor; nothing in any other mode. Before the
    /// write, so what leaving Insert tidies (an empty line's auto-indent)
    /// is what is written.
    fn leave_insert(&self) {
        let view = self.view_id();
        session::with(|session| {
            let inserting = session
                .editor
                .view(view)
                .is_some_and(|view| view.mode() == Mode::Insert);
            if inserting {
                let _ = session.editor.run_command(view, "exit-insert", None);
            }
        });
    }

    /// Writes the file: where it went, or why it could not.
    fn write(&mut self) -> EditorOutcome {
        match session::with(|session| session.save(self.document)) {
            Ok(path) => EditorOutcome::Wrote(path),
            Err(error) => {
                self.notice = Some(TextNotice::WriteFailed(error.to_string()));
                EditorOutcome::Consumed
            }
        }
    }

    /// Reads the file again for `File: Revert`, asking first where that
    /// drops unsaved edits.
    fn ask_revert(&mut self) -> EditorOutcome {
        if !self.is_dirty() {
            self.revert();
            return EditorOutcome::Consumed;
        }
        let question = format!(
            "{} has unsaved edits. Reverting drops them; an undo brings them back.",
            name_of(&self.path)
        );
        EditorOutcome::Prompt(
            PromptRequest::confirm(ASK_REVERT, &question)
                .titled("Discard Unsaved Changes?", "Revert")
                .destructive(),
        )
    }

    /// Reads the file again over the text, saying why where it cannot.
    fn revert(&mut self) {
        if let Err(error) = session::with(|session| session.read_again(self.document)) {
            self.notice = Some(TextNotice::ReadFailed(FileFailure {
                error: error.to_string(),
                path: self.path.clone(),
            }));
        }
    }

    /// Looks at the file on disk (see [`session::Session::check_disk`]);
    /// says what the user should know.
    fn check_disk(&mut self) {
        let name = name_of(&self.path);
        match session::with(|session| session.check_disk(self.document)) {
            DiskChange::ChangedUnderEdits => {
                self.notice = Some(TextNotice::ChangedOnDisk(name));
            }
            DiskChange::Gone => {
                self.notice = Some(TextNotice::DeletedOnDisk(name));
            }
            DiskChange::None | DiskChange::Reloaded => {}
        }
    }

    /// When the tab next looks at its file on disk: at once when it is due,
    /// else when the watch says it changed, never on a timer.
    fn disk_next_look(&self) -> Option<Instant> {
        let now = Instant::now();
        match self.disk_due {
            true => Some(now),
            false => self.disk_watch.as_ref()?.next_look(now),
        }
    }

    /// Puts the lines of the file at `path` (this tab's own when `None`)
    /// below line `line` (from 1; 0 above the first), as `:r` asks.
    fn read_into(&mut self, line: usize, path: Option<String>) {
        let path = path.map_or_else(|| self.path.clone(), |path| self.relative(&path));
        match winter_model::text_file::load(&path) {
            Ok(loaded) => {
                let view = self.view_id();
                let text = loaded.lines.join("\n");
                session::with(|session| session.editor.insert_lines(view, line, &text));
            }
            Err(error) => {
                self.notice = Some(TextNotice::ReadFailed(FileFailure {
                    error: error.to_string(),
                    path,
                }));
            }
        }
    }

    /// `typed` as a path, from the file's folder when it is not absolute.
    fn relative(&self, typed: &str) -> PathBuf {
        let typed = Path::new(typed);
        match (typed.is_absolute(), self.path.parent()) {
            (false, Some(folder)) => folder.join(typed),
            _ => typed.to_path_buf(),
        }
    }

    /// The view's mode, as the status bar names it.
    fn mode(&self) -> Option<EditMode> {
        let view = self.view_id();
        session::with(|session| {
            session
                .editor
                .view(view)
                .map(|view| edit_mode_of(view.mode()))
        })
    }
}

impl Drop for KeptView {
    fn drop(&mut self) {
        if let Some(view) = self.view.take() {
            session::with(|session| session.close_view(self.document, view.view()));
        }
    }
}

impl Page for TextEditor {
    /// Shown, the editor watches its file, and looks at it at once: it may
    /// have changed while out of sight. Not shown, it lets the watch go.
    fn set_shown(&mut self, shown: bool) -> EditorOutcome {
        match (shown, self.shown) {
            (true, false) => {
                self.disk_watch = FileWatch::start(&self.path);
                self.disk_due = true;
            }
            (false, _) => self.disk_watch = None,
            (true, true) => {}
        }
        self.shown = shown;
        EditorOutcome::Consumed
    }

    fn title(&self) -> String {
        name_of(&self.path)
    }

    fn icon(&self) -> Option<IconKind> {
        Some(IconKind::File {
            name: name_of(&self.path),
        })
    }

    /// Nothing: the view is drawn in the pane by [`TextEditor::draw`].
    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> PaintFrame {
        PaintFrame::new(vec![Row::new()])
    }

    /// A key closes the card at the cursor first; Escape does nothing else.
    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        if self.close_card() && key.code == winter_model::input::KeyCode::Escape {
            return EditorOutcome::Consumed;
        }
        if let Some(outcome) = self.inline_key(key) {
            return outcome;
        }
        let Some(cherry_key) = to_cherry(misty_key(key)) else {
            return EditorOutcome::Ignored;
        };
        let before = self.version();
        let view = self.view_id();
        let (handled, requests) = session::with(|session| {
            let handled = session.editor.handle_key(view, cherry_key, clock::now());
            (handled, session.editor.take_requests())
        });
        self.send_asks();
        self.inline_after_key(before);
        match (handled, requests.is_empty()) {
            (Handled::Ignored, true) => EditorOutcome::Ignored,
            (_, true) => EditorOutcome::Consumed,
            (_, false) => self.carry_out(requests),
        }
    }

    /// Writes the file for the window's [`SAVE_ALL`] (a build about to
    /// start); turns the color swatches off or on for [`TOGGLE_SWATCHES`];
    /// runs Cherry's command of any other name, its words joined by
    /// hyphens rather than underscores (`upcase_word` runs `upcase-word`).
    /// `None` for a command Cherry does not have.
    fn run_command(&mut self, name: &str) -> Option<EditorOutcome> {
        if name == SAVE_ALL {
            return Some(self.write());
        }
        if name == FILE_REVERT {
            return Some(self.ask_revert());
        }
        if name == TOGGLE_SWATCHES {
            let on = session::with(|session| session.toggle_swatches());
            self.notice = Some(TextNotice::Swatches(on));
            return Some(EditorOutcome::Consumed);
        }
        if let Some(outcome) = self.run_lsp_command(name) {
            return Some(outcome);
        }
        let id = name.replace('_', "-");
        let view = self.view_id();
        let (ran, requests) = session::with(|session| {
            let ran = session.editor.run_command(view, &id, None);
            (ran, session.editor.take_requests())
        });
        match ran {
            Ok(()) => Some(self.carry_out(requests)),
            Err(CommandError::UnknownId(_)) => None,
            Err(error) => {
                self.notice = Some(TextNotice::CommandFailed(error.to_string()));
                Some(EditorOutcome::Consumed)
            }
        }
    }

    /// When a half-typed key times out, the text edited since its server
    /// last had it is handed over, or an inline source is asked.
    fn next_tick(&self) -> Option<Instant> {
        // The engine's deadline is for what is in sight (highlighting the
        // lines on screen, a key sequence waiting), and asking walks every
        // view: a tab not shown leaves it to those that are.
        let (deadline, unsynced) = session::with(|session| {
            (
                self.shown.then(|| session.editor.deadline()).flatten(),
                session.is_unsynced(self.document),
            )
        });
        let flush = unsynced.then(|| Instant::now() + FLUSH_DELAY);
        let (git, spelling) = session::with(|session| {
            (
                session.git_due(self.document),
                session.spelling_due(self.document),
            )
        });
        [
            deadline.map(clock::instant),
            flush,
            self.disk_next_look(),
            self.lsp_next_tick(),
            self.inline_next_tick(),
            git,
            spelling,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn on_tick(&mut self, now: Instant) -> EditorOutcome {
        let watched = self
            .disk_watch
            .as_mut()
            .is_some_and(|watch| watch.changed(now));
        if std::mem::take(&mut self.disk_due) || watched {
            self.check_disk();
        }
        self.inline_tick(now);
        if let Some(outcome) = self.lsp_tick(now) {
            return outcome;
        }
        if let Some(read) = session::with(|session| session.git_read(self.document, now)) {
            return read;
        }
        let language = self.language();
        let requests = session::with(|session| {
            session.editor.tick(clock::stamp(now));
            session.sync(self.document, &language);
            session.check_spelling(self.document);
            session.editor.take_requests()
        });
        self.carry_out(requests)
    }

    /// Pastes `text` at the cursor, as typed.
    fn on_paste(&mut self, text: String) -> EditorOutcome {
        let view = self.view_id();
        session::with(|session| {
            let Some(cursor) = session.editor.view(view).map(cherry::View::cursor) else {
                return;
            };
            let edit = cherry::Edit::new(cursor..cursor, text);
            session.editor.apply_edits(view, vec![edit]);
        });
        EditorOutcome::Consumed
    }

    fn jump_targets(&self) -> Option<Vec<JumpTarget>> {
        Some(jumps::targets(&session::with(|session| {
            session.lines(self.document)
        })))
    }

    fn jump_styles(&mut self, targets: &[usize], budget: Duration) -> TargetStyles {
        let (lines, version) = session::with(|session| {
            let version = session.editor.document_version(self.document);
            (session.lines(self.document), version)
        });
        if self.jump_walk.0 != version {
            self.jump_walk = (version, Vec::new());
        }
        jumps::styles(&self.path, &lines, targets, budget, &mut self.jump_walk.1)
    }

    fn jump_cursor(&self) -> Option<usize> {
        Some(self.cursor_line_column().0)
    }

    fn key_context(&self, context: &mut KeyContext) {
        let view = self.view_id();
        let (selection, completion) = session::with(|session| {
            session.editor.view(view).map_or((false, false), |view| {
                let selected = !view.selection().primary().is_empty();
                (selected, view.completion().is_some())
            })
        });
        context.completion = completion;
        context.lang = Some(self.language());
        context.lsp = self.port().is_some();
        context.selection = selection;
    }

    fn outline_revision(&self) -> Option<u64> {
        Some(self.outline_revision_now())
    }

    /// The file's outline, from what knows its shape best: Markdown's
    /// headings and LaTeX's sections; else the symbols the language server
    /// says the file defines; else the outline written for its format, as
    /// Cherry knows them (KDL's blocks, JSON's and YAML's keys, and the
    /// rest), the format told by the file's language, else its extension;
    /// else, for code, the definitions its grammar names. Each entry is led
    /// by its line number and indented below the shallowest. `None` for a
    /// file with none of them.
    fn outline_targets(&mut self) -> Option<Vec<JumpTarget>> {
        let extension = self
            .path
            .extension()
            .and_then(|extension| extension.to_str());
        let format = Format::from_language(&self.language())
            .or_else(|| extension.and_then(Format::from_extension));
        let text = session::with(|session| session.lines(self.document)).join("\n");
        let prose = matches!(format, Some(Format::Markdown | Format::Latex));
        let served = self.served_outline().filter(|_| !prose);
        let entries = match (served, format) {
            (Some(served), _) => served,
            (None, Some(format)) => cherry::markup::outline(format, &text),
            (None, None) => syntax::grammars().definitions(&self.path, &text)?,
        };
        let top = entries.iter().map(|entry| entry.level).min().unwrap_or(1);
        let targets = entries
            .into_iter()
            .map(|entry| {
                let title: String = entry.title.chars().take(OUTLINE_TITLE_CHARS).collect();
                let depth = entry.level - top;
                let label = format!(
                    "{:>5}  {}{title}",
                    entry.line + 1,
                    OUTLINE_INDENT.repeat(depth)
                );
                JumpTarget::new(entry.line, label).with_depth(depth)
            })
            .collect();
        Some(targets)
    }

    /// A preview moves the cursor to the target's line, remembering where
    /// it was; calling the jump off goes back there.
    fn on_jump(&mut self, step: JumpStep) {
        let view = self.view_id();
        let line = match step {
            JumpStep::Cancel => {
                if let Some(origin) = self.jump_origin.take() {
                    session::with(|session| session.editor.set_cursor(view, origin));
                }
                return;
            }
            JumpStep::Confirm(line) => {
                self.jump_origin = None;
                line
            }
            JumpStep::Preview(line) => {
                if self.jump_origin.is_none() {
                    self.jump_origin = session::with(|session| {
                        session.editor.view(view).map(cherry::View::cursor)
                    });
                }
                line
            }
        };
        self.move_to(line, 0);
    }

    /// Copy and cut over a selection; paste, undo, and redo elsewhere, each
    /// as the keys that do it, which the menu teaches.
    fn context_items(&self) -> Vec<EditorMenuItem> {
        let mode = self.mode();
        let entries = match mode {
            Some(EditMode::Visual) => &SELECTION_MENU[..],
            _ => &TEXT_MENU[..],
        };
        let items = entries
            .iter()
            .filter_map(|(spec, label)| {
                let keys = winter_model::input::table::keys(spec)?;
                Some(EditorMenuItem::typed(keys, *label))
            })
            .collect();
        menu_groups([items])
    }

    /// Git's answers: to the read of the file as last committed, and to
    /// the read of earlier commit messages a recall steps back through.
    fn on_job(&mut self, reply: JobReply) -> EditorOutcome {
        let JobReply::Command(output) = reply else {
            return EditorOutcome::Consumed;
        };
        if output.tag == git_changes::TAG_GIT_BASE {
            session::with(|session| session.git_answered(self.document, &output));
            return EditorOutcome::Consumed;
        }
        let log = winter_vcs::exec::succeeded(&output).then_some(output.stdout.as_str());
        self.take_messages(output.tag, log)
            .unwrap_or(EditorOutcome::Consumed)
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        if reply.tag == ASK_REVERT {
            if reply.answer.is_some() {
                self.revert();
            }
            return EditorOutcome::Consumed;
        }
        self.on_conflict_prompt(&reply)
            .or_else(|| self.on_git_prompt(&reply))
            .or_else(|| self.on_lsp_prompt(&reply))
            .unwrap_or(EditorOutcome::Ignored)
    }

    /// Edits a server made to files this tab does not hold.
    fn take_requests(&mut self) -> Vec<EditorRequest> {
        std::mem::take(&mut self.lsp.requests)
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take().map(TextNotice::into_notice)
    }

    /// The mode; the source of the ghost text on screen, the file's errors
    /// and warnings (`E2 W1`), the cursor's line and column (from 1), and
    /// the file's length.
    fn status(&self) -> EditorStatus {
        let segments = self.status_parts().map(|parts| {
            let problems = parts
                .problems
                .map(|(errors, warnings)| format!("E{errors} W{warnings}"));
            let place = format!("{}:{}", parts.line, parts.column);
            let total = match parts.lines {
                1 => "1 line".to_owned(),
                n => format!("{n} lines"),
            };
            parts
                .ghost
                .into_iter()
                .chain(problems)
                .chain([place, total])
                .collect()
        });
        EditorStatus {
            mode: self.mode(),
            segments: segments.unwrap_or_default(),
        }
    }

    /// The status segments that say little on their own, in words: the
    /// errors and warnings, and the cursor's place.
    fn status_tooltip(&self, segment: &str) -> Option<String> {
        let parts = self.status_parts()?;
        if let Some((errors, warnings)) = parts.problems
            && segment == format!("E{errors} W{warnings}")
        {
            return Some(format!(
                "{}, {}",
                counted(errors, "error"),
                counted(warnings, "warning")
            ));
        }
        (segment == format!("{}:{}", parts.line, parts.column))
            .then(|| format!("Line {}, column {}", parts.line, parts.column))
    }

    /// Where the view is in the file, in screen rows (a wrapped line
    /// takes several): what the window draws the pane's scrollbar from, as
    /// it draws a terminal's.
    fn scroll_position(&self) -> Option<ScrollPosition> {
        self.with_view(|view| {
            session::with(|session| {
                let rows = session.editor.view_rows(view.view())?;
                Some(ScrollPosition::new(rows.top, rows.shown, rows.total))
            })
        })
    }

    /// Scroll the view so screen row `offset` is at its top: the scrollbar
    /// dragged or clicked.
    fn scroll_to(&mut self, offset: usize) -> EditorOutcome {
        self.with_view(|view| {
            session::with(|session| session.editor.scroll_view_to_row(view.view(), offset));
        });
        EditorOutcome::Consumed
    }

    fn header_path(&self) -> Option<PathBuf> {
        Some(self.path.clone())
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.path.parent().map(Path::to_path_buf)
    }

    fn as_editor(&self) -> Option<&dyn Editor> {
        Some(self)
    }

    fn as_editor_mut(&mut self) -> Option<&mut dyn Editor> {
        Some(self)
    }
}

impl Editor for TextEditor {
    fn is_waited_on(&self) -> bool {
        self.waiter.is_some()
    }

    /// A commit message is written for one commit: buried, it would come
    /// back as the message of a commit already made.
    fn is_kept_when_closed(&self) -> bool {
        !self.is_waited_on() && !self.edits_commit()
    }

    /// The file written as the keyboard leaves the window, when it has edits.
    fn focus_lost(&mut self) -> Vec<PathBuf> {
        let unasked = session::with(|session| session.writes_unasked(self.document));
        if !self.is_dirty() || !unasked {
            return Vec::new();
        }
        match session::with(|session| session.save(self.document)) {
            Ok(path) => vec![path],
            Err(error) => {
                self.notice = Some(TextNotice::WriteFailed(error.to_string()));
                Vec::new()
            }
        }
    }

    /// Back in Normal mode, as the keyboard comes to the tab; a commit
    /// message, the first time it comes, keeps the Insert it opened in.
    fn focus_gained(&mut self) {
        if std::mem::replace(&mut self.focused_before, true) {
            self.leave_insert();
        }
        self.check_disk();
    }

    /// A program blocked on this edit hears that it is over: a rebase's
    /// plan closed unfinished (by the window's close, or a forced quit)
    /// aborts the rebase, as an editor that fails does.
    fn on_closed(&mut self) {
        if !self.finished
            && self.holds_plan()
            && let Some(waiter) = self.waiter.as_mut()
        {
            waiter.reject();
        }
        self.waiter = None;
        if let Some(view) = self.view.get_mut().take() {
            session::with(|session| session.close_view(self.document, view.view()));
        }
    }

    /// The view of the tab going behind, kept for it: its own cursor,
    /// scroll, and mode, which the next tab on this buffer does not share.
    fn save_view(&self) -> Option<Box<dyn Any>> {
        let view = self.view.borrow_mut().take()?;
        Some(Box::new(KeptView {
            document: self.document,
            view: Some(view),
        }))
    }

    /// Shows the view `state` kept for the tab coming in front.
    fn restore_view(&mut self, state: Box<dyn Any>) {
        let Ok(mut kept) = state.downcast::<KeptView>() else {
            return;
        };
        let Some(view) = kept.view.take() else {
            return;
        };
        if let Some(before) = self.view.get_mut().replace(view) {
            session::with(|session| session.close_view(self.document, before.view()));
        }
    }

    fn is_dirty(&self) -> bool {
        session::with(|session| session.editor.is_modified(self.document))
    }

    fn document(&self) -> Option<(PathBuf, String)> {
        let text = session::with(|session| session.lines(self.document)).join("\n");
        Some((self.path.clone(), text))
    }

    fn document_path(&self) -> Option<PathBuf> {
        Some(self.path.clone())
    }

    fn text_revision(&self) -> Option<usize> {
        session::with(|session| session.editor.document_version(self.document)).map(|v| v as usize)
    }

    fn holds_file(&self, path: &Path) -> bool {
        self.path == path
            || matches!((self.path.canonicalize(), path.canonicalize()), (Ok(a), Ok(b)) if a == b)
    }

    /// Lands on `target`'s line and column when it is this tab's file.
    fn open_file(&mut self, target: OpenTarget) -> bool {
        if !self.holds_file(&target.path) {
            return false;
        }
        let line = target.line.unwrap_or(1).saturating_sub(1);
        self.move_to(line, target.column.unwrap_or(1).saturating_sub(1));
        true
    }

    /// A commit message's tab goes back to the work tree it was written
    /// for, the view of which reads it again.
    fn returns_to_git(&self) -> Option<PathBuf> {
        if !self.edits_commit() {
            return None;
        }
        // The message is written in the repository's git directory; the
        // view is of the work tree around it.
        let dir = self.cwd()?;
        Some(match dir.file_name().is_some_and(|name| name == ".git") {
            true => dir.parent().map_or_else(|| dir.clone(), PathBuf::from),
            false => dir,
        })
    }

    fn address(&self) -> Option<String> {
        Some(winter_model::path::abbreviate_home(&self.path))
    }

    /// The file, and the lines a visual selection covers: the cursor's
    /// line alone is where it happens to rest, not a place picked.
    fn file_reference(&self) -> Option<String> {
        let base = winter_model::path::abbreviate_home(&self.path);
        let lines = session::with(|session| {
            let view = session.editor.view(self.view_id())?;
            let anchor = view.visual_anchor()?;
            let text = session.editor.document(self.document)?;
            let line = |offset| text.char_to_position(offset).map_or(0, |at| at.line + 1);
            let (a, b) = (line(anchor), line(view.cursor()));
            Some((a.min(b), a.max(b)))
        });
        Some(match lines {
            None => base,
            Some((start, end)) if start == end => format!("{base}:{start}"),
            Some((start, end)) => format!("{base}:{start}-{end}"),
        })
    }

    /// The file at the cursor's line; nothing for a file git hands the
    /// editor, which git reopens itself.
    fn snapshot(&self) -> Option<OpenTarget> {
        if self.holds_git_file() {
            return None;
        }
        let line = session::with(|session| {
            let cursor = session.editor.view(self.view_id())?.cursor();
            session
                .editor
                .document(self.document)?
                .char_to_position(cursor)
        });
        Some(OpenTarget {
            column: None,
            line: line.map(|at| at.line + 1),
            path: self.path.clone(),
        })
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The widget drawing Cherry's view `view`, its change bars saying what
/// changed since the last commit when the pointer rests on them. The
/// window draws the pane's scrollbar, as it does a terminal's, so the
/// widget's own is left out.
fn view_of(view: ViewId) -> EditorView {
    let mut shown = EditorView::new(view);
    shown.set_scrollbar_hidden(true);
    shown.set_change_notes(ChangeNotes {
        added: CHANGE_ADDED.to_owned(),
        deleted: CHANGE_DELETED.to_owned(),
        modified: CHANGE_MODIFIED.to_owned(),
    });
    shown
}

/// `count` of `noun`, plural but for one: `1 error`, `2 errors`.
fn counted(count: usize, noun: &str) -> String {
    match count {
        1 => format!("1 {noun}"),
        n => format!("{n} {noun}s"),
    }
}

/// Where Cherry starts the rows a wrapped line continues on, as the
/// `editor-wrap-indent` setting `indent` says.
fn break_indent(indent: WrapIndent) -> cherry::BreakIndent {
    match indent {
        WrapIndent::None => cherry::BreakIndent::None,
        WrapIndent::Same => cherry::BreakIndent::Indent,
        WrapIndent::Semantic => cherry::BreakIndent::Text,
    }
}

/// A server's `edit`, as the window passes edits on.
fn file_edit(edit: &TextEdit) -> FileEdit {
    let point = |at: winter_lsp::protocol::Position| EditPoint {
        column: at.character,
        line: at.line,
    };
    FileEdit {
        end: point(edit.range.end),
        new_text: edit.new_text.clone(),
        start: point(edit.range.start),
    }
}

/// A Cherry view's `mode`, as the status bar names it.
pub(crate) fn edit_mode_of(mode: Mode) -> EditMode {
    match mode {
        Mode::Insert | Mode::CommandLine => EditMode::Insert,
        Mode::Replace => EditMode::Replace,
        Mode::Visual | Mode::VisualBlock | Mode::VisualLine => EditMode::Visual,
        _ => EditMode::Normal,
    }
}

/// The name a tab on the file at `path` goes by: the file's.
fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use winter_model::input::KeyCode;

    use super::*;

    #[test]
    fn test_focus_puts_a_file_in_normal_mode_and_a_message_after_its_first() {
        use winter_model::editor::Editor as _;

        let dir = std::env::temp_dir().join(format!("winter-text-focus-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a folder");
        let file = dir.join("notes.txt");
        std::fs::write(&file, "one\n").expect("written");
        let mut editor = TextEditor::new(file, None).expect("opens");
        editor.on_key(&Key::plain(KeyCode::Char('i')));
        editor.focus_gained();
        assert_eq!(editor.mode(), Some(EditMode::Normal), "a file at once");
        let message = dir.join("COMMIT_EDITMSG");
        std::fs::write(&message, "\n# a comment\n").expect("written");
        let mut editor = TextEditor::new(message, None).expect("opens");
        editor.focus_gained();
        assert_eq!(
            editor.mode(),
            Some(EditMode::Insert),
            "as it opened, ready to type"
        );
        editor.focus_gained();
        assert_eq!(editor.mode(), Some(EditMode::Normal), "and after that");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_the_scrollbar_reads_the_view_in_lines_and_scrolls_it_to_a_line() {
        let dir = std::env::temp_dir().join(format!("winter-text-scroll-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a folder");
        let file = dir.join("long.txt");
        let text: String = (0..100).map(|line| format!("line {line}\n")).collect();
        std::fs::write(&file, text).expect("written");
        let mut editor = TextEditor::new(file, None).expect("opens");
        editor.with_view(|view| {
            session::with(|session| session.editor.resize_view(view.view(), 40, 10));
        });
        let at = editor.scroll_position().expect("a view");
        assert_eq!((at.offset, at.visible), (0, 10));
        assert!(at.total >= 100, "{}", at.total);
        assert_eq!(editor.scroll_to(40), EditorOutcome::Consumed);
        assert_eq!(editor.scroll_position().map(|at| at.offset), Some(40));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_a_save_from_insert_mode_writes_and_leaves_insert() {
        let path =
            std::env::temp_dir().join(format!("winter-text-save-{}.txt", std::process::id()));
        std::fs::write(&path, "one\n").expect("written");
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        editor.on_key(&Key::plain(KeyCode::Char('A')));
        editor.on_key(&Key::plain(KeyCode::Char('!')));
        assert_eq!(editor.mode(), Some(EditMode::Insert));
        // As the window runs its `save` key (`C-S-s`) in the editor.
        editor.run_command("save");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "one!\n");
        assert_eq!(editor.mode(), Some(EditMode::Normal), "back in Normal");
        // As the window runs its `save` key (`C-S-s`) in the editor.
        editor.run_command("save");
        assert_eq!(
            editor.cursor_line_column(),
            (0, 3),
            "a save in Normal moves nothing"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_ctrl_and_alt_backspace_delete_the_word_before_the_cursor_in_insert_mode() {
        let path =
            std::env::temp_dir().join(format!("winter-text-kill-word-{}.txt", std::process::id()));
        std::fs::write(&path, "one two three\n").expect("written");
        // The shipped `keybindings.kdl` binds both keys in `editor.insert`.
        let keys = HashMap::from([(
            "editor.insert".to_string(),
            HashMap::from([(
                "backward-kill-word".to_string(),
                vec!["M-DEL".to_string(), "C-DEL".to_string()],
            )]),
        )]);
        assert_eq!(set_user_keys(&keys), Vec::<String>::new());
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        editor.on_key(&Key::plain(KeyCode::Char('A')));
        editor.on_key(&Key::with_ctrl(KeyCode::Backspace));
        editor.on_key(&Key::with_alt(KeyCode::Backspace));
        editor.run_command("save");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "one \n");
        set_user_keys(&HashMap::new());
        let _ = std::fs::remove_file(&path);
    }

    /// How long a test waits for the watch to see a write.
    const WATCH_WAIT: Duration = Duration::from_secs(5);

    /// Tick `editor` as the window would, when it asks, until its text holds
    /// `wanted` or the wait runs out.
    fn tick_until_read(editor: &mut TextEditor, wanted: &str) {
        let end = Instant::now() + WATCH_WAIT;
        while Instant::now() < end {
            if editor.next_tick().is_some_and(|at| at <= Instant::now()) {
                editor.on_tick(Instant::now());
            }
            if editor
                .document()
                .is_some_and(|(_, text)| text.contains(wanted))
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Tick `editor` as the window would until it has something to say.
    fn tick_until_told(editor: &mut TextEditor) -> Option<TextNotice> {
        let end = Instant::now() + WATCH_WAIT;
        while Instant::now() < end {
            if editor.next_tick().is_some_and(|at| at <= Instant::now()) {
                editor.on_tick(Instant::now());
            }
            if let Some(notice) = editor.notice.take() {
                return Some(notice);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    #[test]
    fn test_an_editor_not_shown_asks_for_no_tick_until_it_is_shown() {
        let path =
            std::env::temp_dir().join(format!("winter-text-unshown-{}.rs", std::process::id()));
        std::fs::write(&path, "fn main() {}\n").expect("written");
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        assert_eq!(editor.next_tick(), None, "not shown, nothing to wake for");
        editor.set_shown(true);
        assert!(
            editor.next_tick().is_some(),
            "shown, it asks for a turn to catch up"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_a_shown_editor_watches_its_file_and_asks_for_no_disk_look_while_quiet() {
        let path =
            std::env::temp_dir().join(format!("winter-text-quiet-{}.txt", std::process::id()));
        std::fs::write(&path, "one\n").expect("written");
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        assert_eq!(editor.disk_next_look(), None, "not shown, nothing");
        editor.set_shown(true);
        assert!(editor.disk_next_look().is_some(), "shown, it looks once");
        editor.on_tick(Instant::now());
        assert_eq!(editor.disk_next_look(), None, "then only on a change");
        editor.set_shown(false);
        assert!(editor.disk_watch.is_none(), "the watch let go");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_a_file_changed_on_disk_is_read_again_keeping_the_cursor_on_its_line() {
        let path =
            std::env::temp_dir().join(format!("winter-text-reload-{}.txt", std::process::id()));
        std::fs::write(&path, "one\ntwo\nthree\n").expect("written");
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        editor.set_shown(true);
        editor.on_tick(Instant::now());
        editor.move_to(2, 1);
        std::fs::write(&path, "one\ntwo, from elsewhere\nthree\n").expect("written");
        tick_until_read(&mut editor, "two, from elsewhere");
        let (_, text) = editor.document().expect("a document");
        assert_eq!(text, "one\ntwo, from elsewhere\nthree");
        assert_eq!(editor.cursor_line_column(), (2, 1), "still on \"three\"");
        assert!(editor.take_notice().is_none(), "a reload says nothing");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_revert_asks_before_dropping_unsaved_edits_and_reads_the_file_on_yes() {
        // A revert pressed out of habit over an hour's typing loses it all
        // unless it asks; a no keeps every edit.
        let path =
            std::env::temp_dir().join(format!("winter-text-revert-{}.txt", std::process::id()));
        std::fs::write(&path, "one\n").expect("written");
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        editor.on_key(&Key::plain(KeyCode::Char('A')));
        editor.on_key(&Key::plain(KeyCode::Char('!')));
        editor.on_key(&Key::plain(KeyCode::Escape));

        let asked = editor.run_command(FILE_REVERT);
        let Some(EditorOutcome::Prompt(question)) = asked else {
            panic!("asks first: {asked:?}");
        };
        assert_eq!(question.tag, ASK_REVERT);
        assert!(
            question
                .title
                .as_ref()
                .is_some_and(|title| title.destructive)
        );
        editor.on_prompt(PromptReply {
            answer: None,
            tag: ASK_REVERT,
        });
        assert_eq!(
            editor.document().expect("a document").1,
            "one!",
            "a no keeps it"
        );

        editor.on_prompt(PromptReply {
            answer: Some("y".to_string()),
            tag: ASK_REVERT,
        });
        assert_eq!(editor.document().expect("a document").1, "one");
        assert!(!editor.is_dirty(), "the text is the file's");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_revert_of_a_saved_file_reads_it_without_asking() {
        // Nothing is lost, so a question would only be in the way.
        let path = std::env::temp_dir().join(format!(
            "winter-text-revert-clean-{}.txt",
            std::process::id()
        ));
        std::fs::write(&path, "one\n").expect("written");
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        std::fs::write(&path, "two\n").expect("written");

        assert_eq!(
            editor.run_command(FILE_REVERT),
            Some(EditorOutcome::Consumed)
        );
        assert_eq!(editor.document().expect("a document").1, "two");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_a_file_changed_under_unsaved_edits_keeps_them_and_says_so_once() {
        let path =
            std::env::temp_dir().join(format!("winter-text-under-{}.txt", std::process::id()));
        std::fs::write(&path, "one\n").expect("written");
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        editor.set_shown(true);
        editor.on_tick(Instant::now());
        editor.on_key(&Key::plain(KeyCode::Char('A')));
        editor.on_key(&Key::plain(KeyCode::Char('!')));
        std::fs::write(&path, "one, from elsewhere\n").expect("written");
        let notice = tick_until_told(&mut editor).expect("told");
        let (_, text) = editor.document().expect("a document");
        assert_eq!(text, "one!", "the edit kept");
        assert!(matches!(notice, TextNotice::ChangedOnDisk(_)), "{notice:?}");
        editor.on_tick(Instant::now());
        assert!(
            editor.take_notice().is_none(),
            "once for each change on disk"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_a_file_s_outline_lists_its_format_s_entries_and_goes_to_their_lines() {
        use winter_model::editor::JumpStep;

        let path =
            std::env::temp_dir().join(format!("winter-text-outline-{}.md", std::process::id()));
        std::fs::write(&path, "# Top\n\ntext\n\n## Below\n").expect("written");
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        let entries: Vec<(String, usize, usize)> = editor
            .outline_targets()
            .expect("Markdown has one")
            .into_iter()
            .map(|entry| (entry.label, entry.target, entry.depth))
            .collect();
        assert_eq!(
            entries,
            [
                ("    1  Top".to_string(), 0, 0),
                ("    5    Below".to_string(), 4, 1)
            ]
        );
        editor.on_jump(JumpStep::Confirm(4));
        assert_eq!(editor.cursor_line_column().0, 4);
        let _ = std::fs::remove_file(&path);
        let plain =
            std::env::temp_dir().join(format!("winter-text-plain-{}.txt", std::process::id()));
        std::fs::write(&plain, "words\n").expect("written");
        let mut editor = TextEditor::new(plain.clone(), None).expect("opens");
        assert!(editor.outline_targets().is_none(), "no format, no grammar");
        let _ = std::fs::remove_file(&plain);
    }

    #[test]
    fn test_code_with_no_format_outline_lists_the_definitions_its_grammar_names() {
        let path = std::env::temp_dir().join(format!("winter-text-defs-{}.rs", std::process::id()));
        std::fs::write(&path, "struct S;\n\nfn main() {\n    run();\n}\n").expect("written");
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        let entries: Vec<String> = editor
            .outline_targets()
            .expect("Rust's grammar names its definitions")
            .into_iter()
            .map(|entry| entry.label)
            .collect();
        assert_eq!(entries, ["    1  struct S;", "    3  fn main() {"]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_a_file_named_for_nothing_takes_the_outline_of_its_language() {
        let dir = std::env::temp_dir()
            .join(format!("winter-text-ssh-{}", std::process::id()))
            .join(".ssh");
        std::fs::create_dir_all(&dir).expect("a folder");
        let path = dir.join("config");
        std::fs::write(&path, "Host work\n    User me\nHost home\n").expect("written");
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        let lines: Vec<usize> = editor
            .outline_targets()
            .expect("SSH's configuration has one")
            .into_iter()
            .map(|entry| entry.target)
            .collect();
        assert_eq!(lines, [0, 2], "its Host blocks");
        let _ = std::fs::remove_dir_all(dir.parent().expect("a parent"));
    }

    #[test]
    fn test_the_wrap_settings_reach_the_editor_s_layout() {
        let path =
            std::env::temp_dir().join(format!("winter-text-wrap-{}.txt", std::process::id()));
        std::fs::write(&path, "- item\n").expect("written");
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        let wrap = || {
            session::with(|session| {
                let options = session.editor.options();
                (options.break_indent, options.wrap_mode)
            })
        };
        editor.configure(&settings::EditorSettings {
            wrap_indent: WrapIndent::Same,
            wrap_words: false,
            ..settings::EditorSettings::default()
        });
        assert_eq!(
            wrap(),
            (cherry::BreakIndent::Indent, cherry::WrapMode::Chars)
        );
        editor.configure(&settings::EditorSettings {
            wrap_indent: WrapIndent::Semantic,
            wrap_words: true,
            ..settings::EditorSettings::default()
        });
        assert_eq!(wrap(), (cherry::BreakIndent::Text, cherry::WrapMode::Words));
        let _ = std::fs::remove_file(&path);
    }
}
