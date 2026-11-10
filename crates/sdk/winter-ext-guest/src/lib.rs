//! Builds a Winter extension, written against `winter-ext-api` as one
//! compiled in is, as a WebAssembly component: a file Winter loads at
//! start and runs sandboxed, rather than code linked into it.
//!
//! An extension crate adds one line, and is built for `wasm32-wasip2`:
//!
//! ```ignore
//! winter_ext_guest::export!(MyExtension);
//! ```
//!
//! ```text
//! cargo rustc --release --target wasm32-wasip2 --crate-type cdylib
//! ```
//!
//! Everything the extension registers runs as it would compiled in, with
//! two differences the sandbox makes: what it asks of the [`Host`] is done
//! once Winter's call into it returns, and [`spawn`](winter_ext_api::spawn)ed
//! work runs off Winter's thread but not beside the extension's other calls.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use winter_ext_api::{
    CompletionRequest, Completions, Diagnostic, Document, Event, Extension, Host, InlineAnswer,
    InlineEdit, InlineKind, InlineOffer, InlineRequest, Line, PickerChoice, PickerEvent,
    PickerEvents, PickerId, PickerRow, PickerSpec, Platform, Registry, Repaint, Reply,
    ServerAnswer, ServerEvent, ServerSaid, ServerSpec, ServerStatus, SettingValue, Settings,
    Severity, TextPosition, TextRange, Tone, View, Work, API_VERSION, ID_SEPARATOR,
};

use self::bindings::exports::winter::extension::guest::Guest;
use self::bindings::winter::extension::{host, types};

#[doc(hidden)]
pub use winter_ext_api;
#[doc(hidden)]
pub use wit_bindgen;

/// So the bindings, which name this crate as an extension crate would, can
/// be generated inside it too.
extern crate self as winter_ext_guest;

/// The component's side of the contract with Winter, generated from it.
#[doc(hidden)]
pub mod bindings {
    wit_bindgen::generate!({
        world: "extension",
        path: "../winter-ext-api/wit",
        pub_export_macro: true,
        default_bindings_module: "winter_ext_guest::bindings",
        runtime_path: "winter_ext_guest::wit_bindgen::rt",
    });
}

// ========================================================================
// Constants
// ========================================================================

/// The extension, activated once, as Winter first asks.
static ACTIVATED: OnceLock<Activated> = OnceLock::new();

thread_local! {
    /// The views open, by the number Winter knows each by.
    static VIEWS: RefCell<HashMap<u32, Box<dyn View>>> = RefCell::new(HashMap::new());

    /// Work spawned and not yet run, by its number.
    static WORK: RefCell<HashMap<u32, Work>> = RefCell::new(HashMap::new());

    /// Work run whose reply is not yet back on Winter's thread.
    static REPLIES: RefCell<HashMap<u32, Reply>> = RefCell::new(HashMap::new());

    /// What each open picker is told of what happens in it, by its number.
    static PICKERS: RefCell<HashMap<u64, PickerEvents>> = RefCell::new(HashMap::new());

    /// The number the next view or work is known by.
    static NEXT: Cell<u32> = const { Cell::new(0) };
}

// ========================================================================
// Data Structures
// ========================================================================

/// The extension, and everything it registered as it activated.
struct Activated {
    id: &'static str,
    registry: Registry,
}

/// The [`Host`] an extension acts through inside the component: each call
/// crosses to Winter.
struct GuestHost;

// ========================================================================
// Traits
// ========================================================================

/// How the component makes its extension. [`export!`] implements it.
pub trait Plugin {
    fn extension() -> Box<dyn Extension>;
}

// ========================================================================
// Macros
// ========================================================================

/// Make the extension `$extension` the one this component exports, its
/// crate's `package.kdl` embedded in the section Winter reads before
/// running any of it ([`PACKAGE_SECTION`](winter_ext_api::PACKAGE_SECTION)).
#[macro_export]
macro_rules! export {
    ($extension:expr) => {
        #[used]
        #[link_section = "winter-package"]
        static WINTER_PACKAGE: [u8; include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/package.kdl"
        ))
        .len()] = *include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/package.kdl"));

        struct WinterComponent;

        impl $crate::Plugin for WinterComponent {
            fn extension() -> ::std::boxed::Box<dyn $crate::winter_ext_api::Extension> {
                ::std::boxed::Box::new($extension)
            }
        }

        $crate::bindings::export!(WinterComponent with_types_in $crate::bindings);
    };
}

