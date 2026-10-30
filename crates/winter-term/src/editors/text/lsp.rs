//! The text editor's side of a language server: handing the server the
//! documents it serves as they change, asking it what the keys ask for,
//! and acting on what it answers. Going to a definition and listing the
//! references, saying what a name is, renaming it, the fixes on offer,
//! formatting, with the jumps those make walked back and forward again.
//! What the server finds wrong goes to [`winter_lsp::diagnostics`], which
//! [`super::problems`] steps through.
//!
//! A server is never started from here: the window starts one when asked
//! and hands every editor the servers running (see
//! [`TextEditor::connect_lsp`](crate::editors::text::TextEditor::connect_lsp)). Without one the keys say
//! how to start one.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use winter_lsp::diagnostics::{self, Backend};
use winter_lsp::protocol::{self, CodeAction, Diagnostic, FileEdits, Position, Range, Severity, TextEdit};
use winter_lsp::{servers, Event, Incoming, Port, ServerId};
use crate::model::editor::{
    EditorOutcome, EditorRequest, InfoCard, Notice, PickPlacement, PickRequest, PromptMode, PromptReply, PromptRequest, OpenTarget, LSP_CODE_ACTION,
    LSP_DECLARATION, LSP_DEFINITION, LSP_FORMAT, LSP_HOVER, LSP_IMPLEMENTATION, LSP_REFERENCES, LSP_RENAME, LSP_SIGNATURE, LSP_TYPE_DEFINITION, NAVIGATE_BACK, NAVIGATE_FORWARD,
};
use crate::model::paint::{DiagnosticLevel, Point};

use super::complete::{self, Candidate, Kind, Reply, ServerKind};
use super::completing::language_of;
use super::{Document, TextEditor};

// ========================================================================
// Constants
// ========================================================================

/// How long after an edit the server is handed the new text, so a burst of
/// typing goes over as one change. Asking the server anything hands it
/// first.
const FLUSH_DELAY: Duration = Duration::from_millis(150);

/// How long the pointer rests on a name before the server is asked what it
/// is.
const HOVER_DELAY: Duration = Duration::from_millis(500);

/// The most jumps remembered to go back over.
const MAX_JUMPS: usize = 100;

/// Prompt tags: the new name, and a place picked from a list.
pub(super) const ASK_RENAME: &str = "lsp-rename";
pub(super) const ASK_PLACE: &str = "lsp-place";

/// What to say where no server is there to ask.
pub(super) const NO_SERVER: &str = "no language server running: start one with LSP: Start (palette)";
const NOT_SERVED: &str = "no running language server serves this file";

/// Said when a server's edits arrive for text that has changed since.
const STALE_EDITS: &str = "the file changed while the server worked: ask again";

// ========================================================================
// Data Structures
// ========================================================================

/// Where jumps left from, the latest last, and where going back came from,
/// to go forward to again: the window's, so going back crosses from the file
/// a jump went to into the one it left, whichever tabs show them.
#[derive(Debug, Default)]
struct Jumps {
    back: Vec<Place>,
    forward: Vec<Place>,
}

thread_local! {
    static JUMPS: RefCell<Jumps> = RefCell::new(Jumps::default());
}

/// Act on the window's jump history.
fn with_jumps<R>(act: impl FnOnce(&mut Jumps) -> R) -> R {
    JUMPS.with(|jumps| act(&mut jumps.borrow_mut()))
}

/// The servers an editor talks to and what it is waiting on them for.
#[derive(Debug, Default)]
pub(super) struct Link {
    /// The buffer's revision the fixes on offer were asked about: a fix's
    /// edits are positions in that text, and land wrong in any other.
    actions_revision: Option<usize>,
    /// What each request in flight was for, by server and request id.
    asked: HashMap<(ServerId, u64), Asked>,
    /// The card open over the text.
    card: Option<Card>,
    /// The servers' generation [`Self::ports`] was taken at.
    generation: u64,
    /// Where the pointer rests, waiting to be asked about.
    hover: Option<HoverWait>,
    /// The places last listed, in the list's order, with their labels.
    places: Vec<(String, Place)>,
    /// The word the pointer is on, as its line and first column.
    pointer: Option<(usize, usize)>,
    ports: Vec<Port>,
    /// What the server last offered to complete, for the `lsp` back-end.
    pub(super) served: Option<Served>,
}

/// A document's text as the server it is open with last had it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Synced {
    /// The buffer's revision handed over, or `None` to hand it over again
    /// whatever the revision says: text read in afresh starts counting
    /// again.
    pub(super) revision: Option<usize>,
    pub(super) server: ServerId,
    pub(super) version: i32,
}

/// What a request in flight was for.
#[derive(Clone, Debug, PartialEq)]
enum Asked {
    /// The fixes on offer.
    Actions,
    /// A fix's command, run by the server, which sends its edits back.
    Command,
    Completion(CompletionAsk),
    /// The edits that format the file, and the buffer's revision they were
    /// asked about.
    Format(PathBuf, usize),
    /// Where a name is defined (or declared, typed, implemented), as said.
    Goto(&'static str),
    Hover(HoverAsk),
    References,
    /// The edits that rename a name, and the buffer's revision they were
    /// asked about.
    Rename(usize),
    Signature,
}

/// A completion request: whether it was asked for (`C-SPC`) rather than
/// made while typing, and the row it is about.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CompletionAsk {
    pub(super) asked: bool,
    pub(super) row: usize,
}

/// A hover request: the pane cell the pointer asked from (`None` from the
/// keys), the word it is about, and the character it is on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HoverAsk {
    cell: Option<(usize, usize)>,
    place: (usize, usize),
    word: (usize, usize),
}

/// A card open over the text: from the pointer's cell, or the cursor's.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Card {
    /// What the card is about, as the line and column the pointer rested
    /// on, for a card the pointer opened; the problems there are emphasized
    /// while it is open. `None` for a card asked for at the cursor.
    about: Option<(usize, usize)>,
    cell: Option<(usize, usize)>,
    /// What kind of problems the card is about, and what found them, said
    /// under the rest. `None` for a card about no problem.
    footer: Option<String>,
    text: String,
}

/// Problems as a card says them: their messages, and what kind each is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Described {
    /// Each problem's kind and what found it, one per line.
    pub(super) footer: String,
    /// Each problem's message, a blank line between two.
    pub(super) text: String,
}

/// The pointer resting on a word, until it is time to ask about it.
#[derive(Clone, Copy, Debug)]
struct HoverWait {
    ask: HoverAsk,
    due: Instant,
}

/// A place to jump to: a file, and a position in it as the wire counts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Place {
    pub(super) at: Position,
    pub(super) path: PathBuf,
}

/// What a server offered to complete on one row: the `lsp` back-end's
/// reply, the edits elsewhere each entry brings (an import), and whether
/// typing on asks again.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Served {
    edits: HashMap<String, Vec<TextEdit>>,
    pub(super) incomplete: bool,
    pub(super) reply: Reply,
    pub(super) row: usize,
}

// ========================================================================
// TextEditor: servers and documents
// ========================================================================

