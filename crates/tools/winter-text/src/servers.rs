//! The language servers, as the session talks to them for every document:
//! each document is opened with the server serving its language, its text
//! handed over again a moment after it changes and closed with its last
//! tab; and what every checker publishes about its file (a server, an
//! extension) is laid on its text as undercurls and in its sign column.

use viento::{Decoration, DecorationKind, DocumentId, Sign, Utf16Position};
use winter_lsp::diagnostics;
use winter_lsp::protocol::{self, Diagnostic, Range, Severity};
use winter_lsp::{servers, Port, ServerId};

use super::session::Session;

// ========================================================================
// Constants
// ========================================================================

/// What the problems a document's checkers found are laid on it as.
const PROBLEMS: &str = "problems";

// ========================================================================
// Data Structures
// ========================================================================

/// What the server serving a document last had of it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Synced {
    /// The document's version handed over.
    pub(crate) at: u64,
    /// The server.
    pub(crate) server: ServerId,
    /// The version the server knows the text by.
    pub(crate) version: i32,
}

/// The servers running, as the window last handed them over.
#[derive(Debug, Default)]
pub(crate) struct Servers {
    /// Which hand-over they came in, so one seen already is not taken
    /// again.
    generation: Option<u64>,
    /// The servers.
    ports: Vec<Port>,
}

// ========================================================================
// Session: the servers
// ========================================================================

impl Session {
    /// Takes `ports`, the servers running as of `generation`: a document
    /// whose server stopped forgets it was handed over, to be opened with
    /// whatever serves it next. Whether they were new.
    pub(crate) fn connect(&mut self, generation: u64, ports: &[Port]) -> bool {
        if self.servers.generation == Some(generation) {
            return false;
        }
        self.servers.generation = Some(generation);
        self.servers.ports = ports.to_vec();
        let live: Vec<ServerId> = ports.iter().map(Port::id).collect();
        for file in self.files.values_mut() {
            if file
                .synced
                .is_some_and(|synced| !live.contains(&synced.server))
            {
                file.synced = None;
            }
            file.extension_synced
                .retain(|server, _| live.contains(server));
        }
        true
    }

    /// The server serving `document`, written in `language`, ready.
    pub(crate) fn serving(&self, document: DocumentId, language: &str) -> Option<&Port> {
        let path = &self.files.get(&document)?.path;
        self.servers
            .ports
            .iter()
            .find(|port| port.is_ready() && !port.is_everywhere() && port.serves(language, path))
    }

    /// The server running as `id`.
    pub(crate) fn port(&self, id: ServerId) -> Option<Port> {
        self.servers
            .ports
            .iter()
            .find(|port| port.id() == id)
            .cloned()
    }

    /// Whether `document` changed since its server was last handed it.
    pub(crate) fn is_unsynced(&self, document: DocumentId) -> bool {
        let version = self.editor.document_version(document);
        self.files
            .get(&document)
            .and_then(|file| file.synced)
            .is_some_and(|synced| Some(synced.at) != version)
    }

    /// Hands `document`, written in `language`, to the server serving it
    /// as it now reads: opened where the server has not seen it, its text
    /// sent where it changed since.
    pub(crate) fn sync(&mut self, document: DocumentId, language: &str) {
        let Some(port) = self.serving(document, language).cloned() else {
            return;
        };
        self.complete_with(document, &port);
        let Some(file) = self.files.get(&document) else {
            return;
        };
        if let Some(synced) = self.hand_over(document, &port, file.synced, language) {
            if let Some(file) = self.files.get_mut(&document) {
                file.synced = Some(synced);
            }
        }
    }

