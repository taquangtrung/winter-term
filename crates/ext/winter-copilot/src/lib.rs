//! GitHub Copilot in Winter, as an extension shipped with it: ghost text at
//! the cursor through `textDocument/inlineCompletion`, the edit expected
//! next through `textDocument/copilotInlineEdit`, and the sign-in GitHub's
//! device flow makes, all through `copilot-language-server`.
//!
//! Winter runs the server for it (see `winter_ext_api::ServerSpec`) and hands
//! it every local file open, so this extension only asks and answers:
//!
//! - The server starts the first time a file is open, while
//!   `copilot { enable }` is on, and is asked once up whether it is signed
//!   in. `Copilot: Sign In` copies the device code and has the server open
//!   GitHub's page; `Copilot: Install Language Server` fetches the server
//!   from npm where none is found.
//! - Its inline source offers ghost text (`copilot { completions }`) and
//!   next edits (`copilot { next-edit }`) once the server is up, and tells
//!   the server which it showed and which were taken.
//! - `copilot { server-path }` names the server to run; empty looks on the
//!   `PATH`, then where `Copilot: Install Language Server` put it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

use serde_json::{json, Value};
use winter_ext_api::{
    Event, Extension, Host, InlineAnswer, InlineEdit, InlineKind, InlineOffer, InlineProvider,
    InlineRequest, Install, Platform, Registry, ServerAnswer, ServerSaid, ServerSpec, SettingValue,
    Settings, TextPosition, TextRange,
};

// ========================================================================
// Constants
// ========================================================================

/// The extension's id, which its settings block is named for.
const ID: &str = "copilot";

/// The server's name among the extension's, its program, and what it is
/// started with.
const SERVER: &str = "server";
const PROGRAM: &str = "copilot-language-server";
const ARGS: &[&str] = &["--stdio"];

/// The version `Copilot: Install Language Server` fetches.
const SERVER_VERSION: &str = "1.520.0";

/// What npm calls each machine's package of the server, after
/// `copilot-language-server-`.
const PLATFORMS: [(Platform, &str); 6] = [
    (Platform::LinuxArm64, "linux-arm64"),
    (Platform::LinuxX64, "linux-x64"),
    (Platform::MacArm64, "darwin-arm64"),
    (Platform::MacX64, "darwin-x64"),
    (Platform::WindowsArm64, "win32-arm64"),
    (Platform::WindowsX64, "win32-x64"),
];

/// Who the editor is, as `initialize` tells the server.
const EDITOR_NAME: &str = "winter-term";
const PLUGIN_NAME: &str = "winter-copilot";

/// The inline source's name, and what the status bar calls it.
const SOURCE: &str = "suggestions";
const TITLE: &str = "Copilot";

/// The settings, as `copilot { … }` names them.
const ENABLE: &str = "enable";
const COMPLETIONS: &str = "completions";
const NEXT_EDIT: &str = "next-edit";
const SERVER_PATH: &str = "server-path";

/// The trigger kinds a completion request names: asked for (`C-\`), or
/// while typing.
const INVOKED: u64 = 1;
const AUTOMATIC: u64 = 2;

/// What is said where no server is found.
const NOT_FOUND_ADVICE: &str =
    "run Copilot: Install Language Server, or name one in settings.kdl's copilot { server-path }";

/// Copilot as this extension last heard of it.
static STATE: LazyLock<Mutex<State>> = LazyLock::new(Mutex::default);

// ========================================================================
// Data Structures
// ========================================================================

/// The extension.
pub struct Copilot;

#[cfg(feature = "wasm")]
winter_ext_guest::export!(Copilot);

/// Copilot's inline source.
struct Suggestions;

/// What the extension knows of Copilot.
#[derive(Debug, Default)]
struct State {
    /// What each request in flight is for, by its tag.
    asked: HashMap<u64, Asked>,
    config: Config,
    /// The document the server last heard has the focus, by its URI.
    focused: Option<String>,
    /// The tag the next request goes under.
    next_tag: u64,
    /// Whether the server is up and answering.
    ready: bool,
    /// Whether the server was started and has not stopped since.
    running: bool,
    /// Whether the server says it is signed in, once it has said.
    signed_in: Option<bool>,
    /// Whether starting it was tried, so a server that is not there is not
    /// looked for on every file opened.
    tried: bool,
    /// Who it is signed in as.
    user: Option<String>,
}

/// The `copilot { … }` settings.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Config {
    completions: bool,
    enable: bool,
    next_edit: bool,
    server_path: String,
}

