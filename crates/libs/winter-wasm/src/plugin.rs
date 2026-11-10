//! One loaded extension: its instance, and every way Winter calls into it.
//!
//! Each call takes the instance for its length, on Winter's thread only if
//! it is free, so slow work running off that thread never holds typing up:
//! what finds it taken waits, or is passed over. What the extension asks of
//! Winter is kept while it runs and done once the call is over, so it may
//! ask Winter to call it again.

use std::collections::HashMap;
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::Duration;

use wasmtime::component::ResourceTable;
use wasmtime::{Store, StoreLimits};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};
use winter_ext_api::{
    CompletionRequest, Completions, Diagnostic, Document, Event, Host, InlineAnswer, InlineKind,
    InlineOffer, InlineProvider, InlineRequest, Line, PickerEvent, PickerEvents, PickerId, Repaint,
    Styled, Tone, View,
};

use super::bindings::winter::extension::{host, types};
use super::bindings::Extension;
use super::convert;
use super::ticks;

// ========================================================================
// Constants
// ========================================================================

/// How long a call made on Winter's thread may take: a key, a command, a
/// view drawn.
const INTERACTIVE_BUDGET: Duration = Duration::from_secs(1);

/// How long slow work, run off Winter's thread, may take.
const WORK_BUDGET: Duration = Duration::from_secs(60);

/// What is said of a command run while the extension is busy.
const BUSY: &str = "is busy with earlier work; try again in a moment";

/// What is said of a server an extension may not run.
const NOT_GRANTED: &str = "may not run";

/// What is said, once, of an extension that failed.
const STOPPED: &str = "failed and was stopped";

/// What a view says once its extension has stopped.
const VIEW_STOPPED: &str = "This extension has stopped.";

// ========================================================================
// Data Structures
// ========================================================================

/// One loaded extension, shared by everything it registered.
pub(super) struct Plugin {
    /// What waits for the instance to be free, on Winter's thread.
    deferred: Mutex<Vec<Deferred>>,
    /// The programs it may run as its servers.
    grants: Grants,
    /// The extension's id.
    pub(super) id: &'static str,
    instance: Mutex<Instance>,
    /// The pickers it has open, by the number it gave each, as Winter knows
    /// them.
    pickers: Mutex<HashMap<u64, PickerId>>,
    /// Whether it has failed, and so is called no more.
    stopped: AtomicBool,
}

/// The programs an extension may have Winter run: the program each of its
/// servers names, and those it is granted.
pub(super) struct Grants {
    /// Each server's program, by the server's name.
    pub(super) programs: HashMap<String, String>,
    /// The file names of the programs granted.
    pub(super) run: Vec<String>,
    /// The directories it may write to, where no program it names is run:
    /// it could have put one there.
    pub(super) writable: Vec<PathBuf>,
}

/// The component, running, and its store.
pub(super) struct Instance {
    bindings: Extension,
    store: Store<State>,
}

/// What a call into an extension keeps while it runs.
pub(super) struct State {
    /// What it asked of Winter, to be done once the call is over.
    actions: Vec<Action>,
    /// The file in front as the call began.
    document: Option<Document>,
    pub(super) limits: StoreLimits,
    /// How it has Winter paint again, from any thread, once it has had one.
    repaint: Option<Repaint>,
    table: ResourceTable,
    wasi: WasiCtx,
}

/// Something an extension asked of Winter.
enum Action {
    AnswerInline(AnswerInline),
    CopyText(String),
    InstallServer(String),
    Notify(String),
    OfferInline(OfferInline),
    OpenFile(OpenFile),
    OpenPicker(OpenPicker),
    OpenUrl(String),
    OpenView(String),
    PublishDiagnostics(PublishDiagnostics),
    PushRows(PushRows),
    RunCommand(String),
    ServerNotify(ServerNotify),
    ServerRequest(ServerRequest),
    SetStatus(SetStatus),
    Spawn(u32),
    StartServer(StartServer),
}

/// What an inline source offers now.
struct OfferInline {
    name: String,
    offer: InlineOffer,
}

