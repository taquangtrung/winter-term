//! What a row chosen in a picker has the window do, and the doing of it:
//! the one place a picker's choice reaches the window.

use std::path::PathBuf;

use winter_model::editor::{OpenTarget, SearchHit};
use winter_model::layout::TabId;
use winter_model::path::abbreviate_home;
use winter_picker::SavedLines;

#[cfg(feature = "wasm-extensions")]
use crate::window::extension_installs::ExtensionRequest;
use crate::window::App;

use super::sources::{create_file, ExtensionChoice};

// ========================================================================
// Constants
// ========================================================================

/// The command that runs the last one again, which is not itself the last.
const REPEAT_LAST_COMMAND: &str = "repeat_last_command";

/// Said when the mux daemon cannot be reached.
const MUX_UNREACHABLE: &str = "could not connect to mux daemon";

// ========================================================================
// Data Structures
// ========================================================================

/// What a picker has the window do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) enum WindowOutcome {
    /// Answer the editor's question the picker was opened for, with the
    /// item chosen.
    AnswerEditor(String),
    /// `cd` the focused terminal into a directory.
    ChangeDirectory(String),
    /// Make a new, empty file and open it.
    CreateFile(PathBuf),
    /// Install an extension as a file, or change what one may reach.
    #[cfg(feature = "wasm-extensions")]
    Extension(ExtensionRequest),
    /// Tell the extension whose picker it is what was chosen in it.
    ExtensionChosen(ExtensionChoice),
    /// Go to a tab where it is.
    FocusTab(TabId),
    /// Put a language server first for its languages and install it, by
    /// its name.
    InstallServer(String),
    /// Go to a place Scout or the outline found, in the focused tab.
    Jump(usize),
    /// Kill the buffer a tab shows.
    KillBuffer(TabId),
    /// Do something with a background mux session.
    Mux(MuxRequest),
    /// List a directory in a Dir editor.
    OpenDir(PathBuf),
    /// Open a file, at a line and column when known.
    OpenFile(OpenTarget),
    /// Open a file in a pane beside the focused one.
    OpenFileBeside(OpenTarget),
    /// Open the focused tab's file with another editor, by its kind.
    OpenWith(String),
    /// Paste an entry of the clipboard ring, by its place.
    Paste(usize),
    /// Run a command by its id.
    RunCommand(String),
    /// Run a build task, by the row the task list gave it.
    RunTask(String),
    /// Run a typed command line as the project's build.
    RunTyped(String),
    /// Save rows of lines found to a Grep buffer.
    SaveHits(SaveHits),
    /// Save a picker's rows to a buffer of their own, as lines of text.
    SaveLines(SavedLines),
    /// Show a buffer in the focused pane, bringing it back if it is buried.
    ShowBuffer(TabId),
    /// Type text into the focused terminal.
    TypeIntoTerminal(String),
    /// Switch to, forget, or name a workspace.
    Workspace(WorkspaceRequest),
}

/// The lines a search found, to keep in a Grep buffer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct SaveHits {
    /// Whether the search stopped at its cap with lines left.
    pub(in crate::window) capped: bool,
    /// The words the rows were filtered by, which a search run again from
    /// the buffer does not take in.
    pub(in crate::window) filtered: Option<String>,
    /// The lines, in the order shown.
    pub(in crate::window) hits: Vec<SearchHit>,
    /// What was searched for.
    pub(in crate::window) query: String,
    /// Where the search looked.
    pub(in crate::window) root: PathBuf,
    /// The comment keywords searched for, for a Todo list.
    pub(in crate::window) todo: Option<Vec<String>>,
}

/// Something to do with a background mux session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) enum MuxRequest {
    /// Attach to a session by name.
    Attach(String),
    /// Attach to a session on another host over ssh.
    AttachRemote(MuxRemote),
    /// End a session by name.
    Kill(String),
    /// Start a session.
    New(MuxNew),
}

/// A session on another host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct MuxRemote {
    pub(in crate::window) host: String,
    pub(in crate::window) session: String,
}

/// A session to start: its name, and the command it runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct MuxNew {
    pub(in crate::window) command: Option<String>,
    pub(in crate::window) name: String,
}

/// Something to do with a workspace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) enum WorkspaceRequest {
    /// Name the workspace being marked or renamed.
    Name(String),
    /// Forget a workspace.
    Remove(String),
    /// Switch the window to a workspace.
    Switch(String),
}

// ========================================================================
// App: carrying out
// ========================================================================