/// What a request was for.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Asked {
    /// The command that waits for the sign-in on GitHub to finish.
    FinishSignIn,
    /// Ghost text, for the inline request `ticket`.
    Ghost(u64),
    /// A next edit, for the inline request it names.
    NextEdit(NextEditAsked),
    /// The device flow's code, and the command that finishes it.
    SignIn,
    SignOut,
    /// Whether it is signed in; `quiet` says nothing where it is.
    Status(bool),
    /// Nothing to act on: an acceptance told to the server.
    Told,
}

/// A next edit asked for: the inline request's ticket, and the version of
/// the document it was asked of.
#[derive(Clone, Debug, Eq, PartialEq)]
struct NextEditAsked {
    ticket: u64,
    version: Option<i64>,
}

// ========================================================================
// Extension
// ========================================================================

impl Extension for Copilot {
    fn id(&self) -> &'static str {
        ID
    }

    fn activate(&self, registry: &mut Registry) {
        registry.setting(
            ENABLE,
            SettingValue::Switch(true),
            "Whether Copilot runs at all.",
        );
        registry.setting(
            COMPLETIONS,
            SettingValue::Switch(true),
            "Ghost text at the cursor as you type.",
        );
        registry.setting(
            NEXT_EDIT,
            SettingValue::Switch(true),
            "The edit Copilot expects next after a change, in a card for Tab to make.",
        );
        registry.setting(
            SERVER_PATH,
            SettingValue::Text(String::new()),
            "The server to run; empty looks on the PATH, then where it was installed.",
        );
        registry.server(server_spec());
        registry.inline(SOURCE, TITLE, Suggestions);
        registry.command("copilot_sign_in", "Copilot: Sign In", sign_in);
        registry.command("copilot_sign_out", "Copilot: Sign Out", sign_out);
        registry.command("copilot_status", "Copilot: Show Status", show_status);
        registry.command(
            "copilot_install",
            "Copilot: Install Language Server",
            install,
        );
        registry.on_event(hear);
    }
}

// ========================================================================
// Suggestions
// ========================================================================

impl InlineProvider for Suggestions {
    fn ask(&self, host: &mut dyn Host, request: &InlineRequest) {
        let mut state = state();
        let Some(version) = request.version.filter(|_| state.ready) else {
            return host.answer_inline(request.ticket, InlineAnswer::Nothing);
        };
        if state.focused.as_deref() != Some(request.uri.as_str()) {
            let params = json!({"textDocument": {"uri": request.uri}});
            host.server_notify(SERVER, "textDocument/didFocus", &params.to_string());
            state.focused = Some(request.uri.clone());
        }
        let document = json!({"uri": request.uri, "version": version});
        let position = json!({"line": request.at.line, "character": request.at.column});
        let (method, params, asked) = match request.kind {
            InlineKind::Edit => (
                "textDocument/copilotInlineEdit",
                json!({"textDocument": document, "position": position}),
                Asked::NextEdit(NextEditAsked {
                    ticket: request.ticket,
                    version: request.version,
                }),
            ),
            _ => (
                "textDocument/inlineCompletion",
                json!({
                    "textDocument": document,
                    "position": position,
                    "context": {"triggerKind": if request.invoked { INVOKED } else { AUTOMATIC }},
                    "formattingOptions": {
                        "tabSize": request.tab_size,
                        "insertSpaces": request.insert_spaces,
                    },
                }),
                Asked::Ghost(request.ticket),
            ),
        };
        state.request(host, method, params, asked);
    }

    fn shown(&self, host: &mut dyn Host, kind: InlineKind, receipt: &str) {
        let Some(command) = command_of(receipt) else {
            return;
        };
        let method = match kind {
            InlineKind::Edit => "textDocument/didShowInlineEdit",
            _ => "textDocument/didShowCompletion",
        };
        let arguments = command.get("arguments").cloned().unwrap_or(json!([]));
        let params = json!({"item": {"command": {"arguments": arguments}}});
        host.server_notify(SERVER, method, &params.to_string());
    }

    /// Tell the server through the command it gave with the suggestion.
    fn taken(&self, host: &mut dyn Host, _kind: InlineKind, receipt: &str) {
        let Some(command) = command_of(receipt) else {
            return;
        };
        let params = json!({
            "command": command.get("command").cloned().unwrap_or(Value::Null),
            "arguments": command.get("arguments").cloned().unwrap_or(json!([])),
        });
        state().request(host, "workspace/executeCommand", params, Asked::Told);
    }
}

