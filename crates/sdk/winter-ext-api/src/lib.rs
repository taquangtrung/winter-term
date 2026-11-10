//! The API Winter's extensions are written against.
//!
//! An extension is a Rust crate implementing [`Extension`], compiled into
//! Winter. When Winter starts, each extension is asked to
//! [`activate`](Extension::activate): it adds its commands to the
//! [`Registry`], the [`Event`]s it wants to hear of, the status bar
//! segments it fills, the completion back-ends it answers as, the inline
//! suggestions it offers as you type (an [`InlineProvider`]), the language
//! servers it runs, the [`View`]s it draws in tabs of their own, and the
//! settings it reads from a block of `settings.kdl` named for it. Commands, event handlers, and a
//! view's actions run with a [`Host`], the extension's only way to act on
//! Winter: show a notice, open a file, a view, or a web page, read the file
//! in front, publish problems, set what its segments say, run another
//! command, copy text, [`pick`] from a list in Winter's picker, or
//! [`spawn`] slow work off Winter's thread.
//!
//! A view never sees a key. It declares the keys it answers ([`ViewKey`]),
//! each an action with a label and the keys it starts on, and Winter
//! matches what is pressed against them: the user rebinds them in
//! `keybindings.kdl` and finds them in the Keys editor as Winter's own, and
//! the view is told only which action ran ([`View::on_action`]). Chords
//! held with Ctrl, Alt, or Super are Winter's, so a view may not start on
//! one; a key the view did not declare stays Winter's, whatever it is.
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
//! use winter_ext_api::{Extension, Host, Registry};
//!
//! struct Hello;
//!
//! impl Extension for Hello {
//!     fn id(&self) -> &'static str {
//!         "hello"
//!     }
//!
//!     fn activate(&self, registry: &mut Registry) {
//!         registry.command("hello.greet", "Say Hello", |host: &mut dyn Host| host.notify("hello from an extension"));
//!     }
//! }
//! ```

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// ========================================================================
// Constants
// ========================================================================

/// The version of this API. Raised whenever a change to it would break an
/// extension written against the one before.
pub const API_VERSION: u32 = 2;

/// The custom section of a component that holds its extension's
/// `package.kdl`: what it is, what it asks to reach, and its commands'
/// titles, read before any of its code runs. `winter_ext_guest::export!`
/// writes it, under this name spelled out, as a link section must be.
pub const PACKAGE_SECTION: &str = "winter-package";

/// The id the next picker an extension opens takes.
static NEXT_PICKER: AtomicU64 = AtomicU64::new(1);

/// What stands between an extension's id and the name of a view, status
/// bar segment, or completion back-end in its full id: `hello.board`.
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

/// What is done once an item is chosen from a list offered with
/// [`Host::pick_from`]: given the [`Host`] to act through, and the chosen
/// item's place in the list.
pub type Picked = Box<dyn FnOnce(&mut dyn Host, usize) + Send>;

/// What an extension does with what happens in a picker it opened with
/// [`Host::open_picker`]: given the [`Host`] to act through, the picker, and
/// what happened.
pub type PickerEvents = Box<dyn FnMut(&mut dyn Host, PickerId, PickerEvent) + Send>;

/// Something that happened in Winter, for the extensions that asked to
/// hear of it. More may be added; a handler matches the ones it wants.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Event {
    /// The extension's settings, as they now stand: heard once as Winter
    /// starts, and again each time `settings.kdl` changes them. Heard only
    /// by the extension they are of.
    Configured(Settings),
    /// The text of this file, the one in front, has changed, and the typing
    /// has paused: heard once per pause, not once per key.
    Changed(PathBuf),
    /// An editor tab no longer shows this file: it closed, or turned to
    /// another file.
    Closed(PathBuf),
    /// The file in front is now this one, or no file, as when a terminal
    /// takes the keyboard.
    Focused(Option<PathBuf>),
    /// One of the extension's servers said something. Heard only by the
    /// extension whose server it is.
    Server(ServerEvent),
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

/// A setting an extension reads, declared with [`Registry::setting`]: set
/// in the block of `settings.kdl` named for the extension, as
/// `copilot { enable #true }`.
#[non_exhaustive]
pub struct Setting {
    /// What it is when `settings.kdl` leaves it out, which also says what
    /// kind of value it takes.
    pub default: SettingValue,
    /// One line saying what it does.
    pub doc: String,
    /// The extension that declared it.
    pub extension: &'static str,
    /// Its name in the extension's block.
    pub name: String,
}

/// The value of a setting.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SettingValue {
    /// A whole number.
    Number(i64),
    /// On or off: `#true` or `#false`.
    Switch(bool),
    Text(String),
}

/// An extension's settings as they stand: each one's value, the user's
/// where `settings.kdl` sets it, its default where not.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Settings(BTreeMap<String, SettingValue>);