/// The answer to the inline request `ticket`.
struct AnswerInline {
    answer: InlineAnswer,
    ticket: u64,
}

/// A server to start: by its name, its program where named.
struct StartServer {
    name: String,
    program: Option<String>,
}

/// A request for a server, its answer to come under `tag`.
struct ServerRequest {
    method: String,
    name: String,
    params: String,
    tag: u64,
}

/// A notification for a server.
struct ServerNotify {
    method: String,
    name: String,
    params: String,
}

struct OpenFile {
    line: Option<u32>,
    path: String,
}

/// A picker to open, known to the guest by `number`.
struct OpenPicker {
    number: u64,
    spec: types::PickerSpec,
}

/// Rows for the picker the guest numbers `number`.
struct PushRows {
    done: bool,
    number: u64,
    rows: Vec<types::PickerRow>,
}

struct PublishDiagnostics {
    diagnostics: Vec<types::Diagnostic>,
    path: String,
}

struct SetStatus {
    name: String,
    text: Option<String>,
}

/// A call that found the instance taken, made once it is free.
enum Deferred {
    CloseView(u32),
    Event(types::Event),
    FinishWork(u32),
    PickerEvent(PickerEventCall),
}

/// What happened in the picker the guest numbers `picker`.
struct PickerEventCall {
    event: types::PickerEvent,
    picker: u64,
}

/// Why a call into an extension was not made.
enum Unavailable {
    /// Its slow work holds it.
    Busy,
    /// It failed.
    Stopped,
}

/// An inline source a component answers for.
pub(super) struct WasmInline {
    name: String,
    plugin: Arc<Plugin>,
}

/// A view an extension draws, as a tab of Winter's.
pub(super) struct WasmView {
    /// What it last drew, shown again while the extension is busy.
    drawn: Vec<Line>,
    name: String,
    /// The number the extension knows it by, once opened.
    number: Option<u32>,
    plugin: Arc<Plugin>,
}

// ========================================================================
// Instance
// ========================================================================

impl Instance {
    pub(super) fn new(store: Store<State>, bindings: Extension) -> Self {
        Self { bindings, store }
    }
}

// ========================================================================
// Plugin
// ========================================================================

impl Plugin {
    pub(super) fn new(id: &'static str, instance: Instance, grants: Grants) -> Arc<Self> {
        Arc::new(Self {
            deferred: Mutex::new(Vec::new()),
            grants,
            id,
            instance: Mutex::new(instance),
            pickers: Mutex::default(),
            stopped: AtomicBool::new(false),
        })
    }

    pub(super) fn run_command(self: &Arc<Self>, host: &mut dyn Host, name: &str) {
        let ran = self.call(Some(host), |instance| {
            let guest = instance.bindings.winter_extension_guest();
            guest.call_run_command(&mut instance.store, name)
        });
        if let Err(Unavailable::Busy) = ran {
            host.notify(&format!("{} {BUSY}", self.id));
        }
    }

    pub(super) fn handle_event(self: &Arc<Self>, host: &mut dyn Host, event: &Event) {
        let Some(event) = convert::to_wit_event(event) else {
            return;
        };
        let handled = self.call(Some(host), |instance| {
            let guest = instance.bindings.winter_extension_guest();
            guest.call_handle_event(&mut instance.store, &event)
        });
        if let Err(Unavailable::Busy) = handled {
            self.defer(Deferred::Event(event));
        }
    }

    pub(super) fn complete(
        self: &Arc<Self>,
        name: &str,
        request: &CompletionRequest,
    ) -> Option<Completions> {
        let request = convert::to_wit_request(request);
        let offered = self.call(None, |instance| {
            let guest = instance.bindings.winter_extension_guest();
            guest.call_complete(&mut instance.store, name, &request)
        });
        offered.ok().flatten().map(convert::to_completions)
    }

    /// Run the work numbered `work` off Winter's thread, waiting for the
    /// instance as long as it takes.
    fn run_work(self: &Arc<Self>, work: u32) {
        let Ok(mut instance) = self.lock(true) else {
            return;
        };
        instance.store.set_epoch_deadline(ticks(WORK_BUDGET));
        let ran = {
            let Instance { bindings, store } = &mut *instance;
            bindings.winter_extension_guest().call_run_work(store, work)
        };
        if let Err(error) = ran {
            self.stop(&mut instance, &error);
        }
    }