// ========================================================================
// Activated
// ========================================================================

impl Activated {
    /// The full id of what the extension names `name`.
    fn full_id(&self, name: &str) -> String {
        format!("{}{ID_SEPARATOR}{name}", self.id)
    }

    /// What the extension added, by the names it gave them.
    fn manifest(&self) -> types::Manifest {
        let name = |id: &str| self.local_name(id).to_string();
        let registry = &self.registry;
        types::Manifest {
            api_version: API_VERSION,
            commands: registry
                .commands()
                .iter()
                .map(|command| types::Titled {
                    name: command.id.clone(),
                    title: command.title.clone(),
                })
                .collect(),
            completions: registry
                .completions()
                .iter()
                .map(|source| name(&source.id))
                .collect(),
            id: self.id.to_string(),
            inlines: registry
                .inlines()
                .iter()
                .map(|inline| types::Titled {
                    name: name(&inline.id),
                    title: inline.title.clone(),
                })
                .collect(),
            listens: !registry.subscriptions().is_empty(),
            segments: registry
                .segments()
                .iter()
                .map(|segment| name(&segment.id))
                .collect(),
            servers: registry
                .servers()
                .iter()
                .map(|server| to_wit_server_spec(&server.spec))
                .collect(),
            settings: registry
                .settings()
                .iter()
                .map(|setting| types::Setting {
                    default: to_wit_setting_value(&setting.default),
                    doc: setting.doc.clone(),
                    name: setting.name.clone(),
                })
                .collect(),
            views: registry
                .views()
                .iter()
                .map(|view| types::ViewSpec {
                    keys: view
                        .keys
                        .iter()
                        .map(|key| types::ViewKey {
                            id: key.id.clone(),
                            keys: key.keys.clone(),
                            label: key.label.clone(),
                        })
                        .collect(),
                    name: name(&view.id),
                    title: view.title.clone(),
                })
                .collect(),
        }
    }

    /// The name the extension gave what is known by the full id `id`.
    fn local_name<'a>(&self, id: &'a str) -> &'a str {
        id.strip_prefix(self.id)
            .and_then(|rest| rest.strip_prefix(ID_SEPARATOR))
            .unwrap_or(id)
    }
}

// ========================================================================
// Guest
// ========================================================================

impl<T: Plugin> Guest for T {
    fn activate() -> types::Manifest {
        let activated = ACTIVATED.get_or_init(|| {
            let extension = T::extension();
            Activated {
                id: extension.id(),
                registry: Registry::activate_all([extension.as_ref()]),
            }
        });
        activated.manifest()
    }

    fn run_command(name: String) {
        let activated = activated();
        if let Some(command) = activated.registry.find(&name) {
            (command.run)(&mut GuestHost);
        }
    }

    fn handle_event(event: types::Event) {
        let event = to_event(event);
        for subscription in activated().registry.subscriptions() {
            (subscription.handle)(&mut GuestHost, &event);
        }
    }

    fn complete(name: String, request: types::CompletionRequest) -> Option<types::Completions> {
        let activated = activated();
        let source = activated
            .registry
            .find_completion(&activated.full_id(&name))?;
        let path = PathBuf::from(request.path);
        let at = (request.row, request.column);
        let request = CompletionRequest::new(&path, &request.language, &request.line, at);
        (source.complete)(&request).map(to_wit_completions)
    }

    fn open_view(name: String) -> u32 {
        let activated = activated();
        let kind = activated
            .registry
            .find_view(&activated.full_id(&name))
            .expect("Winter opens only the views the manifest names");
        let view = next_number();
        VIEWS.with(|views| views.borrow_mut().insert(view, (kind.open)()));
        view
    }

    fn close_view(view: u32) {
        VIEWS.with(|views| views.borrow_mut().remove(&view));
    }

    fn render(view: u32, rows: u32, columns: u32) -> Vec<types::Line> {
        with_view(view, Vec::new(), |view| {
            let lines = view.render(rows as usize, columns as usize);
            lines.into_iter().map(to_wit_line).collect()
        })
    }

    fn cursor_line(view: u32) -> Option<u32> {
        with_view(view, None, |view| {
            view.cursor_line()
                .map(|line| u32::try_from(line).unwrap_or(u32::MAX))
        })
    }

    fn page(view: u32) -> Option<String> {
        with_view(view, None, |view| view.page())
    }