/// A language server an extension runs, declared with
/// [`Registry::server`] and started with [`Host::start_server`]: a server
/// reading every file, as an assistant's does. Winter starts it, hands it
/// every local file open and each change to it, answers what it asks of an
/// editor, and passes on what it says as [`Event::Server`]. Files on another
/// machine are never handed to it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct ServerSpec {
    /// What it is started with after its program.
    pub args: Vec<String>,
    /// What `initialize` tells it beyond the editor's capabilities
    /// (`initializationOptions`), as JSON.
    pub init_options: Option<String>,
    /// Where [`Host::install_server`] fetches it from, for each platform.
    pub install: Vec<Install>,
    /// Its name among the extension's servers.
    pub name: String,
    /// The file name of its program, looked for on the `PATH` and where
    /// [`Host::install_server`] put it. A WebAssembly extension may run it
    /// only where its `package.kdl` grants it (`run "<program>"`).
    pub program: String,
}

/// Where a server's program is fetched from for one platform: a `.tar.gz`
/// archive, and the file in it that is the program.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Install {
    /// The program's path inside the archive.
    pub member: String,
    pub platform: Platform,
    /// An `https` address.
    pub url: String,
}

/// A machine Winter runs on.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum Platform {
    LinuxArm64,
    LinuxX64,
    MacArm64,
    MacX64,
    WindowsArm64,
    WindowsX64,
}

/// A server an extension declared, as Winter keeps it.
#[non_exhaustive]
pub struct Server {
    /// The extension that declared it.
    pub extension: &'static str,
    /// Its full id, `extension.name`.
    pub id: String,
    pub spec: ServerSpec,
}

/// What one of an extension's servers said.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ServerEvent {
    pub said: ServerSaid,
    /// The server's name among the extension's.
    pub server: String,
}

/// What a server says.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ServerSaid {
    /// The answer to a request sent with [`Host::server_request`].
    Answer(ServerAnswer),
    /// It stopped, or could not start, and why.
    Exited(String),
    /// Something it wants read: a failure or a warning.
    Message(String),
    /// It asked for a page to be opened: one a sign-in goes on at.
    OpenUrl(String),
    /// It has started, and takes requests now.
    Ready,
    /// What it says of its own state.
    Status(ServerStatus),
}

/// The answer to a request: the tag it was sent with, and what it returned
/// as JSON, or the error it gave.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ServerAnswer {
    pub result: Result<String, String>,
    pub tag: u64,
}

/// What a server says of its own state: whether it is in error (not signed
/// in, say), and what it says of it.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ServerStatus {
    pub error: bool,
    pub message: Option<String>,
}

/// The two kinds of inline suggestion.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum InlineKind {
    /// An edit somewhere else, expected after a change: shown in a card for
    /// `Tab` to make.
    Edit,
    /// Text at the cursor, drawn dim as you type: ghost text.
    Ghost,
}

/// What an inline source offers now, as its extension says with
/// [`Host::offer_inline`]: Winter asks it only for what it offers, so an
/// extension switched off, or with its server not yet up, is never asked.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub struct InlineOffer {
    /// Next edits.
    pub edit: bool,
    /// Ghost text.
    pub ghost: bool,
}

/// A place in a document as language servers count it: lines from zero,
/// and columns from zero in UTF-16 code units.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct TextPosition {
    pub column: u32,
    pub line: u32,
}

/// The text between two places in a document.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct TextRange {
    pub end: TextPosition,
    pub start: TextPosition,
}

/// Winter asking an inline source for a suggestion at the cursor, once the
/// typing pauses. The answer goes back with [`Host::answer_inline`] under
/// its ticket, as late as it comes: one for a cursor since moved on is put
/// away.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct InlineRequest {
    /// The cursor.
    pub at: TextPosition,
    /// Whether the document indents with spaces rather than tabs.
    pub insert_spaces: bool,
    /// Whether it was asked for (`C-\`), rather than come by typing: an
    /// answer of nothing is then said.
    pub invoked: bool,
    pub kind: InlineKind,
    pub path: PathBuf,
    /// How many columns a tab stands for.
    pub tab_size: u32,
    /// What the answer goes back under.
    pub ticket: u64,
    /// The document's URI, as its servers were handed it.
    pub uri: String,
    /// The version of the document the extension's servers have been handed,
    /// as `textDocument/didChange` numbered it, where they have it.
    pub version: Option<i64>,
}

/// What an inline source answers.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum InlineAnswer {
    /// The edit expected next.
    Edit(InlineEdit),
    /// Ghost text: each suggestion for the cursor's line, the first shown
    /// first, `M-n` and `M-p` stepping through the rest.
    Ghost(Vec<InlineEdit>),
    /// Nothing to suggest.
    Nothing,
}

/// One suggestion: `text` in place of what `range` covers, and the
/// source's own note of it, handed back when it shows and when it is taken.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct InlineEdit {
    pub range: TextRange,
    pub receipt: String,
    pub text: String,
}