// ========================================================================
// State
// ========================================================================

impl State {
    /// Send the server `method`, its answer to be acted on as `asked` says.
    fn request(&mut self, host: &mut dyn Host, method: &str, params: Value, asked: Asked) {
        self.next_tag += 1;
        let tag = self.next_tag;
        self.asked.insert(tag, asked);
        host.server_request(SERVER, tag, method, &params.to_string());
    }

    /// Start the server, unless it runs or starting it was tried, or
    /// whatever was tried where `again`.
    fn start(&mut self, host: &mut dyn Host, again: bool) {
        if self.running || (self.tried && !again) {
            return;
        }
        self.tried = true;
        self.running = true;
        let program = Some(&self.config.server_path)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
        host.start_server(SERVER, program);
    }

    /// Say what the inline source offers, as the settings and the server
    /// now stand.
    fn offer(&self, host: &mut dyn Host) {
        let on = self.config.enable && self.ready;
        let offer = InlineOffer::new(on && self.config.completions, on && self.config.next_edit);
        host.offer_inline(SOURCE, offer);
    }

    /// Act on what the server said.
    fn heard(&mut self, host: &mut dyn Host, said: &ServerSaid) {
        match said {
            ServerSaid::Answer(answer) => self.answered(host, answer),
            ServerSaid::Exited(why) => {
                self.ready = false;
                self.running = false;
                self.signed_in = None;
                self.focused = None;
                self.offer(host);
                let said = match why.starts_with("no ") {
                    true => format!("Copilot: {why}: {NOT_FOUND_ADVICE}"),
                    false => format!("Copilot stopped: {why}"),
                };
                host.notify(&said);
            }
            ServerSaid::Message(message) => host.notify(&format!("Copilot: {message}")),
            ServerSaid::OpenUrl(url) => host.open_url(url),
            ServerSaid::Ready => {
                self.ready = true;
                self.offer(host);
                let params = json!({"localChecksOnly": false});
                self.request(host, "checkStatus", params, Asked::Status(true));
            }
            ServerSaid::Status(status) if status.error => {
                self.signed_in = Some(false);
                if let Some(message) = &status.message {
                    host.notify(&format!("Copilot: {message}"));
                }
            }
            // A state not in error, or what a later API adds: nothing to do.
            _ => {}
        }
    }

    /// Act on the answer to a request.
    fn answered(&mut self, host: &mut dyn Host, answer: &ServerAnswer) {
        let Some(asked) = self.asked.remove(&answer.tag) else {
            return;
        };
        let value = answer
            .result
            .as_ref()
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(text).ok());
        match (asked, value) {
            (Asked::Ghost(ticket), Some(value)) => host.answer_inline(ticket, ghost_of(&value)),
            (Asked::NextEdit(asked), Some(value)) => {
                host.answer_inline(asked.ticket, edit_of(&value, asked.version))
            }
            (Asked::Ghost(ticket) | Asked::NextEdit(NextEditAsked { ticket, .. }), None) => {
                host.answer_inline(ticket, InlineAnswer::Nothing)
            }
            (Asked::Told, _) => {}
            (_, None) => {
                let error = answer
                    .result
                    .as_ref()
                    .err()
                    .map_or("an unreadable answer", String::as_str);
                host.notify(&format!("Copilot: {error}"));
            }
            (Asked::Status(quiet), Some(value)) => self.status_heard(host, &value, quiet),
            (Asked::SignIn, Some(value)) => self.sign_in_heard(host, &value),
            (Asked::FinishSignIn, Some(_)) => {
                let params = json!({"localChecksOnly": false});
                self.request(host, "checkStatus", params, Asked::Status(false));
            }
            (Asked::SignOut, Some(_)) => {
                self.signed_in = Some(false);
                self.user = None;
                host.notify("Copilot: signed out");
            }
        }
    }

    /// Take in whether the server is signed in, saying so unless `quiet`
    /// and it is.
    fn status_heard(&mut self, host: &mut dyn Host, value: &Value, quiet: bool) {
        let status = value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let signed_in = matches!(status, "OK" | "AlreadySignedIn" | "MaybeOk");
        self.signed_in = Some(signed_in);
        self.user = value
            .get("user")
            .and_then(Value::as_str)
            .map(str::to_string);
        match (signed_in, quiet) {
            (true, true) => {}
            (true, false) => host.notify(&self.status_said()),
            (false, _) => host.notify("Copilot: not signed in: run Copilot: Sign In"),
        }
    }

    /// Hand on the device flow's code, and wait for the sign-in to finish;
    /// signed in already, say so.
    fn sign_in_heard(&mut self, host: &mut dyn Host, value: &Value) {
        let code = value.get("userCode").and_then(Value::as_str);
        let command = value.get("command").cloned();
        let (Some(code), Some(command)) = (code, command) else {
            let params = json!({"localChecksOnly": false});
            return self.request(host, "checkStatus", params, Asked::Status(false));
        };
        host.copy_text(code);
        host.notify(&format!(
            "Copilot: enter the code {code} (copied) on the GitHub page opening now"
        ));
        let params = json!({
            "command": command.get("command").cloned().unwrap_or(Value::Null),
            "arguments": command.get("arguments").cloned().unwrap_or(json!([])),
        });
        self.request(
            host,
            "workspace/executeCommand",
            params,
            Asked::FinishSignIn,
        );
    }

    /// Whether Copilot is running and who it is signed in as.
    fn status_said(&self) -> String {
        match (self.ready, self.signed_in, &self.user) {
            (false, _, _) if !self.config.enable => {
                "Copilot: off (copilot { enable #false })".to_string()
            }
            (false, _, _) if self.running => "Copilot: starting".to_string(),
            (false, _, _) => "Copilot: not running".to_string(),
            (true, Some(true), Some(user)) => format!("Copilot: signed in as {user}"),
            (true, Some(true), None) => "Copilot: signed in".to_string(),
            (true, Some(false), _) => "Copilot: not signed in: run Copilot: Sign In".to_string(),
            (true, None, _) => "Copilot: starting".to_string(),
        }
    }
}