    fn on_action(view: u32, action: String) {
        with_view(view, (), |view| view.on_action(&mut GuestHost, &action));
    }

    fn on_click(view: u32, line: u32, column: u32) -> bool {
        with_view(view, false, |view| {
            view.on_click(&mut GuestHost, line as usize, column as usize)
        })
    }

    fn on_scroll(view: u32, lines: i32) -> bool {
        with_view(view, false, |view| view.on_scroll(lines as isize))
    }

    fn run_work(work: u32) {
        let Some(run) = WORK.with(|pending| pending.borrow_mut().remove(&work)) else {
            return;
        };
        let reply = run();
        REPLIES.with(|replies| replies.borrow_mut().insert(work, reply));
    }

    fn finish_work(work: u32) {
        if let Some(reply) = REPLIES.with(|replies| replies.borrow_mut().remove(&work)) {
            reply(&mut GuestHost);
        }
    }

    fn ask_inline(name: String, request: types::InlineRequest) {
        let activated = activated();
        if let Some(inline) = activated.registry.find_inline(&activated.full_id(&name)) {
            inline
                .provider
                .ask(&mut GuestHost, &to_inline_request(request));
        }
    }

    fn inline_shown(name: String, kind: types::InlineKind, receipt: String) {
        let activated = activated();
        if let Some(inline) = activated.registry.find_inline(&activated.full_id(&name)) {
            inline
                .provider
                .shown(&mut GuestHost, to_inline_kind(kind), &receipt);
        }
    }

    fn inline_taken(name: String, kind: types::InlineKind, receipt: String) {
        let activated = activated();
        if let Some(inline) = activated.registry.find_inline(&activated.full_id(&name)) {
            inline
                .provider
                .taken(&mut GuestHost, to_inline_kind(kind), &receipt);
        }
    }

    fn tell_picker(picker: u64, event: types::PickerEvent) {
        let Some(mut events) = PICKERS.with(|open| open.borrow_mut().remove(&picker)) else {
            return;
        };
        let (event, done) = match event {
            types::PickerEvent::Chosen(choice) => (
                PickerEvent::Chosen(PickerChoice::new(choice.action, choice.row as usize)),
                true,
            ),
            types::PickerEvent::Closed => (PickerEvent::Closed, true),
            types::PickerEvent::Query(query) => (PickerEvent::Query(query), false),
        };
        events(&mut GuestHost, PickerId(picker), event);
        if !done {
            PICKERS.with(|open| open.borrow_mut().insert(picker, events));
        }
    }
}

// ========================================================================
// GuestHost
// ========================================================================

impl Host for GuestHost {
    fn notify(&mut self, message: &str) {
        host::notify(message);
    }

    fn open_file(&mut self, path: &Path, line: Option<usize>) {
        let line = line.map(|line| u32::try_from(line).unwrap_or(u32::MAX));
        host::open_file(&path.to_string_lossy(), line);
    }

    fn open_view(&mut self, name: &str) {
        host::open_view(name);
    }

    fn document(&self) -> Option<Document> {
        host::front_document().map(|document| Document::new(document.path.into(), document.text))
    }

    fn publish_diagnostics(&mut self, path: &Path, diagnostics: Vec<Diagnostic>) {
        let diagnostics: Vec<_> = diagnostics.into_iter().map(to_wit_diagnostic).collect();
        host::publish_diagnostics(&path.to_string_lossy(), &diagnostics);
    }

    fn run_command(&mut self, id: &str) {
        host::run_command(id);
    }

    fn open_url(&mut self, url: &str) {
        host::open_url(url);
    }

    fn copy_text(&mut self, text: &str) {
        host::copy_text(text);
    }

    fn start_server(&mut self, name: &str, program: Option<PathBuf>) {
        let program = program.map(|path| path.to_string_lossy().into_owned());
        host::start_server(name, program.as_deref());
    }

    fn install_server(&mut self, name: &str) {
        host::install_server(name);
    }

    fn server_request(&mut self, name: &str, tag: u64, method: &str, params: &str) {
        host::server_request(name, tag, method, params);
    }

    fn server_notify(&mut self, name: &str, method: &str, params: &str) {
        host::server_notify(name, method, params);
    }

    fn offer_inline(&mut self, name: &str, offer: InlineOffer) {
        let offer = types::InlineOffer {
            edit: offer.edit,
            ghost: offer.ghost,
        };
        host::offer_inline(name, offer);
    }