/// An inline source an extension added, as Winter keeps it.
#[non_exhaustive]
pub struct ProvidedInline {
    /// The extension that added it.
    pub extension: &'static str,
    /// Its full id, `extension.name`.
    pub id: String,
    /// What answers for it.
    pub provider: Box<dyn InlineProvider>,
    /// What the status bar and notices call it.
    pub title: String,
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

/// A picker an extension opens with [`Host::open_picker`]: its title, the
/// rows it starts with, whether more are pushed after, and whether the
/// extension hears the query to give rows for it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct PickerSpec {
    /// Whether the extension hears each query, once the typing pauses, and
    /// gives rows for it with [`Host::push_rows`]; otherwise Winter matches
    /// the rows itself by what is typed.
    pub hears_query: bool,
    /// The rows it starts with.
    pub rows: Vec<PickerRow>,
    /// Whether more rows are pushed after, until a push says it is done.
    pub streamed: bool,
    /// What the picker is headed by.
    pub title: String,
}

/// One row of an extension's picker.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct PickerRow {
    /// What it can do, the first run by `Enter`, the rest offered by the
    /// action menu. None: it has one action, `choose`.
    pub actions: Vec<PickerAction>,
    /// Drawn dimmer after the label.
    pub description: Option<String>,
    /// A second line under the label.
    pub detail: Option<String>,
    /// The file it stands for, if it stands for one: drawn with its type's
    /// icon, and previewed in the side panel unless `preview` says more.
    pub file: Option<PathBuf>,
    /// What the row is, and what is typed is matched against.
    pub label: String,
    /// Text the side panel shows for it.
    pub preview: Option<String>,
}

/// One thing a row of an extension's picker can do.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct PickerAction {
    /// What the extension is told was chosen.
    pub id: String,
    /// What the action menu calls it.
    pub title: String,
}

/// Which of an extension's pickers something happened in.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PickerId(pub u64);

/// What happened in an extension's picker.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PickerEvent {
    /// A row's action was chosen; the picker has closed.
    Chosen(PickerChoice),
    /// The picker was put away with nothing chosen: work giving it rows may
    /// stop.
    Closed,
    /// The query settled on this, for a picker that hears it: give its rows
    /// with [`Host::push_rows`], which start again from none.
    Query(String),
}

/// The action chosen, and the row it was chosen on: the row's place among
/// those given since the picker opened, or since the last query for one
/// that hears it.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct PickerChoice {
    /// The action's id: `choose` for a row with no actions of its own.
    pub action: String,
    /// The row's place.
    pub row: usize,
}

/// One item of a list offered with [`Host::pick_from`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct PickItem {
    /// The file it stands for, if it stands for one: the picker draws the
    /// icon of the file's type before it.
    pub file: Option<PathBuf>,
    /// A lightweight hint drawn dim on its right, such as a key or a count.
    pub hint: Option<String>,
    /// What the picker shows, and what is typed is matched against.
    pub label: String,
}

/// A kind of tab an extension draws, through a [`View`].
#[non_exhaustive]
pub struct ViewKind {
    /// The extension that added it.
    pub extension: &'static str,
    /// Its full id, `extension.name`: also the block of `keybindings.kdl`
    /// that rebinds its keys.
    pub id: String,
    /// The keys its views answer, as declared.
    pub keys: Vec<ViewKey>,
    /// What makes the view a newly opened tab shows.
    pub open: Opener,
    /// What its tab is called.
    pub title: String,
}

/// One action a [`View`] answers, and the keys it starts on: what the view
/// is told when it runs, what the Keys editor and the help list call it,
/// and its default keys in `keybindings.kdl` notation (`"j"`, `"<down>"`,
/// `"S-g"`, `"g g"`). Winter matches presses against these; the view never
/// sees a key.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ViewKey {
    /// What [`View::on_action`] is told, and what `keybindings.kdl`
    /// rebinds it by: unique within its view.
    pub id: String,
    /// The keys it starts on, before the user rebinds it. A chord held with
    /// Ctrl, Alt, or Super is refused: those are Winter's.
    pub keys: Vec<String>,
    /// What it does, as the help list says.
    pub label: String,
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
    /// The inline sources added, in the order they were.
    inlines: Vec<ProvidedInline>,
    /// The servers declared, in the order they were.
    servers: Vec<Server>,
    /// The settings declared, in the order they were.
    settings: Vec<Setting>,
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
    /// [`ID_SEPARATOR`]. Its views', segments', and completion back-ends'
    /// ids start with it; its commands' ids are as it names them.
    fn id(&self) -> &'static str;

    /// Add what the extension contributes to `registry`.
    fn activate(&self, registry: &mut Registry);
}

/// A tab an extension draws and drives: what it shows, and what it does
/// when one of the actions its kind declared runs. It lives on Winter's
/// thread, and each call into it must be quick.
pub trait View {
    /// The lines to show in a tab `rows` high and `columns` wide; more are
    /// cut off at its bottom, and a longer line at its right.
    fn render(&mut self, rows: usize, columns: usize) -> Vec<Line>;

    /// Do the action `action`, one of the [`ViewKey`]s its kind declared,
    /// through `host`: Winter matched a key the user bound to it. What is
    /// asked of `host` is done once this returns, and [`Host::document`]
    /// answers `None`, the view being in front.
    fn on_action(&mut self, host: &mut dyn Host, action: &str);

