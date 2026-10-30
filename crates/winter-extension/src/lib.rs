//! The API Winter's extensions are written against.
//!
//! An extension is a Rust crate implementing [`Extension`], compiled into
//! Winter. When Winter starts, each extension is asked to
//! [`activate`](Extension::activate): it adds its commands to the
//! [`Registry`], the [`Event`]s it wants to hear of, the status bar
//! segments it fills, the completion back-ends it answers as, and the
//! [`View`]s it draws in tabs of their own. Commands, event handlers, and a
//! view's keys run with a [`Host`], the extension's only way to act on
//! Winter: show a notice, open a file or a view, read the file in front,
//! publish problems, set what its segments say, run another command, or
//! [`spawn`] slow work off Winter's thread.
//!
//! Everything an extension registers runs on Winter's thread, between one
//! key and the next, so it must be quick; anything slow (reading many
//! files, running a program) goes to [`spawn`], which hands its result back
//! on Winter's thread when it is done.
//!
//! Extensions depend on this crate alone, never on Winter's own crates, so
//! Winter's insides can change without breaking them; [`API_VERSION`] moves
//! when this crate does in a way an extension must notice.
//!
//! ```
//! use winter_extension::{Extension, Host, Registry};
//!
//! struct Hello;
//!
//! impl Extension for Hello {
//!     fn id(&self) -> &'static str {
//!         "hello"
//!     }
//!
//!     fn activate(&self, registry: &mut Registry) {
//!         registry.command("greet", "Say Hello", |host: &mut dyn Host| host.notify("hello from an extension"));
//!     }
//! }
//! ```

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ========================================================================
// Constants
// ========================================================================

/// The version of this API. Raised whenever a change to it would break an
/// extension written against the one before.
pub const API_VERSION: u32 = 1;

/// What stands between an extension's id and a command's name in the
/// command's full id: `hello.greet`.
pub const ID_SEPARATOR: char = '.';

// ========================================================================
// Data Structures
// ========================================================================

/// What a command does when it runs: given the [`Host`] to act through.
pub type Handler = Box<dyn Fn(&mut dyn Host) + Send + Sync>;

/// What an extension does when an [`Event`] happens.
pub type EventHandler = Box<dyn Fn(&mut dyn Host, &Event) + Send + Sync>;

/// What a completion back-end offers at the cursor a request describes.
pub type Completer = Box<dyn Fn(&CompletionRequest) -> Option<Completions> + Send + Sync>;

/// What makes a fresh [`View`], each time one is opened.
pub type Opener = Box<dyn Fn() -> Box<dyn View> + Send + Sync>;

/// One line a [`View`] draws: runs of text, left to right from its first
/// column.
pub type Line = Vec<Styled>;

/// What slow work hands back when it is done: run on Winter's thread, with
/// the [`Host`] to act through.
pub type Reply = Box<dyn FnOnce(&mut dyn Host) + Send>;

/// Slow work, run off Winter's thread, ending in its [`Reply`].
pub type Work = Box<dyn FnOnce() -> Reply + Send>;

/// Something that happened in Winter, for the extensions that asked to
/// hear of it. More may be added; a handler matches the ones it wants.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Event {
    /// The text of this file, the one in front, has changed, and the typing
    /// has paused: heard once per pause, not once per key.
    Changed(PathBuf),
    /// An editor tab no longer shows this file: it closed, or turned to
    /// another file.
    Closed(PathBuf),
    /// The file in front is now this one, or no file, as when a terminal
    /// takes the keyboard.
    Focused(Option<PathBuf>),
    /// An editor tab now shows this file: a new tab, or one turned to it.
    Opened(PathBuf),
    /// This file was written to disk, by a save or as the keyboard left it.
    Saved(PathBuf),
}

/// An extension's handler for the [`Event`]s, as Winter calls it.
#[non_exhaustive]
pub struct Subscription {
    /// The extension that asked.
    pub extension: &'static str,
    /// What it does when an event happens.
    pub handle: EventHandler,
}

/// A status bar segment an extension fills, through
/// [`Host::set_status`].
#[non_exhaustive]
pub struct Segment {
    /// The extension that added it.
    pub extension: &'static str,
    /// Its full id, `extension.name`: what `status-bar { segments ... }`
    /// names it by.
    pub id: String,
}

/// A completion back-end an extension answers as.
#[non_exhaustive]
pub struct CompletionSource {
    /// What it offers at a cursor.
    pub complete: Completer,
    /// The extension that added it.
    pub extension: &'static str,
    /// Its full id, `extension.name`: what a language's completion
    /// back-ends name it by.
    pub id: String,
}