    fn answer_inline(&mut self, ticket: u64, answer: InlineAnswer) {
        let answer = match answer {
            InlineAnswer::Edit(edit) => types::InlineAnswer::Edit(to_wit_inline_edit(edit)),
            InlineAnswer::Ghost(edits) => {
                types::InlineAnswer::Ghost(edits.into_iter().map(to_wit_inline_edit).collect())
            }
            // An answer added after this crate was built: nothing to show.
            _ => types::InlineAnswer::Nothing,
        };
        host::answer_inline(ticket, &answer);
    }

    fn set_status(&mut self, name: &str, text: Option<String>) {
        host::set_status(name, text.as_deref());
    }

    fn spawn_work(&mut self, work: Work) {
        let number = next_number();
        WORK.with(|pending| pending.borrow_mut().insert(number, work));
        host::spawn(number);
    }

    fn open_picker(&mut self, id: PickerId, spec: PickerSpec, events: PickerEvents) {
        PICKERS.with(|open| open.borrow_mut().insert(id.0, events));
        let spec = types::PickerSpec {
            hears_query: spec.hears_query,
            rows: spec.rows.into_iter().map(to_wit_picker_row).collect(),
            streamed: spec.streamed,
            title: spec.title,
        };
        host::open_picker(id.0, &spec);
    }

    fn push_rows(&mut self, picker: PickerId, rows: Vec<PickerRow>, done: bool) {
        let rows: Vec<_> = rows.into_iter().map(to_wit_picker_row).collect();
        host::push_rows(picker.0, &rows, done);
    }