    /// The line of what [`Self::render`] returned to band as the cursor's,
    /// if any.
    fn cursor_line(&self) -> Option<usize> {
        None
    }

    /// The view as a page, in place of [`Self::render`]'s lines: a fragment
    /// of HTML, which Winter lays out and paints with its own engine, in a
    /// stylesheet of the theme's colors (`--fg`, `--bg`, `--muted`,
    /// `--accent`, `--border`, `--tint`, `--info`, `--warning`, `--error`
    /// as CSS variables). No script runs, and nothing it names (a picture,
    /// a stylesheet, a site) is fetched. An element with a `data-action`
    /// attribute naming one of the kind's [`ViewKey`]s runs that action
    /// when clicked; no other pointer event reaches the view. `None`, the
    /// default, shows the lines; a Winter built without its web engine
    /// shows the lines too, so a view with a page keeps them. A page taller
    /// than its tab scrolls, and the element with a `data-current`
    /// attribute is scrolled into view each time the page changes.
    fn page(&mut self) -> Option<String> {
        None
    }

    /// Act on a click at `column` of `line` of what [`Self::render`]
    /// returned, through `host` as [`Self::on_action`] does; `false` leaves it
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

/// Where an extension's inline suggestions come from: asked as typing
/// pauses, and told what became of what it offered. Each call runs on
/// Winter's thread and must be quick: an ask sends a request and returns,
/// its answer handed back later with [`Host::answer_inline`].
pub trait InlineProvider: Send + Sync {
    /// Ask for a suggestion, as `request` says.
    fn ask(&self, host: &mut dyn Host, request: &InlineRequest);

    /// The suggestion `receipt` is the note of now shows.
    fn shown(&self, _host: &mut dyn Host, _kind: InlineKind, _receipt: &str) {}

    /// The suggestion `receipt` is the note of was taken, all of it.
    fn taken(&self, _host: &mut dyn Host, _kind: InlineKind, _receipt: &str) {}
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

    /// Open the web page at `url` in the browser; only an `https` page is
    /// opened. Each one opened is noted in the notice log.
    fn open_url(&mut self, url: &str);

    /// Put `text` on the clipboard, noted in the notice log.
    fn copy_text(&mut self, text: &str);

    /// Start this extension's server `name`, declared with
    /// [`Registry::server`]: `program` where given, else its program as
    /// found. Whether it started is heard as [`ServerSaid::Ready`] or
    /// [`ServerSaid::Exited`]. One running already is left running.
    fn start_server(&mut self, name: &str, program: Option<PathBuf>);

    /// Fetch this extension's server `name` for this machine, as its
    /// [`Install`]s say, where it is found the next time it starts.
    fn install_server(&mut self, name: &str);

    /// Send this extension's running server `name` the request `method`,
    /// with `params` as JSON; its answer comes as [`ServerSaid::Answer`]
    /// under `tag`, which the extension chooses. One not running answers
    /// at once with an error.
    fn server_request(&mut self, name: &str, tag: u64, method: &str, params: &str);

    /// Send this extension's running server `name` the notification
    /// `method`, with `params` as JSON.
    fn server_notify(&mut self, name: &str, method: &str, params: &str);

    /// Say what this extension's inline source `name` offers from now on;
    /// it offers nothing until said.
    fn offer_inline(&mut self, name: &str, offer: InlineOffer);

    /// Answer the inline request `ticket`.
    fn answer_inline(&mut self, ticket: u64, answer: InlineAnswer);

    /// Have this extension's status bar segment `name` say `text`, or
    /// nothing with `None`, until told otherwise. A name the extension did
    /// not add with [`Registry::status_segment`] is ignored.
    fn set_status(&mut self, name: &str, text: Option<String>);

    /// Run `work` off Winter's thread, and its reply back on it once done.
    /// [`spawn`] is the typed way to call this.
    fn spawn_work(&mut self, work: Work);

    /// Open the picker `id`, of the extension's own, as `spec` says, telling
    /// `events` what happens in it: a row's action chosen, or the query
    /// settled for a picker that hears it. [`open_picker`] is the typed way
    /// to call this.
    fn open_picker(&mut self, id: PickerId, spec: PickerSpec, events: PickerEvents);

    /// Give `picker` more rows, after those it has; `done` once there are
    /// no more to come.
    fn push_rows(&mut self, picker: PickerId, rows: Vec<PickerRow>, done: bool);