impl TextEditor {
    /// Take the servers running, handing each the documents it serves.
    pub(super) fn connect(&mut self, generation: u64, ports: &[Port]) {
        if self.link.generation == generation {
            return;
        }
        self.link.generation = generation;
        self.link.ports = ports.to_vec();
        let live: Vec<ServerId> = ports.iter().map(Port::id).collect();
        for doc in &mut self.docs {
            if doc.synced().is_some_and(|synced| !live.contains(&synced.server)) {
                doc.set_synced(None);
                doc.diagnostics.clear();
            }
            if doc.pilot_synced().is_some_and(|synced| !live.contains(&synced.server)) {
                doc.set_pilot_synced(None);
            }
        }
        self.sync_documents();
        self.refresh_diagnostics(None);
    }

    /// Hand every server the documents it serves as they now read: open
    /// the ones it has not seen, and send the text of those changed since.
    pub(super) fn sync_documents(&mut self) {
        for doc in &mut self.docs {
            // A server is never handed a reading of the text older than
            // what another tab on it has already given it.
            doc.buffer.catch_up();
            let synced = serving(&self.link.ports, doc).and_then(|port| hand_over(port, doc, doc.synced()));
            doc.set_synced(synced);
        }
    }

    /// Tell the server a document was written.
    pub(super) fn tell_saved(&mut self) {
        self.sync_documents();
        if let Some(port) = serving(&self.link.ports, self.doc()) {
            port.notify("textDocument/didSave", protocol::document_params(&self.doc().path));
        }
    }

    /// Tell the servers every document is closed, as the editor goes.
    pub(super) fn tell_closed(&mut self) {
        for at in 0..self.docs.len() {
            self.tell_doc_closed(at);
        }
    }

    /// Tell the servers document `at` is closed, under the path they opened
    /// it as. A file another tab still has open stays open with them.
    pub(super) fn tell_doc_closed(&mut self, at: usize) {
        let doc = &mut self.docs[at];
        for synced in doc.detach() {
            if let Some(port) = self.link.ports.iter().find(|port| port.id() == synced.server) {
                port.notify("textDocument/didClose", protocol::document_params(&doc.path));
            }
        }
    }

    /// Have the next hand-over send the text whatever its revision says,
    /// for a buffer whose text was read in afresh.
    pub(super) fn resend_text(&mut self) {
        let doc = self.doc_mut();
        let resent = |synced: Option<Synced>| synced.map(|synced| Synced { revision: None, ..synced });
        let (synced, pilot) = (resent(doc.synced()), resent(doc.pilot_synced()));
        doc.set_synced(synced);
        doc.set_pilot_synced(pilot);
    }

    /// Whether the document in front is served by a running server.
    pub(super) fn is_served(&self) -> bool {
        serving(&self.link.ports, self.doc()).is_some()
    }

    /// Whether a language server is running, Copilot aside.
    pub(super) fn has_language_server(&self) -> bool {
        self.link.ports.iter().any(|port| !port.is_everywhere())
    }

    /// The Copilot server, among the servers running, once it is ready.
    pub(super) fn copilot_port(&self) -> Option<&Port> {
        self.link.ports.iter().find(|port| port.is_everywhere() && port.is_ready())
    }

    /// The server serving the document in front, if any.
    pub(super) fn serving_port(&self) -> Option<&Port> {
        serving(&self.link.ports, self.doc())
    }

    /// Take what every back-end says is wrong in the documents, every one
    /// of them or the one at `path`. Says whether any changed.
    pub(super) fn refresh_diagnostics(&mut self, path: Option<&Path>) -> bool {
        let mut changed = false;
        for doc in &mut self.docs {
            if path.is_some_and(|path| path != doc.path) {
                continue;
            }
            let now: Vec<Diagnostic> = diagnostics::in_file(&doc.path)
                .into_iter()
                .map(|found| found.diagnostic)
                .collect();
            if now != doc.diagnostics {
                doc.diagnostics = now;
                changed = true;
            }
        }
        changed
    }

    /// When the servers next need something of this editor: the text of a
    /// document edited since it was handed over, or the pointer's word.
    pub(super) fn lsp_next_tick(&self) -> Option<Instant> {
        let flush = self.has_unsynced_edits().then(|| Instant::now() + FLUSH_DELAY);
        let hover = self.link.hover.map(|wait| wait.due);
        [flush, hover].into_iter().flatten().min()
    }

    /// Whether a document was edited since a server was last handed its text.
    pub(super) fn has_unsynced_edits(&self) -> bool {
        self.docs.iter().any(|doc| {
            doc.synced()
                .is_some_and(|synced| synced.revision != Some(doc.buffer.revision()))
        })
    }

    /// Hand edited text over, and ask about the word the pointer has rested
    /// on long enough.
    pub(super) fn lsp_tick(&mut self, now: Instant) -> bool {
        self.sync_documents();
        let Some(wait) = self.link.hover.filter(|wait| wait.due <= now) else {
            return false;
        };
        self.link.hover = None;
        let (line, col) = wait.ask.place;
        let found = diagnostics_at(&self.doc().diagnostics, self.buffer().lines(), line, col);
        let asked = self.serving_port().filter(|port| port.provides("hoverProvider")).cloned();
        if let Some(port) = asked {
            let at = protocol::position_in(self.buffer().lines(), line, col);
            let params = protocol::at_params(&self.doc().path, at);
            if let Some(id) = port.request("textDocument/hover", params) {
                self.link.asked.insert((port.id(), id), Asked::Hover(wait.ask));
                return false;
            }
        }
        if found.is_empty() {
            return false;
        }
        let described = describe(&found);
        self.link.card = Some(Card {
            about: wait.ask.cell.map(|_| wait.ask.place),
            cell: wait.ask.cell,
            footer: Some(described.footer),
            text: described.text,
        });
        true
    }
}

// ========================================================================
// TextEditor: what the servers said
// ========================================================================

impl TextEditor {
    /// Act on what a server said: an answer to one of this editor's
    /// requests, new problems in one of its documents, or edits to make.
    pub(super) fn on_lsp_message(&mut self, incoming: &Incoming) -> EditorOutcome {
        let ours = self.link.ports.iter().any(|port| port.id() == incoming.server);
        match &incoming.event {
            Event::Answer { id, result } => {
                let Some(asked) = self.link.asked.remove(&(incoming.server, *id)) else {
                    return EditorOutcome::Ignored;
                };
                match result {
                    Ok(value) => self.answered(incoming.server, asked, value),
                    Err(error) => {
                        // A completion or the pointer's hover failing is
                        // nothing the reader asked for.
                        let quiet = matches!(asked, Asked::Completion(_))
                            || matches!(asked, Asked::Hover(ask) if ask.cell.is_some());
                        if !quiet {
                            self.modal.report(Notice::error(error.clone()));
                        }
                        match asked {
                            Asked::Actions => self.server_fixes_failed(),
                            _ => EditorOutcome::Consumed,
                        }
                    }
                }
            }
            Event::Diagnostics(path) if ours => match self.refresh_diagnostics(Some(path)) {
                true => EditorOutcome::Consumed,
                false => EditorOutcome::Ignored,
            },
            Event::ApplyEdit(edits) if ours => {
                self.apply_file_edits(edits);
                EditorOutcome::Consumed
            }
            _ => EditorOutcome::Ignored,
        }
    }

