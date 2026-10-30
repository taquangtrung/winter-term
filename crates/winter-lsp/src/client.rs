//! A running language server: the process, a thread writing to it, a thread
//! reading what it says and answering the questions it asks, and the
//! [`Port`] an editor talks to it through.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{BufRead, BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::protocol;
use super::rpc;
use super::servers::Plan;
use crate::diagnostics::{self, Backend};

// ========================================================================
// Constants
// ========================================================================

/// The id `initialize` is sent under; every later request counts on from it.
const INITIALIZE_ID: u64 = 0;

/// How many of the last lines a server wrote to its error stream are kept,
/// to say why it stopped.
const STDERR_LINES: usize = 20;

/// How long a server asked to shut down is given before it is killed.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// How long a server is given to answer `shutdown` before it is told to
/// `exit` all the same.
const SHUTDOWN_REPLY_WAIT: Duration = Duration::from_secs(1);

/// How long quitting waits for the servers to end by themselves before the
/// ones still running are killed, so none outlives the window.
const QUIT_GRACE: Duration = Duration::from_millis(300);

/// How often a server shutting down is looked at, until it has gone.
const SHUTDOWN_POLL: Duration = Duration::from_millis(50);

/// The JSON-RPC error a request for a method this client does not answer
/// gets.
const METHOD_NOT_FOUND: i64 = -32601;

/// The most messages queued for a server that is not reading them. Each
/// change sends the whole document, so a server stuck short of reading its
/// input would otherwise hold a copy of the file per flush, without end;
/// past this, messages are dropped, and the next whole-document change puts
/// a server that comes back in step again.
const MAX_QUEUED_MESSAGES: usize = 256;

// ========================================================================
// Data Structures
// ========================================================================

/// Which running server a message is from.
pub type ServerId = u64;

/// What each piece of work a server reports progress on is called, and the
/// latest it said of it, by the token it reports under.
type Progress = BTreeMap<String, (String, String)>;

/// What a server said that the window passes on to the editors.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The answer to a request.
    Answer {
        /// The request's id.
        id: u64,
        /// What it returned, or the error it gave.
        result: Result<Value, String>,
    },
    /// The server asked for edits to be made, as a code action's command
    /// does.
    ApplyEdit(protocol::FileEdits),
    /// New diagnostics for a document: what the store holds for it under
    /// [`Backend::Lsp`] has already changed.
    Diagnostics(PathBuf),
    /// The server has stopped, and why.
    Exited(String),
    /// Something the server wants read: a failure or a warning.
    Message(String),
    /// What the server is busy with, or `None` once it is done.
    Progress(Option<String>),
    /// The server has answered `initialize` and takes documents now.
    Ready,
    /// The server asked for a page to be opened in the browser: the page a
    /// sign-in goes on at.
    OpenUrl(String),
    /// What the server says of its own state: whether it is in error (not
    /// signed in, say), and what it says of it.
    Status {
        /// Whether the server is in error.
        error: bool,
        /// What it says, where it says anything.
        message: Option<String>,
    },
}

/// A message from a server, and which server it is from.
#[derive(Clone, Debug, PartialEq)]
pub struct Incoming {
    /// What it said.
    pub event: Event,
    /// Which server said it.
    pub server: ServerId,
}

/// A handle on a running server, for an editor to send it documents and
/// requests through. Cheap to clone; every clone is the same server.
#[derive(Clone)]
pub struct Port(Arc<Shared>);

/// What every handle on a server shares.
struct Shared {
    /// What the server said it can do, once it has answered `initialize`.
    capabilities: Mutex<Value>,
    /// Whether the server reads every file, whatever its language or project.
    everywhere: bool,
    id: ServerId,
    languages: Vec<String>,
    /// The program, for saying which server something is about.
    name: String,
    next_request: AtomicU64,
    ready: AtomicBool,
    root: PathBuf,
    /// Whether the server has answered `shutdown`, which `exit` waits for.
    shutdown_answered: AtomicBool,
    /// The id `shutdown` was asked under, nought before it is.
    shutdown_id: AtomicU64,
    /// The last lines the server wrote to its error stream.
    stderr: Mutex<VecDeque<String>>,
    /// The writer thread's queue, until the server is stopped.
    to_server: Mutex<Option<SyncSender<Value>>>,
}

/// One server the window started, and the process it runs as.
struct Server {
    child: Child,
    port: Port,
}