impl App {
    /// Do what a picker's chosen row asked, with `focused` the tab in front.
    /// The answer to an editor is carried by the picker's overlay, which
    /// holds the question, and never reaches here.
    pub(in crate::window) fn carry_out(&mut self, outcome: WindowOutcome, focused: TabId) {
        match outcome {
            WindowOutcome::AnswerEditor(_) => {}
            WindowOutcome::ChangeDirectory(dir) => self.change_directory(&dir, focused),
            WindowOutcome::CreateFile(path) => self.create_and_open(path),
            #[cfg(feature = "wasm-extensions")]
            WindowOutcome::Extension(asked) => self.carry_out_extension(asked),
            WindowOutcome::ExtensionChosen(choice) => self.tell_extension_choice(choice, focused),
            WindowOutcome::FocusTab(tab) => self.switch_to_tab(tab),
            WindowOutcome::InstallServer(name) => self.install_server(&name),
            WindowOutcome::Jump(target) => self.confirm_jump(target, focused),
            WindowOutcome::KillBuffer(tab) => self.kill_buffer(tab),
            WindowOutcome::Mux(request) => self.carry_out_mux(request, focused),
            WindowOutcome::OpenDir(dir) => self.open_dir_editor_at(dir),
            WindowOutcome::OpenFile(target) => self.open_path_editor(target),
            WindowOutcome::OpenFileBeside(target) => self.open_beside(focused, target),
            WindowOutcome::OpenWith(kind) => self.open_focused_with(&kind, focused),
            WindowOutcome::Paste(at) => self.paste_from_ring(at),
            WindowOutcome::RunCommand(id) => {
                if id != REPEAT_LAST_COMMAND {
                    self.recent.last_palette_command = Some(id.clone());
                }
                self.run_command(&id, focused);
            }
            WindowOutcome::RunTask(row) => self.choose_task(&row),
            WindowOutcome::RunTyped(command) => self.run_typed_command(&command),
            WindowOutcome::SaveHits(save) => self.save_hits(save),
            WindowOutcome::SaveLines(saved) => self.save_lines(saved),
            WindowOutcome::ShowBuffer(tab) => self.show_buffer(tab),
            WindowOutcome::TypeIntoTerminal(text) => {
                if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                    terminal.write(text.as_bytes());
                }
            }
            WindowOutcome::Workspace(request) => self.carry_out_workspace(request),
        }
        self.dirty = true;
    }

    /// Make the file at `path` and open it as every other way into a file
    /// does, over the focused pane; a file that cannot be made says why.
    fn create_and_open(&mut self, path: PathBuf) {
        match create_file(&path) {
            Ok(()) => self.open_path_editor(OpenTarget::file(path)),
            Err(e) => self.set_error(format!("{}: {e}", abbreviate_home(&path))),
        }
    }

    /// `cd` the terminal `focused` into `dir`, quoted so its name cannot run
    /// anything, and refused when it holds a control character, which a
    /// shell's own report of where it is could carry to start a second
    /// command.
    fn change_directory(&mut self, dir: &str, focused: TabId) {
        if dir.chars().any(char::is_control) {
            return;
        }
        if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
            let escaped = dir.replace('\'', "'\\''");
            terminal.write(format!("cd '{escaped}'\n").as_bytes());
        }
    }

    fn carry_out_mux(&mut self, request: MuxRequest, focused: TabId) {
        match request {
            MuxRequest::Attach(name) => self.new_mux_tab(&name),
            MuxRequest::AttachRemote(remote) => {
                self.new_mux_tab_remote_at(&remote.host, &remote.session);
            }
            MuxRequest::Kill(name) => {
                let socket = winter_mux::server::default_socket_path();
                match winter_mux::client::MuxClient::connect(&socket) {
                    Ok(mut client) => {
                        let _ = client.kill(&name);
                        self.set_notice(format!("killed mux session '{name}'"));
                    }
                    Err(_) => self.set_error(MUX_UNREACHABLE),
                }
            }
            MuxRequest::New(new) => {
                // Started where the user is: the focused terminal's directory,
                // when its shell has said.
                let cwd = self
                    .open_tabs
                    .terminal(&focused)
                    .and_then(|terminal| terminal.cwd());
                self.spawn_mux_tab_at(
                    &winter_mux::server::default_socket_path(),
                    &new.name,
                    new.command.as_deref(),
                    cwd.as_deref(),
                    focused,
                );
            }
        }
    }

    fn carry_out_workspace(&mut self, request: WorkspaceRequest) {
        match request {
            WorkspaceRequest::Name(name) => self.name_workspace(&name),
            WorkspaceRequest::Remove(name) => {
                let done = self.remove_workspace(&name);
                self.report(done);
            }
            WorkspaceRequest::Switch(name) => {
                let done = self.switch_workspace(&name);
                self.report(done);
            }
        }
    }
}