/// Where completion is asked for: the cursor's line, and the cursor on it.
/// Columns count in UTF-16 code units, as a [`Diagnostic`]'s do.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct CompletionRequest<'a> {
    /// The cursor's column on [`Self::line`].
    pub column: u32,
    /// The document's language, as settings name it: `rust`, `latex`.
    pub language: &'a str,
    /// The text of the cursor's line.
    pub line: &'a str,
    pub path: &'a Path,
    /// The cursor's line, from zero.
    pub row: u32,
}

/// What a completion back-end offers: the column the text it would
/// replace starts at, and what could replace it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct Completions {
    pub items: Vec<CompletionItem>,
    /// The first column replaced, at or before the cursor's.
    pub start: u32,
}

/// One thing that could go at the cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct CompletionItem {
    /// More about it, drawn dim beside it.
    pub detail: Option<String>,
    /// What goes in.
    pub insert: String,
    /// What the popup shows, and what is typed is matched against.
    pub label: String,
}

/// A kind of tab an extension draws, through a [`View`].
#[non_exhaustive]
pub struct ViewKind {
    /// The extension that added it.
    pub extension: &'static str,
    /// Its full id, `extension.name`.
    pub id: String,
    /// What makes the view a newly opened tab shows.
    pub open: Opener,
    /// What its tab is called.
    pub title: String,
}

/// A run of a [`Line`] sharing one tone.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Styled {
    pub text: String,
    pub tone: Tone,
}

/// How a run of text is colored. Winter picks the color from the theme,
/// so a view never names one. More may be added.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum Tone {
    /// Emphasized, in the theme's accent.
    Accent,
    /// De-emphasized: detail, hints.
    Dim,
    /// In the hue of an error.
    Error,
    /// A heading over what follows it.
    Heading,
    /// In the hue of an information.
    Info,
    /// Ordinary text.
    #[default]
    Normal,
    /// In the hue of a warning.
    Warning,
}

/// A key pressed while a [`View`] has the keyboard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct KeyPress {
    pub alt: bool,
    pub ctrl: bool,
    pub key: KeyName,
    pub shift: bool,
}

/// Which key a [`KeyPress`] is, whatever modifiers were held. More may be
/// added.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum KeyName {
    Backspace,
    /// A printable character, the space bar's included.
    Char(char),
    Delete,
    Down,
    End,
    Enter,
    Escape,
    /// A function key, numbered from 1.
    F(u8),
    Home,
    Left,
    PageDown,
    PageUp,
    Right,
    Tab,
    Up,
}

/// A way to have Winter paint again, from any thread: for a [`View`]
/// showing what changes outside Winter's own calls into it.
#[derive(Clone)]
pub struct Repaint(Arc<dyn Fn() + Send + Sync>);

/// A command an extension added, as Winter lists, binds, and runs it.
#[non_exhaustive]
pub struct Command {
    /// The extension that added it.
    pub extension: &'static str,
    /// Its full id, `extension.name`: what a key binding names it by.
    pub id: String,
    /// What it does when it runs.
    pub run: Handler,
    /// What the command palette calls it.
    pub title: String,
}

/// Every extension's contributions, gathered as each activates.
#[derive(Default)]
pub struct Registry {
    /// The commands added, in the order they were.
    commands: Vec<Command>,
    /// The completion back-ends added, in the order they were.
    completions: Vec<CompletionSource>,
    /// The status bar segments added, in the order they were.
    segments: Vec<Segment>,
    /// The kinds of view added, in the order they were.
    views: Vec<ViewKind>,
    /// The handlers asking to hear of events, in the order they asked.
    subscriptions: Vec<Subscription>,
    /// The extension activating now, whose id what it adds is filed under.
    current: &'static str,
}

/// A file as the editor in front holds it: its path and its text, edits
/// included.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Document {
    pub path: PathBuf,
    pub text: String,
}

/// One problem an extension found in a file. Positions count as language
/// servers count them: lines from zero, and columns from zero in UTF-16
/// code units, which for ASCII text are characters.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Diagnostic {
    /// One past the last column it covers, on [`Self::line`].
    pub end: u32,
    pub line: u32,
    pub message: String,
    pub severity: Severity,
    /// The first column it covers.
    pub start: u32,
}

/// How bad a [`Diagnostic`] says the problem is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity {
    Error,
    Hint,
    Info,
    Warning,
}

// ========================================================================
// Traits
// ========================================================================

