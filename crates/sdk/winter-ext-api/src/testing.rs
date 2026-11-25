//! A [`Host`] for an extension's tests, so they never implement one: what
//! the extension asks of Winter is kept to be looked at, slow work is done
//! at once, and a picker can be answered as it opens.
//!
//! ```
//! use winter_ext_api::testing::Recorder;
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
//!         registry.command("greet", "Say Hello", |host: &mut dyn Host| host.notify("hello"));
//!     }
//! }
//!
//! let registry = Registry::activate_all([&Hello as &dyn Extension]);
//! let mut host = Recorder::default();
//! (registry.find("hello.greet").expect("added").run)(&mut host);
//! assert_eq!(host.said, ["hello"]);
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{
    Diagnostic, Document, Host, InlineAnswer, InlineOffer, PickerChoice, PickerEvent, PickerEvents,
    PickerId, PickerRow, PickerSpec, Repaint, Request, Status, Work,
};

// ========================================================================
// Data Structures
// ========================================================================

/// A host keeping everything an extension asks of it, in the order asked.
/// Slow work handed to [`spawn`](super::spawn) runs at once, its reply
/// with it. Fields are added as [`Host`] grows, never taken away.
#[derive(Default)]
#[non_exhaustive]
pub struct Recorder {
    /// Each answer to an inline request, under its ticket.
    pub answered: Vec<(u64, InlineAnswer)>,
    /// The action chosen on the last row of each picker as it opens.
    choose: Option<String>,
    /// The commands run, by full id.
    pub commands: Vec<String>,
    /// What was put on the clipboard.
    pub copied: Vec<String>,
    /// The file in front, as [`Host::document`] answers.
    document: Option<Document>,
    /// The web requests made, each under its tag.
    pub fetched: Vec<(u64, Request)>,
    /// The servers fetched, by name.
    pub installed: Vec<String>,
    /// What each inline source said it offers, by name.
    pub offered: Vec<(String, InlineOffer)>,
    /// The files opened, at the line given.
    pub opened: Vec<(PathBuf, Option<usize>)>,
    /// The pickers opened, as each was asked for.
    pub pickers: Vec<PickerSpec>,
    /// The problems published, by file.
    pub published: Vec<(PathBuf, Vec<Diagnostic>)>,
    /// The rows given pickers after they opened.
    pub pushed: Vec<PickerRow>,
    /// The programs run, each under its tag.
    pub ran: Vec<(u64, Program)>,
    /// The notices shown.
    pub said: Vec<String>,
    /// The secrets the user was asked for, by key.
    pub secrets: Vec<String>,
    /// The requests and notifications sent to servers.
    pub sent: Vec<ServerMessage>,
    /// The servers started, by name, at the program given.
    pub started: Vec<(String, Option<PathBuf>)>,
    /// What each status bar segment shows now, by name; one cleared is
    /// absent.
    pub statuses: BTreeMap<String, Status>,
    /// The programs opened in terminal tabs.
    pub terminals: Vec<Program>,
    /// When each timer set rings, by name, as last set.
    pub timers: BTreeMap<String, Duration>,
    /// The web pages opened.
    pub urls: Vec<String>,
    /// The views opened, by name.
    pub views: Vec<String>,
    /// The files watched.
    pub watched: Vec<PathBuf>,
}

/// A program and what it was run with.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Program {
    pub args: Vec<String>,
    pub program: String,
}

/// What was sent to one of the extension's servers: a request, under its
/// tag, or a notification, with none.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ServerMessage {
    pub method: String,
    /// The server, by name.
    pub name: String,
    /// The parameters, as JSON.
    pub params: String,
    pub tag: Option<u64>,
}

// ========================================================================
// Recorder
// ========================================================================

impl Recorder {
    /// A recorder whose editor in front holds `document`.
    pub fn with_document(mut self, document: Document) -> Self {
        self.document = Some(document);
        self
    }

    /// A recorder choosing `action` on the last row of each picker as it
    /// opens, as the user would.
    pub fn choosing(mut self, action: impl Into<String>) -> Self {
        self.choose = Some(action.into());
        self
    }
}