    /// Offer `items` in Winter's picker, titled `title`, and do `picked`
    /// with the place of the one chosen. Put away without a choice, or
    /// replaced by another list, nothing is done. [`pick`] is the typed way
    /// to call this; a picker opened with [`Host::open_picker`] underneath.
    fn pick_from(&mut self, title: &str, items: Vec<PickItem>, picked: Picked) {
        let rows = items.into_iter().map(PickerRow::from_item).collect();
        let mut picked = Some(picked);
        let events: PickerEvents = Box::new(move |host, _, event| {
            if let (PickerEvent::Chosen(choice), Some(picked)) = (event, picked.take()) {
                picked(host, choice.row);
            }
        });
        self.open_picker(
            PickerId::fresh(),
            PickerSpec::new(title).with_rows(rows),
            events,
        );
    }

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
    pub fn new(
        path: &'a Path,
        language: &'a str,
        line: &'a str,
        (row, column): (u32, u32),
    ) -> Self {
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
    pub fn new(
        severity: Severity,
        message: impl Into<String>,
        line: u32,
        columns: Range<u32>,
    ) -> Self {
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
// ViewKey
// ========================================================================

impl ViewKey {
    /// The action `id`, said to do `label`, starting on `keys`.
    pub fn new(id: &str, label: &str, keys: &[&str]) -> Self {
        Self {
            id: id.to_string(),
            keys: keys.iter().map(|key| key.to_string()).collect(),
            label: label.to_string(),
        }
    }
}

// ========================================================================
// PickerSpec
// ========================================================================

impl PickerSpec {
    /// A picker headed by `title`, with no rows yet.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Self::default()
        }
    }

    /// The same, starting with `rows`.
    pub fn with_rows(self, rows: Vec<PickerRow>) -> Self {
        Self { rows, ..self }
    }

    /// The same, its rows pushed after it opens.
    pub fn streamed(self) -> Self {
        Self {
            streamed: true,
            ..self
        }
    }

    /// The same, hearing each query to give rows for it.
    pub fn hearing_query(self) -> Self {
        Self {
            hears_query: true,
            ..self
        }
    }
}

// ========================================================================
// PickerRow
// ========================================================================

impl PickerRow {
    /// A row showing `label`.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            ..Self::default()
        }
    }

    /// The same, with `text` dimmer after its label.
    pub fn with_description(self, text: impl Into<String>) -> Self {
        Self {
            description: Some(text.into()),
            ..self
        }
    }

    /// The same, with `text` on a second line.
    pub fn with_detail(self, text: impl Into<String>) -> Self {
        Self {
            detail: Some(text.into()),
            ..self
        }
    }

    /// The same, standing for the file at `path`.
    pub fn with_file(self, path: impl Into<PathBuf>) -> Self {
        Self {
            file: Some(path.into()),
            ..self
        }
    }

    /// The same, the side panel showing `text` for it.
    pub fn with_preview(self, text: impl Into<String>) -> Self {
        Self {
            preview: Some(text.into()),
            ..self
        }
    }

    /// The same, able to do `id`, called `title` in the action menu.
    pub fn with_action(mut self, id: impl Into<String>, title: impl Into<String>) -> Self {
        self.actions.push(PickerAction {
            id: id.into(),
            title: title.into(),
        });
        self
    }

    /// The row a [`PickItem`] is, as [`Host::pick_from`] offers it.
    fn from_item(item: PickItem) -> Self {
        Self {
            description: item.hint,
            file: item.file,
            label: item.label,
            ..Self::default()
        }
    }
}

// ========================================================================
// PickerId
// ========================================================================

impl PickerId {
    /// An id no picker opened before has had.
    pub fn fresh() -> Self {
        Self(NEXT_PICKER.fetch_add(1, Ordering::Relaxed))
    }
}

// ========================================================================
// PickerChoice
// ========================================================================

impl PickerChoice {
    /// The action `action` chosen on the row at `row`.
    pub fn new(action: impl Into<String>, row: usize) -> Self {
        Self {
            action: action.into(),
            row,
        }
    }
}

// ========================================================================
// PickItem
// ========================================================================