/// Every server the window has running, and what they say.
pub struct Hub {
    events: Receiver<Incoming>,
    events_to: Sender<Incoming>,
    /// Moves whenever a server is ready or gone, for editors to find out
    /// which servers they can use.
    generation: u64,
    next_server: ServerId,
    /// What each server is busy with.
    progress: HashMap<ServerId, String>,
    /// The servers being stopped, until each has ended.
    reapers: Vec<Reaper>,
    servers: Vec<Server>,
}

/// A stopped server's process being seen out on a thread of its own: told
/// to shut down, then to exit, then killed if it has not.
struct Reaper {
    /// Set to have it kill the process now rather than wait any longer.
    hurry: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

// ========================================================================
// Port Implementations
// ========================================================================

impl std::fmt::Debug for Port {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Port({} {} at {})", self.0.id, self.0.name, self.0.root.display())
    }
}

impl Port {
    /// Which server this is.
    pub fn id(&self) -> ServerId {
        self.0.id
    }

    /// The program the server runs as.
    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// The project the server serves.
    pub fn root(&self) -> &Path {
        &self.0.root
    }

    /// The languages the server serves.
    pub fn languages(&self) -> &[String] {
        &self.0.languages
    }

    /// Whether the server serves `path`, written in `language`.
    pub fn serves(&self, language: &str, path: &Path) -> bool {
        if self.0.everywhere {
            return true;
        }
        self.0.languages.iter().any(|served| served == language) && path.starts_with(&self.0.root)
    }

    /// Whether the server reads every file rather than a project's (Copilot),
    /// and so answers none of a language server's questions.
    pub fn is_everywhere(&self) -> bool {
        self.0.everywhere
    }

    /// Whether the server has answered `initialize` and is still running.
    pub fn is_ready(&self) -> bool {
        self.0.ready.load(Ordering::Acquire)
    }

    /// Whether the server says it answers `capability`
    /// (`definitionProvider`, `renameProvider`).
    pub fn provides(&self, capability: &str) -> bool {
        let capabilities = self.0.capabilities.lock().map(|c| c.get(capability).cloned());
        matches!(capabilities, Ok(Some(value)) if !value.is_null() && value != Value::Bool(false))
    }