    /// Hand the work numbered `work`'s result back, on Winter's thread.
    fn finish_work(self: &Arc<Self>, host: &mut dyn Host, work: u32) {
        let finished = self.call(Some(host), |instance| {
            let guest = instance.bindings.winter_extension_guest();
            guest.call_finish_work(&mut instance.store, work)
        });
        if let Err(Unavailable::Busy) = finished {
            self.defer(Deferred::FinishWork(work));
        }
    }

    /// Tell the guest what happened in the picker it numbers `picker`, on
    /// Winter's thread; a picker done with is forgotten.
    fn picker_event(self: &Arc<Self>, host: &mut dyn Host, picker: u64, event: PickerEvent) {
        if matches!(event, PickerEvent::Chosen(_) | PickerEvent::Closed) {
            self.pickers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&picker);
        }
        let Some(event) = convert::to_wit_picker_event(event) else {
            return;
        };
        let told = self.call(Some(host), |instance| {
            let guest = instance.bindings.winter_extension_guest();
            guest.call_tell_picker(&mut instance.store, picker, &event)
        });
        if let Err(Unavailable::Busy) = told {
            self.defer(Deferred::PickerEvent(PickerEventCall { event, picker }));
        }
    }

    /// Call into the instance with `call`, on Winter's thread, then do what
    /// the extension asked: with `host` there to do it with, or kept until
    /// a call that has one.
    fn call<R>(
        self: &Arc<Self>,
        mut host: Option<&mut dyn Host>,
        call: impl FnOnce(&mut Instance) -> wasmtime::Result<R>,
    ) -> Result<R, Unavailable> {
        let mut instance = match self.lock(false) {
            Ok(instance) => instance,
            Err(unavailable) => {
                if let (Unavailable::Stopped, Some(host)) = (&unavailable, host) {
                    self.replay(host, instance_actions(self));
                }
                return Err(unavailable);
            }
        };
        if let Some(host) = host.as_deref_mut() {
            let state = instance.store.data_mut();
            state.document = host.document();
            state.repaint.get_or_insert_with(|| host.repaint());
        }
        instance.store.set_epoch_deadline(ticks(INTERACTIVE_BUDGET));
        let result = self
            .flush_deferred(&mut instance, host.is_some())
            .and_then(|()| call(&mut instance));
        if let Err(error) = &result {
            self.stop(&mut instance, error);
        }
        // Asked for with no host to do it, it waits in the store for a
        // call that has one.
        let actions = match host.is_some() {
            true => mem::take(&mut instance.store.data_mut().actions),
            false => Vec::new(),
        };
        drop(instance);
        if let Some(host) = host {
            self.replay(host, actions);
        }
        result.map_err(|_| Unavailable::Stopped)
    }

    /// Make the calls that found the instance taken; those needing Winter's
    /// help only if `hosted`.
    fn flush_deferred(&self, instance: &mut Instance, hosted: bool) -> wasmtime::Result<()> {
        let pending = mem::take(&mut *self.deferred.lock().unwrap_or_else(|e| e.into_inner()));
        let guest = instance.bindings.winter_extension_guest();
        let store = &mut instance.store;
        let mut kept = Vec::new();
        for deferred in pending {
            match deferred {
                Deferred::CloseView(view) => guest.call_close_view(&mut *store, view)?,
                Deferred::Event(event) if hosted => guest.call_handle_event(&mut *store, &event)?,
                Deferred::FinishWork(work) if hosted => {
                    guest.call_finish_work(&mut *store, work)?
                }
                Deferred::PickerEvent(told) if hosted => {
                    guest.call_tell_picker(&mut *store, told.picker, &told.event)?
                }
                waiting @ (Deferred::Event(_)
                | Deferred::FinishWork(_)
                | Deferred::PickerEvent(_)) => kept.push(waiting),
            }
        }
        self.deferred
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(kept);
        Ok(())
    }

    /// Do what the extension asked of Winter, through `host`.
    fn replay(self: &Arc<Self>, host: &mut dyn Host, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::AnswerInline(answered) => {
                    host.answer_inline(answered.ticket, answered.answer)
                }
                Action::CopyText(text) => host.copy_text(&text),
                Action::OfferInline(offered) => host.offer_inline(&offered.name, offered.offer),
                Action::InstallServer(name) => host.install_server(&name),
                Action::ServerNotify(told) => {
                    host.server_notify(&told.name, &told.method, &told.params)
                }
                Action::ServerRequest(asked) => {
                    host.server_request(&asked.name, asked.tag, &asked.method, &asked.params)
                }
                Action::StartServer(start) => {
                    let program = start.program.map(PathBuf::from);
                    match self.grants.may_run(&start.name, program.as_deref()) {
                        Ok(()) => host.start_server(&start.name, program),
                        Err(program) => host.notify(&format!(
                            "the {} extension {NOT_GRANTED} {program}: grant it with \
                             `winter extension grant {} run {program}`",
                            self.id, self.id
                        )),
                    }
                }
                Action::Notify(message) => host.notify(&message),
                Action::OpenFile(open) => {
                    host.open_file(Path::new(&open.path), open.line.map(|line| line as usize))
                }
                Action::OpenUrl(url) => host.open_url(&url),
                Action::OpenView(name) => host.open_view(&name),
                Action::OpenPicker(open) => {
                    let (plugin, number, id) = (Arc::clone(self), open.number, PickerId::fresh());
                    self.pickers
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(number, id);
                    let events: PickerEvents =
                        Box::new(move |host, _, event| plugin.picker_event(host, number, event));
                    host.open_picker(id, convert::to_picker_spec(open.spec), events);
                }
                Action::PushRows(push) => {
                    let pickers = self.pickers.lock().unwrap_or_else(|e| e.into_inner());
                    let Some(id) = pickers.get(&push.number).copied() else {
                        continue;
                    };
                    drop(pickers);
                    let rows = push.rows.into_iter().map(convert::to_picker_row).collect();
                    host.push_rows(id, rows, push.done);
                }
                Action::PublishDiagnostics(publish) => host.publish_diagnostics(
                    Path::new(&publish.path),
                    publish
                        .diagnostics
                        .into_iter()
                        .map(convert::to_diagnostic)
                        .collect::<Vec<Diagnostic>>(),
                ),
                Action::RunCommand(id) => host.run_command(&id),
                Action::SetStatus(status) => host.set_status(&status.name, status.text),
                Action::Spawn(work) => {
                    let plugin = Arc::clone(self);
                    host.spawn_work(Box::new(move || {
                        plugin.run_work(work);
                        Box::new(move |host: &mut dyn Host| plugin.finish_work(host, work))
                    }));
                }
            }
        }
    }

    /// Take the instance: waiting for it if `wait`, else only if free.
    fn lock(&self, wait: bool) -> Result<MutexGuard<'_, Instance>, Unavailable> {
        if self.stopped.load(Ordering::Relaxed) {
            return Err(Unavailable::Stopped);
        }
        match wait {
            true => self.instance.lock().map_err(|_| Unavailable::Stopped),
            false => match self.instance.try_lock() {
                Ok(instance) => Ok(instance),
                Err(TryLockError::WouldBlock) => Err(Unavailable::Busy),
                Err(TryLockError::Poisoned(_)) => Err(Unavailable::Stopped),
            },
        }
    }

    /// Stop the extension for `error`, keeping a notice of it to show.
    fn stop(&self, instance: &mut Instance, error: &wasmtime::Error) {
        if self.stopped.swap(true, Ordering::Relaxed) {
            return;
        }
        let notice = format!("the {} extension {STOPPED}: {error:#}", self.id);
        instance
            .store
            .data_mut()
            .actions
            .push(Action::Notify(notice));
    }

    fn defer(&self, deferred: Deferred) {
        self.deferred
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(deferred);
    }
}