impl PickItem {
    /// An item the picker shows as `label`.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            file: None,
            hint: None,
            label: label.into(),
        }
    }

    /// The same, with `hint` drawn dim on its right.
    pub fn with_hint(self, hint: impl Into<String>) -> Self {
        Self {
            hint: Some(hint.into()),
            ..self
        }
    }

    /// The same, standing for the file at `path`, drawn with its icon.
    pub fn with_file(self, path: impl Into<PathBuf>) -> Self {
        Self {
            file: Some(path.into()),
            ..self
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
// InlineOffer
// ========================================================================

impl InlineOffer {
    /// Offering ghost text where `ghost`, and next edits where `edit`.
    pub fn new(ghost: bool, edit: bool) -> Self {
        Self { edit, ghost }
    }

    /// Whether it offers `kind`.
    pub fn offers(&self, kind: InlineKind) -> bool {
        match kind {
            InlineKind::Edit => self.edit,
            InlineKind::Ghost => self.ghost,
        }
    }
}

// ========================================================================
// TextPosition
// ========================================================================

impl TextPosition {
    /// Column `column` of line `line`.
    pub fn new(line: u32, column: u32) -> Self {
        Self { column, line }
    }
}

// ========================================================================
// TextRange
// ========================================================================

impl TextRange {
    /// From `start` up to `end`.
    pub fn new(start: TextPosition, end: TextPosition) -> Self {
        Self { end, start }
    }
}

// ========================================================================
// InlineRequest
// ========================================================================

impl InlineRequest {
    /// Asking for `kind` at `at` in the file at `path`, to be answered
    /// under `ticket`; the rest as typing asks, in a document indented with
    /// spaces, four to a tab, that no server has.
    pub fn new(ticket: u64, kind: InlineKind, path: PathBuf, at: TextPosition) -> Self {
        Self {
            at,
            insert_spaces: true,
            invoked: false,
            kind,
            path,
            tab_size: 4,
            ticket,
            uri: String::new(),
            version: None,
        }
    }
}

// ========================================================================
// InlineEdit
// ========================================================================

impl InlineEdit {
    /// `text` in place of what `range` covers, noted as `receipt`.
    pub fn new(range: TextRange, text: impl Into<String>, receipt: impl Into<String>) -> Self {
        Self {
            range,
            receipt: receipt.into(),
            text: text.into(),
        }
    }
}

// ========================================================================
// ServerSpec
// ========================================================================

impl ServerSpec {
    /// A server named `name`, its program `program`, started with no
    /// arguments.
    pub fn new(name: impl Into<String>, program: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            program: program.into(),
            ..Self::default()
        }
    }

    /// The same, started with `args` after its program.
    pub fn with_args(self, args: &[&str]) -> Self {
        Self {
            args: args.iter().map(|arg| arg.to_string()).collect(),
            ..self
        }
    }

    /// The same, `initialize` telling it `options`, as JSON.
    pub fn with_init_options(self, options: impl Into<String>) -> Self {
        Self {
            init_options: Some(options.into()),
            ..self
        }
    }

    /// The same, fetched for `platform` as `install` says.
    pub fn with_install(mut self, install: Install) -> Self {
        self.install.push(install);
        self
    }
}

// ========================================================================
// Install
// ========================================================================

impl Install {
    /// The program at `member` in the archive at `url`, for `platform`.
    pub fn new(platform: Platform, url: impl Into<String>, member: impl Into<String>) -> Self {
        Self {
            member: member.into(),
            platform,
            url: url.into(),
        }
    }
}

// ========================================================================
// Platform
// ========================================================================

impl Platform {
    /// The machine this is built for, if it is one Winter knows.
    pub fn this_machine() -> Option<Self> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "aarch64") => Some(Self::LinuxArm64),
            ("linux", "x86_64") => Some(Self::LinuxX64),
            ("macos", "aarch64") => Some(Self::MacArm64),
            ("macos", "x86_64") => Some(Self::MacX64),
            ("windows", "aarch64") => Some(Self::WindowsArm64),
            ("windows", "x86_64") => Some(Self::WindowsX64),
            _ => None,
        }
    }
}

// ========================================================================
// ServerEvent
// ========================================================================

impl ServerEvent {
    /// `said` by the server `server`.
    pub fn new(server: impl Into<String>, said: ServerSaid) -> Self {
        Self {
            said,
            server: server.into(),
        }
    }
}

// ========================================================================
// ServerAnswer
// ========================================================================

impl ServerAnswer {
    /// The answer to the request tagged `tag`.
    pub fn new(tag: u64, result: Result<String, String>) -> Self {
        Self { result, tag }
    }
}

// ========================================================================
// ServerStatus
// ========================================================================

impl ServerStatus {
    /// In error or not, saying `message`.
    pub fn new(error: bool, message: Option<String>) -> Self {
        Self { error, message }
    }
}

// ========================================================================
// Settings
// ========================================================================

impl Settings {
    /// Settings holding `values`, by name.
    pub fn new(values: BTreeMap<String, SettingValue>) -> Self {
        Self(values)
    }

    /// The value of the setting `name`.
    pub fn get(&self, name: &str) -> Option<&SettingValue> {
        self.0.get(name)
    }

    /// The setting `name`, where it is a switch.
    pub fn switch(&self, name: &str) -> Option<bool> {
        match self.get(name)? {
            SettingValue::Switch(on) => Some(*on),
            SettingValue::Number(_) | SettingValue::Text(_) => None,
        }
    }

    /// The setting `name`, where it is a number.
    pub fn number(&self, name: &str) -> Option<i64> {
        match self.get(name)? {
            SettingValue::Number(number) => Some(*number),
            SettingValue::Switch(_) | SettingValue::Text(_) => None,
        }
    }

    /// The setting `name`, where it is text.
    pub fn text(&self, name: &str) -> Option<&str> {
        match self.get(name)? {
            SettingValue::Text(text) => Some(text),
            SettingValue::Number(_) | SettingValue::Switch(_) => None,
        }
    }

    /// Every setting and its value, by name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &SettingValue)> {
        self.0.iter().map(|(name, value)| (name.as_str(), value))
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

    /// Add a command whose id is `id`, as written, listed as `title`, that
    /// does `run`. When loaded as
    /// a WebAssembly extension, `title` is the code default and can be
    /// overridden by the extension's `package.kdl` manifest.
    pub fn command(
        &mut self,
        id: &str,
        title: &str,
        run: impl Fn(&mut dyn Host) + Send + Sync + 'static,
    ) {
        self.commands.push(Command {
            extension: self.current,
            id: id.to_string(),
            run: Box::new(run),
            title: title.to_string(),
        });
    }

