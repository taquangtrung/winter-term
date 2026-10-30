//! The window's language servers: started when the user asks (or for a
//! language `lsp { auto-start }` names), handed to every editor, and what
//! they say passed on to the editors it is for.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use crate::editors::text::TextEditor;
use winter_lsp::{servers, Event, Hub, Incoming};
use crate::model::editor::{EditorOutcome, Notice};
use crate::model::layout::TabId;
use crate::wake::Waker;
use crate::window::App;

// ========================================================================
// Data Structures
// ========================================================================

/// The servers running, and what the window has done with them.
pub(in crate::window) struct LspState {
    /// The servers' generation and the count of editors they were last
    /// handed to, so they are handed over again only when either moves.
    handed: (u64, usize),
    pub(super) hub: Hub,
    /// Copilot, run through the same hub.
    pub(super) copilot: super::copilot::CopilotState,
    /// The languages and projects a server was started for unasked, so one
    /// that will not start is not tried again on every frame.
    tried: HashSet<(String, PathBuf)>,
    /// The project each language's server would serve from each directory,
    /// worked out once: finding it searches `PATH` and every directory up
    /// from the file, which on every frame was a stream of system calls.
    roots: HashMap<(String, PathBuf), PathBuf>,
}

// ========================================================================
// LspState Implementations
// ========================================================================

impl LspState {
    /// No servers, their messages waking the loop through `waker`.
    pub(in crate::window) fn new(waker: &Waker) -> Self {
        let (to, from) = mpsc::channel();
        Self {
            handed: (0, 0),
            hub: Hub::new(to, waker.relay(from)),
            copilot: super::copilot::CopilotState::default(),
            roots: HashMap::new(),
            tried: HashSet::new(),
        }
    }

    /// What the busiest server is doing, for the status bar.
    pub(in crate::window) fn busy(&self) -> Option<String> {
        self.hub.busy()
    }
}

// ========================================================================
// App: language servers
// ========================================================================

impl App {
    /// Start the server for the file in front, which every editor of the
    /// project's files is handed once it is ready.
    pub(in crate::window) fn start_lsp(&mut self) {
        let Some((language, path)) = self.lsp_subject() else {
            self.set_error("LSP: the tab in front is not a file on this machine");
            return;
        };
        if let Some(port) = self.host.lsp.hub.serving(&language, &path) {
            let said = format!("LSP: {} already serves {}", port.name(), port.root().display());
            self.set_notice(said);
            return;
        }
        self.start_server(&language, &path);
    }

    /// Stop the server serving the file in front, or every server where
    /// none serves it.
    pub(in crate::window) fn stop_lsp(&mut self) {
        let serving = self
            .lsp_subject()
            .and_then(|(language, path)| self.host.lsp.hub.serving(&language, &path).cloned());
        match serving {
            Some(port) => {
                self.host.lsp.hub.stop(port.id());
                self.set_notice(format!("LSP: stopped {}", port.name()));
            }
            None => {
                // Copilot is no language server, and is left running.
                let running: Vec<_> = self.host.lsp.hub.ports().into_iter().filter(|port| !port.is_everywhere()).collect();
                if running.is_empty() {
                    self.set_notice("LSP: no language server is running");
                    return;
                }
                for port in running {
                    self.host.lsp.hub.stop(port.id());
                }
                self.set_notice("LSP: stopped every language server");
            }
        }
        self.hand_servers_over();
    }

    /// Stop the server serving the file in front and start it again.
    pub(in crate::window) fn restart_lsp(&mut self) {
        let Some((language, path)) = self.lsp_subject() else {
            self.set_error("LSP: the tab in front is not a file on this machine");
            return;
        };
        if let Some(port) = self.host.lsp.hub.serving(&language, &path).cloned() {
            self.host.lsp.hub.stop(port.id());
        }
        self.start_server(&language, &path);
    }

    /// Say which servers are running and what each is doing.
    pub(in crate::window) fn show_lsp_status(&mut self) {
        let lines = self.host.lsp.hub.summary();
        match lines.is_empty() {
            true => self.set_notice("LSP: no language server is running; LSP: Start runs one for the file in front"),
            false => self.set_notice(format!("LSP: {}", lines.join("\n"))),
        }
    }

    /// Pass on what the servers said, hand the editors the servers running,
    /// and start the servers settings start unasked. Says whether anything
    /// needs drawing again.
    pub(in crate::window) fn deliver_lsp(&mut self) -> bool {
        let said = self.host.lsp.hub.drain();
        let any = !said.is_empty();
        for incoming in said {
            if !self.copilot_heard(&incoming) {
                self.pass_on(incoming);
            }
        }
        self.hand_servers_over();
        self.auto_start_servers();
        self.start_copilot_if_wanted();
        any
    }