    /// Act on the answer to `asked`.
    fn answered(&mut self, server: ServerId, asked: Asked, value: &Value) -> EditorOutcome {
        match asked {
            Asked::Actions => self.server_fixes(server, protocol::code_actions(value)),
            Asked::Command => EditorOutcome::Consumed,
            Asked::Completion(ask) => {
                self.take_served(ask, value);
                EditorOutcome::Consumed
            }
            Asked::Format(_, revision) | Asked::Rename(revision) if revision != self.buffer().revision() => {
                // The server worked from text that has changed since: its
                // edits are positions in that text, and would land wrong.
                self.modal.report(Notice::info(STALE_EDITS));
                EditorOutcome::Consumed
            }
            Asked::Format(path, _) => {
                let edits = protocol::text_edits(value);
                let said = match edits.is_empty() {
                    true => "already formatted".to_string(),
                    false => {
                        self.apply_file_edits(&vec![(path, edits)]);
                        "formatted".to_string()
                    }
                };
                self.modal.report(Notice::info(said).unlogged());
                EditorOutcome::Consumed
            }
            Asked::Goto(what) => {
                let found = protocol::locations(value);
                match found.as_slice() {
                    [] => self.modal.report(Notice::info(format!("no {what} found"))),
                    [only] => {
                        let place = Place {
                            at: only.range.start,
                            path: only.path.clone(),
                        };
                        self.go_to(&place, true);
                    }
                    _ => return self.list_places(&format!("{what}s"), found.iter().map(location_place).collect()),
                }
                EditorOutcome::Consumed
            }
            Asked::Hover(ask) => {
                // The pointer has moved on: what it rested on no longer matters.
                if ask.cell.is_some() && self.link.pointer != Some(ask.word) {
                    return EditorOutcome::Consumed;
                }
                let (line, col) = ask.place;
                let found = diagnostics_at(&self.doc().diagnostics, self.buffer().lines(), line, col);
                let described = (!found.is_empty()).then(|| describe(&found));
                let parts: Vec<String> = [
                    described.as_ref().map(|described| described.text.clone()),
                    protocol::hover_text(value),
                ]
                .into_iter()
                .flatten()
                .collect();
                match parts.is_empty() {
                    true if ask.cell.is_none() => {
                        self.modal.report(Notice::info("nothing to say about this").unlogged());
                    }
                    true => {}
                    false => {
                        self.link.card = Some(Card {
                            about: ask.cell.map(|_| ask.place),
                            cell: ask.cell,
                            footer: described.map(|described| described.footer),
                            text: parts.join("\n\n"),
                        });
                    }
                }
                EditorOutcome::Consumed
            }
            Asked::References => {
                let found = protocol::locations(value);
                match found.is_empty() {
                    true => {
                        self.modal.report(Notice::info("no references found"));
                        EditorOutcome::Consumed
                    }
                    false => self.list_places("references", found.iter().map(location_place).collect()),
                }
            }
            Asked::Rename(_) => {
                let edits = protocol::workspace_edits(value);
                let files = edits.len();
                self.apply_file_edits(&edits);
                let said = match files {
                    0 => "nothing to rename".to_string(),
                    1 => "renamed in 1 file".to_string(),
                    n => format!("renamed in {n} files, not yet saved"),
                };
                self.modal.report(Notice::info(said));
                EditorOutcome::Consumed
            }
            Asked::Signature => {
                match protocol::signature_text(value) {
                    Some(text) => self.show_card(text),
                    None => self.modal.report(Notice::info("no signature here").unlogged()),
                }
                EditorOutcome::Consumed
            }
        }
    }
}

// ========================================================================
// TextEditor: the commands
// ========================================================================

impl TextEditor {
    /// Run language server command `name`.
    pub(super) fn run_lsp_command(&mut self, name: &str) -> EditorOutcome {
        match name {
            LSP_DEFINITION => self.goto("textDocument/definition", "definitionProvider", "definition"),
            LSP_DECLARATION => self.goto("textDocument/declaration", "declarationProvider", "declaration"),
            LSP_TYPE_DEFINITION => {
                self.goto("textDocument/typeDefinition", "typeDefinitionProvider", "type definition")
            }
            LSP_IMPLEMENTATION => {
                self.goto("textDocument/implementation", "implementationProvider", "implementation")
            }
            LSP_REFERENCES => {
                let mut params = self.here_params();
                params["context"] = json!({"includeDeclaration": true});
                self.request_here("textDocument/references", "referencesProvider", params, Asked::References)
            }
            LSP_HOVER => self.hover_here(),
            LSP_SIGNATURE => {
                let params = self.here_params();
                self.request_here("textDocument/signatureHelp", "signatureHelpProvider", params, Asked::Signature)
            }
            LSP_RENAME => self.ask_rename(),
            LSP_CODE_ACTION => self.ask_fixes(),
            LSP_FORMAT => self.format(),
            NAVIGATE_BACK => self.navigate(true),
            NAVIGATE_FORWARD => self.navigate(false),
            _ => EditorOutcome::Ignored,
        }
    }

    /// The document and the cursor's place in it, as a request about the
    /// cursor is sent with.
    fn here_params(&self) -> Value {
        let at = protocol::position_in(self.buffer().lines(), self.buffer().row(), self.buffer().col());
        protocol::at_params(&self.doc().path, at)
    }

    /// Where a name is defined, declared, typed, or implemented.
    fn goto(&mut self, method: &str, capability: &str, what: &'static str) -> EditorOutcome {
        let params = self.here_params();
        self.request_here(method, capability, params, Asked::Goto(what))
    }

    /// Ask the server serving the document in front `method`, which it
    /// answers where it says it provides `capability`, saying why not where
    /// it cannot be asked.
    fn request_here(&mut self, method: &str, capability: &str, params: Value, asked: Asked) -> EditorOutcome {
        self.sync_documents();
        let Some(port) = self.serving_port().cloned() else {
            let said = match !self.has_language_server() {
                true => NO_SERVER,
                false => NOT_SERVED,
            };
            self.modal.report(Notice::info(said));
            return EditorOutcome::Consumed;
        };
        if !port.provides(capability) {
            let what = capability.trim_end_matches("Provider");
            self.modal.report(Notice::info(format!("{} does not answer {what}", port.name())));
            return EditorOutcome::Consumed;
        }
        match port.request(method, params) {
            Some(id) => {
                self.link.asked.insert((port.id(), id), asked);
            }
            None => self.modal.report(Notice::info(format!("{} is still starting", port.name()))),
        }
        EditorOutcome::Consumed
    }

    /// What the name under the cursor is, and the problems found there;
    /// the problems alone where no server can say more.
    fn hover_here(&mut self) -> EditorOutcome {
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let can_ask = self
            .serving_port()
            .is_some_and(|port| port.provides("hoverProvider"));
        if !can_ask {
            let found = diagnostics_at(&self.doc().diagnostics, self.buffer().lines(), row, col);
            if !found.is_empty() {
                self.show_problems_card(describe(&found));
                return EditorOutcome::Consumed;
            }
        }
        let ask = HoverAsk {
            cell: None,
            place: (row, col),
            word: (row, col),
        };
        let params = self.here_params();
        self.request_here("textDocument/hover", "hoverProvider", params, Asked::Hover(ask))
    }