    fn repaint(&self) -> Repaint {
        Repaint::new(host::request_repaint)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The extension, which Winter activates before asking anything else.
fn activated() -> &'static Activated {
    ACTIVATED
        .get()
        .expect("Winter activates an extension before anything else")
}

/// A number no view or work has had.
fn next_number() -> u32 {
    NEXT.with(|next| {
        let number = next.get();
        next.set(number.wrapping_add(1));
        number
    })
}

/// What `act` makes of the view numbered `view`, or `fallback` if none is
/// open. The view is taken out while it acts, so what it does may open or
/// close views without upsetting the others.
fn with_view<R>(view: u32, fallback: R, act: impl FnOnce(&mut dyn View) -> R) -> R {
    let Some(mut open) = VIEWS.with(|views| views.borrow_mut().remove(&view)) else {
        return fallback;
    };
    let result = act(open.as_mut());
    VIEWS.with(|views| views.borrow_mut().insert(view, open));
    result
}

fn to_event(event: types::Event) -> Event {
    match event {
        types::Event::Configured(entries) => Event::Configured(Settings::new(
            entries
                .into_iter()
                .map(|entry| (entry.name, to_setting_value(entry.value)))
                .collect(),
        )),
        types::Event::Server(event) => {
            Event::Server(ServerEvent::new(event.server, to_server_said(event.said)))
        }
        types::Event::Changed(path) => Event::Changed(path.into()),
        types::Event::Closed(path) => Event::Closed(path.into()),
        types::Event::Focused(path) => Event::Focused(path.map(PathBuf::from)),
        types::Event::Opened(path) => Event::Opened(path.into()),
        types::Event::Saved(path) => Event::Saved(path.into()),
    }
}

fn to_inline_kind(kind: types::InlineKind) -> InlineKind {
    match kind {
        types::InlineKind::Edit => InlineKind::Edit,
        types::InlineKind::Ghost => InlineKind::Ghost,
    }
}

fn to_inline_request(request: types::InlineRequest) -> InlineRequest {
    let at = TextPosition::new(request.at.line, request.at.column);
    let kind = to_inline_kind(request.kind);
    let mut made = InlineRequest::new(request.ticket, kind, request.path.into(), at);
    made.insert_spaces = request.insert_spaces;
    made.invoked = request.invoked;
    made.tab_size = request.tab_size;
    made.uri = request.uri;
    made.version = request.version;
    made
}

fn to_wit_inline_edit(edit: InlineEdit) -> types::InlineEdit {
    let at = |at: TextPosition| types::TextPosition {
        column: at.column,
        line: at.line,
    };
    let TextRange { end, start, .. } = edit.range;
    types::InlineEdit {
        range: types::TextRange {
            end: at(end),
            start: at(start),
        },
        receipt: edit.receipt,
        text: edit.text,
    }
}

fn to_server_said(said: types::ServerSaid) -> ServerSaid {
    match said {
        types::ServerSaid::Answer(answer) => {
            ServerSaid::Answer(ServerAnswer::new(answer.tag, answer.result))
        }
        types::ServerSaid::Exited(why) => ServerSaid::Exited(why),
        types::ServerSaid::Message(message) => ServerSaid::Message(message),
        types::ServerSaid::OpenUrl(url) => ServerSaid::OpenUrl(url),
        types::ServerSaid::Ready => ServerSaid::Ready,
        types::ServerSaid::Status(status) => {
            ServerSaid::Status(ServerStatus::new(status.error, status.message))
        }
    }
}

fn to_wit_server_spec(spec: &ServerSpec) -> types::ServerSpec {
    types::ServerSpec {
        args: spec.args.clone(),
        init_options: spec.init_options.clone(),
        install: spec
            .install
            .iter()
            .filter_map(|install| {
                Some(types::Install {
                    member: install.member.clone(),
                    platform: to_wit_platform(install.platform)?,
                    url: install.url.clone(),
                })
            })
            .collect(),
        name: spec.name.clone(),
        program: spec.program.clone(),
    }
}

/// `platform` as the contract names it, or `None` for one added after
/// this crate was built.
fn to_wit_platform(platform: Platform) -> Option<types::Platform> {
    Some(match platform {
        Platform::LinuxArm64 => types::Platform::LinuxArm64,
        Platform::LinuxX64 => types::Platform::LinuxX64,
        Platform::MacArm64 => types::Platform::MacArm64,
        Platform::MacX64 => types::Platform::MacX64,
        Platform::WindowsArm64 => types::Platform::WindowsArm64,
        Platform::WindowsX64 => types::Platform::WindowsX64,
        _ => return None,
    })
}

fn to_setting_value(value: types::SettingValue) -> SettingValue {
    match value {
        types::SettingValue::Number(number) => SettingValue::Number(number),
        types::SettingValue::Switch(on) => SettingValue::Switch(on),
        types::SettingValue::Text(text) => SettingValue::Text(text),
    }
}

fn to_wit_setting_value(value: &SettingValue) -> types::SettingValue {
    match value {
        SettingValue::Number(number) => types::SettingValue::Number(*number),
        SettingValue::Switch(on) => types::SettingValue::Switch(*on),
        SettingValue::Text(text) => types::SettingValue::Text(text.clone()),
        // A kind added after this crate was built: read as text.
        _ => types::SettingValue::Text(String::new()),
    }
}

fn to_wit_completions(completions: Completions) -> types::Completions {
    types::Completions {
        items: completions
            .items
            .into_iter()
            .map(|item| types::CompletionItem {
                detail: item.detail,
                insert: item.insert,
                label: item.label,
            })
            .collect(),
        start: completions.start,
    }
}

fn to_wit_diagnostic(diagnostic: Diagnostic) -> types::Diagnostic {
    types::Diagnostic {
        end: diagnostic.end,
        line: diagnostic.line,
        message: diagnostic.message,
        severity: match diagnostic.severity {
            Severity::Error => types::Severity::Error,
            Severity::Hint => types::Severity::Hint,
            Severity::Info => types::Severity::Info,
            Severity::Warning => types::Severity::Warning,
        },
        start: diagnostic.start,
    }
}

fn to_wit_line(line: Line) -> types::Line {
    line.into_iter()
        .map(|styled| types::Styled {
            tone: to_wit_tone(styled.tone),
            text: styled.text,
        })
        .collect()
}

fn to_wit_picker_row(row: PickerRow) -> types::PickerRow {
    types::PickerRow {
        actions: row
            .actions
            .into_iter()
            .map(|action| types::PickerAction {
                id: action.id,
                title: action.title,
            })
            .collect(),
        description: row.description,
        detail: row.detail,
        file: row.file.map(|path| path.to_string_lossy().into_owned()),
        label: row.label,
        preview: row.preview,
    }
}

fn to_wit_tone(tone: Tone) -> types::Tone {
    match tone {
        Tone::Accent => types::Tone::Accent,
        Tone::Dim => types::Tone::Dim,
        Tone::Error => types::Tone::Error,
        Tone::Heading => types::Tone::Heading,
        Tone::Info => types::Tone::Info,
        Tone::Normal => types::Tone::Normal,
        Tone::Warning => types::Tone::Warning,
        // A tone added after this crate was built: drawn as ordinary text.
        _ => types::Tone::Normal,
    }
}