    /// The characters after which the server offers completions unasked.
    pub fn trigger_characters(&self) -> Vec<char> {
        let Ok(capabilities) = self.0.capabilities.lock() else {
            return Vec::new();
        };
        capabilities
            .pointer("/completionProvider/triggerCharacters")
            .and_then(Value::as_array)
            .map(|characters| {
                characters
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(|text| text.chars().next())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Tell the server something, once it is ready. Says whether it went.
    pub fn notify(&self, method: &str, params: Value) -> bool {
        self.is_ready() && self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    /// Ask the server something, once it is ready: the id its answer comes
    /// back under, as an [`Event::Answer`].
    pub fn request(&self, method: &str, params: Value) -> Option<u64> {
        if !self.is_ready() {
            return None;
        }
        let id = self.0.next_request.fetch_add(1, Ordering::Relaxed);
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .then_some(id)
    }

    /// Queue `message` for the writer thread.
    fn send(&self, message: Value) -> bool {
        let Ok(to_server) = self.0.to_server.lock() else {
            return false;
        };
        to_server.as_ref().is_some_and(|to| to.try_send(message).is_ok())
    }

    /// A ready server serving `language` under `root` that runs nowhere,
    /// saying it can do what `capabilities` says, and what is sent to it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn fake(language: &str, root: &Path, capabilities: Value) -> (Port, Receiver<Value>) {
        let (to_server, sent) = mpsc::sync_channel(MAX_QUEUED_MESSAGES);
        let port = Port(Arc::new(Shared {
            capabilities: Mutex::new(capabilities),
            everywhere: false,
            id: 1,
            languages: vec![language.to_string()],
            name: "fake".to_string(),
            next_request: AtomicU64::new(1),
            ready: AtomicBool::new(true),
            root: root.to_path_buf(),
            shutdown_answered: AtomicBool::new(false),
            shutdown_id: AtomicU64::new(0),
            stderr: Mutex::new(VecDeque::new()),
            to_server: Mutex::new(Some(to_server)),
        }));
        (port, sent)
    }

    /// What the server last wrote to its error stream.
    fn stderr_tail(&self) -> String {
        self.0
            .stderr
            .lock()
            .map(|lines| lines.iter().cloned().collect::<Vec<_>>().join("\n"))
            .unwrap_or_default()
    }
}

// ========================================================================
// Hub Implementations
// ========================================================================

impl Hub {
    /// A hub with no servers, whose servers' messages arrive on `events`
    /// once sent to `events_to`.
    pub fn new(events_to: Sender<Incoming>, events: Receiver<Incoming>) -> Self {
        Self {
            events,
            events_to,
            generation: 0,
            next_server: 1,
            progress: HashMap::new(),
            reapers: Vec::new(),
            servers: Vec::new(),
        }
    }

    /// Start the server `plan` names, and ask it to `initialize`.
    pub fn start(&mut self, plan: &Plan) -> Result<Port, String> {
        let (program, args) = plan.command.split_first().ok_or("an empty server command")?;
        // The toolchain Winter itself was run under (by `cargo run`) is not
        // the project's: rustup's proxies pick the project's own when this
        // is left unset.
        let mut child = Command::new(program)
            .args(args)
            .current_dir(&plan.root)
            .env_remove("RUSTUP_TOOLCHAIN")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("{program}: {e}"))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            return Err(format!("{program}: no pipes to talk through"));
        };
        let (to_server, outgoing) = mpsc::sync_channel(MAX_QUEUED_MESSAGES);
        let id = self.next_server;
        self.next_server += 1;
        let port = Port(Arc::new(Shared {
            capabilities: Mutex::new(Value::Null),
            everywhere: plan.everywhere,
            id,
            languages: plan.languages.clone(),
            name: program.clone(),
            next_request: AtomicU64::new(INITIALIZE_ID + 1),
            ready: AtomicBool::new(false),
            root: plan.root.clone(),
            shutdown_answered: AtomicBool::new(false),
            shutdown_id: AtomicU64::new(0),
            stderr: Mutex::new(VecDeque::new()),
            to_server: Mutex::new(Some(to_server.clone())),
        }));
        spawn_writer(stdin, outgoing);
        spawn_reader(stdout, port.clone(), to_server.clone(), self.events_to.clone());
        if let Some(stderr) = child.stderr.take() {
            spawn_stderr(stderr, port.clone());
        }
        let mut params = protocol::initialize_params(&plan.root);
        if let Some(options) = &plan.init_options {
            params["initializationOptions"] = options.clone();
        }
        let initialize = json!({
            "jsonrpc": "2.0",
            "id": INITIALIZE_ID,
            "method": "initialize",
            "params": params,
        });
        let _ = to_server.try_send(initialize);
        self.servers.push(Server {
            child,
            port: port.clone(),
        });
        Ok(port)
    }

    /// The running language server that serves `path`, written in
    /// `language`: never one reading every file (Copilot).
    pub fn serving(&self, language: &str, path: &Path) -> Option<&Port> {
        self.servers
            .iter()
            .map(|server| &server.port)
            .find(|port| !port.is_everywhere() && port.serves(language, path))
    }

    /// Every running server.
    pub fn ports(&self) -> Vec<Port> {
        self.servers.iter().map(|server| server.port.clone()).collect()
    }

    /// A number that moves whenever a server is ready or gone.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Ask server `id` to shut down, and kill it if it has not within a
    /// moment. Says whether there was such a server.
    pub fn stop(&mut self, id: ServerId) -> bool {
        let Some(at) = self.servers.iter().position(|server| server.port.id() == id) else {
            return false;
        };
        let server = self.servers.remove(at);
        self.progress.remove(&id);
        self.forget_problems_of(&server.port);
        self.generation += 1;
        self.reap(server);
        true
    }

    /// Stop every server.
    pub fn stop_all(&mut self) {
        let ids: Vec<ServerId> = self.servers.iter().map(|server| server.port.id()).collect();
        for id in ids {
            self.stop(id);
        }
    }

    /// What the servers have said since this was last asked. A server that
    /// is ready or gone moves [`Self::generation`]; one that is gone is
    /// forgotten, its problems with it.
    pub fn drain(&mut self) -> Vec<Incoming> {
        let mut said = Vec::new();
        while let Ok(incoming) = self.events.try_recv() {
            match &incoming.event {
                Event::Ready => self.generation += 1,
                Event::Exited(_) => {
                    let Some(at) = self.servers.iter().position(|s| s.port.id() == incoming.server) else {
                        // A server stopped on purpose is already forgotten;
                        // its reader closing is no news to report.
                        self.progress.remove(&incoming.server);
                        continue;
                    };
                    let server = self.servers.remove(at);
                    self.forget_problems_of(&server.port);
                    self.generation += 1;
                    // The reader also gives up on output it cannot frame, with
                    // the process still running: waiting on it here would
                    // freeze the window, so it is reaped, and killed if need
                    // be, off this thread.
                    self.reap(server);
                    self.progress.remove(&incoming.server);
                }
                Event::Progress(Some(what)) => {
                    self.progress.insert(incoming.server, what.clone());
                }
                Event::Progress(None) => {
                    self.progress.remove(&incoming.server);
                }
                _ => {}
            }
            said.push(incoming);
        }
        said
    }

    /// One line per running server: which, where, and what it is doing.
    pub fn summary(&self) -> Vec<String> {
        self.servers
            .iter()
            .map(|server| {
                let port = &server.port;
                let state = match (port.is_ready(), self.progress.get(&port.id())) {
                    (_, Some(what)) => what.clone(),
                    (true, None) => "ready".to_string(),
                    (false, None) => "starting".to_string(),
                };
                format!("{} ({}) at {}: {state}", port.name(), port.languages().join(", "), port.root().display())
            })
            .collect()
    }

    /// What the busiest server is doing, for the status bar.
    pub fn busy(&self) -> Option<String> {
        self.servers.iter().find_map(|server| {
            let what = self.progress.get(&server.port.id())?;
            Some(format!("{}: {what}", server.port.name()))
        })
    }

    /// Drop what `port` said is wrong, and only what it said: another
    /// server's problems in the same files stay.
    fn forget_problems_of(&self, port: &Port) {
        diagnostics::forget(Backend::Lsp(port.id()));
    }

    /// See `server` out (see [`shut_down`]), keeping its reaper until it has
    /// ended, and letting go of the ones that have.
    fn reap(&mut self, server: Server) {
        self.reapers.retain(|reaper| !reaper.is_finished());
        self.reapers.push(shut_down(server));
    }
}

// ========================================================================
// Reaper
// ========================================================================

impl Reaper {
    /// Whether the server has been seen out.
    fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(thread::JoinHandle::is_finished)
    }
}