    /// Ask for the new name, starting from the word under the cursor.
    fn ask_rename(&mut self) -> EditorOutcome {
        if let Some(refused) = self.refuse_edits() {
            return refused;
        }
        if !self.serving_port().is_some_and(|port| port.provides("renameProvider")) {
            let revision = self.buffer().revision();
            return self.request_here("textDocument/rename", "renameProvider", Value::Null, Asked::Rename(revision));
        }
        let line: Vec<char> = self.line_of(self.buffer().row()).chars().collect();
        let col = self.buffer().col().min(line.len());
        let start = (0..col).rev().take_while(|&at| complete::is_word_char(line[at])).last().unwrap_or(col);
        let end = (col..line.len()).take_while(|&at| complete::is_word_char(line[at])).last().map_or(col, |at| at + 1);
        EditorOutcome::Prompt(PromptRequest {
            initial: line[start..end].iter().collect(),
            label: "Rename to: ".to_string(),
            mode: PromptMode::Text,
            tag: ASK_RENAME,
        })
    }

    /// Ask the server serving the file for the fixes and refactorings at
    /// the cursor, or over the selection, the quick fixes for the problems
    /// found there first. Whether it was asked: a file no server serves, or
    /// served by one that offers no fixes, is not.
    pub(super) fn ask_actions(&mut self) -> bool {
        let Some(port) = self
            .serving_port()
            .filter(|port| port.provides("codeActionProvider"))
            .cloned()
        else {
            return false;
        };
        let lines = self.buffer().lines();
        let (start, end) = match self.selection() {
            Some(selection) => (selection.start, (selection.end.0, selection.end.1 + 1)),
            None => {
                let here = (self.buffer().row(), self.buffer().col());
                (here, here)
            }
        };
        let range = Range {
            end: protocol::position_in(lines, end.0, end.1),
            start: protocol::position_in(lines, start.0, start.1),
        };
        let found: Vec<Diagnostic> = diagnostics::of(Backend::Lsp(port.id()), &self.doc().path)
            .into_iter()
            .filter(|diagnostic| {
                diagnostic.range.start.line as usize <= end.0 && diagnostic.range.end.line as usize >= start.0
            })
            .collect();
        let params = protocol::code_action_params(&self.doc().path, range, &found);
        self.sync_documents();
        let Some(id) = port.request("textDocument/codeAction", params) else {
            return false;
        };
        self.link.asked.insert((port.id(), id), Asked::Actions);
        self.link.actions_revision = Some(self.buffer().revision());
        true
    }

    /// Format the file, with its own indentation: tabs where it indents
    /// with them.
    fn format(&mut self) -> EditorOutcome {
        if let Some(refused) = self.refuse_edits() {
            return refused;
        }
        let tabs = self.buffer().lines().iter().any(|line| line.starts_with('\t'));
        let path = self.doc().path.clone();
        let params = protocol::format_params(&path, crate::model::paint::TAB_WIDTH, !tabs);
        let revision = self.buffer().revision();
        self.request_here("textDocument/formatting", "documentFormattingProvider", params, Asked::Format(path, revision))
    }

    /// A refusal to change a buffer that takes no edits.
    pub(super) fn refuse_edits(&mut self) -> Option<EditorOutcome> {
        if !self.modal.is_read_only() {
            return None;
        }
        self.modal.report(Notice::info("this buffer is read-only"));
        Some(EditorOutcome::Consumed)
    }

    /// The answer to one of this module's prompts, or `None` for another's.
    pub(super) fn on_lsp_prompt(&mut self, reply: &PromptReply) -> Option<EditorOutcome> {
        let answer = reply.answer.as_deref();
        match reply.tag {
            ASK_RENAME => Some(match answer.map(str::trim).filter(|name| !name.is_empty()) {
                Some(name) => {
                    let mut params = self.here_params();
                    params["newName"] = json!(name);
                    let revision = self.buffer().revision();
                    self.request_here("textDocument/rename", "renameProvider", params, Asked::Rename(revision))
                }
                None => EditorOutcome::Consumed,
            }),
            ASK_PLACE => {
                let place = answer.and_then(|answer| {
                    self.link.places.iter().find(|(label, _)| label == answer).map(|(_, place)| place.clone())
                });
                if let Some(place) = place {
                    self.go_to(&place, true);
                }
                Some(EditorOutcome::Consumed)
            }
            _ => None,
        }
    }

    /// Make a fix's edits, and have the server run its command.
    pub(super) fn take_action(&mut self, server: ServerId, action: CodeAction) {
        if let Some(edits) = &action.edits {
            if self.link.actions_revision != Some(self.buffer().revision()) {
                self.modal.report(Notice::info(STALE_EDITS));
                return;
            }
            self.apply_file_edits(edits);
        }
        let Some(command) = action.command else {
            return;
        };
        let Some(port) = self.link.ports.iter().find(|port| port.id() == server).cloned() else {
            return;
        };
        let params = json!({
            "command": command.get("command").cloned().unwrap_or(Value::Null),
            "arguments": command.get("arguments").cloned().unwrap_or(json!([])),
        });
        if let Some(id) = port.request("workspace/executeCommand", params) {
            self.link.asked.insert((server, id), Asked::Command);
        }
    }
}

// ========================================================================
// TextEditor: edits from a server
// ========================================================================

impl TextEditor {
    /// Make `edits`, file by file: this editor's own here, and another
    /// file's in the buffer holding it, which the window finds or opens and
    /// leaves unsaved, as the edits were made to it rather than to the disk.
    pub(super) fn apply_file_edits(&mut self, edits: &FileEdits) {
        for (path, file_edits) in edits {
            if file_edits.is_empty() {
                continue;
            }
            // Matched where the files really are: a server names the file
            // a link opened, and its edits were sent off as another file's.
            let key = super::buffers::key_of(path);
            match self.docs.iter().position(|doc| super::buffers::key_of(&doc.path) == key) {
                Some(at) => self.apply_to_buffer(at, file_edits),
                None => self.requests.push(EditorRequest::Edit {
                    edits: file_edits.clone(),
                    path: path.clone(),
                }),
            }
        }
        self.sync_documents();
    }

    /// Make `edits` to buffer `at` as one undo step: the lines between the
    /// first and the last that changed are put in at once, and the cursor
    /// keeps its place, moved down by what was put in above it.
    pub(super) fn apply_to_buffer(&mut self, at: usize, edits: &[TextEdit]) {
        let buffer = &mut self.docs[at].buffer;
        let (row, col) = (buffer.row(), buffer.col());
        let before = buffer.lines().to_vec();
        let after = protocol::apply_edits(&before, edits);
        let Some((first, removed, inserted)) = changed_span(&before, &after) else {
            return;
        };
        let shift = match first + removed <= row {
            true => inserted as isize - removed as isize,
            false => 0,
        };
        buffer.replace_rows(first, removed, after[first..first + inserted].to_vec());
        let row = (row as isize + shift).max(0) as usize;
        buffer.move_to(row.min(buffer.lines().len() - 1), col);
    }
}

// ========================================================================
// TextEditor: completion from a server
// ========================================================================

impl TextEditor {
    /// Ask the server serving the document for completions at the cursor,
    /// quietly: `asked` where the reader asked (`C-SPC`), `trigger` where a
    /// character the server named was typed. Says whether it was asked.
    pub(super) fn ask_server_completion(&mut self, asked: bool, trigger: Option<char>) -> bool {
        self.sync_documents();
        let Some(port) = self.serving_port().filter(|port| port.provides("completionProvider")).cloned() else {
            return false;
        };
        let mut params = self.here_params();
        params["context"] = match trigger {
            Some(c) => json!({"triggerKind": 2, "triggerCharacter": c.to_string()}),
            None => json!({"triggerKind": 1}),
        };
        let Some(id) = port.request("textDocument/completion", params) else {
            return false;
        };
        let row = self.buffer().row();
        // Only the latest request on a row is waited on.
        self.link.asked.retain(|_, waiting| !matches!(waiting, Asked::Completion(_)));
        self.link.asked.insert((port.id(), id), Asked::Completion(CompletionAsk { asked, row }));
        true
    }

