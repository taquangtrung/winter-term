//! The text editor tab: a view of a file open in Viento, the editor every
//! text tab shares ([`session`]), drawn by misty-viento's `EditorView` in
//! its pane. Viento owns the text, the cursor, the modes, and every editing
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

use misty::theme::Theme;
use misty::{EventCx, Font, Frame, Input, Rect};
use misty_viento::{clock, EditorResponse, EditorView};
use viento::{CommandError, DocumentId, Handled, HostRequest, Mode, NoticeLevel, ViewId};
use winter_base::edit_wait::EditWaiter;
use winter_lsp::diagnostics;
use winter_lsp::protocol::TextEdit;
use winter_lsp::{Event, Incoming, Port};
use winter_model::editor::EditPoint;
use winter_model::editor::{
    menu_groups, Editor, EditorCommand, EditorMenuItem, EditorOutcome, EditorRequest, EditorStatus,
    FileEdit, JobReply, JumpStep, JumpTarget, Notice, OpenTarget, Page, PromptReply, SharedText,
    TargetStyles, SAVE_ALL,
};
use winter_model::input::Key;
use winter_model::paint::{Frame as PaintFrame, IconKind, Row};
use winter_model::textedit::modal::EditMode;

mod completing;
pub mod diff;
mod git_changes;
mod hints;
mod jumps;
mod keys;
mod language;
mod markup;
mod requests;
mod servers;
mod session;
pub mod settings;

pub use keys::KEY_TABLES;
pub use language::known_languages;
pub use misty_viento::HighlightStyle;
pub use session::save_through;
pub use settings::history_dir;

// ========================================================================
// Constants
// ========================================================================

/// The text editor's commands the palette runs by name: none of Winter's
/// own, as Viento runs every editing command.
pub const COMMANDS: &[EditorCommand] = &[];

/// What a tab's title ends with while its file has edits not written.
pub const DIRTY_MARK: &str = " [+]";

/// The right-click menu's entries, as the keys Viento binds them to and
/// their labels: over a selection, and elsewhere.
const SELECTION_MENU: [(&str, &str); 2] = [("y", "Copy"), ("d", "Cut")];
const TEXT_MENU: [(&str, &str); 3] = [("p", "Paste"), ("u", "Undo"), ("C-r", "Redo")];

/// How long typing pauses before a language server is handed the text.
const FLUSH_DELAY: Duration = Duration::from_millis(150);

/// What is said when a close would drop edits not written.
const UNSAVED: &str = "No write since last change (add ! to override)";

// ========================================================================
// Data Structures
// ========================================================================