// ========================================================================
// Grants
// ========================================================================

impl Grants {
    /// Whether the server `name` may be run: `program` where named, else
    /// its own, each by its file name, and a program named never one in a
    /// directory the extension may write to. One that may not is handed
    /// back by its name.
    fn may_run(&self, name: &str, program: Option<&Path>) -> Result<(), String> {
        if let Some(program) = program.filter(|program| self.is_writable(program)) {
            return Err(program.display().to_string());
        }
        let own = self.programs.get(name).map(String::as_str);
        // Granted by the name it has without Windows' `.exe`.
        let file_name = program
            .and_then(Path::file_name)
            .and_then(|file_name| file_name.to_str())
            .map(|file_name| {
                let suffix = std::env::consts::EXE_SUFFIX;
                file_name
                    .strip_suffix(suffix)
                    .filter(|_| !suffix.is_empty())
                    .unwrap_or(file_name)
            })
            .or(own)
            .unwrap_or(name);
        match own.is_some() && self.run.iter().any(|granted| granted == file_name) {
            true => Ok(()),
            false => Err(file_name.to_string()),
        }
    }
}

impl Grants {
    /// Whether `program`, its links followed, lies in a directory the
    /// extension may write to. One not there to follow is not run anyway.
    fn is_writable(&self, program: &Path) -> bool {
        let real = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let program = real(program);
        self.writable
            .iter()
            .any(|dir| program.starts_with(real(dir)))
    }
}