impl Host for Recorder {
    fn notify(&mut self, message: &str) {
        self.said.push(message.to_string());
    }

    fn open_file(&mut self, path: &Path, line: Option<usize>) {
        self.opened.push((path.to_path_buf(), line));
    }

    fn open_view(&mut self, name: &str) {
        self.views.push(name.to_string());
    }

    fn document(&self) -> Option<Document> {
        self.document.clone()
    }

    fn publish_diagnostics(&mut self, path: &Path, diagnostics: Vec<Diagnostic>) {
        self.published.push((path.to_path_buf(), diagnostics));
    }

    fn run_command(&mut self, id: &str) {
        self.commands.push(id.to_string());
    }

    fn open_url(&mut self, url: &str) {
        self.urls.push(url.to_string());
    }

    fn copy_text(&mut self, text: &str) {
        self.copied.push(text.to_string());
    }

    fn start_server(&mut self, name: &str, program: Option<PathBuf>) {
        self.started.push((name.to_string(), program));
    }

    fn install_server(&mut self, name: &str) {
        self.installed.push(name.to_string());
    }

    fn server_request(&mut self, name: &str, tag: u64, method: &str, params: &str) {
        self.sent
            .push(ServerMessage::new(name, Some(tag), method, params));
    }

    fn server_notify(&mut self, name: &str, method: &str, params: &str) {
        self.sent
            .push(ServerMessage::new(name, None, method, params));
    }

    fn offer_inline(&mut self, name: &str, offer: InlineOffer) {
        self.offered.push((name.to_string(), offer));
    }

    fn answer_inline(&mut self, ticket: u64, answer: InlineAnswer) {
        self.answered.push((ticket, answer));
    }

    fn set_status(&mut self, name: &str, status: Option<Status>) {
        match status {
            Some(status) => self.statuses.insert(name.to_string(), status),
            None => self.statuses.remove(name),
        };
    }

    fn fetch(&mut self, tag: u64, request: Request) {
        self.fetched.push((tag, request));
    }

    fn set_timer(&mut self, name: &str, after: Duration) {
        self.timers.insert(name.to_string(), after);
    }

    fn run(&mut self, tag: u64, program: &str, args: &[String]) {
        self.ran.push((tag, Program::new(program, args)));
    }

    fn open_terminal(&mut self, program: &str, args: &[String]) {
        self.terminals.push(Program::new(program, args));
    }

    fn watch_file(&mut self, path: &Path) {
        self.watched.push(path.to_path_buf());
    }

    fn ask_secret(&mut self, key: &str, _label: &str) {
        self.secrets.push(key.to_string());
    }

    /// UTC's own time, so what a test shows does not hang on where it runs.
    fn utc_offset(&self, _at: i64) -> i32 {
        0
    }

    fn spawn_work(&mut self, work: Work) {
        let reply = work();
        reply(self);
    }

    fn open_picker(&mut self, id: PickerId, spec: PickerSpec, mut events: PickerEvents) {
        let last = spec.rows.len().checked_sub(1);
        self.pickers.push(spec);
        if let (Some(action), Some(row)) = (self.choose.clone(), last) {
            let choice = PickerChoice::new(action, row);
            events(self, id, PickerEvent::Chosen(choice));
        }
    }

    fn push_rows(&mut self, _picker: PickerId, rows: Vec<PickerRow>, _done: bool) {
        self.pushed.extend(rows);
    }

    fn repaint(&self) -> Repaint {
        Repaint::new(|| {})
    }
}

// ========================================================================
// Program
// ========================================================================

impl Program {
    fn new(program: &str, args: &[String]) -> Self {
        Self {
            args: args.to_vec(),
            program: program.to_string(),
        }
    }
}

// ========================================================================
// ServerMessage
// ========================================================================

impl ServerMessage {
    fn new(name: &str, tag: Option<u64>, method: &str, params: &str) -> Self {
        Self {
            method: method.to_string(),
            name: name.to_string(),
            params: params.to_string(),
            tag,
        }
    }
}
