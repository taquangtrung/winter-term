//! The one Viento editor every text tab shows a view of, on the window's
//! thread: a file open in several tabs is one document, a Buffer, its
//! edits, history, and marks shared, and it is closed when the last tab on
//! it goes. Each document remembers the file it came from: where it is, the
//! layout a write has to keep (line endings, a final newline), and what the
//! file was when last read or written, to tell a write from elsewhere.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use misty::SystemClipboard;
use misty_viento::EditorClipboard;
use viento::{CompletionTicket, DocumentId, Editor, EditorOptions, Position, ViewId};
use winter_lsp::protocol::Diagnostic;
use winter_lsp::ServerId;
use winter_model::text_file::{self, FileShape, FileStamp};
use winter_syntax::SyntaxHighlighter;

use super::completing::Asks;
use super::git_changes::GitBase;
use super::language::grammar_of;
use super::servers::{Servers, Synced};
use super::spelling::Spelling;
use super::swatch::Swatched;

// ========================================================================
// Constants
// ========================================================================

/// The most lines pinned at the top of a view, as Winter has always pinned.
const STICKY_ROWS: usize = 3;

thread_local! {
    /// The window thread's editor and the files of its documents.
    static SESSION: RefCell<Session> = RefCell::new(Session::new());
}

// ========================================================================
// Data Structures
// ========================================================================

/// The editor and the file behind each of its documents.
pub(crate) struct Session {
    /// The editor.
    pub(crate) editor: Editor,
    /// The file each document came from, by document.
    pub(crate) files: HashMap<DocumentId, OpenFile>,
    /// The language servers running.
    pub(crate) servers: Servers,
    /// The completion tickets of the asks sent to servers, by server and
    /// request id.
    pub(crate) completions: HashMap<(ServerId, u64), CompletionTicket>,
    /// Where checking each prose document's spelling stands.
    pub(crate) spelling: HashMap<DocumentId, Spelling>,
    /// Whether the colors files write are drawn on the colors they name.
    pub(crate) swatches_on: bool,
}

/// The file a document came from.
#[derive(Clone, Debug)]
pub(crate) struct OpenFile {
    /// The asks its completion provider noted, for its tab to send.
    pub(crate) asks: Asks,
    /// The server completing it, once one is.
    pub(crate) completing: Option<ServerId>,
    /// The document's version its markup's markers were found in.
    pub(crate) concealed: Option<u64>,
    /// What each extension server handed it last had of it, by server.
    pub(crate) extension_synced: HashMap<ServerId, Synced>,
    /// Its text as last committed, and the marks of what changed since.
    pub(crate) git: GitBase,
    /// Where it is.
    pub(crate) path: PathBuf,
    /// What its checkers last found wrong in it, as laid on its text.
    pub(crate) problems: Vec<Diagnostic>,
    /// The layout a write keeps.
    shape: FileShape,
    /// What the file was when last read or written.
    stamp: FileStamp,
    /// The color swatches laid on it, once they are.
    pub(crate) swatched: Option<Swatched>,
    /// What the server serving it last had of it.
    pub(crate) synced: Option<Synced>,
    /// The views showing the document, oldest first.
    views: Vec<ViewId>,
}

// ========================================================================
// Session Implementations
// ========================================================================

impl Session {
    /// An editor with Winter's choices, no document open, its unnamed
    /// register the system clipboard.
    fn new() -> Self {
        let mut editor = Editor::new(options(&EditorOptions::default()));
        editor.set_clipboard(EditorClipboard::new(SystemClipboard::new()));
        Self {
            editor,
            files: HashMap::new(),
            servers: Servers::default(),
            completions: HashMap::new(),
            spelling: HashMap::new(),
            swatches_on: true,
        }
    }

    /// A new view of the file at `path`: of the document a tab has it open
    /// in already, else of a document read from it, colored by its grammar.
    pub(crate) fn open_view(&mut self, path: &Path) -> io::Result<(DocumentId, ViewId)> {
        let document = match self.document_of(path) {
            Some(document) => document,
            None => self.read(path)?,
        };
        let view = self
            .editor
            .new_view(document)
            .ok_or_else(|| io::Error::other("the document closed as it opened"))?;
        if let Some(file) = self.files.get_mut(&document) {
            file.views.push(view);
        }
        Ok((document, view))
    }

    /// A new view of `document`, open already, starting where its last
    /// view left its cursor.
    pub(crate) fn add_view(&mut self, document: DocumentId) -> ViewId {
        let view = self
            .editor
            .new_view(document)
            .expect("a tab's document stays open while the tab does");
        if let Some(file) = self.files.get_mut(&document) {
            file.views.push(view);
        }
        view
    }

    /// Drops `view`, and its document with it when no other view shows it,
    /// its server told.
    pub(crate) fn close_view(&mut self, document: DocumentId, view: ViewId) {
        self.editor.close_view(view);
        let Some(file) = self.files.get_mut(&document) else {
            return;
        };
        file.views.retain(|shown| *shown != view);
        if !file.views.is_empty() {
            return;
        }
        self.tell_closed(document);
        self.stop_spelling(document);
        if self.editor.close_document(document) {
            self.files.remove(&document);
        }
    }