    /// Hands `document`, written in `language`, to each of `servers` that
    /// is running and ready, as [`Self::sync`] hands it to its own: what an
    /// extension's servers read before its inline source is asked. The
    /// version the first of them knows the text by.
    pub(crate) fn sync_extension(
        &mut self,
        document: DocumentId,
        servers: &[ServerId],
        language: &str,
    ) -> Option<i32> {
        let ports: Vec<Port> = servers
            .iter()
            .filter_map(|id| self.port(*id))
            .filter(Port::is_ready)
            .collect();
        for port in &ports {
            let before = self
                .files
                .get(&document)?
                .extension_synced
                .get(&port.id())
                .copied();
            if let Some(synced) = self.hand_over(document, port, before, language) {
                if let Some(file) = self.files.get_mut(&document) {
                    file.extension_synced.insert(port.id(), synced);
                }
            }
        }
        let file = self.files.get(&document)?;
        let first = ports.first()?;
        file.extension_synced
            .get(&first.id())
            .map(|synced| synced.version)
    }

    /// Hands `document`, written in `language`, to `port`, which last had
    /// it as `before`: opened where it has not seen it, its text sent where
    /// it changed since. What `port` has of it now, where anything was sent.
    fn hand_over(
        &self,
        document: DocumentId,
        port: &Port,
        before: Option<Synced>,
        language: &str,
    ) -> Option<Synced> {
        let at = self.editor.document_version(document)?;
        let file = self.files.get(&document)?;
        let text = self.lines(document).join("\n") + "\n";
        match before {
            Some(synced) if synced.server == port.id() && synced.at == at => None,
            Some(synced) if synced.server == port.id() => {
                let version = synced.version + 1;
                let params = protocol::change_params(&file.path, version, &text);
                port.notify("textDocument/didChange", params)
                    .then_some(Synced {
                        at,
                        server: port.id(),
                        version,
                    })
            }
            _ => {
                let id = servers::language_id(language);
                let params = protocol::open_params(&file.path, id, 1, &text);
                port.notify("textDocument/didOpen", params)
                    .then_some(Synced {
                        at,
                        server: port.id(),
                        version: 1,
                    })
            }
        }
    }

    /// Tells `document`'s server it was written.
    pub(crate) fn tell_saved(&self, document: DocumentId) {
        self.tell(document, "textDocument/didSave");
    }

    /// Tells `document`'s server it is closed, its last tab gone, and the
    /// extensions' servers handed it too.
    pub(crate) fn tell_closed(&self, document: DocumentId) {
        self.tell(document, "textDocument/didClose");
        let Some(file) = self.files.get(&document) else {
            return;
        };
        for server in file.extension_synced.keys() {
            if let Some(port) = self.port(*server) {
                port.notify(
                    "textDocument/didClose",
                    protocol::document_params(&file.path),
                );
            }
        }
    }

    /// Lays what every checker found in `document`'s file on its text:
    /// each problem undercurled in its severity's color, its line marked in
    /// the sign column. Whether any changed.
    pub(crate) fn refresh_problems(&mut self, document: DocumentId) -> bool {
        let Some(file) = self.files.get_mut(&document) else {
            return false;
        };
        let found: Vec<Diagnostic> = diagnostics::in_file(&file.path)
            .into_iter()
            .map(|found| found.diagnostic)
            .collect();
        if found == file.problems {
            return false;
        }
        file.problems = found;
        let problems = file.problems.clone();
        let Some(text) = self.editor.document(document) else {
            return false;
        };
        let offset = |at: protocol::Position| {
            let position = Utf16Position {
                column: at.character as usize,
                line: at.line as usize,
            };
            text.utf16_position_to_char(position)
                .unwrap_or_else(|| text.len_chars())
        };
        let range = |range: Range| offset(range.start)..offset(range.end);
        let decorations = problems
            .iter()
            .map(|problem| Decoration::new(range(problem.range), kind_of(problem.severity)))
            .collect();
        let signs = problems
            .iter()
            .map(|problem| {
                let kind = kind_of(problem.severity);
                Sign::new(problem.range.start.line as usize, mark_of(kind), kind)
            })
            .collect();
        self.editor.set_decorations(document, PROBLEMS, decorations);
        self.editor.set_signs(document, PROBLEMS, signs);
        true
    }