    /// Whether `c`, just typed, is one the server offers completions after.
    pub(super) fn is_trigger(&self, c: char) -> bool {
        self.serving_port().is_some_and(|port| port.trigger_characters().contains(&c))
    }

    /// Take a server's completions, and open or narrow the popup with them.
    fn take_served(&mut self, ask: CompletionAsk, value: &Value) {
        let row = self.buffer().row();
        if ask.row != row || !self.modal.is_typing() {
            return;
        }
        let list = protocol::completions(value);
        let line = self.line_of(row);
        let col = self.buffer().col().min(line.chars().count());
        self.link.served = Some(served(list, &line, row, col));
        self.server_offered(ask);
    }

    /// The edits elsewhere a server's entry `label` brings, such as the
    /// import it needs.
    pub(super) fn served_edits(&self, label: &str) -> Vec<TextEdit> {
        self.link
            .served
            .as_ref()
            .and_then(|served| served.edits.get(label).cloned())
            .unwrap_or_default()
    }

    /// Make the edits an entry brought, in the buffer in front.
    pub(super) fn apply_served_edits(&mut self, edits: &[TextEdit]) {
        if !edits.is_empty() {
            let at = self.at.min(self.docs.len() - 1);
            self.apply_to_buffer(at, edits);
        }
    }
}

// ========================================================================
// TextEditor: places
// ========================================================================

impl TextEditor {
    /// Where the cursor is, as a place to come back to.
    pub(super) fn place_here(&self) -> Place {
        Place {
            at: protocol::position_in(self.buffer().lines(), self.buffer().row(), self.buffer().col()),
            path: self.doc().path.clone(),
        }
    }

    /// Go to `place`, and remember where this left from when `remember` is
    /// set. A place in another file is the window's to show, in that file's
    /// buffer.
    pub(super) fn go_to(&mut self, place: &Place, remember: bool) {
        let here = self.place_here();
        if remember && here != *place {
            with_jumps(|jumps| {
                jumps.back.push(here);
                if jumps.back.len() > MAX_JUMPS {
                    jumps.back.remove(0);
                }
                jumps.forward.clear();
            });
        }
        if place.path != self.doc().path {
            let (row, col) = protocol::place_of(&self.lines_of(&place.path), place.at);
            self.requests.push(EditorRequest::Open(OpenTarget {
                column: Some(col + 1),
                line: Some(row + 1),
                path: place.path.clone(),
            }));
            return;
        }
        let (row, col) = protocol::place_of(self.buffer().lines(), place.at);
        let row = row.min(self.buffer().lines().len() - 1);
        self.buffer_mut().move_to(row, col);
        if !self.shows_cursor() {
            self.put_cursor_rows_down(self.viewport.max(1) / 3);
        }
        // No flash: the line flashes only when an editor takes the
        // keyboard, which a jump within the file in front does not.
    }

    /// Back over the last jump, or forward again over one gone back over.
    fn navigate(&mut self, back: bool) -> EditorOutcome {
        let taken = match back {
            true => with_jumps(|jumps| jumps.back.pop()),
            false => with_jumps(|jumps| jumps.forward.pop()),
        };
        let Some(place) = taken else {
            let said = match back {
                true => "nowhere to go back to",
                false => "nowhere to go forward to",
            };
            self.modal.report(Notice::info(said).unlogged());
            return EditorOutcome::Consumed;
        };
        let here = self.place_here();
        match back {
            true => with_jumps(|jumps| jumps.forward.push(here)),
            false => with_jumps(|jumps| jumps.back.push(here)),
        }
        self.go_to(&place, false);
        EditorOutcome::Consumed
    }

    /// Show `places` as a list to pick one from, labelled with the file, the
    /// line, and the line's text; straight there where there is only one.
    fn list_places(&mut self, what: &str, places: Vec<Place>) -> EditorOutcome {
        if let [only] = places.as_slice() {
            let only = only.clone();
            self.go_to(&only, true);
            return EditorOutcome::Consumed;
        }
        let mut files: HashMap<PathBuf, Vec<String>> = HashMap::new();
        let labels: Vec<String> = places
            .iter()
            .map(|place| {
                let lines = files.entry(place.path.clone()).or_insert_with(|| self.lines_of(&place.path));
                let (row, col) = protocol::place_of(lines, place.at);
                let text = lines.get(row).map_or("", |line| line.trim());
                format!("{}:{}:{}  {text}", self.shown_path(&place.path), row + 1, col + 1)
            })
            .collect();
        self.show_places(what, labels, places)
    }

    /// The list of `labels` standing for `places`, to pick one from.
    pub(super) fn show_places(&mut self, what: &str, labels: Vec<String>, places: Vec<Place>) -> EditorOutcome {
        // Two places reading the same would be one answer; the second is
        // told apart by its order.
        let mut seen = std::collections::HashSet::new();
        let labels: Vec<String> = labels
            .into_iter()
            .enumerate()
            .map(|(at, label)| match seen.insert(label.clone()) {
                true => label,
                false => format!("{label} ({})", at + 1),
            })
            .collect();
        self.link.places = labels.iter().cloned().zip(places).collect();
        EditorOutcome::Pick(PickRequest {
            icons: Vec::new(),
            items: labels,
            label: format!("{} {what}", self.link.places.len()),
            placement: PickPlacement::Centered,
            tag: ASK_PLACE,
        })
    }

    /// A file's lines: the buffer's where it is open, else as on disk.
    fn lines_of(&self, path: &Path) -> Vec<String> {
        let key = super::buffers::key_of(path);
        match self.docs.iter().find(|doc| super::buffers::key_of(&doc.path) == key) {
            Some(doc) => doc.buffer.lines().to_vec(),
            None => std::fs::read_to_string(path)
                .map(|text| text.lines().map(str::to_string).collect())
                .unwrap_or_default(),
        }
    }

    /// `path` as a list shows it: under the server's project where it is
    /// in it, else from the home directory.
    pub(super) fn shown_path(&self, path: &Path) -> String {
        self.link
            .ports
            .iter()
            .find_map(|port| path.strip_prefix(port.root()).ok())
            .map(|relative| relative.display().to_string())
            .unwrap_or_else(|| crate::model::path::abbreviate_home(path))
    }
}

// ========================================================================
// TextEditor: the card and the pointer
// ========================================================================

impl TextEditor {
    /// The card open over the text, for the window to draw.
    pub(super) fn card(&self) -> Option<InfoCard> {
        self.link.card.as_ref().map(|card| InfoCard {
            cell: card.cell,
            footer: card.footer.clone(),
            text: card.text.clone(),
        })
    }

    /// Open a card at the cursor saying `text`.
    pub(super) fn show_card(&mut self, text: String) {
        self.link.card = Some(Card {
            about: None,
            cell: None,
            footer: None,
            text,
        });
    }

    /// Open a card at the cursor about the problems `described` says.
    pub(super) fn show_problems_card(&mut self, described: Described) {
        self.link.card = Some(Card {
            about: None,
            cell: None,
            footer: Some(described.footer),
            text: described.text,
        });
    }