impl Drop for Hub {
    /// Stop every server, and see each one ended before the window is: a
    /// reaper left running when the process ends dies with it, and a server
    /// ignoring `exit` and its closed input would outlive the window.
    fn drop(&mut self) {
        self.stop_all();
        let deadline = Instant::now() + QUIT_GRACE;
        while Instant::now() < deadline && self.reapers.iter().any(|reaper| !reaper.is_finished()) {
            thread::sleep(SHUTDOWN_POLL);
        }
        for reaper in &mut self.reapers {
            reaper.hurry.store(true, Ordering::Release);
            if let Some(thread) = reaper.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

// ========================================================================
// Threads
// ========================================================================

/// Write what is queued to the server until the queue closes, which closes
/// the server's input.
fn spawn_writer(stdin: ChildStdin, outgoing: Receiver<Value>) {
    let _ = thread::Builder::new()
        .name("winter lsp writer".into())
        .spawn(move || {
            let mut out = BufWriter::new(stdin);
            while let Ok(message) = outgoing.recv() {
                if rpc::write_message(&mut out, &message).is_err() {
                    return;
                }
            }
        });
}

/// Read what the server says until it stops: answers and notifications
/// passed on as events, the questions it asks answered here.
fn spawn_reader(stdout: ChildStdout, port: Port, to_server: SyncSender<Value>, events: Sender<Incoming>) {
    let _ = thread::Builder::new()
        .name("winter lsp reader".into())
        .spawn(move || {
            let mut input = BufReader::new(stdout);
            let mut progress: Progress = BTreeMap::new();
            let tell = |event: Event| {
                let _ = events.send(Incoming {
                    event,
                    server: port.id(),
                });
            };
            let reason = loop {
                let message = match rpc::read_message(&mut input) {
                    Ok(Some(message)) => message,
                    Ok(None) => break "it closed its output".to_string(),
                    Err(e) => break e.to_string(),
                };
                for event in read(&message, &port, &to_server, &mut progress) {
                    tell(event);
                }
            };
            port.0.ready.store(false, Ordering::Release);
            if let Ok(mut to_server) = port.0.to_server.lock() {
                *to_server = None;
            }
            let tail = port.stderr_tail();
            let said = match tail.lines().last() {
                Some(last) => format!("{reason}: {last}"),
                None => reason,
            };
            tell(Event::Exited(said));
        });
}

/// Keep the last lines the server writes to its error stream.
fn spawn_stderr(stderr: std::process::ChildStderr, port: Port) {
    let _ = thread::Builder::new()
        .name("winter lsp stderr".into())
        .spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else {
                    return;
                };
                if let Ok(mut lines) = port.0.stderr.lock() {
                    if lines.len() == STDERR_LINES {
                        lines.pop_front();
                    }
                    lines.push_back(line);
                }
            }
        });
}