    /// `E2 W1`: how many errors and warnings `document`'s file has, while a
    /// server serves it; `None` for a clean file.
    pub(crate) fn problem_counts(&self, document: DocumentId) -> Option<String> {
        let file = self.files.get(&document)?;
        file.synced?;
        let count = |severity| {
            file.problems
                .iter()
                .filter(|problem| problem.severity == severity)
                .count()
        };
        let (errors, warnings) = (count(Severity::Error), count(Severity::Warning));
        (errors + warnings > 0).then(|| format!("E{errors} W{warnings}"))
    }

    /// Sends `method` about `document` to the server it was handed to.
    fn tell(&self, document: DocumentId, method: &str) {
        let Some(file) = self.files.get(&document) else {
            return;
        };
        let Some(synced) = file.synced else {
            return;
        };
        let port = self
            .servers
            .ports
            .iter()
            .find(|port| port.id() == synced.server);
        if let Some(port) = port {
            port.notify(method, protocol::document_params(&file.path));
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The kind of decoration a problem of `severity` is.
fn kind_of(severity: Severity) -> DecorationKind {
    match severity {
        Severity::Error => DecorationKind::Error,
        Severity::Warning => DecorationKind::Warning,
        Severity::Information => DecorationKind::Info,
        Severity::Hint => DecorationKind::Hint,
    }
}

/// What the sign column shows by a line with a problem of `kind`.
fn mark_of(kind: DecorationKind) -> &'static str {
    match kind {
        DecorationKind::Error => "E",
        DecorationKind::Warning => "W",
        DecorationKind::Info => "I",
        _ => "H",
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::mpsc::Receiver;

    use serde_json::{json, Value};
    use winter_lsp::diagnostics::Backend;

    use super::*;
    use crate::session;

    /// A Rust file of its own holding `text`, under a folder named for `tag`.
    fn file(tag: &str, text: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("winter-servers-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a folder");
        let path = dir.join("main.rs");
        std::fs::write(&path, text).expect("written");
        path
    }

    /// The methods sent to a fake server so far.
    fn methods(sent: &Receiver<Value>) -> Vec<String> {
        sent.try_iter()
            .filter_map(|message| message["method"].as_str().map(str::to_owned))
            .collect()
    }

    #[test]
    fn test_a_document_is_opened_changed_and_closed_with_its_server() {
        let path = file("sync", "fn main() {}\n");
        let (port, sent) = Port::fake("rust", path.parent().expect("a folder"), json!({}));
        session::with(|session| {
            let (document, view) = session.open_view(&path).expect("opens");
            session.connect(1, &[port]);
            session.sync(document, "rust");
            session.sync(document, "rust");
            assert_eq!(methods(&sent), ["textDocument/didOpen"], "once, unchanged");
            session
                .editor
                .apply_edits(view, vec![viento::Edit::new(0..0, "// ")]);
            assert!(session.is_unsynced(document));
            session.sync(document, "rust");
            assert_eq!(methods(&sent), ["textDocument/didChange"]);
            session.close_view(document, view);
            assert_eq!(methods(&sent), ["textDocument/didClose"]);
        });
    }

    #[test]
    fn test_what_a_checker_publishes_is_undercurled_and_signed() {
        let path = file("problems", "let x = 1;\nlet y = 2;\n");
        let found = Diagnostic {
            code: None,
            message: "unused".to_owned(),
            range: Range {
                end: protocol::Position {
                    character: 5,
                    line: 1,
                },
                start: protocol::Position {
                    character: 4,
                    line: 1,
                },
            },
            severity: Severity::Warning,
            source: None,
        };
        diagnostics::publish(Backend::Extension("probe"), &path, vec![found]);
        session::with(|session| {
            let (document, _) = session.open_view(&path).expect("opens");
            assert!(session.refresh_problems(document));
            assert!(!session.refresh_problems(document), "nothing new");
            let laid = session.editor.decorations(document, PROBLEMS);
            assert_eq!(
                laid,
                [Decoration::new(15..16, DecorationKind::Warning)],
                "the y"
            );
            let signs = session.editor.signs(document, PROBLEMS);
            assert_eq!(signs, [Sign::new(1, "W", DecorationKind::Warning)]);
        });
        diagnostics::forget(Backend::Extension("probe"));
    }
}