/// An extension: something compiled into Winter that adds to it when it
/// starts.
pub trait Extension: Send + Sync {
    /// Its id, unique among extensions: lower case, no spaces, no
    /// [`ID_SEPARATOR`]. Its commands' ids start with it.
    fn id(&self) -> &'static str;

    /// Add what the extension contributes to `registry`.
    fn activate(&self, registry: &mut Registry);
}

/// A tab an extension draws and drives: what it shows, and what it does
/// with the keys pressed while it has the keyboard. It lives on Winter's
/// thread, and each call into it must be quick.
pub trait View {
    /// The lines to show in a tab `rows` high and `columns` wide; more are
    /// cut off at its bottom, and a longer line at its right.
    fn render(&mut self, rows: usize, columns: usize) -> Vec<Line>;

    /// Act on `key`, through `host`; `false` leaves it to Winter's own
    /// bindings. What is asked of `host` is done once this returns, and
    /// [`Host::document`] answers `None`, the view being in front.
    fn on_key(&mut self, host: &mut dyn Host, key: &KeyPress) -> bool;

    /// The line of what [`Self::render`] returned to band as the cursor's,
    /// if any.
    fn cursor_line(&self) -> Option<usize> {
        None
    }

    /// Act on a click at `column` of `line` of what [`Self::render`]
    /// returned, through `host` as [`Self::on_key`] does; `false` leaves it
    /// to Winter.
    fn on_click(&mut self, _host: &mut dyn Host, _line: usize, _column: usize) -> bool {
        false
    }

    /// Scroll by `lines`, positive upwards, as the wheel turned; `false`
    /// leaves it to Winter.
    fn on_scroll(&mut self, _lines: isize) -> bool {
        false
    }
}

/// What a running command may ask of Winter.
pub trait Host {
    /// Show `message` as a notice.
    fn notify(&mut self, message: &str);

    /// Open the file at `path`, at `line` (from one) where given.
    fn open_file(&mut self, path: &Path, line: Option<usize>);

    /// Open this extension's view `name`, added with [`Registry::view`], in
    /// a tab beside the one in front, or go to the tab already showing it.
    fn open_view(&mut self, name: &str);

    /// The file the editor in front is editing, if it edits one.
    fn document(&self) -> Option<Document>;

    /// Take `diagnostics` as everything this extension finds wrong in
    /// `path` now, replacing what it said of it before; none says the file
    /// is clean. They join the language servers' problems: underlined, and
    /// stepped through by the problem commands.
    fn publish_diagnostics(&mut self, path: &Path, diagnostics: Vec<Diagnostic>);

    /// Run the command Winter, or any extension, knows as `id`.
    fn run_command(&mut self, id: &str);

    /// Have this extension's status bar segment `name` say `text`, or
    /// nothing with `None`, until told otherwise. A name the extension did
    /// not add with [`Registry::status_segment`] is ignored.
    fn set_status(&mut self, name: &str, text: Option<String>);

    /// Run `work` off Winter's thread, and its reply back on it once done.
    /// [`spawn`] is the typed way to call this.
    fn spawn_work(&mut self, work: Work);

    /// What has Winter paint again from any thread, to be kept and used
    /// whenever what a [`View`] shows changes on its own.
    fn repaint(&self) -> Repaint;
}

// ========================================================================
// CompletionItem
// ========================================================================

impl CompletionItem {
    /// `insert`, shown in the popup as `label`.
    pub fn new(label: impl Into<String>, insert: impl Into<String>) -> Self {
        Self {
            detail: None,
            insert: insert.into(),
            label: label.into(),
        }
    }

    /// The same, with `detail` drawn dim beside it.
    pub fn with_detail(self, detail: impl Into<String>) -> Self {
        Self {
            detail: Some(detail.into()),
            ..self
        }
    }
}

// ========================================================================
// CompletionRequest
// ========================================================================

impl<'a> CompletionRequest<'a> {
    /// Completion asked for in the file at `path`, written in `language`,
    /// with the cursor at `(row, column)` on `line`.
    pub fn new(path: &'a Path, language: &'a str, line: &'a str, (row, column): (u32, u32)) -> Self {
        Self {
            column,
            language,
            line,
            path,
            row,
        }
    }
}

// ========================================================================
// Completions
// ========================================================================

impl Completions {
    /// `items`, each replacing the text from column `start` to the cursor.
    pub fn new(start: u32, items: Vec<CompletionItem>) -> Self {
        Self { items, start }
    }
}

// ========================================================================
// Diagnostic
// ========================================================================