    /// Close the card, saying whether one was open.
    pub(super) fn close_card(&mut self) -> bool {
        self.link.card.take().is_some()
    }

    /// The pointer rests on cell `at` of the pane, or has left it: wait to
    /// ask about the word under it, and close a card about another one.
    pub(super) fn hover_at(&mut self, at: Option<Point>) -> EditorOutcome {
        let found = at.and_then(|at| self.word_at_cell(at).map(|(place, word)| (at, place, word)));
        let Some((at, place, word)) = found else {
            self.link.hover = None;
            self.link.pointer = None;
            let from_pointer = self.link.card.as_ref().is_some_and(|card| card.cell.is_some());
            if from_pointer {
                self.link.card = None;
                return EditorOutcome::Consumed;
            }
            return EditorOutcome::Ignored;
        };
        if self.link.pointer == Some(word) {
            return EditorOutcome::Ignored;
        }
        self.link.pointer = Some(word);
        let closed = match self.link.card.as_ref().is_some_and(|card| card.cell.is_some()) {
            true => self.close_card(),
            false => false,
        };
        self.link.hover = Some(HoverWait {
            ask: HoverAsk {
                cell: Some((at.row, at.col)),
                place,
                word,
            },
            due: Instant::now() + HOVER_DELAY,
        });
        match closed {
            true => EditorOutcome::Consumed,
            false => EditorOutcome::Ignored,
        }
    }

    /// The character painted at pane cell `at` and where the word holding
    /// it starts, both as a line and a column; `None` off the text or off
    /// any word.
    fn word_at_cell(&self, at: Point) -> Option<((usize, usize), (usize, usize))> {
        let (line, col) = self.char_at_cell(at)?;
        let chars: Vec<char> = self.line_of(line).chars().collect();
        if !chars.get(col).is_some_and(|c| complete::is_word_char(*c)) {
            return None;
        }
        let start = (0..=col).rev().take_while(|&at| complete::is_word_char(chars[at])).last()?;
        Some(((line, col), (line, start)))
    }
}

// ========================================================================
// TextEditor: painting what the servers found
// ========================================================================

impl TextEditor {
    /// The wavy line under each character of line `index`: the worst of the
    /// problems covering it, hints left unmarked.
    pub(super) fn squiggles(&self, index: usize, length: usize) -> Vec<Option<DiagnosticLevel>> {
        let mut marks = vec![None; length];
        let lines = self.buffer().lines();
        let mut found: Vec<&Diagnostic> = self
            .doc()
            .diagnostics
            .iter()
            .filter(|d| (d.range.start.line as usize..=d.range.end.line as usize).contains(&index))
            .collect();
        // The worst marks last, so it is what is left showing.
        found.sort_by_key(|d| std::cmp::Reverse(d.severity));
        for diagnostic in found {
            let Some(level) = level_of(diagnostic.severity) else {
                continue;
            };
            let (start, end) = columns_on(diagnostic.range, lines, index, length);
            for mark in marks.iter_mut().take(end).skip(start) {
                *mark = Some(level);
            }
        }
        marks
    }

    /// The columns of line `index` (of `length` characters) covered by the
    /// problems the card the pointer opened is about, emphasized while it is
    /// open so the eye finds what the card describes. None without one.
    pub(super) fn hovered_problem_columns(&self, index: usize, length: usize) -> Vec<(usize, usize)> {
        let Some((line, col)) = self.link.card.as_ref().and_then(|card| card.about) else {
            return Vec::new();
        };
        let lines = self.buffer().lines();
        diagnostics_at(&self.doc().diagnostics, lines, line, col)
            .iter()
            .filter(|d| (d.range.start.line as usize..=d.range.end.line as usize).contains(&index))
            .map(|d| columns_on(d.range, lines, index, length))
            .collect()
    }

    /// How many errors and warnings the servers found in the document in
    /// front, for the status bar.
    pub(super) fn problem_counts(&self) -> Option<String> {
        self.doc().synced()?;
        let count = |severity| self.doc().diagnostics.iter().filter(|d| d.severity == severity).count();
        let (errors, warnings) = (count(Severity::Error), count(Severity::Warning));
        (errors + warnings > 0).then(|| format!("E{errors} W{warnings}"))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The ready server in `ports` serving `doc`: a file on this machine, read,
/// in a language the server serves, under its project.
fn serving<'a>(ports: &'a [Port], doc: &Document) -> Option<&'a Port> {
    if doc.unread || crate::remote::parse(&doc.path).is_some() {
        return None;
    }
    let language = language_of(&doc.path, doc.grammar);
    ports
        .iter()
        .find(|port| port.is_ready() && !port.is_everywhere() && port.serves(&language, &doc.path))
}

/// Hand `port` `doc` as it now reads, `synced` saying what it last had:
/// open it where it has not seen it, send its text where it has changed.
/// What the server has afterwards, `synced` again where nothing could go.
pub(super) fn hand_over(port: &Port, doc: &Document, synced: Option<Synced>) -> Option<Synced> {
    let revision = doc.buffer.revision();
    match synced {
        Some(synced) if synced.server == port.id() && synced.revision == Some(revision) => Some(synced),
        Some(synced) if synced.server == port.id() => {
            let version = synced.version + 1;
            let params = protocol::change_params(&doc.path, version, &text_of(doc));
            match port.notify("textDocument/didChange", params) {
                true => Some(Synced {
                    revision: Some(revision),
                    server: port.id(),
                    version,
                }),
                false => Some(synced),
            }
        }
        _ => {
            let language = language_of(&doc.path, doc.grammar);
            let id = servers::language_id(&language);
            let params = protocol::open_params(&doc.path, id, 1, &text_of(doc));
            port.notify("textDocument/didOpen", params).then_some(Synced {
                revision: Some(revision),
                server: port.id(),
                version: 1,
            })
        }
    }
}

/// A document's text as the server is handed it, its last line ended.
fn text_of(doc: &Document) -> String {
    let mut text = doc.buffer.lines().join("\n");
    text.push('\n');
    text
}

/// The place a location names.
fn location_place(location: &protocol::Location) -> Place {
    Place {
        at: location.range.start,
        path: location.path.clone(),
    }
}