/// A text tab: one view of a file open in the shared editor.
#[derive(Debug)]
pub struct TextEditor {
    /// The document shown, the file's.
    document: DocumentId,
    /// Whether a save has the language server format the file first.
    format_on_save: bool,
    /// The language server's inlay hints and lenses in the text.
    hints: hints::Hints,
    /// What the tab is waiting on its language server for.
    lsp: requests::Lsp,
    /// Where the cursor was before Scout's preview moved it, to go back to
    /// should the jump be called off.
    jump_origin: Option<usize>,
    /// Where each line Scout's colors have walked to begins, and the text's
    /// version it was walked in.
    jump_walk: (Option<u64>, Vec<winter_syntax::LineState>),
    /// The checkers' publications last laid on the text, by their store's
    /// generation.
    problems_seen: Option<u64>,
    /// The language each extension settings add is written in.
    languages: BTreeMap<String, String>,
    /// A notice for the window, not yet taken.
    notice: Option<Notice>,
    /// Where the file is.
    path: PathBuf,
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

// ========================================================================
// TextEditor Implementations
// ========================================================================

impl TextEditor {
    /// A tab on `path`'s contents, on the document another tab has it open
    /// in if one has, landing on `line` (from 1) when whatever asked for the
    /// file knew which one mattered.
    pub fn new(path: PathBuf, line: Option<usize>) -> io::Result<Self> {
        let (document, view) = session::with(|session| session.open_view(&path))?;
        let mut editor = Self {
            document,
            format_on_save: false,
            jump_origin: None,
            hints: hints::Hints::default(),
            lsp: requests::Lsp::default(),
            jump_walk: (None, Vec::new()),
            languages: BTreeMap::new(),
            problems_seen: None,
            notice: None,
            path,
            view: RefCell::new(Some(EditorView::new(view))),
            waiter: None,
        };
        if let Some(line) = line {
            editor.move_to(line.saturating_sub(1), 0);
        }
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

    /// Sends the completion asks Viento noted while a key was handled, the
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
        let grammar = language::grammar_of(&self.path);
        language::language_of(&self.path, grammar, &self.languages)
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
            let wanted = viento::EditorOptions {
                auto_complete: settings.completion.auto,
                conceal: markup::conceal_mode(settings.markup_markers),
                indent_guides: settings.indent_guides,
                ..*options
            };
            if wanted != *options {
                let options = wanted;
                session.editor.set_options(session::options(&options));
            }
        });
        self.languages.clone_from(&settings.languages);
        self.format_on_save = settings.format_on_save;
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
                let position = viento::Utf16Position {
                    column: point.column as usize,
                    line: point.line as usize,
                };
                text.utf16_position_to_char(position)
                    .unwrap_or_else(|| text.len_chars())
            };
            let mut changes: Vec<viento::Edit> = edits
                .iter()
                .map(|edit| viento::Edit::new(offset(edit.start)..offset(edit.end), &edit.new_text))
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

    /// Draws the view in `area` of `frame` in `font`, the highlighter's
    /// styles drawn as `styles` gives them, by number.
    pub fn draw(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        font: &Font,
        styles: Vec<HighlightStyle>,
    ) {
        session::with(|session| {
            session.git_shown(self.document);
            session.conceal_markup(self.document);
        });
        let generation = diagnostics::generation();
        if self.problems_seen != Some(generation) {
            self.problems_seen = Some(generation);
            session::with(|session| session.refresh_problems(self.document));
        }
        self.with_view(|view| {
            view.set_font(Some(font.clone()));
            view.set_highlight_styles(styles);
            session::with(|session| {
                use misty::Widget;
                view.lend(&mut session.editor).draw(frame, area, theme);
            });
        });
        if let Some(cursor) = self.with_view(|view| view.cursor_rect()) {
            self.draw_card(frame, cursor, area, theme);
        }
    }

    /// Hands the view `input` from the pointer or an input method: a press
    /// moves the cursor, a drag selects, the wheel scrolls, composed text is
    /// typed. What it asks of the window.
    pub fn handle_input(&mut self, input: &Input, cx: &mut EventCx<'_>) -> EditorOutcome {
        let response = self.with_view(|view| {
            session::with(|session| {
                use misty::Widget;
                view.lend(&mut session.editor).handle(input, cx)
            })
        });
        self.send_asks();
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
            EditorView::new(session::with(|session| session.add_view(self.document)))
        });
        act(view)
    }

    /// The Viento view the tab in front shows.
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

    /// Carries out what the editor asked, oldest first: what the window is
    /// to do, the last request's that asks anything of it.
    fn carry_out(&mut self, requests: Vec<HostRequest>) -> EditorOutcome {
        let mut outcome = EditorOutcome::Consumed;
        for request in requests {
            match request {
                HostRequest::Notice(notice) => {
                    self.notice = Some(match notice.level {
                        NoticeLevel::Error => Notice::error(notice.message),
                        NoticeLevel::Warning => Notice::warning(notice.message),
                        _ => Notice::info(notice.message),
                    });
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
                HostRequest::Quit(quit) => {
                    if quit.force || !self.is_dirty() {
                        outcome = EditorOutcome::Close;
                    } else {
                        self.notice = Some(Notice::error(UNSAVED));
                    }
                }
                HostRequest::ReadInto(read) => self.read_into(read.line, read.path),
                HostRequest::Save(_) => {
                    outcome = self.format_before_save().unwrap_or_else(|| self.write());
                }
                _ => {}
            }
        }
        outcome
    }

    /// Writes the file: where it went, or why it could not.
    fn write(&mut self) -> EditorOutcome {
        match session::with(|session| session.save(self.document)) {
            Ok(path) => EditorOutcome::Wrote(path),
            Err(error) => {
                self.notice = Some(Notice::error(error.to_string()));
                EditorOutcome::Consumed
            }
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
            Err(error) => self.notice = Some(Notice::error(format!("{}: {error}", path.display()))),
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
        session::with(|session| {
            let mode = session.editor.view(self.view_id())?.mode();
            Some(match mode {
                Mode::Insert | Mode::CommandLine => EditMode::Insert,
                Mode::Replace => EditMode::Replace,
                Mode::Visual | Mode::VisualBlock | Mode::VisualLine => EditMode::Visual,
                _ => EditMode::Normal,
            })
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
        let view = self.view_id();
        let (handled, requests) = session::with(|session| {
            let handled = session
                .editor
                .handle_key(view, keys::to_viento(key), clock::now());
            (handled, session.editor.take_requests())
        });
        self.send_asks();
        match (handled, requests.is_empty()) {
            (Handled::Ignored, true) => EditorOutcome::Ignored,
            (_, true) => EditorOutcome::Consumed,
            (_, false) => self.carry_out(requests),
        }
    }

    /// Writes the file for the window's [`SAVE_ALL`] (a build about to
    /// start); runs Viento's command of any other name, its words joined by
    /// hyphens rather than underscores (`upcase_word` runs `upcase-word`).
    /// `None` for a command Viento does not have.
    fn run_command(&mut self, name: &str) -> Option<EditorOutcome> {
        if name == SAVE_ALL {
            return Some(self.write());
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
                self.notice = Some(Notice::error(error.to_string()));
                Some(EditorOutcome::Consumed)
            }
        }
    }

    /// When a half-typed key times out, or the text edited since its
    /// server last had it is handed over.
    fn next_tick(&self) -> Option<Instant> {
        let (deadline, unsynced) = session::with(|session| {
            (
                session.editor.deadline(),
                session.is_unsynced(self.document),
            )
        });
        let flush = unsynced.then(|| Instant::now() + FLUSH_DELAY);
        let git = session::with(|session| session.git_due(self.document));
        [
            deadline.map(clock::instant),
            flush,
            self.lsp_next_tick(),
            git,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn on_tick(&mut self, now: Instant) -> EditorOutcome {
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
            session.editor.take_requests()
        });
        self.carry_out(requests)
    }

    /// Pastes `text` at the cursor, as typed.
    fn on_paste(&mut self, text: String) -> EditorOutcome {
        let view = self.view_id();
        session::with(|session| {
            let Some(cursor) = session.editor.view(view).map(viento::View::cursor) else {
                return;
            };
            let edit = viento::Edit::new(cursor..cursor, text);
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
        self.snapshot()
            .and_then(|target| target.line)
            .map(|line| line - 1)
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
                        session.editor.view(view).map(viento::View::cursor)
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

    /// Git's answer to the read of the file as last committed.
    fn on_job(&mut self, reply: JobReply) -> EditorOutcome {
        if let JobReply::Command(output) = reply {
            if output.tag == git_changes::TAG_GIT_BASE {
                session::with(|session| session.git_answered(self.document, &output));
            }
        }
        EditorOutcome::Consumed
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        self.on_lsp_prompt(&reply).unwrap_or(EditorOutcome::Ignored)
    }

    /// Edits a server made to files this tab does not hold.
    fn take_requests(&mut self) -> Vec<EditorRequest> {
        std::mem::take(&mut self.lsp.requests)
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }

    /// The mode, the cursor's line and column (from 1), and the file's
    /// length.
    fn status(&self) -> EditorStatus {
        let place = session::with(|session| {
            let cursor = session.editor.view(self.view_id())?.cursor();
            let text = session.editor.document(self.document)?;
            let at = text.char_to_position(cursor)?;
            let total = match text.len_lines() {
                1 => "1 line".to_owned(),
                n => format!("{n} lines"),
            };
            let problems = session.problem_counts(self.document);
            let place = format!("{}:{}", at.line + 1, at.column + 1);
            Some(problems.into_iter().chain([place, total]).collect())
        });
        EditorStatus {
            mode: self.mode(),
            segments: place.unwrap_or_default(),
        }
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
                self.notice = Some(Notice::error(error.to_string()));
                Vec::new()
            }
        }
    }

    /// A program blocked on this edit hears that it is over.
    fn on_closed(&mut self) {
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

    fn snapshot(&self) -> Option<OpenTarget> {
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

/// The name a tab on the file at `path` goes by: the file's.
fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}
