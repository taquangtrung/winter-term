//! A text editor window: misty's title bar over Cherry's modal editor,
//! editing the file named on the command line (or an unnamed buffer).
//! The Vim grammar where you navigate, Emacs keys where you type: `i`
//! inserts, Esc returns to Normal mode, `Z Z` saves and quits, `Z Q` quits
//! without saving. The status bar at the bottom names the mode when it is
//! not Normal, shows the keys of a pending sequence or count, the file, and
//! the cursor's line and column, and shows the editor's messages. The
//! input method is on while the view takes text (Insert mode, a prompt):
//! what it composes shows underlined at the cursor, and what it commits is
//! typed. Registers `+` and `*` are the system clipboard and the primary
//! selection: `"+yy` copies a line for other apps, `"+p` pastes theirs.
//! A file whose extension names a bundled grammar (`.rs`, `.py`, `.json`,
//! `.toml`, `.md`) is syntax highlighted, and `af`/`if` and `ac`/`ic`
//! select functions and classes. The mouse moves the cursor, drags select,
//! a double click selects a word and a triple click a line, and the wheel
//! scrolls. In Insert mode `C-x C-n` completes the word before the
//! cursor from the document's words in a popup menu (`C-n` and `C-p`
//! step through it). Lines over the width limit are marked with a warning
//! undercurl past the limit and a sign in the sign column. A key prefix
//! left waiting (`g`, `Z`, `"`, or `C-x` in Insert mode) opens a which-key
//! popup listing the editor's own continuations and what each runs.
//!
//! ```sh
//! cargo run -p winter-cherry --features syntax --example editor -- notes.txt
//! ```

use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Instant;

use misty::key::Key;
use misty::keys;
use misty::notify::Routing;
use misty::text::{Span, StyledText};
use misty::theme::{Theme, ThemeToken};
use misty::{
    AppContext, Application, Decorations, Error, Frame, ImeFocus, Input, ModeBadge, Notice,
    Notifier, Size, StatusBar, StatusLine, SystemClipboard, TitleBar, TitleBarResponse, WhichKey,
    WhichKeyEntry, Widget, WindowId, WindowOptions,
};
use winter_cherry::cherry::{
    Decoration, DecorationKind, DocumentId, Editor, EditorOptions, HostRequest, Mode, Sign,
};
use winter_cherry::cherry_syntax::{self, Language, SyntaxHighlighter};
use winter_cherry::keys::from_cherry;
use winter_cherry::{EditorClipboard, EditorResponse, EditorView};

// ========================================================================
// Constants
// ========================================================================

/// The title of an unnamed buffer.
const UNNAMED: &str = "[unnamed]";

/// What the window title adds after the file name.
const TITLE_SUFFIX: &str = " - misty editor";

/// The smallest size the window may be resized to, in logical pixels.
const MIN_SIZE: Size = Size::new(400, 240);

/// The longest a line may be, in characters, before the rest of it is
/// marked.
const LINE_LIMIT: usize = 100;

/// Who owns the decorations and signs marking long lines.
const LONG_LINES: &str = "long-lines";

/// The sign marking a long line.
const LONG_LINE_SIGN: &str = ">";

/// What the which-key popup says of a key that only leads to more keys.
const MORE_KEYS: &str = "more keys";

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: a title bar, one editor view, and a status bar.
struct EditorApp {
    /// The documents and views.
    editor: Editor,
    /// The one view, as a widget.
    editor_view: EditorView,
    /// Whether the input method is on: while the view takes text.
    ime: ImeFocus,
    /// Where the document is saved, or `None` for an unnamed buffer.
    path: Option<PathBuf>,
    /// The editor's messages for the user, shown on the status bar.
    notifier: Notifier,
    /// The bar along the bottom.
    status_bar: StatusBar,
    /// The colors the window is drawn in.
    theme: Theme,
    /// The window's title bar.
    title_bar: TitleBar,
    /// The popup listing what may follow a waiting key prefix.
    which_key: WhichKey,
    /// The open window, once it is open.
    window: Option<WindowId>,
}