/// What one message from the server comes to, answering it where it is a
/// question: `initialize`'s answer readies the server, a diagnostic
/// notification updates the problems, and progress is followed by token.
fn read(
    message: &Value,
    port: &Port,
    to_server: &SyncSender<Value>,
    progress: &mut Progress,
) -> Vec<Event> {
    let method = message.get("method").and_then(Value::as_str);
    let id = message.get("id");
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    match (method, id) {
        (None, Some(id)) => {
            let Some(id) = id.as_u64() else {
                return Vec::new();
            };
            let result = match message.get("error") {
                Some(error) => Err(error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("the request failed")
                    .to_string()),
                None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            // The answer to `shutdown` is the stopping's own, which no
            // editor asked for: it lets `exit` go.
            if id != INITIALIZE_ID && id == port.0.shutdown_id.load(Ordering::Acquire) {
                port.0.shutdown_answered.store(true, Ordering::Release);
                return Vec::new();
            }
            if id != INITIALIZE_ID {
                return vec![Event::Answer { id, result }];
            }
            match result {
                Ok(result) => {
                    if let Ok(mut capabilities) = port.0.capabilities.lock() {
                        *capabilities = result.get("capabilities").cloned().unwrap_or(Value::Null);
                    }
                    let _ = to_server.try_send(json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}));
                    port.0.ready.store(true, Ordering::Release);
                    vec![Event::Ready]
                }
                // A server that refused to start is gone as far as anyone
                // asking it is concerned: said as its end, the hub forgets
                // and stops it, and asking for it again starts it afresh
                // rather than finding one that is never ready.
                Err(error) => vec![Event::Exited(format!("it did not start: {error}"))],
            }
        }
        (Some(method), Some(id)) => {
            let (answer, events) = answer_request(method, &params, port);
            let reply = match answer {
                Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                Err(code) => json!({"jsonrpc": "2.0", "id": id,
                    "error": {"code": code, "message": format!("{method} is not supported")}}),
            };
            let _ = to_server.try_send(reply);
            events
        }
        (Some(method), None) => notification(method, &params, port, progress),
        (None, None) => Vec::new(),
    }
}

/// The answer to a question the server asked, and what it comes to for the
/// editors.
fn answer_request(method: &str, params: &Value, port: &Port) -> (Result<Value, i64>, Vec<Event>) {
    match method {
        "workspace/configuration" => {
            let count = params.get("items").and_then(Value::as_array).map_or(0, Vec::len);
            (Ok(Value::Array(vec![Value::Null; count])), Vec::new())
        }
        "workspace/applyEdit" => {
            let edits = params.get("edit").map(protocol::workspace_edits).unwrap_or_default();
            (Ok(json!({"applied": true})), vec![Event::ApplyEdit(edits)])
        }
        "workspace/workspaceFolders" => {
            let uri = protocol::uri_of(port.root());
            (Ok(json!([{"uri": uri, "name": port.root().display().to_string()}])), Vec::new())
        }
        "window/showDocument" => {
            let uri = params.get("uri").and_then(Value::as_str).unwrap_or_default().to_string();
            (Ok(json!({"success": true})), vec![Event::OpenUrl(uri)])
        }
        "window/workDoneProgress/create"
        | "client/registerCapability"
        | "client/unregisterCapability"
        | "window/showMessageRequest"
        | "workspace/diagnostic/refresh"
        | "workspace/semanticTokens/refresh"
        | "workspace/inlayHint/refresh"
        | "workspace/codeLens/refresh" => (Ok(Value::Null), Vec::new()),
        _ => (Err(METHOD_NOT_FOUND), Vec::new()),
    }
}