impl Diagnostic {
    /// A problem as bad as `severity`, said by `message`, over `columns`
    /// of line `line`.
    pub fn new(severity: Severity, message: impl Into<String>, line: u32, columns: Range<u32>) -> Self {
        Self {
            end: columns.end,
            line,
            message: message.into(),
            severity,
            start: columns.start,
        }
    }
}

// ========================================================================
// Document
// ========================================================================

impl Document {
    /// The file at `path`, holding `text`.
    pub fn new(path: PathBuf, text: String) -> Self {
        Self { path, text }
    }
}

// ========================================================================
// KeyPress
// ========================================================================

impl KeyPress {
    /// `key`, pressed with no modifier held. The modifiers are set on the
    /// press after.
    pub fn new(key: KeyName) -> Self {
        Self {
            alt: false,
            ctrl: false,
            key,
            shift: false,
        }
    }
}

// ========================================================================
// Repaint
// ========================================================================

impl Repaint {
    /// A repaint that does `request` when asked for.
    pub fn new(request: impl Fn() + Send + Sync + 'static) -> Self {
        Self(Arc::new(request))
    }

    /// Have Winter paint again soon.
    pub fn request(&self) {
        (self.0)()
    }
}

// ========================================================================
// Styled
// ========================================================================

impl Styled {
    /// `text`, colored as `tone` says.
    pub fn new(tone: Tone, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tone,
        }
    }
}

// ========================================================================
// Registry
// ========================================================================