    /// Add an action handler whose id is `id`, as written, that does `run`,
    /// without hardcoding a display title. The display title will be
    /// populated from the extension's `package.kdl`.
    pub fn action(&mut self, id: &str, run: impl Fn(&mut dyn Host) + Send + Sync + 'static) {
        self.command(id, "", run);
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

    /// Add a kind of view named `name`, its tabs called `title`, answering
    /// `keys`, each showing what `open` makes. [`Host::open_view`] opens
    /// one.
    pub fn view(
        &mut self,
        name: &str,
        title: &str,
        keys: Vec<ViewKey>,
        open: impl Fn() -> Box<dyn View> + Send + Sync + 'static,
    ) {
        self.views.push(ViewKind {
            extension: self.current,
            id: self.full_id(name),
            keys,
            open: Box::new(open),
            title: title.to_string(),
        });
    }

    /// Declare a setting named `name`, `default` where `settings.kdl` leaves
    /// it out, said by `doc` to do what it does. The extension hears what
    /// its settings are through [`Event::Configured`].
    pub fn setting(&mut self, name: &str, default: SettingValue, doc: &str) {
        self.settings.push(Setting {
            default,
            doc: doc.to_string(),
            extension: self.current,
            name: name.to_string(),
        });
    }

    /// Add an inline source named `name`, called `title` where its
    /// suggestions show, answered by `provider`. It offers nothing until
    /// the extension says so with [`Host::offer_inline`].
    pub fn inline(&mut self, name: &str, title: &str, provider: impl InlineProvider + 'static) {
        self.inlines.push(ProvidedInline {
            extension: self.current,
            id: self.full_id(name),
            provider: Box::new(provider),
            title: title.to_string(),
        });
    }

    /// Every inline source added, in the order they were.
    pub fn inlines(&self) -> &[ProvidedInline] {
        &self.inlines
    }

    /// The inline source whose full id is `id`.
    pub fn find_inline(&self, id: &str) -> Option<&ProvidedInline> {
        self.inlines.iter().find(|inline| inline.id == id)
    }

    /// Declare a server the extension runs, as `spec` says, started with
    /// [`Host::start_server`] by its name.
    pub fn server(&mut self, spec: ServerSpec) {
        self.servers.push(Server {
            extension: self.current,
            id: self.full_id(&spec.name),
            spec,
        });
    }

    /// Every server declared, in the order they were.
    pub fn servers(&self) -> &[Server] {
        &self.servers
    }

    /// The server whose full id is `id`.
    pub fn find_server(&self, id: &str) -> Option<&Server> {
        self.servers.iter().find(|server| server.id == id)
    }

    /// Every setting declared, in the order they were.
    pub fn settings(&self) -> &[Setting] {
        &self.settings
    }

    /// The settings `extension` declared, at their defaults.
    pub fn default_settings(&self, extension: &str) -> Settings {
        let defaults = self
            .settings
            .iter()
            .filter(|setting| setting.extension == extension)
            .map(|setting| (setting.name.clone(), setting.default.clone()))
            .collect();
        Settings::new(defaults)
    }

    /// Every kind of view added, in the order they were.
    pub fn views(&self) -> &[ViewKind] {
        &self.views
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

    /// The command whose id is `id`.
    pub fn find(&self, id: &str) -> Option<&Command> {
        self.commands.iter().find(|command| command.id == id)
    }

    /// Override the title of the command whose id is `id`.
    pub fn set_command_title(&mut self, id: &str, title: &str) {
        if let Some(command) = self.commands.iter_mut().find(|command| command.id == id) {
            command.title = title.to_string();
        }
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
/// use winter_ext_api::{spawn, Host};
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

/// Open a picker of the extension's own, as `spec` says, telling `events`
/// what happens in it; the picker's id, for pushing rows to it with
/// [`Host::push_rows`].
///
/// ```
/// use winter_ext_api::{open_picker, Host, PickerEvent, PickerRow, PickerSpec};
///
/// fn ask(host: &mut dyn Host) {
///     let spec = PickerSpec::new("Words").hearing_query();
///     open_picker(host, spec, Box::new(|host, picker, event| match event {
///         PickerEvent::Query(query) => {
///             host.push_rows(picker, vec![PickerRow::new(query.to_uppercase())], true)
///         }
///         PickerEvent::Chosen(choice) => host.notify(&choice.action),
///         _ => {}
///     }));
/// }
/// ```
pub fn open_picker(host: &mut dyn Host, spec: PickerSpec, events: PickerEvents) -> PickerId {
    let id = PickerId::fresh();
    host.open_picker(id, spec, events);
    id
}

/// Offer `choices` in Winter's picker, titled `title`, each shown as its
/// [`PickItem`], then `done` with the value of the one chosen: how an
/// extension asks for one of many things.
///
/// ```
/// use std::path::PathBuf;
/// use winter_ext_api::{pick, Host, PickItem};
///
/// fn open_one(host: &mut dyn Host, paths: Vec<PathBuf>) {
///     let choices = paths
///         .into_iter()
///         .map(|path| (PickItem::new(path.display().to_string()), path))
///         .collect();
///     pick(host, "Open", choices, |host, path| host.open_file(&path, None));
/// }
/// ```
pub fn pick<T: Send + 'static>(
    host: &mut dyn Host,
    title: &str,
    choices: Vec<(PickItem, T)>,
    done: impl FnOnce(&mut dyn Host, T) + Send + 'static,
) {
    let (items, mut values): (Vec<_>, Vec<_>) = choices.into_iter().unzip();
    host.pick_from(
        title,
        items,
        Box::new(move |host: &mut dyn Host, chosen| {
            if chosen < values.len() {
                done(host, values.swap_remove(chosen));
            }
        }),
    );
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// An extension adding a command with id `name`, and a segment and a
    /// completion back-end, each named `name`.
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
        let segment = registry.find_segment("first.go").expect("a segment");
        assert_eq!(segment.extension, "first");
        let completion = registry.find_completion("second.go").expect("a back-end");
        assert_eq!(completion.extension, "second");
        assert!(registry.find_completion("go").is_none());
    }

    /// An extension declaring one switch, named `name`.
    struct Declares(&'static str, &'static str);

    impl Extension for Declares {
        fn id(&self) -> &'static str {
            self.0
        }

        fn activate(&self, registry: &mut Registry) {
            registry.setting(self.1, SettingValue::Switch(true), "On.");
        }
    }

    #[test]
    fn test_an_extension_s_default_settings_are_its_own_alone() {
        let (first, second) = (Declares("first", "on"), Declares("second", "off"));
        let registry = Registry::activate_all([&first as &dyn Extension, &second]);
        let settings = registry.default_settings("first");
        assert_eq!(settings.switch("on"), Some(true));
        assert_eq!(settings.get("off"), None, "the other extension's");
        assert_eq!(settings.text("on"), None, "a switch is not text");
    }

    #[test]
    fn test_command_id_is_as_written_not_prefixed_by_extension_id() {
        let registry = Registry::activate_all([&Adds("first", "other.go") as &dyn Extension]);
        let command = registry
            .find("other.go")
            .expect("found by the id as written");
        assert_eq!(command.extension, "first");
        assert!(
            registry.find("first.other.go").is_none(),
            "no prefix is added"
        );
    }

    #[test]
    fn test_set_command_title_overrides_existing_title() {
        let first = Adds("first", "go");
        let mut registry = Registry::activate_all([&first as &dyn Extension]);
        assert_eq!(
            registry.find("go").map(|c| c.title.as_str()),
            Some("A command")
        );
        registry.set_command_title("go", "Custom Title");
        assert_eq!(
            registry.find("go").map(|c| c.title.as_str()),
            Some("Custom Title")
        );
    }

    #[test]
    fn test_action_registers_command_with_empty_title() {
        struct ActionExt;
        impl Extension for ActionExt {
            fn id(&self) -> &'static str {
                "act"
            }
            fn activate(&self, registry: &mut Registry) {
                registry.action("act.do_thing", |_| {});
            }
        }
        let registry = Registry::activate_all([&ActionExt as &dyn Extension]);
        let cmd = registry.find("act.do_thing").expect("command registered");
        assert_eq!(cmd.title, "");
    }