/// What a notification from the server comes to.
fn notification(
    method: &str,
    params: &Value,
    port: &Port,
    progress: &mut Progress,
) -> Vec<Event> {
    match method {
        "textDocument/publishDiagnostics" => {
            let Some(published) = protocol::published(params) else {
                return Vec::new();
            };
            diagnostics::publish(Backend::Lsp(port.id()), &published.path, published.diagnostics);
            vec![Event::Diagnostics(published.path)]
        }
        "didChangeStatus" => {
            let kind = params.get("kind").and_then(Value::as_str).unwrap_or("Normal");
            let message = params.get("message").and_then(Value::as_str).map(str::to_string);
            vec![Event::Status {
                error: kind == "Error",
                message,
            }]
        }
        "window/showMessage" => {
            // Errors and warnings; the server's chatter is left unsaid.
            let kind = params.get("type").and_then(Value::as_u64).unwrap_or(4);
            let text = params.get("message").and_then(Value::as_str).unwrap_or_default();
            match kind <= 2 && !text.is_empty() {
                true => vec![Event::Message(format!("{}: {text}", port.name()))],
                false => Vec::new(),
            }
        }
        "$/progress" => {
            let token = match params.get("token") {
                Some(Value::String(token)) => token.clone(),
                Some(other) => other.to_string(),
                None => return Vec::new(),
            };
            let value = params.get("value").cloned().unwrap_or(Value::Null);
            let kind = value.get("kind").and_then(Value::as_str).unwrap_or_default();
            if kind == "end" {
                progress.remove(&token);
            } else {
                let title = value
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| progress.get(&token).map(|(title, _)| title.clone()))
                    .unwrap_or_default();
                let mut said = title.clone();
                if let Some(message) = value.get("message").and_then(Value::as_str) {
                    said = format!("{said}: {message}");
                }
                if let Some(percent) = value.get("percentage").and_then(Value::as_u64) {
                    said = format!("{said} {percent}%");
                }
                progress.insert(token, (title, said));
            }
            vec![Event::Progress(progress.values().next_back().map(|(_, said)| said.clone()))]
        }
        _ => Vec::new(),
    }
}