impl Registry {
    /// Gather the contributions of `extensions`, each activated in turn.
    pub fn activate_all<'a>(extensions: impl IntoIterator<Item = &'a dyn Extension>) -> Self {
        let mut registry = Self::default();
        for extension in extensions {
            registry.current = extension.id();
            extension.activate(&mut registry);
        }
        registry
    }

    /// Add a command named `name`, listed as `title`, that does `run`. Its
    /// full id is the activating extension's id, then `name`.
    pub fn command(
        &mut self,
        name: &str,
        title: &str,
        run: impl Fn(&mut dyn Host) + Send + Sync + 'static,
    ) {
        self.commands.push(Command {
            extension: self.current,
            id: self.full_id(name),
            run: Box::new(run),
            title: title.to_string(),
        });
    }

    /// Every command added, in the order they were.
    pub fn commands(&self) -> &[Command] {
        &self.commands
    }

    /// Add a completion back-end named `name`, offering what `complete`
    /// does. It runs on Winter's thread as the popup opens, so it must be
    /// quick: work anything slow out ahead, and offer what was found.
    pub fn completion(
        &mut self,
        name: &str,
        complete: impl Fn(&CompletionRequest) -> Option<Completions> + Send + Sync + 'static,
    ) {
        self.completions.push(CompletionSource {
            complete: Box::new(complete),
            extension: self.current,
            id: self.full_id(name),
        });
    }

    /// Every completion back-end added, in the order they were.
    pub fn completions(&self) -> &[CompletionSource] {
        &self.completions
    }

    /// The completion back-end whose full id is `id`.
    pub fn find_completion(&self, id: &str) -> Option<&CompletionSource> {
        self.completions.iter().find(|source| source.id == id)
    }

    /// Add a status bar segment named `name`, saying nothing until
    /// [`Host::set_status`] gives it something to.
    pub fn status_segment(&mut self, name: &str) {
        self.segments.push(Segment {
            extension: self.current,
            id: self.full_id(name),
        });
    }

    /// Every status bar segment added, in the order they were.
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Add a kind of view named `name`, its tabs called `title`, each
    /// showing what `open` makes. [`Host::open_view`] opens one.
    pub fn view(
        &mut self,
        name: &str,
        title: &str,
        open: impl Fn() -> Box<dyn View> + Send + Sync + 'static,
    ) {
        self.views.push(ViewKind {
            extension: self.current,
            id: self.full_id(name),
            open: Box::new(open),
            title: title.to_string(),
        });
    }

    /// The kind of view whose full id is `id`.
    pub fn find_view(&self, id: &str) -> Option<&ViewKind> {
        self.views.iter().find(|view| view.id == id)
    }

    /// The status bar segment whose full id is `id`.
    pub fn find_segment(&self, id: &str) -> Option<&Segment> {
        self.segments.iter().find(|segment| segment.id == id)
    }

    /// The full id of what the activating extension names `name`.
    fn full_id(&self, name: &str) -> String {
        format!("{}{ID_SEPARATOR}{name}", self.current)
    }

    /// The command whose full id is `id`.
    pub fn find(&self, id: &str) -> Option<&Command> {
        self.commands.iter().find(|command| command.id == id)
    }

    /// Have `handle` hear of every [`Event`], from now on.
    pub fn on_event(&mut self, handle: impl Fn(&mut dyn Host, &Event) + Send + Sync + 'static) {
        self.subscriptions.push(Subscription {
            extension: self.current,
            handle: Box::new(handle),
        });
    }

    /// Every handler asking to hear of events, in the order they asked.
    pub fn subscriptions(&self) -> &[Subscription] {
        &self.subscriptions
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Run `work` off Winter's thread, then `done` with its result back on
/// Winter's thread, with the [`Host`] to act through: how an extension
/// does anything slow without holding up typing.
///
/// ```
/// use winter_extension::{spawn, Host};
///
/// fn count_lines(host: &mut dyn Host, path: std::path::PathBuf) {
///     spawn(
///         host,
///         move || std::fs::read_to_string(&path).map_or(0, |text| text.lines().count()),
///         |host, lines| host.notify(&format!("{lines} lines")),
///     );
/// }
/// ```
pub fn spawn<T: Send + 'static>(
    host: &mut dyn Host,
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(&mut dyn Host, T) + Send + 'static,
) {
    host.spawn_work(Box::new(move || {
        let result = work();
        Box::new(move |host: &mut dyn Host| done(host, result))
    }));
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// An extension adding a command, a segment, and a completion
    /// back-end, each named `name`.
    struct Adds(&'static str, &'static str);

    impl Extension for Adds {
        fn id(&self) -> &'static str {
            self.0
        }

        fn activate(&self, registry: &mut Registry) {
            registry.command(self.1, "A command", |host| host.notify("ran"));
            registry.status_segment(self.1);
            registry.completion(self.1, |_| None);
        }
    }

    #[test]
    fn test_what_each_extension_adds_is_filed_under_its_own_id() {
        let first = Adds("first", "go");
        let second = Adds("second", "go");
        let registry = Registry::activate_all([&first as &dyn Extension, &second]);
        let ids: Vec<&str> = registry
            .commands()
            .iter()
            .map(|command| command.id.as_str())
            .collect();
        assert_eq!(ids, ["first.go", "second.go"], "the same name, kept apart");
        assert_eq!(
            registry.find("second.go").map(|command| command.extension),
            Some("second")
        );
        assert!(registry.find("go").is_none(), "a bare name is no id");
        let segment = registry.find_segment("first.go").expect("a segment");
        assert_eq!(segment.extension, "first");
        let completion = registry.find_completion("second.go").expect("a back-end");
        assert_eq!(completion.extension, "second");
        assert!(registry.find_completion("go").is_none());
    }

    /// A host running work at once, keeping what it is told.
    #[derive(Default)]
    struct Immediate {
        said: Vec<String>,
    }

    impl Host for Immediate {
        fn notify(&mut self, message: &str) {
            self.said.push(message.to_string());
        }

        fn open_file(&mut self, _path: &Path, _line: Option<usize>) {}

        fn open_view(&mut self, _name: &str) {}

        fn document(&self) -> Option<Document> {
            None
        }

        fn publish_diagnostics(&mut self, _path: &Path, _diagnostics: Vec<Diagnostic>) {}

        fn run_command(&mut self, _id: &str) {}

        fn set_status(&mut self, _name: &str, _text: Option<String>) {}

        fn spawn_work(&mut self, work: Work) {
            let reply = work();
            reply(self);
        }

        fn repaint(&self) -> Repaint {
            Repaint::new(|| {})
        }
    }

    #[test]
    fn test_spawned_work_hands_its_result_to_what_runs_after() {
        let mut host = Immediate::default();
        spawn(
            &mut host,
            || 6 * 7,
            |host, answer| host.notify(&answer.to_string()),
        );
        assert_eq!(host.said, ["42"]);
    }

    /// An extension that hears of saves.
    struct Listens;

    impl Extension for Listens {
        fn id(&self) -> &'static str {
            "listens"
        }

        fn activate(&self, registry: &mut Registry) {
            registry.on_event(|host, event| {
                if let Event::Saved(path) = event {
                    host.notify(&path.display().to_string());
                }
            });
        }
    }

    #[test]
    fn test_an_event_handler_is_filed_under_its_extension() {
        let registry = Registry::activate_all([&Listens as &dyn Extension]);
        let [subscription] = registry.subscriptions() else {
            panic!("one handler");
        };
        assert_eq!(subscription.extension, "listens");
        let mut host = Immediate::default();
        (subscription.handle)(&mut host, &Event::Saved(PathBuf::from("/a.md")));
        assert_eq!(host.said, ["/a.md"]);
    }
}