/// Where `before` and `after` differ: the first line that does, how many
/// lines from there `before` loses, and how many `after` has in their
/// place. `None` where they read the same.
fn changed_span(before: &[String], after: &[String]) -> Option<(usize, usize, usize)> {
    let first = before.iter().zip(after).take_while(|(a, b)| a == b).count();
    if first == before.len() && first == after.len() {
        return None;
    }
    let room = before.len().min(after.len()) - first;
    let common_tail = before
        .iter()
        .rev()
        .zip(after.iter().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    Some((first, before.len() - first - common_tail, after.len() - first - common_tail))
}

/// The problems in `found` covering character `col` of line `row`.
fn diagnostics_at(found: &[Diagnostic], lines: &[String], row: usize, col: usize) -> Vec<Diagnostic> {
    let at = protocol::position_in(lines, row, col);
    let covering: Vec<&Diagnostic> = found
        .iter()
        .filter(|d| d.range.start <= at && (at < d.range.end || d.range.start == d.range.end && at == d.range.start))
        .collect();
    // A hint is drawn nowhere, so one is told of only where nothing drawn is.
    let drawn: Vec<&Diagnostic> = covering
        .iter()
        .copied()
        .filter(|d| level_of(d.severity).is_some())
        .collect();
    let told = match drawn.is_empty() {
        true => covering,
        false => drawn,
    };
    // What the pointer is on is the tightest of them: a wide problem round
    // a narrow one is about the text around it, not the text here.
    let Some(tightest) = told.iter().map(|d| span_of(d.range)).min() else {
        return Vec::new();
    };
    told.into_iter()
        .filter(|d| span_of(d.range) == tightest)
        .cloned()
        .collect()
}

/// How much text `range` covers, to tell the tighter of two ranges: the
/// lines it runs across, then the characters on a line it keeps to.
fn span_of(range: Range) -> (u32, u32) {
    // A range a server sends ending before it starts covers nothing, rather
    // than wrapping round to four billion lines.
    let lines = range.end.line.saturating_sub(range.start.line);
    match lines {
        0 => (0, range.end.character.saturating_sub(range.start.character)),
        _ => (lines, 0),
    }
}

/// The columns of line `index` (of `length` characters) `range` covers,
/// half-open: to the end of a line it runs past, and one character where
/// it covers none.
fn columns_on(range: Range, lines: &[String], index: usize, length: usize) -> (usize, usize) {
    let start = match range.start.line as usize == index {
        true => protocol::place_of(lines, range.start).1,
        false => 0,
    };
    let end = match range.end.line as usize == index {
        true => protocol::place_of(lines, range.end).1,
        false => length,
    };
    match end <= start {
        true if start >= length => (length.saturating_sub(1), length),
        true => (start, start + 1),
        false => (start, end),
    }
}

/// The hue a problem as bad as `severity` is marked in; none for a hint.
fn level_of(severity: Severity) -> Option<DiagnosticLevel> {
    match severity {
        Severity::Error => Some(DiagnosticLevel::Error),
        Severity::Warning => Some(DiagnosticLevel::Warning),
        Severity::Information => Some(DiagnosticLevel::Info),
        Severity::Hint => None,
    }
}

/// What a list or a card calls a problem as bad as `severity`.
pub(super) fn severity_word(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Information => "info",
        Severity::Hint => "hint",
    }
}