    /// Writes `document` to its file, laid out as the file was, and counts
    /// it saved. Where it is.
    pub(crate) fn save(&mut self, document: DocumentId) -> io::Result<PathBuf> {
        let file = self
            .files
            .get(&document)
            .cloned()
            .ok_or_else(|| io::Error::other("the document has no file"))?;
        let stamp = text_file::save(&file.path, &self.lines(document), file.shape)?;
        self.files
            .entry(document)
            .and_modify(|open| open.stamp = stamp);
        self.editor.mark_saved(document);
        self.tell_saved(document);
        Ok(file.path)
    }

    /// Whether `document` can be written without a question first: its
    /// file not changed on disk since it was read or written. A save that
    /// cannot ask (as the keyboard leaves the window) skips the rest.
    pub(crate) fn writes_unasked(&self, document: DocumentId) -> bool {
        self.files
            .get(&document)
            .is_some_and(|file| !text_file::is_stale(&file.path, &file.stamp))
    }

    /// The lines of `document`, without their breaks.
    pub(crate) fn lines(&self, document: DocumentId) -> Vec<String> {
        let Some(text) = self.editor.document(document) else {
            return Vec::new();
        };
        (0..text.len_lines())
            .filter_map(|line| text.line_text(line))
            .collect()
    }

    /// The char offset of `line` and `column` (both from 0, the column in
    /// chars) in `document`, each clamped to the text.
    pub(crate) fn offset_of(&self, document: DocumentId, line: usize, column: usize) -> usize {
        let Some(text) = self.editor.document(document) else {
            return 0;
        };
        let line = line.min(text.len_lines().saturating_sub(1));
        let length = text.line_text(line).map_or(0, |text| text.chars().count());
        text.position_to_char(Position {
            column: column.min(length),
            line,
        })
        .unwrap_or(0)
    }

    /// The document a tab has the file at `path` open in.
    fn document_of(&self, path: &Path) -> Option<DocumentId> {
        let key = key_of(path);
        self.files
            .iter()
            .find(|(_, file)| key_of(&file.path) == key)
            .map(|(document, _)| *document)
    }

    /// A document read from the file at `path`, an empty one for a file not
    /// made yet, colored by the file's grammar.
    fn read(&mut self, path: &Path) -> io::Result<DocumentId> {
        let (lines, shape, stamp) = match text_file::load(path) {
            Ok(loaded) => (loaded.lines, loaded.shape, loaded.stamp),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                (Vec::new(), FileShape::default(), FileStamp::default())
            }
            Err(error) => return Err(error),
        };
        let document = self.editor.open_document(&lines.join("\n"));
        let grammar = grammar_of(path).or_else(|| {
            lines
                .first()
                .and_then(|line| winter_syntax::of_first_line(line))
        });
        if let Some(grammar) = grammar {
            let highlighter = SyntaxHighlighter::new(grammar);
            self.editor
                .set_highlighter(document, Some(Box::new(highlighter)));
        }
        let name = path.file_name().map(|name| name.to_string_lossy());
        self.editor.set_document_name(document, name.as_deref());
        self.files.insert(
            document,
            OpenFile {
                asks: Asks::default(),
                completing: None,
                concealed: None,
                extension_synced: HashMap::new(),
                git: GitBase::default(),
                path: path.to_path_buf(),
                problems: Vec::new(),
                shape,
                stamp,
                swatched: None,
                synced: None,
                views: Vec::new(),
            },
        );
        self.spell_from_the_start(document, path);
        Ok(document)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `act` on the window thread's session.
pub(crate) fn with<R>(act: impl FnOnce(&mut Session) -> R) -> R {
    SESSION.with(|session| act(&mut session.borrow_mut()))
}

/// The editor's options for `settings`'s choices: Winter's own on top,
/// as the window has always edited.
pub(crate) fn options(base: &EditorOptions) -> EditorOptions {
    EditorOptions {
        auto_indent: true,
        break_indent: true,
        escape_drops_prefix: true,
        light_occurrences: true,
        line_numbers: true,
        literal_search: true,
        match_brackets: true,
        screen_row_motions: true,
        sign_column: true,
        sticky_scroll: STICKY_ROWS,
        unnamed_clipboard: true,
        ..*base
    }
}

/// Writes `lines` to the file at `path` through the document a tab has it
/// open in, laid out as that file is: every tab on it then shows what was
/// written, saved. `None` where no tab has the file open, for the caller to
/// write it itself.
pub fn save_through(path: &Path, lines: &[String]) -> Option<io::Result<FileStamp>> {
    with(|session| {
        let document = session.document_of(path)?;
        let shape = session.files.get(&document)?.shape;
        Some(text_file::save(path, lines, shape).inspect(|stamp| {
            let view = session
                .files
                .get(&document)
                .and_then(|file| file.views.first());
            if let Some(view) = view.copied() {
                replace_text(&mut session.editor, view, &lines.join("\n"));
            }
            session.editor.mark_saved(document);
            session
                .files
                .entry(document)
                .and_modify(|open| open.stamp = stamp.clone());
        }))
    })
}

/// What a file is known by: where it really is, through every symlink.
fn key_of(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Replaces the whole text of the document `view` shows with `text`, as
/// one edit, every view of it following.
fn replace_text(editor: &mut Editor, view: ViewId, text: &str) {
    let document = editor.view(view).map(viento::View::document);
    let Some(length) = document
        .and_then(|document| editor.document(document))
        .map(viento::Document::len_chars)
    else {
        return;
    };
    editor.apply_edits(view, vec![viento::Edit::new(0..length, text)]);
}