    /// The language and the file of the tab in front, where it is a file a
    /// server could read.
    fn lsp_subject(&self) -> Option<(String, PathBuf)> {
        let focused = self.layout().focused();
        self.open_tabs.editor(&focused)?.editor.with_kind(TextEditor::lsp_subject).flatten()
    }

    /// Start the server for `path`, written in `language`, saying so.
    fn start_server(&mut self, language: &str, path: &std::path::Path) {
        let setting = self.config.lsp.servers.get(language).map(Vec::as_slice);
        let started = servers::plan(language, path, setting)
            .and_then(|plan| self.host.lsp.hub.start(&plan).map(|port| (port, plan)));
        match started {
            Ok((port, plan)) => {
                self.set_notice(format!("LSP: starting {} for {}", port.name(), plan.root.display()));
            }
            Err(why) => self.set_error(format!("LSP: {why}")),
        }
    }

    /// Hand every editor the servers running, where they or the editors
    /// have changed since last time.
    fn hand_servers_over(&mut self) {
        let generation = self.host.lsp.hub.generation();
        let editors = self.open_tabs.editors().count();
        if self.host.lsp.handed == (generation, editors) {
            return;
        }
        self.host.lsp.handed = (generation, editors);
        let ports = self.host.lsp.hub.ports();
        for slot in self.open_tabs.buffers_mut() {
            slot.editor.with_kind_mut(|text: &mut TextEditor| text.connect_lsp(generation, &ports));
        }
    }

    /// Start the server of each language settings start unasked, for the
    /// open files written in one.
    fn auto_start_servers(&mut self) {
        if self.config.lsp.auto_start.is_empty() {
            return;
        }
        let wanted: Vec<(String, PathBuf)> = self
            .open_tabs
            .editors()
            .filter_map(|(_, slot)| slot.editor.with_kind(TextEditor::lsp_subject).flatten())
            .filter(|(language, _)| self.config.lsp.auto_start.contains(language))
            .collect();
        for (language, path) in wanted {
            if self.host.lsp.hub.serving(&language, &path).is_some() {
                continue;
            }
            let dir = path.parent().map_or_else(|| path.clone(), Path::to_path_buf);
            let key = (language.clone(), dir);
            let root = match self.host.lsp.roots.get(&key) {
                Some(root) => root.clone(),
                None => {
                    let setting = self.config.lsp.servers.get(&language).map(Vec::as_slice);
                    let root = servers::plan(&language, &path, setting).map_or_else(|_| path.clone(), |plan| plan.root);
                    self.host.lsp.roots.insert(key, root.clone());
                    root
                }
            };
            if self.host.lsp.tried.insert((language.clone(), root)) {
                self.start_server(&language, &path);
            }
        }
    }

    /// Pass one message on: what the window says itself, and what goes to
    /// the editors, the one in front first. An answer or an edit is acted
    /// on by the first editor it is for.
    fn pass_on(&mut self, incoming: Incoming) {
        let name = self
            .host.lsp
            .hub
            .ports()
            .into_iter()
            .find(|port| port.id() == incoming.server)
            .map_or_else(|| "language server".to_string(), |port| port.name().to_string());
        match &incoming.event {
            Event::Ready => self.notify(Notice::info(format!("LSP: {name} is ready")).unlogged()),
            Event::Exited(why) => self.set_error(format!("LSP: {name} stopped: {why}")),
            Event::Message(text) => self.notify(Notice::warning(text.clone())),
            Event::Progress(_) | Event::OpenUrl(_) | Event::Status { .. } => self.dirty = true,
            Event::Answer { .. } | Event::ApplyEdit(_) | Event::Diagnostics(_) => {
                let once = !matches!(incoming.event, Event::Diagnostics(_));
                let focused = self.layout().focused();
                let mut tabs: Vec<TabId> = self.open_tabs.editors().map(|(tab, _)| *tab).collect();
                tabs.sort_by_key(|tab| *tab != focused);
                for tab in tabs {
                    let Some(outcome) = self
                        .open_tabs
                        .editor_mut(&tab)
                        .and_then(|slot| slot.editor.with_kind_mut(|text: &mut TextEditor| text.on_lsp(&incoming)))
                    else {
                        continue;
                    };
                    if outcome == EditorOutcome::Ignored {
                        continue;
                    }
                    self.act_on_editor_outcome(tab, outcome);
                    self.dirty = true;
                    if once {
                        break;
                    }
                }
            }
        }
    }
}