/// `found` as a card says it: each problem's message, and at its foot what
/// kind each is and what found it, as VS Code puts `rustc(E0308)` under a
/// problem.
pub(super) fn describe(found: &[Diagnostic]) -> Described {
    let text = found
        .iter()
        .map(|d| d.message.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut footer: Vec<String> = Vec::new();
    for d in found {
        let by = match (d.source.as_deref(), d.code.as_deref()) {
            (Some(source), Some(code)) => format!(" {source}({code})"),
            (Some(by), None) | (None, Some(by)) => format!(" {by}"),
            (None, None) => String::new(),
        };
        let line = format!("{}{by}", severity_word(d.severity));
        if !footer.contains(&line) {
            footer.push(line);
        }
    }
    Described {
        footer: footer.join("\n"),
        text,
    }
}

/// A server's completions as the `lsp` back-end's reply on row `row`, the
/// cursor at `col` of `line`: replacing from where the server says, else
/// from the start of the word being typed.
fn served(list: protocol::CompletionList, line: &str, row: usize, col: usize) -> Served {
    let chars: Vec<char> = line.chars().collect();
    let word_start = (0..col.min(chars.len()))
        .rev()
        .take_while(|&at| complete::is_word_char(chars[at]))
        .last()
        .unwrap_or(col);
    let start = list
        .entries
        .iter()
        .find_map(|entry| entry.replace_from.filter(|from| from.line as usize == row))
        .map_or(word_start, |from| protocol::to_chars(line, from.character).min(col));
    let mut edits = HashMap::new();
    let candidates: Vec<Candidate> = list
        .entries
        .into_iter()
        .map(|entry| {
            let label = match entry.label.starts_with(&entry.filter) {
                true => entry.label,
                false => entry.filter,
            };
            if !entry.additional.is_empty() {
                edits.insert(label.clone(), entry.additional);
            }
            let insert = match entry.snippet {
                true => simple_snippet(&entry.insert),
                false => entry.insert,
            };
            Candidate {
                detail: entry.detail,
                insert,
                kind: Kind::Server(ServerKind {
                    mark: mark_of(entry.kind),
                    snippet: entry.snippet,
                }),
                label,
                source: "lsp",
            }
        })
        .collect();
    Served {
        edits,
        incomplete: list.incomplete,
        reply: Reply { candidates, start },
        row,
    }
}

/// The letter the popup marks a server's completion kind with.
fn mark_of(kind: Option<u64>) -> char {
    match kind {
        Some(2) => 'm',
        Some(3 | 4) => 'f',
        Some(5 | 10) => 'p',
        Some(6 | 12) => 'v',
        Some(7 | 8 | 13 | 22 | 25) => 't',
        Some(9) => 'M',
        Some(14) => 'k',
        Some(15) => 's',
        Some(17) => 'F',
        Some(19) => 'd',
        Some(20) => 'e',
        Some(21) => 'c',
        Some(24) => 'o',
        _ => 'w',
    }
}

/// A server's snippet in the plain form [`complete::expand`] reads: each
/// field's text its own (a nested field's text taken in), a choice its
/// first option, a variable nothing, and an escaped character itself.
fn simple_snippet(body: &str) -> String {
    flatten_snippet(body, true)
}

/// [`simple_snippet`] at the top of a snippet, keeping its fields, or
/// inside a field's text, where only the text is kept.
fn flatten_snippet(body: &str, top: bool) -> String {
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::new();
    let mut at = 0;
    while at < chars.len() {
        match chars[at] {
            '\\' if at + 1 < chars.len() => {
                out.push(chars[at + 1]);
                at += 2;
            }
            '$' if chars.get(at + 1) == Some(&'{') => {
                let (inner, next) = braced(&chars, at + 2);
                let number: String = inner.chars().take_while(char::is_ascii_digit).collect();
                let rest = &inner[number.len()..];
                let text = match (number.is_empty(), rest.chars().next()) {
                    (false, Some(':')) => flatten_snippet(&rest[1..], false),
                    (false, Some('|')) => {
                        rest[1..].trim_end_matches('|').split(',').next().unwrap_or_default().to_string()
                    }
                    (false, None) => String::new(),
                    // A variable, with its default where it gives one.
                    _ => flatten_snippet(rest.split_once(':').map_or("", |(_, default)| default), false),
                };
                match (top && !number.is_empty(), text.is_empty()) {
                    (true, true) => out.push_str(&format!("${number}")),
                    (true, false) => out.push_str(&format!("${{{number}:{text}}}")),
                    (false, _) => out.push_str(&text),
                }
                at = next;
            }
            '$' if !top && chars.get(at + 1).is_some_and(char::is_ascii_digit) => {
                at += 1;
                while chars.get(at).is_some_and(char::is_ascii_digit) {
                    at += 1;
                }
            }
            c => {
                out.push(c);
                at += 1;
            }
        }
    }
    out
}

/// The text between a `{` opened before `from` and the `}` closing it,
/// nested braces included, and where reading goes on after it.
fn braced(chars: &[char], from: usize) -> (String, usize) {
    let mut depth = 1;
    let mut at = from;
    while at < chars.len() {
        match chars[at] {
            '\\' => at += 1,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return (chars[from..at].iter().collect(), at + 1);
                }
            }
            _ => {}
        }
        at += 1;
    }
    (chars[from..].iter().collect(), chars.len())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::editor::Editor;
    use crate::model::input::{Key, KeyCode};

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn test_a_format_answered_after_an_edit_is_dropped_not_applied() {
        // The server's edits are positions in the text it was sent; applied
        // to text typed into since, they corrupted it.
        let dir = std::env::temp_dir().join(format!("winter-lsp-stale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("a.rs");
        std::fs::write(&path, "fn main(){}\n").expect("file");
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        let asked_at = editor.buffer().revision();
        editor.on_key(&Key::plain(KeyCode::Char('i')));
        editor.on_key(&Key::plain(KeyCode::Char('x')));
        assert_ne!(editor.buffer().revision(), asked_at, "the text moved on");
        let insert = json!([{"range": {"start": {"line": 0, "character": 9}, "end": {"line": 0, "character": 9}}, "newText": " "}]);
        editor.answered(0, Asked::Format(path, asked_at), &insert);
        assert_eq!(editor.buffer().lines()[0], "xfn main(){}", "nothing was formatted into it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_a_server_snippet_is_read_down_to_fields_with_plain_text() {
        assert_eq!(simple_snippet("push(${1:value})$0"), "push(${1:value})$0");
        assert_eq!(simple_snippet("f(${1:a${2:b}})"), "f(${1:ab})", "a nested field's text is taken in");
        assert_eq!(simple_snippet("${1|one,two|} \\$x ${TM_FILENAME}"), "${1:one} $x ");
        assert_eq!(simple_snippet("${2}"), "$2");
    }

    #[test]
    fn test_the_changed_span_is_what_lies_between_the_common_head_and_tail() {
        let before = lines(&["a", "b", "c", "d"]);
        assert_eq!(changed_span(&before, &lines(&["a", "x", "y", "c", "d"])), Some((1, 1, 2)));
        assert_eq!(changed_span(&before, &lines(&["a", "d"])), Some((1, 2, 0)));
        assert_eq!(changed_span(&before, &before), None);
        assert_eq!(changed_span(&lines(&["a", "a"]), &lines(&["a"])), Some((1, 1, 0)), "a repeated line");
    }

    #[test]
    fn test_a_range_marks_its_columns_and_an_empty_one_a_single_character() {
        let text = lines(&["let x = 1;", "x"]);
        let range = |from: (u32, u32), to: (u32, u32)| Range {
            end: Position { character: to.1, line: to.0 },
            start: Position { character: from.1, line: from.0 },
        };
        assert_eq!(columns_on(range((0, 4), (0, 5)), &text, 0, 10), (4, 5));
        assert_eq!(columns_on(range((0, 8), (1, 1)), &text, 0, 10), (8, 10), "to the end of the line");
        assert_eq!(columns_on(range((0, 3), (0, 3)), &text, 0, 10), (3, 4));
        assert_eq!(columns_on(range((0, 10), (0, 10)), &text, 0, 10), (9, 10), "past the end, the last");
    }

    #[test]
    fn test_completions_replace_from_where_the_server_says_or_the_word_s_start() {
        let entry = |from: Option<u32>| protocol::CompletionEntry {
            additional: Vec::new(),
            detail: None,
            filter: "push".to_string(),
            insert: "push(${1:x})".to_string(),
            kind: Some(2),
            label: "push(…)".to_string(),
            replace_from: from.map(|character| Position { character, line: 3 }),
            snippet: true,
            sort: String::new(),
        };
        let list = |from| protocol::CompletionList {
            entries: vec![entry(from)],
            incomplete: false,
        };
        let line = "v.pu";
        let by_word = served(list(None), line, 3, 4);
        assert_eq!(by_word.reply.start, 2);
        assert_eq!(by_word.reply.candidates[0].label, "push(…)");
        assert_eq!(served(list(Some(1)), line, 3, 4).reply.start, 1);
        let mark = by_word.reply.candidates[0].kind;
        assert_eq!(mark, Kind::Server(ServerKind { mark: 'm', snippet: true }));
    }

    #[test]
    fn test_a_problem_covers_its_range_and_an_empty_one_its_place() {
        let text = lines(&["abc"]);
        let at = |from: u32, to: u32| Diagnostic {
            code: None,
            message: String::new(),
            range: Range {
                end: Position { character: to, line: 0 },
                start: Position { character: from, line: 0 },
            },
            severity: Severity::Error,
            source: None,
        };
        let found = [at(0, 2), at(2, 2)];
        assert_eq!(diagnostics_at(&found, &text, 0, 1).len(), 1);
        assert_eq!(diagnostics_at(&found, &text, 0, 2).len(), 1, "the end is not covered, the empty one is");
        assert!(diagnostics_at(&found, &text, 0, 3).is_empty());
    }

    #[test]
    fn test_the_pointer_is_told_only_of_the_tightest_drawn_problem_under_it() {
        let text = lines(&["let x: u32 = \"no\";", "take(x);"]);
        let problem = |severity, message: &str, start: (u32, u32), end: (u32, u32)| Diagnostic {
            code: Some("E0308".to_string()),
            message: message.to_string(),
            range: Range {
                end: Position { character: end.1, line: end.0 },
                start: Position { character: start.1, line: start.0 },
            },
            severity,
            source: Some("rustc".to_string()),
        };
        let found = [
            problem(Severity::Warning, "a block-wide lint", (0, 0), (1, 8)),
            problem(Severity::Error, "mismatched types", (0, 13), (0, 17)),
            problem(Severity::Hint, "expected due to this", (0, 7), (0, 10)),
            problem(Severity::Hint, "a related span", (0, 14), (0, 16)),
        ];
        let told = |row, col| -> Vec<String> {
            diagnostics_at(&found, &text, row, col).into_iter().map(|d| d.message).collect()
        };
        assert_eq!(told(0, 15), ["mismatched types"], "not the lint round it, nor a hint in it");
        assert_eq!(told(0, 8), ["a block-wide lint"], "a hint is not drawn, the lint is");
        assert_eq!(told(1, 2), ["a block-wide lint"]);
        let hints = [problem(Severity::Hint, "expected due to this", (0, 7), (0, 10))];
        assert_eq!(diagnostics_at(&hints, &text, 0, 8).len(), 1, "a hint alone is told of");
    }

    #[test]
    fn test_a_problem_card_says_the_message_and_at_its_foot_its_kind_and_source() {
        let at = Range {
            end: Position { character: 1, line: 0 },
            start: Position { character: 0, line: 0 },
        };
        let problem = |severity, source: Option<&str>, code: Option<&str>| Diagnostic {
            code: code.map(str::to_string),
            message: "mismatched types\nexpected `u32`, found `&str`".to_string(),
            range: at,
            severity,
            source: source.map(str::to_string),
        };
        let described = describe(&[
            problem(Severity::Error, Some("rustc"), Some("E0308")),
            problem(Severity::Error, Some("rustc"), Some("E0308")),
            problem(Severity::Warning, None, None),
        ]);
        assert!(described.text.starts_with("mismatched types\nexpected"), "{}", described.text);
        assert!(!described.text.contains("rustc"), "the source is at the foot");
        assert_eq!(described.footer, "error rustc(E0308)\nwarning", "each kind once");
    }
}