// ========================================================================
// EditorApp Implementations
// ========================================================================

impl EditorApp {
    /// Opens the file at `path`, or an empty buffer when there is none or
    /// it does not exist yet, in a window titled `title`.
    fn open(path: Option<PathBuf>, title: &str) -> Self {
        // No cards here: every message is the status bar's.
        let mut notifier = Notifier::new();
        notifier.set_routing(Routing {
            cards_from: None,
            status_bar: true,
        });
        let text = match &path {
            Some(path) => match fs::read_to_string(path) {
                Ok(text) => {
                    notifier.notify(Notice::info(format!("opened {}", path.display())));
                    text
                }
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    notifier.notify(Notice::info(format!("new file {}", path.display())));
                    String::new()
                }
                Err(error) => {
                    let message = format!("cannot read {}: {error}", path.display());
                    notifier.notify(Notice::error(message));
                    String::new()
                }
            },
            None => String::new(),
        };
        let options = EditorOptions {
            line_numbers: true,
            sign_column: true,
            ..EditorOptions::default()
        };
        let mut editor = Editor::new(options);
        // Registers `+` and `*` reach other apps; without a system
        // clipboard they work inside the editor only.
        let clipboard = SystemClipboard::new();
        if !clipboard.is_system() {
            notifier.notify(Notice::warning(
                "no system clipboard: copies stay in the editor",
            ));
        }
        editor.set_clipboard(EditorClipboard::new(clipboard));
        let document = editor.open_document(&text);
        if let Some(path) = &path {
            highlight(&mut editor, document, path, &mut notifier);
        }
        mark_long_lines(&mut editor, document);
        let view = editor
            .new_view(document)
            .expect("the document was just opened");
        Self {
            editor,
            editor_view: EditorView::new(view),
            ime: ImeFocus::new(),
            path,
            notifier,
            status_bar: StatusBar::new(),
            theme: Theme::dark(),
            title_bar: TitleBar::new(title),
            which_key: WhichKey::new(),
            window: None,
        }
    }

    /// Acts on what the editor asked, in order.
    fn act_on(&mut self, cx: &mut AppContext<'_, ()>, requests: Vec<HostRequest>) {
        for request in requests {
            match request {
                HostRequest::Notice(notice) => self.notifier.notify(Notice::info(notice.message)),
                HostRequest::Quit(_) => cx.exit(),
                HostRequest::Save(save) => self.save(save.document),
                // A request added after this was written: nothing to do.
                _ => {}
            }
        }
        if let Some(window) = self.window {
            cx.request_redraw(window);
        }
    }

    /// Writes `document` to the file it came from.
    fn save(&mut self, id: DocumentId) {
        let (Some(path), Some(document)) = (&self.path, self.editor.document(id)) else {
            let message = "no file to save to: name one on the command line";
            self.notifier.notify(Notice::warning(message));
            return;
        };
        let lines = document.len_lines();
        match fs::write(path, document.text_with_line_endings()) {
            Ok(()) => {
                self.editor.mark_saved(id);
                let message = format!("wrote {lines} lines to {}", path.display());
                self.notifier.notify(Notice::info(message));
            }
            Err(error) => {
                let message = format!("cannot write {}: {error}", path.display());
                self.notifier.notify(Notice::error(message));
            }
        }
    }

    /// Hands `input` to the editor view and acts on what the editor asks,
    /// following its pending keys after a key.
    fn feed(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        let response = self
            .editor_view
            .lend(&mut self.editor)
            .handle(input, &mut cx.event_cx(window));
        self.answer(cx, response);
        if input.key().is_some() {
            self.follow_pending(cx);
        }
    }

    /// Tells the which-key popup the view's pending keys and the editor's
    /// continuations of them, titled by the commands they run.
    fn follow_pending(&mut self, cx: &mut AppContext<'_, ()>) {
        let view = self.editor_view.view();
        let Some(prefix) = self.editor.view(view).map(|view| {
            view.pending_keys()
                .iter()
                .map(|&key| from_cherry(key))
                .collect::<Vec<Key>>()
        }) else {
            return;
        };
        let entries = self
            .editor
            .key_hints(view)
            .into_iter()
            .map(|hint| {
                let title = match (hint.title, &hint.command) {
                    (Some(title), _) => title.to_owned(),
                    (None, Some(command)) => command.as_str().to_owned(),
                    (None, None) => MORE_KEYS.to_owned(),
                };
                WhichKeyEntry::new(from_cherry(hint.key), title, hint.continues)
            })
            .collect();
        if let (true, Some(window)) = (self.which_key.set_pending(&prefix, entries), self.window) {
            cx.request_redraw(window);
        }
    }

    /// Acts on what the editor asked in `response`, and marks long lines
    /// again, since the text may have changed.
    fn answer(&mut self, cx: &mut AppContext<'_, ()>, response: EditorResponse) {
        match response {
            EditorResponse::Handled => {}
            EditorResponse::Ignored => return,
            EditorResponse::Requests(requests) => self.act_on(cx, requests),
        }
        if let Some(view) = self.editor.view(self.editor_view.view()) {
            let document = view.document();
            mark_long_lines(&mut self.editor, document);
        }
    }

    /// What the status bar shows now: the mode unless Normal, the pending
    /// count and keys, the file, and the cursor's line and column.
    fn status_line(&self) -> StatusLine {
        let file = self
            .path
            .as_ref()
            .map_or_else(|| UNNAMED.to_owned(), |path| path.display().to_string());
        let line = StatusLine::new(Span::new(file, ThemeToken::Muted.into()));
        let Some(view) = self.editor.view(self.editor_view.view()) else {
            return line;
        };
        let line = match mode_badge(view.mode()) {
            Some(badge) => line.with_mode(badge),
            None => line,
        };
        let count = view
            .count()
            .map_or_else(String::new, |count| count.to_string());
        let pending: Vec<Key> = view
            .pending_keys()
            .iter()
            .map(|&key| from_cherry(key))
            .collect();
        let line = line.with_pending(format!("{count}{}", keys::label(&pending)));
        match self
            .editor
            .document(view.document())
            .and_then(|document| document.char_to_position(view.cursor()))
        {
            Some(position) => line.with_right(StyledText::plain(
                format!("Ln {}, Col {}", position.line + 1, position.column + 1),
                ThemeToken::Muted.into(),
            )),
            None => line,
        }
    }
}