// ========================================================================
// State
// ========================================================================

impl State {
    pub(super) fn new(wasi: WasiCtx) -> Self {
        Self {
            actions: Vec::new(),
            document: None,
            limits: StoreLimits::default(),
            repaint: None,
            table: ResourceTable::new(),
            wasi,
        }
    }
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl types::Host for State {}

impl host::Host for State {
    fn notify(&mut self, message: String) {
        self.actions.push(Action::Notify(message));
    }

    fn open_file(&mut self, path: String, line: Option<u32>) {
        self.actions.push(Action::OpenFile(OpenFile { line, path }));
    }

    fn open_view(&mut self, name: String) {
        self.actions.push(Action::OpenView(name));
    }

    fn front_document(&mut self) -> Option<types::Document> {
        self.document.as_ref().map(convert::to_wit_document)
    }

    fn publish_diagnostics(&mut self, path: String, diagnostics: Vec<types::Diagnostic>) {
        self.actions
            .push(Action::PublishDiagnostics(PublishDiagnostics {
                diagnostics,
                path,
            }));
    }

    fn run_command(&mut self, id: String) {
        self.actions.push(Action::RunCommand(id));
    }

    fn open_url(&mut self, url: String) {
        self.actions.push(Action::OpenUrl(url));
    }

    fn copy_text(&mut self, text: String) {
        self.actions.push(Action::CopyText(text));
    }

    fn start_server(&mut self, name: String, program: Option<String>) {
        self.actions
            .push(Action::StartServer(StartServer { name, program }));
    }

    fn install_server(&mut self, name: String) {
        self.actions.push(Action::InstallServer(name));
    }

    fn server_request(&mut self, name: String, tag: u64, method: String, params: String) {
        self.actions.push(Action::ServerRequest(ServerRequest {
            method,
            name,
            params,
            tag,
        }));
    }

    fn offer_inline(&mut self, name: String, offer: types::InlineOffer) {
        self.actions.push(Action::OfferInline(OfferInline {
            name,
            offer: convert::to_inline_offer(offer),
        }));
    }

    fn answer_inline(&mut self, ticket: u64, answer: types::InlineAnswer) {
        self.actions.push(Action::AnswerInline(AnswerInline {
            answer: convert::to_inline_answer(answer),
            ticket,
        }));
    }

    fn server_notify(&mut self, name: String, method: String, params: String) {
        self.actions.push(Action::ServerNotify(ServerNotify {
            method,
            name,
            params,
        }));
    }

    fn set_status(&mut self, name: String, text: Option<String>) {
        self.actions
            .push(Action::SetStatus(SetStatus { name, text }));
    }