/// Ask `server` to shut down and exit, and kill it if it is still there
/// after a moment, off the calling thread.
fn shut_down(server: Server) -> Reaper {
    let Server { mut child, port } = server;
    // `exit` waits for `shutdown`'s answer, as the protocol has it: sent at
    // once, a server could exit with a failure or skip its cleanup. The id
    // is noted before the request goes, so the answer cannot outrun it.
    let id = port.0.next_request.load(Ordering::Relaxed);
    port.0.shutdown_id.store(id, Ordering::Release);
    let asked = port.request("shutdown", Value::Null).is_some();
    port.0.ready.store(false, Ordering::Release);
    // The queue is the reaper's from here: no one else sends anything more.
    let writer = port.0.to_server.lock().ok().and_then(|mut to_server| to_server.take());
    let hurry = Arc::new(AtomicBool::new(false));
    let hurried = Arc::clone(&hurry);
    let waiting = move || !hurried.load(Ordering::Acquire);
    let thread = thread::Builder::new()
        .name("winter lsp reaper".into())
        .spawn(move || {
            let answered = Instant::now() + SHUTDOWN_REPLY_WAIT;
            while asked && waiting() && !port.0.shutdown_answered.load(Ordering::Acquire) && Instant::now() < answered {
                thread::sleep(SHUTDOWN_POLL);
            }
            if let Some(writer) = writer {
                let _ = writer.try_send(json!({"jsonrpc": "2.0", "method": "exit", "params": null}));
            }
            let deadline = Instant::now() + SHUTDOWN_GRACE;
            while waiting() && Instant::now() < deadline {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return;
                }
                thread::sleep(SHUTDOWN_POLL);
            }
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            let _ = child.kill();
            let _ = child.wait();
        })
        .ok();
    Reaper { hurry, thread }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A port onto no process, for reading messages against.
    fn port() -> (Port, Receiver<Value>) {
        let (to_server, sent) = mpsc::sync_channel(MAX_QUEUED_MESSAGES);
        let port = Port(Arc::new(Shared {
            capabilities: Mutex::new(Value::Null),
            everywhere: false,
            id: 7,
            languages: vec!["rust".to_string()],
            name: "fake".to_string(),
            next_request: AtomicU64::new(1),
            ready: AtomicBool::new(false),
            root: PathBuf::from("/project"),
            shutdown_answered: AtomicBool::new(false),
            shutdown_id: AtomicU64::new(0),
            stderr: Mutex::new(VecDeque::new()),
            to_server: Mutex::new(Some(to_server.clone())),
        }));
        (port, sent)
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_no_server_outlives_the_hub_however_it_ignores_being_stopped() {
        // The reapers were let go: when the window ended they died with it,
        // and a server ignoring `exit` and its closed input ran on.
        let child = std::process::Command::new("sleep").arg("30").spawn().expect("sleep runs");
        let pid = child.id();
        let (events_to, events) = mpsc::channel();
        let mut hub = Hub::new(events_to, events);
        let (port, _sent) = port();
        hub.servers.push(Server { child, port });
        let started = Instant::now();
        drop(hub);
        let alive = std::path::Path::new(&format!("/proc/{pid}")).exists()
            && !std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| stat.contains(") Z "));
        assert!(!alive, "the server was seen out");
        assert!(started.elapsed() < SHUTDOWN_GRACE, "without waiting out the whole grace");
    }

    #[test]
    fn test_a_server_that_stops_reading_holds_no_more_than_the_queue() {
        // Every flush queued the whole document; a server stuck short of
        // reading its input grew memory by a file per flush, without end.
        let (port, _unread) = port();
        port.0.ready.store(true, Ordering::Release);
        let sent = (0..MAX_QUEUED_MESSAGES * 2)
            .filter(|_| port.notify("textDocument/didChange", Value::Null))
            .count();
        assert_eq!(sent, MAX_QUEUED_MESSAGES);
    }

    #[test]
    fn test_initialize_s_answer_readies_the_server_and_says_initialized() {
        let (port, sent) = port();
        let answer = json!({"id": 0, "result": {"capabilities": {"renameProvider": true,
            "completionProvider": {"triggerCharacters": [".", ":"]}}}});
        let to = port.0.to_server.lock().expect("lock").clone().expect("open");
        let events = read(&answer, &port, &to, &mut BTreeMap::new());
        assert_eq!(events, [Event::Ready]);
        assert!(port.is_ready());
        assert!(port.provides("renameProvider"));
        assert!(!port.provides("hoverProvider"));
        assert_eq!(port.trigger_characters(), ['.', ':']);
        assert_eq!(sent.try_recv().expect("initialized")["method"], "initialized");
    }

    #[test]
    fn test_a_refused_initialize_ends_the_server_rather_than_leaving_it_never_ready() {
        // Said only as a message, it stayed among the servers, never ready:
        // starting it again answered that it already served the language.
        let (port, _sent) = port();
        let refusal = json!({"id": 0, "error": {"code": -32603, "message": "no workspace"}});
        let to = port.0.to_server.lock().expect("lock").clone().expect("open");
        let events = read(&refusal, &port, &to, &mut BTreeMap::new());
        assert_eq!(events, [Event::Exited("it did not start: no workspace".to_string())]);
        assert!(!port.is_ready());
    }

    #[test]
    fn test_the_answer_to_shutdown_lets_exit_go_and_reaches_no_editor() {
        // `exit` went out with `shutdown` unanswered, which the protocol
        // forbids: some servers then exit failing, or skip their cleanup.
        let (port, _sent) = port();
        port.0.shutdown_id.store(12, Ordering::Release);
        let to = port.0.to_server.lock().expect("lock").clone().expect("open");
        let answer = json!({"id": 12, "result": null});
        assert!(read(&answer, &port, &to, &mut BTreeMap::new()).is_empty(), "no editor asked for it");
        assert!(port.0.shutdown_answered.load(Ordering::Acquire));
        let other = json!({"id": 13, "result": null});
        assert_eq!(read(&other, &port, &to, &mut BTreeMap::new()).len(), 1, "another answer still reaches its editor");
    }

    #[test]
    fn test_a_question_from_the_server_is_answered_and_an_unknown_one_refused() {
        let (port, sent) = port();
        let to = port.0.to_server.lock().expect("lock").clone().expect("open");
        let asked = json!({"id": 3, "method": "workspace/configuration", "params": {"items": [{}, {}]}});
        assert!(read(&asked, &port, &to, &mut BTreeMap::new()).is_empty());
        assert_eq!(sent.try_recv().expect("answer")["result"], json!([null, null]));
        let unknown = json!({"id": "x", "method": "workspace/whatever"});
        read(&unknown, &port, &to, &mut BTreeMap::new());
        assert_eq!(sent.try_recv().expect("refusal")["error"]["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn test_diagnostics_land_in_the_store_and_an_empty_list_clears_them() {
        let (port, _sent) = port();
        let to = port.0.to_server.lock().expect("lock").clone().expect("open");
        let range = json!({"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}});
        let found = json!({"method": "textDocument/publishDiagnostics",
            "params": {"uri": "file:///lsp-client-test/a.rs", "diagnostics": [{"range": range, "message": "bad"}]}});
        let events = read(&found, &port, &to, &mut BTreeMap::new());
        assert_eq!(events, [Event::Diagnostics(PathBuf::from("/lsp-client-test/a.rs"))]);
        let path = Path::new("/lsp-client-test/a.rs");
        assert_eq!(diagnostics::of(Backend::Lsp(7), path).len(), 1);
        let cleared = json!({"method": "textDocument/publishDiagnostics",
            "params": {"uri": "file:///lsp-client-test/a.rs", "diagnostics": []}});
        read(&cleared, &port, &to, &mut BTreeMap::new());
        assert!(diagnostics::of(Backend::Lsp(7), path).is_empty());
    }

    #[test]
    fn test_progress_is_followed_by_token_until_each_ends() {
        let (port, _sent) = port();
        let to = port.0.to_server.lock().expect("lock").clone().expect("open");
        let mut progress = BTreeMap::new();
        let step = |value: Value, progress: &mut Progress| {
            let message = json!({"method": "$/progress", "params": {"token": "t", "value": value}});
            read(&message, &port, &to, progress)
        };
        let begun = step(json!({"kind": "begin", "title": "Indexing"}), &mut progress);
        assert_eq!(begun, [Event::Progress(Some("Indexing".to_string()))]);
        let reported = step(json!({"kind": "report", "message": "3/9", "percentage": 33}), &mut progress);
        assert_eq!(reported, [Event::Progress(Some("Indexing: 3/9 33%".to_string()))]);
        assert_eq!(step(json!({"kind": "end"}), &mut progress), [Event::Progress(None)]);
    }

    /// Start rust-analyzer on a crate of one file and ask it where a name
    /// is defined and what it is. Ignored: it needs rust-analyzer installed
    /// and takes seconds; run it with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn test_rust_analyzer_answers_a_definition_and_a_hover() {
        let dir = std::env::temp_dir().join(format!("winter-lsp-ra-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).expect("dirs");
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n")
            .expect("manifest");
        let text = "fn answer() -> u8 { 42 }\nfn main() { let _ = answer(); }\n";
        let file = dir.join("src/main.rs");
        std::fs::write(&file, text).expect("source");
        let (to, from) = mpsc::channel();
        let mut hub = Hub::new(to, from);
        let plan = super::super::servers::plan("rust", &file, None).expect("rust-analyzer installed");
        let port = hub.start(&plan).expect("started");
        let deadline = Instant::now() + Duration::from_secs(60);
        let wait = |hub: &mut Hub, found: &dyn Fn(&Event) -> bool| -> Option<Event> {
            while Instant::now() < deadline {
                if let Some(event) = hub.drain().into_iter().map(|i| i.event).find(|e| found(e)) {
                    return Some(event);
                }
                thread::sleep(Duration::from_millis(50));
            }
            None
        };
        assert!(wait(&mut hub, &|e| *e == Event::Ready).is_some(), "ready");
        assert!(port.notify("textDocument/didOpen", protocol::open_params(&file, "rust", 1, text)));
        let at = protocol::Position { character: 21, line: 1 };
        let mut location = Vec::new();
        // The first answers come before the crate is loaded; ask until one
        // lands.
        while location.is_empty() && Instant::now() < deadline {
            let id = port.request("textDocument/definition", protocol::at_params(&file, at)).expect("asked");
            if let Some(Event::Answer { result: Ok(value), .. }) =
                wait(&mut hub, &|e| matches!(e, Event::Answer { id: got, .. } if *got == id))
            {
                location = protocol::locations(&value);
            }
            thread::sleep(Duration::from_millis(200));
        }
        assert_eq!(location.first().map(|l| (l.path.clone(), l.range.start.line)), Some((file.clone(), 0)));
        let id = port.request("textDocument/hover", protocol::at_params(&file, at)).expect("asked");
        let hover = wait(&mut hub, &|e| matches!(e, Event::Answer { id: got, .. } if *got == id));
        let Some(Event::Answer { result: Ok(value), .. }) = hover else {
            panic!("no hover: {hover:?}");
        };
        assert!(protocol::hover_text(&value).is_some_and(|text| text.contains("fn answer() -> u8")));
        hub.stop_all();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_nothing_is_sent_before_the_server_is_ready() {
        let (port, sent) = port();
        assert_eq!(port.request("textDocument/hover", Value::Null), None);
        assert!(!port.notify("textDocument/didOpen", Value::Null));
        assert!(sent.try_recv().is_err());
        assert!(port.serves("rust", Path::new("/project/src/a.rs")));
        assert!(!port.serves("rust", Path::new("/elsewhere/a.rs")));
        assert!(!port.serves("python", Path::new("/project/a.py")));
    }
}