impl Application for EditorApp {
    type Message = ();

    fn init(&mut self, _cx: &mut AppContext<'_, ()>, window: WindowId) {
        self.window = Some(window);
    }

    fn input(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        match self.title_bar.handle(input, &mut cx.event_cx(window)) {
            TitleBarResponse::Close => {
                cx.exit();
                return;
            }
            // No menubar and no tabs in this bar.
            TitleBarResponse::Handled
            | TitleBarResponse::OpenMenu
            | TitleBarResponse::OpenMenuTitle(_)
            | TitleBarResponse::Tabs(_) => return,
            TitleBarResponse::Ignored => {}
        }
        match input {
            Input::CloseRequested => cx.exit(),
            _ => self.feed(cx, window, input),
        }
    }

    fn about_to_wait(&mut self, cx: &mut AppContext<'_, ()>) -> Option<Instant> {
        let window = self.window?;
        let editor_deadline = self
            .editor_view
            .about_to_wait(&mut self.editor, &mut cx.event_cx(window));
        let requests = self.editor.take_requests();
        if !requests.is_empty() {
            self.act_on(cx, requests);
        }
        // A prefix that timed out is no longer pending.
        self.follow_pending(cx);
        let which_key_deadline = self.which_key.about_to_wait(&mut cx.event_cx(window));
        // Messages go when their time is up, and the bar shows what is left.
        if self.notifier.toasts.expire(Instant::now()) {
            cx.request_redraw(window);
        }
        let flash_deadline = self.notifier.toasts.next_expiry();
        let caret = self.editor_view.ime_area(&self.editor);
        self.ime.follow(&mut cx.event_cx(window), caret);
        [editor_deadline, which_key_deadline, flash_deadline]
            .into_iter()
            .flatten()
            .min()
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        frame.clear(self.theme.palette.background.into());
        let bounds = frame.bounds();
        let area = self.title_bar.draw(frame, bounds, &self.theme);
        let line = self.status_line();
        let text_area = self
            .status_bar
            .lend(&line, &self.notifier)
            .draw(frame, area, &self.theme);
        self.editor_view
            .lend(&mut self.editor)
            .draw(frame, text_area, &self.theme);
        self.which_key.draw(frame, text_area, &self.theme);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The badge naming `mode`, or `None` for Normal mode, which needs none.
fn mode_badge(mode: Mode) -> Option<ModeBadge> {
    match mode {
        Mode::CommandLine => Some(ModeBadge::new("COMMAND", ThemeToken::Accent)),
        Mode::Insert => Some(ModeBadge::new("INSERT", ThemeToken::Accent)),
        Mode::Normal => None,
        Mode::OperatorPending => Some(ModeBadge::new("PENDING", ThemeToken::Muted)),
        Mode::Replace => Some(ModeBadge::new("REPLACE", ThemeToken::Danger)),
        Mode::Visual => Some(ModeBadge::new("VISUAL", ThemeToken::Warning)),
        Mode::VisualBlock => Some(ModeBadge::new("VISUAL BLOCK", ThemeToken::Warning)),
        Mode::VisualLine => Some(ModeBadge::new("VISUAL LINE", ThemeToken::Warning)),
        // A mode added after this was written: no badge.
        _ => None,
    }
}

/// Highlights `document` by the grammar its file's extension names, if
/// one is bundled, with the structural text objects; says on the status
/// bar when the grammar fails to load.
fn highlight(editor: &mut Editor, document: DocumentId, path: &Path, notifier: &mut Notifier) {
    let Some(language) = path
        .extension()
        .and_then(|extension| extension.to_str())
        .and_then(Language::from_extension)
    else {
        return;
    };
    let Some(text) = editor.document(document) else {
        return;
    };
    match SyntaxHighlighter::new(language, text) {
        Ok(highlighter) => {
            editor.set_highlighter(document, Some(Box::new(highlighter)));
            // Fails only when the ids are taken, which nothing here does.
            let _ = cherry_syntax::register_text_objects(editor);
        }
        Err(error) => notifier.notify(Notice::error(format!("no highlighting: {error}"))),
    }
}

/// Marks every line of `document` longer than [`LINE_LIMIT`] characters:
/// a warning undercurl under the characters past the limit and a sign in
/// the sign column.
fn mark_long_lines(editor: &mut Editor, document: DocumentId) {
    let Some(text) = editor.document(document) else {
        return;
    };
    let mut decorations = Vec::new();
    let mut signs = Vec::new();
    let mut line_start = 0;
    for (line, content) in text.to_string().split('\n').enumerate() {
        let length = content.chars().count();
        if length > LINE_LIMIT {
            let range = line_start + LINE_LIMIT..line_start + length;
            decorations.push(Decoration::new(range, DecorationKind::Warning));
            signs.push(Sign::new(line, LONG_LINE_SIGN, DecorationKind::Warning));
        }
        line_start += length + 1;
    }
    editor.set_decorations(document, LONG_LINES, decorations);
    editor.set_signs(document, LONG_LINES, signs);
}

/// The window title for the file at `path`, or for an unnamed buffer.
fn title_for(path: Option<&PathBuf>) -> String {
    let name = path.map_or_else(|| UNNAMED.to_owned(), |path| path.display().to_string());
    format!("{name}{TITLE_SUFFIX}")
}

fn main() -> Result<(), Error> {
    let path = env::args_os().nth(1).map(PathBuf::from);
    let title = title_for(path.as_ref());
    let app = EditorApp::open(path, &title);
    let options = WindowOptions::new(title)
        .with_decorations(Decorations::Frameless)
        .with_min_size(MIN_SIZE);
    misty::run(app, options)
}