    fn spawn(&mut self, work: u32) {
        self.actions.push(Action::Spawn(work));
    }

    fn open_picker(&mut self, picker: u64, spec: types::PickerSpec) {
        self.actions.push(Action::OpenPicker(OpenPicker {
            number: picker,
            spec,
        }));
    }

    fn push_rows(&mut self, picker: u64, rows: Vec<types::PickerRow>, done: bool) {
        self.actions.push(Action::PushRows(PushRows {
            done,
            number: picker,
            rows,
        }));
    }

    fn request_repaint(&mut self) {
        if let Some(repaint) = &self.repaint {
            repaint.request();
        }
    }
}

// ========================================================================
// WasmInline
// ========================================================================

impl WasmInline {
    pub(super) fn new(plugin: Arc<Plugin>, name: String) -> Self {
        Self { name, plugin }
    }
}

impl InlineProvider for WasmInline {
    /// Asked while the extension is busy, it is not asked: typing on asks
    /// again.
    fn ask(&self, host: &mut dyn Host, request: &InlineRequest) {
        let request = convert::to_wit_inline_request(request);
        let _ = self.plugin.call(Some(host), |instance| {
            let guest = instance.bindings.winter_extension_guest();
            guest.call_ask_inline(&mut instance.store, &self.name, &request)
        });
    }

    fn shown(&self, host: &mut dyn Host, kind: InlineKind, receipt: &str) {
        let kind = convert::to_wit_inline_kind(kind);
        let _ = self.plugin.call(Some(host), |instance| {
            let guest = instance.bindings.winter_extension_guest();
            guest.call_inline_shown(&mut instance.store, &self.name, kind, receipt)
        });
    }

    fn taken(&self, host: &mut dyn Host, kind: InlineKind, receipt: &str) {
        let kind = convert::to_wit_inline_kind(kind);
        let _ = self.plugin.call(Some(host), |instance| {
            let guest = instance.bindings.winter_extension_guest();
            guest.call_inline_taken(&mut instance.store, &self.name, kind, receipt)
        });
    }
}

// ========================================================================
// WasmView
// ========================================================================

impl WasmView {
    /// The extension's view `name`, opened as soon as the extension is free.
    pub(super) fn open(plugin: Arc<Plugin>, name: String) -> Self {
        let mut view = Self {
            drawn: Vec::new(),
            name,
            number: None,
            plugin,
        };
        view.ensure_open();
        view
    }

    /// The number the extension knows the view by, opening it if it has
    /// not been yet.
    fn ensure_open(&mut self) -> Option<u32> {
        if self.number.is_none() {
            let name = &self.name;
            self.number = self
                .plugin
                .call(None, |instance| {
                    let guest = instance.bindings.winter_extension_guest();
                    guest.call_open_view(&mut instance.store, name)
                })
                .ok();
        }
        self.number
    }

    /// What was last drawn, or, once the extension has stopped, a line
    /// saying so.
    fn drawn_or_stopped(&self) -> Vec<Line> {
        match self.plugin.stopped.load(Ordering::Relaxed) {
            true => vec![vec![Styled::new(Tone::Error, VIEW_STOPPED)]],
            false => self.drawn.clone(),
        }
    }
}

impl View for WasmView {
    fn render(&mut self, rows: usize, columns: usize) -> Vec<Line> {
        let Some(view) = self.ensure_open() else {
            return self.drawn_or_stopped();
        };
        let rows = u32::try_from(rows).unwrap_or(u32::MAX);
        let columns = u32::try_from(columns).unwrap_or(u32::MAX);
        let drawn = self.plugin.call(None, |instance| {
            let guest = instance.bindings.winter_extension_guest();
            guest.call_render(&mut instance.store, view, rows, columns)
        });
        match drawn {
            Ok(lines) => {
                self.drawn = lines.into_iter().map(convert::to_line).collect();
                self.drawn.clone()
            }
            Err(_) => self.drawn_or_stopped(),
        }
    }

