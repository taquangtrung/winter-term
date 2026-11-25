//! What the window says of the work it does for its editors (builds,
//! language servers, opening files, the clipboard), kept typed until it is
//! posted: the one place its wording and severity are decided, so callers
//! and tests name a variant rather than a sentence. What the system or a
//! program said travels as a variant's data.

use std::path::PathBuf;

use winter_base::remote::SUDO_UNAVAILABLE;
use winter_lsp::servers::INSTALL_COMMAND_TITLE;
use winter_model::editor::Notice;

use super::compile::TRUST_TASKS_TITLE;
use super::extension_servers::undeclared;
use crate::editors::compile::task::PROJECT_TASKS;

// ========================================================================
// Data Structures
// ========================================================================

/// One message for the status bar.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) enum HostNotice {
    /// A command is back on its built-in keys.
    BackOnBuiltIn(String),
    /// A build is starting, by its command line.
    Building(String),
    /// The system clipboard would not take the text.
    ClipboardFailed,
    /// An editor's text was copied, said as the editor asked.
    Copied(String),
    /// Lines were copied as rich text, this many.
    CopiedRich(usize),
    /// A file could not be opened in the system's viewer.
    CouldNotOpen(FileFailure),
    /// A language server was installed already.
    Installed(Installed),
    /// A language server is being installed.
    Installing(Installing),
    /// No language server serves the file in front.
    LspAlreadyServes(Serving),
    /// A language server stopped on its own.
    LspExited(Exited),
    /// A language server stopped before it was ready.
    LspExitedEarly(Exited),
    /// Winter knows no way to install a language server, by its name.
    LspNoInstall(String),
    /// A language server installs only with programs missing here.
    LspNoInstaller(NoInstaller),
    /// No language server is known for a language.
    LspNoneKnown(String),
    /// No language server is running.
    LspNoneRunning,
    /// No language server is running, as the status says it.
    LspNoneRunningStatus,
    /// A language server is ready, by its name.
    LspReady(String),
    /// A language server said something, in its words.
    LspSaid(String),
    /// A language server could not start, with why.
    LspStartFailed(String),
    /// A language server is starting.
    LspStarting(Serving),
    /// What each running language server is doing.
    LspStatus(Vec<String>),
    /// A language server stopped, by its name.
    LspStopped(String),
    /// Every language server was stopped.
    LspStoppedAll,
    /// No containers are running to browse.
    NoContainers,
    /// The tab in front shows no file to open with another editor.
    NoFileToOpenWith,
    /// No SSH hosts are configured to browse.
    NoHosts,
    /// Markdown preview wants a Markdown file in front.
    NoMarkdown,
    /// No other editor shows the file, by its name.
    NoOtherEditor(String),
    /// The project has no tasks file, by its root.
    NoTasksFile(PathBuf),
    /// The tab in front is not a LaTeX file on this machine.
    NotLatex,
    /// The tab in front is not a file on this machine, for a language
    /// server.
    NotLocalFile,
    /// A file could not be opened in an editor, with the system's error.
    OpenFailed(String),
    /// A file could not be read, with the system's error.
    ReadFailed(FileFailure),
    /// A key binding could not be saved, with why.
    RebindFailed(String),
    /// A chord now runs a command.
    Rebound(Rebound),
    /// The scratch file could not be made, with the system's error.
    ScratchFailed(String),
    /// An extension's server was not fetched, with why.
    ServerNotFetched(NotFetched),
    /// An extension's server is declared nowhere.
    ServerUndeclared(ExtensionServer),
    /// An extension's server cannot be fetched for this machine.
    ServerUnfetchable(ExtensionServer),
    /// A file that is no text is shown as its bytes, with why it is no text.
    ShownAsBytes(String),
    /// A run still going cannot be let go of.
    StillRunning,
    /// Sudo failed, with why.
    SudoFailed(String),
    /// Sudo did not take the password.
    SudoRefused,
    /// Sudo is not on this system.
    SudoUnavailable,
    /// Finding the project's tasks went wrong, as the tasks reader said.
    TasksProblem(String),
    /// The project's on-save tasks did not run, not being trusted.
    TasksUntrusted,
    /// The project's tasks file is trusted now, by its path.
    Trusted(PathBuf),
}

/// A file that failed, with the system's error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct FileFailure {
    /// The system's error.
    pub(in crate::window) error: String,
    /// The file.
    pub(in crate::window) path: PathBuf,
}

/// A language server and the root it serves.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct Serving {
    /// The project's root.
    pub(in crate::window) root: PathBuf,
    /// The server's name.
    pub(in crate::window) server: String,
}

/// A language server installed already.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct Installed {
    /// The languages it serves, joined.
    pub(in crate::window) languages: String,
    /// Its name.
    pub(in crate::window) name: String,
}

/// A language server being installed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct Installing {
    /// The command installing it.
    pub(in crate::window) command: String,
    /// Its name.
    pub(in crate::window) name: String,
}

/// A language server whose installers are all missing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct NoInstaller {
    /// Its name.
    pub(in crate::window) name: String,
    /// The installers tried.
    pub(in crate::window) tried: Vec<String>,
}

/// A language server that stopped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct Exited {
    /// Its name.
    pub(in crate::window) name: String,
    /// Why, as it said.
    pub(in crate::window) why: String,
}

/// A chord bound to a command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct Rebound {
    /// The chord.
    pub(in crate::window) chord: String,
    /// The command.
    pub(in crate::window) command: String,
}