    /// A host running work at once, keeping what it is told, and choosing
    /// the last item of any list it is offered.
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

        fn open_url(&mut self, _url: &str) {}

        fn copy_text(&mut self, _text: &str) {}

        fn start_server(&mut self, _name: &str, _program: Option<PathBuf>) {}

        fn install_server(&mut self, _name: &str) {}

        fn server_request(&mut self, _name: &str, _tag: u64, _method: &str, _params: &str) {}

        fn server_notify(&mut self, _name: &str, _method: &str, _params: &str) {}

        fn offer_inline(&mut self, _name: &str, _offer: InlineOffer) {}

        fn answer_inline(&mut self, _ticket: u64, _answer: InlineAnswer) {}

        fn set_status(&mut self, _name: &str, _text: Option<String>) {}

        fn spawn_work(&mut self, work: Work) {
            let reply = work();
            reply(self);
        }

        /// Choose the last row at once.
        fn open_picker(&mut self, id: PickerId, spec: PickerSpec, mut events: PickerEvents) {
            self.said.push(spec.title);
            if let Some(last) = spec.rows.len().checked_sub(1) {
                let choice = PickerChoice::new("choose", last);
                events(self, id, PickerEvent::Chosen(choice));
            }
        }

        fn push_rows(&mut self, _picker: PickerId, _rows: Vec<PickerRow>, _done: bool) {}

        fn repaint(&self) -> Repaint {
            Repaint::new(|| {})
        }
    }

    #[test]
    fn test_a_pick_hands_the_chosen_item_s_value_not_the_first_s() {
        let mut host = Immediate::default();
        let choices = ["a", "b", "c"]
            .into_iter()
            .map(|name| (PickItem::new(name), name.to_uppercase()))
            .collect();
        pick(&mut host, "Letters", choices, |host, value| {
            host.notify(&value)
        });
        assert_eq!(host.said, ["Letters", "C"]);
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