    fn on_action(&mut self, host: &mut dyn Host, action: &str) {
        let Some(view) = self.ensure_open() else {
            return;
        };
        // An action asked while the extension is busy goes unanswered.
        let _ = self.plugin.call(Some(host), |instance| {
            let guest = instance.bindings.winter_extension_guest();
            guest.call_on_action(&mut instance.store, view, action)
        });
    }

    fn page(&mut self) -> Option<String> {
        let view = self.ensure_open()?;
        let page = self.plugin.call(None, |instance| {
            let guest = instance.bindings.winter_extension_guest();
            guest.call_page(&mut instance.store, view)
        });
        page.ok().flatten()
    }

    fn cursor_line(&self) -> Option<usize> {
        let view = self.number?;
        let line = self.plugin.call(None, |instance| {
            let guest = instance.bindings.winter_extension_guest();
            guest.call_cursor_line(&mut instance.store, view)
        });
        line.ok().flatten().map(|line| line as usize)
    }

    fn on_click(&mut self, host: &mut dyn Host, line: usize, column: usize) -> bool {
        let Some(view) = self.ensure_open() else {
            return false;
        };
        let line = u32::try_from(line).unwrap_or(u32::MAX);
        let column = u32::try_from(column).unwrap_or(u32::MAX);
        let taken = self.plugin.call(Some(host), |instance| {
            let guest = instance.bindings.winter_extension_guest();
            guest.call_on_click(&mut instance.store, view, line, column)
        });
        taken.unwrap_or(false)
    }

    fn on_scroll(&mut self, lines: isize) -> bool {
        let Some(view) = self.ensure_open() else {
            return false;
        };
        let lines = i32::try_from(lines).unwrap_or(if lines < 0 { i32::MIN } else { i32::MAX });
        let taken = self.plugin.call(None, |instance| {
            let guest = instance.bindings.winter_extension_guest();
            guest.call_on_scroll(&mut instance.store, view, lines)
        });
        taken.unwrap_or(false)
    }
}

impl Drop for WasmView {
    fn drop(&mut self) {
        let Some(view) = self.number else {
            return;
        };
        let closed = self.plugin.call(None, |instance| {
            let guest = instance.bindings.winter_extension_guest();
            guest.call_close_view(&mut instance.store, view)
        });
        if let Err(Unavailable::Busy) = closed {
            self.plugin.defer(Deferred::CloseView(view));
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What a stopped extension asked of Winter and has not had done: the
/// notice of its failing, chiefly.
fn instance_actions(plugin: &Plugin) -> Vec<Action> {
    let mut instance = match plugin.instance.try_lock() {
        Ok(instance) => instance,
        Err(_) => return Vec::new(),
    };
    mem::take(&mut instance.store.data_mut().actions)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn grants(run: &[&str]) -> Grants {
        Grants {
            programs: [("pilot".to_string(), "pilot-server".to_string())].into(),
            run: run.iter().map(|program| program.to_string()).collect(),
            writable: vec![PathBuf::from("/data/pilot")],
        }
    }

    #[test]
    fn test_a_program_in_a_directory_the_extension_writes_is_never_run() {
        let planted = Path::new("/data/pilot/bin/pilot-server");
        assert!(
            grants(&["pilot-server"])
                .may_run("pilot", Some(planted))
                .is_err(),
            "granted by name, but put there by the extension"
        );
    }

    #[test]
    fn test_a_server_runs_only_where_its_program_is_granted() {
        assert_eq!(grants(&["pilot-server"]).may_run("pilot", None), Ok(()));
        assert_eq!(
            grants(&[]).may_run("pilot", None),
            Err("pilot-server".to_string()),
            "not granted"
        );
        assert!(
            grants(&["pilot-server"]).may_run("other", None).is_err(),
            "a server it never declared"
        );
    }

    #[test]
    fn test_a_program_named_in_place_of_its_own_is_granted_by_its_file_name() {
        let granted = grants(&["pilot-server"]);
        let same = Path::new("/opt/pilot/pilot-server");
        assert_eq!(granted.may_run("pilot", Some(same)), Ok(()));
        let other = Path::new("/bin/sh");
        assert_eq!(granted.may_run("pilot", Some(other)), Err("sh".to_string()));
    }
}