// ========================================================================
// Config
// ========================================================================

impl Default for Config {
    /// As the settings declare them by default.
    fn default() -> Self {
        Self {
            completions: true,
            enable: true,
            next_edit: true,
            server_path: String::new(),
        }
    }
}

impl Config {
    /// The settings as `settings` gives them, the defaults where it gives
    /// none.
    fn of(settings: &Settings) -> Self {
        let base = Self::default();
        Self {
            completions: settings.switch(COMPLETIONS).unwrap_or(base.completions),
            enable: settings.switch(ENABLE).unwrap_or(base.enable),
            next_edit: settings.switch(NEXT_EDIT).unwrap_or(base.next_edit),
            server_path: settings
                .text(SERVER_PATH)
                .map_or(base.server_path, str::to_string),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Copilot as this extension last heard of it.
fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The server, and where it is fetched from for each machine.
fn server_spec() -> ServerSpec {
    let version = env!("CARGO_PKG_VERSION");
    let options = json!({
        "editorInfo": {"name": EDITOR_NAME, "version": version},
        "editorPluginInfo": {"name": PLUGIN_NAME, "version": version},
    });
    let spec = ServerSpec::new(SERVER, PROGRAM)
        .with_args(ARGS)
        .with_init_options(options.to_string());
    PLATFORMS.iter().fold(spec, |spec, (platform, npm)| {
        let package = format!("{PROGRAM}-{npm}");
        let url = format!(
            "https://registry.npmjs.org/@github/{package}/-/{package}-{SERVER_VERSION}.tgz"
        );
        let program = match platform {
            Platform::WindowsArm64 | Platform::WindowsX64 => format!("{PROGRAM}.exe"),
            _ => PROGRAM.to_string(),
        };
        spec.with_install(Install::new(*platform, url, format!("package/{program}")))
    })
}

/// What the extension does on hearing `event`.
fn hear(host: &mut dyn Host, event: &Event) {
    let mut state = state();
    match event {
        Event::Configured(settings) => {
            state.config = Config::of(settings);
            state.offer(host);
        }
        Event::Opened(_) | Event::Focused(Some(_)) if state.config.enable => {
            state.start(host, false)
        }
        Event::Server(event) if event.server == SERVER => state.heard(host, &event.said),
        _ => {}
    }
}

/// `Copilot: Sign In`: GitHub's device flow, the server started first where
/// it is not running.
fn sign_in(host: &mut dyn Host) {
    let mut state = state();
    if !state.running {
        state.start(host, true);
    }
    match state.ready {
        true => state.request(host, "signIn", json!({}), Asked::SignIn),
        false => host.notify("Copilot: still starting; sign in again in a moment"),
    }
}

/// `Copilot: Sign Out`.
fn sign_out(host: &mut dyn Host) {
    let mut state = state();
    match state.ready {
        true => state.request(host, "signOut", json!({}), Asked::SignOut),
        false => host.notify("Copilot: not running"),
    }
}

/// `Copilot: Show Status`.
fn show_status(host: &mut dyn Host) {
    let said = state().status_said();
    host.notify(&said);
}

/// `Copilot: Install Language Server`: fetched from npm, and started the
/// next time a file is opened.
fn install(host: &mut dyn Host) {
    state().tried = false;
    host.install_server(SERVER);
}

/// The command a suggestion's receipt holds, where it holds one.
fn command_of(receipt: &str) -> Option<Value> {
    serde_json::from_str::<Value>(receipt)
        .ok()
        .filter(|command| !command.is_null())
}

/// The ghost text an `inlineCompletion` answer offers.
fn ghost_of(value: &Value) -> InlineAnswer {
    let suggestions = value
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let range = range_of(item.get("range")?)?;
            let text = item.get("insertText")?.as_str()?;
            let command = item.get("command").cloned().unwrap_or(Value::Null);
            Some(InlineEdit::new(range, text, command.to_string()))
        })
        .collect();
    InlineAnswer::Ghost(suggestions)
}

/// The next edit a `copilotInlineEdit` answer offers, where it is for the
/// version of the document asked of.
fn edit_of(value: &Value, version: Option<i64>) -> InlineAnswer {
    let Some(edit) = value
        .get("edits")
        .and_then(Value::as_array)
        .and_then(|edits| edits.first())
    else {
        return InlineAnswer::Nothing;
    };
    let answered = edit
        .pointer("/textDocument/version")
        .and_then(Value::as_i64);
    if answered.is_some() && answered != version {
        return InlineAnswer::Nothing;
    }
    let (Some(range), Some(text)) = (
        edit.get("range").and_then(range_of),
        edit.get("text").and_then(Value::as_str),
    ) else {
        return InlineAnswer::Nothing;
    };
    let command = edit.get("command").cloned().unwrap_or(Value::Null);
    InlineAnswer::Edit(InlineEdit::new(range, text, command.to_string()))
}

/// A range from the wire.
fn range_of(value: &Value) -> Option<TextRange> {
    let at = |key: &str| -> Option<TextPosition> {
        let point = value.get(key)?;
        let line = u32::try_from(point.get("line")?.as_u64()?).ok()?;
        let column = u32::try_from(point.get("character")?.as_u64()?).ok()?;
        Some(TextPosition::new(line, column))
    };
    Some(TextRange::new(at("start")?, at("end")?))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::path::Path;

    use winter_ext_api::{
        Diagnostic, Document, PickerEvents, PickerId, PickerRow, PickerSpec, Repaint, ServerEvent,
        Work,
    };

    use super::*;

    /// A host keeping what it is asked.
    #[derive(Default)]
    struct Recorder {
        answered: Vec<(u64, InlineAnswer)>,
        copied: Vec<String>,
        offered: Vec<InlineOffer>,
        requested: Vec<(u64, String, Value)>,
        said: Vec<String>,
        started: Vec<Option<PathBuf>>,
    }

    impl Host for Recorder {
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

        fn copy_text(&mut self, text: &str) {
            self.copied.push(text.to_string());
        }

        fn start_server(&mut self, _name: &str, program: Option<PathBuf>) {
            self.started.push(program);
        }

        fn install_server(&mut self, _name: &str) {}

        fn server_request(&mut self, _name: &str, tag: u64, method: &str, params: &str) {
            let params = serde_json::from_str(params).expect("JSON");
            self.requested.push((tag, method.to_string(), params));
        }

        fn server_notify(&mut self, _name: &str, _method: &str, _params: &str) {}

        fn offer_inline(&mut self, _name: &str, offer: InlineOffer) {
            self.offered.push(offer);
        }

        fn answer_inline(&mut self, ticket: u64, answer: InlineAnswer) {
            self.answered.push((ticket, answer));
        }

        fn set_status(&mut self, _name: &str, _text: Option<String>) {}

        fn spawn_work(&mut self, work: Work) {
            let reply = work();
            reply(self);
        }

        fn open_picker(&mut self, _id: PickerId, _spec: PickerSpec, _events: PickerEvents) {}

        fn push_rows(&mut self, _picker: PickerId, _rows: Vec<PickerRow>, _done: bool) {}

        fn repaint(&self) -> Repaint {
            Repaint::new(|| {})
        }
    }

    fn server_said(said: ServerSaid) -> Event {
        Event::Server(ServerEvent::new(SERVER, said))
    }

    /// The answer `value` to the request `tag`.
    fn answer(tag: u64, value: Value) -> Event {
        server_said(ServerSaid::Answer(ServerAnswer::new(
            tag,
            Ok(value.to_string()),
        )))
    }

    #[test]
    fn test_ghost_text_is_offered_once_the_server_is_up_and_answered_by_ticket() {
        let mut host = Recorder::default();
        let path = Settings::new(
            [(
                SERVER_PATH.to_string(),
                SettingValue::Text("/opt/cls".into()),
            )]
            .into(),
        );
        hear(&mut host, &Event::Configured(path));
        assert_eq!(
            host.offered,
            [InlineOffer::new(false, false)],
            "not before it is up"
        );
        hear(&mut host, &Event::Opened("/a.rs".into()));
        hear(&mut host, &Event::Opened("/b.rs".into()));
        assert_eq!(
            host.started,
            [Some(PathBuf::from("/opt/cls"))],
            "started once, as named"
        );
        hear(&mut host, &server_said(ServerSaid::Ready));
        assert_eq!(host.offered.last(), Some(&InlineOffer::new(true, true)));
        let (status, method, _) = host.requested.last().cloned().expect("asked");
        assert_eq!(method, "checkStatus");
        hear(
            &mut host,
            &answer(status, json!({"status": "OK", "user": "octo"})),
        );
        assert!(host.said.is_empty(), "signed in, said nothing");

        let mut request = InlineRequest::new(
            5,
            InlineKind::Ghost,
            "/a.rs".into(),
            TextPosition::new(2, 3),
        );
        request.uri = "file:///a.rs".to_string();
        request.version = Some(4);
        Suggestions.ask(&mut host, &request);
        let (tag, method, params) = host.requested.last().cloned().expect("asked");
        assert_eq!(method, "textDocument/inlineCompletion");
        assert_eq!(
            params["textDocument"],
            json!({"uri": "file:///a.rs", "version": 4})
        );
        assert_eq!(params["position"], json!({"line": 2, "character": 3}));
        let item = json!({
            "insertText": "let x = 1;",
            "range": {"start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 3}},
            "command": {"command": "accept", "arguments": [7]},
        });
        hear(&mut host, &answer(tag, json!({"items": [item]})));
        let [(5, InlineAnswer::Ghost(ghost))] = host.answered.as_slice() else {
            panic!("ghost text for ticket 5: {:?}", host.answered);
        };
        assert_eq!(ghost[0].text, "let x = 1;");
        assert_eq!(ghost[0].range.end, TextPosition::new(2, 3));
        let receipt = ghost[0].receipt.clone();
        Suggestions.taken(&mut host, InlineKind::Ghost, &receipt);
        let (_, method, params) = host.requested.last().cloned().expect("told");
        assert_eq!(method, "workspace/executeCommand");
        assert_eq!(params, json!({"command": "accept", "arguments": [7]}));

        sign_in(&mut host);
        let (tag, method, _) = host.requested.last().cloned().expect("asked");
        assert_eq!(method, "signIn");
        let command = json!({"command": "finish", "arguments": []});
        hear(
            &mut host,
            &answer(tag, json!({"userCode": "AB-12", "command": command})),
        );
        assert_eq!(host.copied, ["AB-12"]);
        assert!(host.said.last().is_some_and(|said| said.contains("AB-12")));
    }

    #[test]
    fn test_a_next_edit_for_another_version_of_the_document_is_nothing() {
        let edit = |version| {
            json!({"edits": [{
                "textDocument": {"version": version},
                "range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 2}},
                "text": "fn",
            }]})
        };
        let InlineAnswer::Edit(made) = edit_of(&edit(3), Some(3)) else {
            panic!("an edit");
        };
        assert_eq!(made.text, "fn");
        assert_eq!(made.receipt, "null", "no command to tell");
        assert_eq!(edit_of(&edit(2), Some(3)), InlineAnswer::Nothing);
        assert_eq!(command_of(&made.receipt), None);
    }

    #[test]
    fn test_the_server_is_fetched_for_every_machine_npm_has_it_for() {
        let spec = server_spec();
        assert_eq!(spec.install.len(), PLATFORMS.len());
        let windows = spec
            .install
            .iter()
            .find(|install| install.platform == Platform::WindowsX64)
            .expect("windows");
        assert_eq!(windows.member, "package/copilot-language-server.exe");
        assert!(windows
            .url
            .ends_with("copilot-language-server-win32-x64-1.520.0.tgz"));
    }
}