/// An extension's server, by the extension and the server's name or
/// program.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct ExtensionServer {
    /// The extension.
    pub(in crate::window) extension: String,
    /// The server's name, or its program.
    pub(in crate::window) server: String,
}

/// An extension's server that was not fetched.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct NotFetched {
    /// The server.
    pub(in crate::window) server: ExtensionServer,
    /// Why it was not.
    pub(in crate::window) why: String,
}

// ========================================================================
// HostNotice
// ========================================================================

impl From<HostNotice> for Notice {
    /// The notice the status bar shows: the one place its wording and
    /// severity are decided.
    fn from(notice: HostNotice) -> Self {
        use HostNotice::*;
        match notice {
            BackOnBuiltIn(command) => {
                Notice::info(format!("{command} is back on its built-in keys"))
            }
            Building(command) => Notice::info(format!("building: {command}")),
            ClipboardFailed => Notice::error("could not copy to clipboard"),
            Copied(said) => Notice::info(said).unlogged(),
            CopiedRich(lines) => {
                Notice::info(format!("copied {lines} lines as rich text")).unlogged()
            }
            CouldNotOpen(FileFailure { error, path }) => {
                Notice::error(format!("could not open {}: {error}", path.display()))
            }
            Installed(self::Installed { languages, name }) => Notice::info(format!(
                "LSP: {name} is installed and serves {languages}; LSP: Start runs it"
            )),
            Installing(self::Installing { command, name }) => Notice::info(format!(
                "LSP: installing {name} ({command}); LSP: Start runs it once done"
            )),
            LspAlreadyServes(Serving { root, server }) => {
                Notice::info(format!("LSP: {server} already serves {}", root.display()))
            }
            LspExited(Exited { name, why }) => Notice::error(format!("LSP: {name} stopped: {why}")),
            LspExitedEarly(Exited { name, why }) => Notice::error(format!(
                "LSP: {name} stopped before it was ready: {why}; if it is not installed, \
                 {INSTALL_COMMAND_TITLE} installs it"
            )),
            LspNoInstall(name) => Notice::error(format!(
                "LSP: Winter knows no way to install {name}; install it yourself"
            )),
            LspNoInstaller(NoInstaller { name, tried }) => Notice::error(format!(
                "LSP: {name} installs with {}, none of which is installed here",
                tried.join(", ")
            )),
            LspNoneKnown(language) => Notice::refusal(format!(
                "LSP: no language server is known for {language}: add one to settings.kdl's \
                 lsp block"
            )),
            LspNoneRunning => Notice::info("LSP: no language server is running"),
            LspNoneRunningStatus => Notice::info(
                "LSP: no language server is running; LSP: Start runs one for the file in front",
            ),
            LspReady(name) => Notice::info(format!("LSP: {name} is ready")).unlogged(),
            LspSaid(text) => Notice::warning(text),
            LspStartFailed(why) => Notice::error(format!("LSP: {why}")),
            LspStarting(Serving { root, server }) => {
                Notice::info(format!("LSP: starting {server} for {}", root.display()))
            }
            LspStatus(lines) => Notice::info(format!("LSP: {}", lines.join("\n"))),
            LspStopped(name) => Notice::info(format!("LSP: stopped {name}")),
            LspStoppedAll => Notice::info("LSP: stopped every language server"),
            NoContainers => {
                Notice::refusal("no running containers (is docker installed and reachable?)")
            }
            NoFileToOpenWith => Notice::refusal("open with: this tab shows no file"),
            NoHosts => Notice::refusal("no hosts in ~/.ssh/config"),
            NoMarkdown => {
                Notice::refusal("Markdown: Preview needs a Markdown file on this machine in front")
            }
            NoOtherEditor(name) => {
                Notice::refusal(format!("open with: no other editor shows {name}"))
            }
            NoTasksFile(root) => {
                Notice::refusal(format!("no {PROJECT_TASKS} in {}", root.display()))
            }
            NotLatex => Notice::refusal("LaTeX: not a LaTeX file on this machine"),
            NotLocalFile => Notice::refusal("LSP: the tab in front is not a file on this machine"),
            OpenFailed(error) => Notice::error(error),
            ReadFailed(FileFailure { error, path }) => {
                Notice::error(format!("{}: {error}", path.display()))
            }
            RebindFailed(reason) => Notice::error(reason),
            Rebound(self::Rebound { chord, command }) => {
                Notice::info(format!("{chord} now runs {command}"))
            }
            ScratchFailed(error) => Notice::error(format!("scratch: {error}")),
            ServerNotFetched(NotFetched { server, why }) => Notice::error(format!(
                "{}: {} not fetched: {why}",
                server.extension, server.server
            )),
            ServerUndeclared(ExtensionServer { extension, server }) => {
                Notice::error(format!("{extension}: {}", undeclared(&server)))
            }
            ServerUnfetchable(ExtensionServer { extension, server }) => Notice::error(format!(
                "{extension}: {server} cannot be fetched for this machine"
            )),
            ShownAsBytes(why) => Notice::info(format!("{why}: shown as its bytes")),
            StillRunning => Notice::refusal("Output: stop the run first (C-g in its tab)"),
            SudoFailed(reason) => Notice::error(reason),
            SudoRefused => Notice::error("sudo did not take the password"),
            SudoUnavailable => Notice::error(SUDO_UNAVAILABLE),
            TasksProblem(problem) => Notice::error(problem),
            TasksUntrusted => Notice::error(format!(
                "on-save tasks in {PROJECT_TASKS} did not run: trust them with \
                 \"{TRUST_TASKS_TITLE}\""
            )),
            Trusted(path) => Notice::info(format!("trusted {}", path.display())),
        }
    }
}
