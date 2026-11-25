//! What the window itself tells the user (its layout and tabs, sessions,
//! workspaces, settings, pickers, keys, macros, extensions, and its own
//! runtime), kept typed until it is posted: the one place its wording and
//! severity are decided, so callers and tests name a variant rather than a
//! sentence. The terminal and the window's work for its editors have
//! enums of their own (`terminal::notice`, `editor::notice`).
//!
//! What the system, a configuration file, or an extension said travels as a
//! variant's data.

use std::path::PathBuf;

use winter_model::editor::Notice;
use winter_model::paint::MarkupMarkers;
use winter_model::path::abbreviate_home;

use super::appearance::toggle_word;
use super::layout::tabs::NOTHING_CLOSED;
use super::picker::replace::{files_count, lines_count};
use crate::editors::extension::failure;

// ========================================================================
// Data Structures
// ========================================================================

/// One message for the status bar.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) enum WindowNotice {
    /// A managed agent could not be started or listed, and why.
    AgentFailed(String),
    /// Something was done to a managed agent: merged, discarded, stopped.
    AgentDone(String),
    /// An agent started by hand moved onto its own branch, this one.
    AgentMoved(String),
    /// A managed agent started, on its branch.
    AgentStarted(String),
    /// Reading the window's threads is already under way.
    AlreadyGathering,
    /// A tab could not attach to a mux session.
    AttachFailed(Failure),
    /// A tab attached to a mux session, by its name.
    AttachedMux(String),
    /// A tab attached to a session on another host.
    AttachedRemote(HostSession),
    /// A terminal running a program was buried, by its name.
    Buried(String),
    /// A page cannot draw what it holds, by its title.
    CannotDraw(String),
    /// A configuration file did not read, as its reader said.
    ConfigProblem(String),
    /// A file a picker was asked to make could not be made.
    CreateFailed(FileFailure),
    /// A keyboard macro is being recorded to the default register.
    DefiningMacro,
    /// Describe key waits for a key.
    DescribeAsk,
    /// Describe key waits for the next key of a sequence, by those so far.
    DescribePending(String),
    /// What a key sequence runs.
    Described(Described),
    /// A tab detached from its mux session, by its name.
    Detached(String),
    /// The details of an error were copied.
    DetailsCopied,
    /// An extension's text was copied, by the extension.
    ExtensionCopied(String),
    /// An extension could not make a web request, by the extension.
    ExtensionNoRequest(String),
    /// An extension could not run a program.
    ExtensionNoRun(ExtensionProgram),
    /// An extension could not start its work, by the extension.
    ExtensionNoWork(String),
    /// An extension asked to open a page that is not https.
    ExtensionNotHttps(ExtensionUrl),
    /// An extension's page could not be opened.
    ExtensionOpenFailed(ExtensionOpenFailure),
    /// An extension opened a page.
    ExtensionOpened(ExtensionUrl),
    /// An extension said something, in its words.
    ExtensionSaid(String),
    /// An extension panicked and was stopped, by the extension.
    ExtensionStopped(String),
    /// Reading the window's threads could not start, with why.
    GatherFailed(String),
    /// Reading the window's threads has begun.
    Gathering,
    /// Installing an extension failed, with why.
    InstallFailed(String),
    /// An extension was installed, by its id.
    Installed(String),
    /// A handler panicked and the window carried on.
    InternalError(InternalError),
    /// A program kept running could not be kept, with the system's error.
    KeepFailed(String),
    /// The last shell exited, the window staying open.
    LastShellExited,
    /// A keyboard macro was recorded to the default register.
    MacroDefined,
    /// A keyboard macro was recorded to a register, by its name.
    MacroRecorded(char),
    /// Replaying a macro went too deep.
    MacroRecursion,
    /// How Markdown's markers show now.
    MarkupMarkers(MarkupMarkers),
    /// A mux session failed, as the server said.
    MuxFailed(String),
    /// A mux session was killed, by its name.
    MuxKilled(String),
    /// A mux session started.
    MuxStarted(MuxStarted),
    /// The mux daemon could not be reached.
    MuxUnreachable,
    /// A new window could not be started, with the system's error.
    NewWindowFailed(String),
    /// The path of this binary is not known, for a new window.
    NewWindowNoBinary,
    /// No state directory to keep extension approvals in.
    NoApprovalStore,
    /// No macro has been recorded.
    NoMacro,
    /// No other workspace to go to.
    NoOtherWorkspace,
    /// The line under the cursor names no place.
    NoPlace,
    /// No command was run before to repeat.
    NoPreviousCommand,
    /// A remote mux session was saved with no host.
    NoRemoteHost(String),
    /// No state directory to write a report to.
    NoReportDir,
    /// The shell could not start, nor the default one.
    NoShell(String),
    /// A page has no command by that name.
    NoSuchCommand(TitledCommand),
    /// The other sidebar holds every view.
    NoViewLeft,
    /// No state directory to keep workspaces in.
    NoWorkspaceStore,
    /// No workspaces have been marked.
    NoWorkspaces,
    /// Only a sidebar's view moves to the other sidebar.
    NotInASidebar,
    /// An extension was turned down, by its id.
    NotInstalled(String),
    /// The pane is not attached to a mux session.
    NotMux,
    /// No closed tab to reopen.
    NothingClosed,
    /// No task of the tab's is running to stop.
    NothingRunning,
    /// Nothing to outline in the tab in front; a refusal when no tab is.
    NothingToOutline,
    /// The picker's search found nothing to replace.
    NothingToReplace,
    /// No picker was open before to resume.
    NothingToResume,
    /// The picker has no rows to save.
    NothingToSave,
    /// The notifications were cleared.
    NotificationsCleared,
    /// The only tab cannot be detached.
    OnlyTab,
    /// A terminal's output reader hit an internal error and was reset.
    OutputReset(String),
    /// Profiling was turned on (`true`) or off.
    Profiling(bool),
    /// Rainbow parentheses were turned on (`true`) or off.
    RainbowParens(bool),
    /// A tab could not re-attach to a mux session.
    ReattachFailed(Failure),
    /// A tab could not re-attach to a session on another host.
    ReattachRemoteFailed(RemoteFailure),
    /// A macro is being recorded to a register, by its name.
    RecordingMacro(char),
    /// The path of this binary is not known, for a reload.
    ReloadNoBinary,
    /// A reload could not save the session to hand over, with why.
    ReloadSaveFailed(String),
    /// A reload could not start the new binary.
    ReloadStartFailed(Failure),
    /// The new binary does not run.
    ReloadWontRun(Failure),
    /// A picker's pattern is no regular expression, with why.
    ReplaceBadPattern(String),
    /// Matches were replaced across files.
    Replaced(Replaced),
    /// Restoring a pane failed, the default shell tried instead.
    RestoreFailed(String),
    /// The clipboard ring was cleared.
    RingCleared,
    /// An extension's secret could not be kept, with why.
    SecretFailed(String),
    /// An extension's secret was forgotten.
    SecretForgotten(SecretKey),
    /// An extension's secret was saved.
    SecretSaved(SecretKey),
    /// Sentence highlighting was turned on (`true`) or off.
    SentenceHighlight(bool),
    /// A setting was saved for every workspace while this one sets its own.
    SettingOverridden,
    /// settings.kdl has errors, so a change applies but is not saved.
    SettingsNotSaved,
    /// settings.kdl could not be written, with the system's error.
    SettingsSaveFailed(String),
    /// The shell could not start, the default one tried instead.
    ShellFailed(String),
    /// A host could not be reached over SSH.
    SshFailed(Failure),
    /// A Git panel action failed, as git said.
    StageFailed(String),
    /// Tasks of the tab's are being stopped, by their names.
    Stopping(Vec<String>),
    /// The terminal gutter is shown (`true`) or hidden.
    TerminalGutter(bool),
    /// A theme name has characters a file name cannot take.
    ThemeBadName,
    /// A theme could not be created, with the system's error.
    ThemeCreateFailed(String),
    /// A theme was created, by its name.
    ThemeCreated(String),
    /// A theme did not read, as its reader said.
    ThemeProblem(String),
    /// A key sequence runs nothing, by the keys.
    Undefined(String),
    /// A workspace command went through, as it reported itself.
    WorkspaceDone(String),
    /// A workspace command did not, as it reported itself.
    WorkspaceProblem(String),
    /// A workspace could not be saved.
    WorkspaceSaveFailed(Failure),
    /// A report could not be written, with why.
    WriteFailed(String),
}

/// Something named that failed, with the system's error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct Failure {
    /// The system's error.
    pub(in crate::window) error: String,
    /// What failed: a session, a host, a workspace, a binary.
    pub(in crate::window) what: String,
}

/// A file that failed, with the system's error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct FileFailure {
    /// The system's error.
    pub(in crate::window) error: String,
    /// The file.
    pub(in crate::window) path: PathBuf,
}

/// A session on another host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct HostSession {
    /// The host.
    pub(in crate::window) host: String,
    /// The session.
    pub(in crate::window) session: String,
}

/// A session on another host that failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct RemoteFailure {
    /// The system's error.
    pub(in crate::window) error: String,
    /// The host and session.
    pub(in crate::window) session: HostSession,
}

/// A mux session that started.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct MuxStarted {
    /// Its size, in columns.
    pub(in crate::window) cols: u16,
    /// What it runs, when not a shell.
    pub(in crate::window) command: Option<String>,
    /// Its name.
    pub(in crate::window) name: String,
    /// Its size, in rows.
    pub(in crate::window) rows: u16,
}

/// What a key sequence runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct Described {
    /// The keys.
    pub(in crate::window) keys: String,
    /// What they run, as the keymap describes it.
    pub(in crate::window) what: String,
}

/// A command by name, on a page by title.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct TitledCommand {
    /// The command's name.
    pub(in crate::window) name: String,
    /// The page's title.
    pub(in crate::window) title: String,
}

/// A page an extension asked to open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct ExtensionUrl {
    /// The extension.
    pub(in crate::window) extension: String,
    /// The page.
    pub(in crate::window) url: String,
}

/// A page an extension asked to open, which would not.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct ExtensionOpenFailure {
    /// The system's error.
    pub(in crate::window) error: String,
    /// The extension and the page.
    pub(in crate::window) page: ExtensionUrl,
}

/// A program an extension asked to run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct ExtensionProgram {
    /// The extension.
    pub(in crate::window) extension: String,
    /// The program.
    pub(in crate::window) program: String,
}

/// An extension's secret, by the extension and its key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct SecretKey {
    /// The extension.
    pub(in crate::window) extension: String,
    /// The secret's key.
    pub(in crate::window) key: String,
}

/// A handler that panicked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct InternalError {
    /// Where the crash log is, when there is one.
    pub(in crate::window) log: Option<PathBuf>,
    /// What it panicked with.
    pub(in crate::window) why: String,
}

/// What a replace across files did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) struct Replaced {
    /// Files written.
    pub(in crate::window) files: usize,
    /// Lines left alone, changed since the search.
    pub(in crate::window) refused: usize,
    /// Lines written.
    pub(in crate::window) written: usize,
}

// ========================================================================
// WindowNotice
// ========================================================================

impl From<WindowNotice> for Notice {
    /// The notice the status bar shows: the one place its wording and
    /// severity are decided.
    fn from(notice: WindowNotice) -> Self {
        use WindowNotice::*;
        match notice {
            AlreadyGathering => Notice::info("already reading the window's threads"),
            AttachFailed(Failure { error, what }) => Notice::error(format!(
                "could not attach to '{what}' ({error}); start the server with 'winter mux serve'"
            )),
            AgentDone(what) => Notice::info(format!("agent: {what}")),
            AgentFailed(why) => Notice::error(format!("agent: {why}")),
            AgentMoved(branch) => Notice::info(format!(
                "moved the agent to {branch}, where it starts again with its resume line; \
                 uncommitted edits stay where they were"
            )),
            AgentStarted(branch) => Notice::info(format!(
                "started an agent on {branch}, in a worktree of its own: Agent: List shows it"
            )),
            AttachedMux(session) => Notice::info(format!("attached to mux session '{session}'")),
            AttachedRemote(HostSession { host, session }) => {
                Notice::info(format!("attached to '{host}:{session}'"))
            }
            Buried(name) => Notice::info(format!(
                "{name} runs on, buried: back from Buffer: Switch to…"
            )),
            CannotDraw(name) => Notice::error(format!("{name}: cannot draw this")),
            ConfigProblem(problem) => Notice::error(problem),
            CreateFailed(FileFailure { error, path }) => {
                Notice::error(format!("{}: {error}", abbreviate_home(&path)))
            }
            DefiningMacro => Notice::info("defining keyboard macro…"),
            DescribeAsk => Notice::info("Describe key: press a key"),
            DescribePending(keys) => {
                Notice::info(format!("Describe key: {keys}, press the next key"))
            }
            Described(self::Described { keys, what }) => Notice::info(format!("{keys} {what}")),
            DetailsCopied => Notice::info("copied the details").unlogged(),
            Detached(name) => Notice::info(format!("detached from mux session '{name}'")),
            ExtensionCopied(extension) => {
                Notice::info(format!("{extension}: copied to the clipboard"))
            }
            ExtensionNoRequest(extension) => Notice::info(format!(
                "the {extension} extension could not make its request"
            )),
            ExtensionNoRun(ExtensionProgram { extension, program }) => {
                Notice::info(format!("the {extension} extension could not run {program}"))
            }
            ExtensionNoWork(extension) => Notice::info(format!(
                "the {extension} extension could not start its work"
            )),
            ExtensionNotHttps(ExtensionUrl { extension, url }) => Notice::error(format!(
                "the {extension} extension asked to open {url}, which is not an https page"
            )),
            ExtensionOpenFailed(ExtensionOpenFailure { error, page }) => Notice::error(format!(
                "{}: could not open {}: {error}",
                page.extension, page.url
            )),
            ExtensionOpened(ExtensionUrl { extension, url }) => {
                Notice::info(format!("{extension}: opened {url}"))
            }
            ExtensionSaid(message) => Notice::info(message),
            ExtensionStopped(extension) => Notice::info(failure(&extension)),
            GatherFailed(error) => {
                Notice::error(format!("could not start reading the threads: {error}"))
            }
            Gathering => Notice::info("reading the window's threads: it pauses for a few seconds"),
            InstallFailed(why) => Notice::error(why),
            Installed(id) => Notice::info(format!(
                "installed {id}: restart Winter for it to take effect"
            )),
            InternalError(self::InternalError { log, why }) => {
                let log = log
                    .map(|path| format!("; details in {}", path.display()))
                    .unwrap_or_default();
                Notice::error(format!(
                    "Winter hit an internal error and carried on: {why}{log}"
                ))
            }
            KeepFailed(error) => {
                Notice::error(format!("keeping a running program failed ({error})"))
            }
            LastShellExited => Notice::info(
                "the last shell exited: the window stays open for what is unsaved or running",
            ),
            MacroDefined => Notice::info("keyboard macro defined"),
            MacroRecorded(register) => Notice::info(format!("macro recorded to @{register}")),
            MacroRecursion => Notice::refusal("macro replay recursion limit reached"),
            MarkupMarkers(markers) => Notice::info(format!(
                "Markup markers: {}",
                match markers {
                    self::MarkupMarkers::Visible => "visible",
                    self::MarkupMarkers::ShowOnCursor => "shown where the cursor is",
                    self::MarkupMarkers::AlwaysHidden => "always hidden",
                }
            )),
            MuxFailed(message) => Notice::error(format!("mux: {message}")),
            MuxKilled(name) => Notice::info(format!("killed mux session '{name}'")),
            MuxStarted(self::MuxStarted {
                cols,
                command,
                name,
                rows,
            }) => Notice::info(match command {
                Some(command) => format!("started '{name}' ({cols}x{rows}): {command}"),
                None => format!("started '{name}' ({cols}x{rows})"),
            }),
            MuxUnreachable => Notice::error("could not connect to mux daemon"),
            NewWindowFailed(error) => Notice::error(format!("failed to spawn new window: {error}")),
            NewWindowNoBinary => Notice::error("new_window: could not determine binary path"),
            NoApprovalStore => Notice::error("there is no state directory to keep approvals in"),
            NoMacro => Notice::refusal("no macro recorded"),
            NoOtherWorkspace => Notice::info("no other workspace to go to"),
            NoPlace => Notice::refusal("no place on this line"),
            NoPreviousCommand => Notice::info("no previous command to repeat"),
            NoRemoteHost(saved) => Notice::error(format!(
                "re-attaching to a remote mux session failed: no host in '{saved}'"
            )),
            NoReportDir => Notice::error("there is no state directory to write to"),
            NoShell(error) => Notice::error(format!("could not start a shell: {error}")),
            NoSuchCommand(TitledCommand { name, title }) => {
                Notice::refusal(format!("{title} has no command {name}"))
            }
            NoViewLeft => Notice::info("the other sidebar holds every view"),
            NoWorkspaceStore => Notice::error("no state directory to keep workspaces in"),
            NoWorkspaces => Notice::info("no workspaces yet: mark a folder as one first"),
            NotInASidebar => Notice::info("only a sidebar's view moves to the other sidebar"),
            NotInstalled(id) => Notice::info(format!("{id} was not installed")),
            NotMux => Notice::info("this pane is not attached to a mux session"),
            NothingClosed => Notice::info(NOTHING_CLOSED),
            NothingRunning => Notice::info("nothing running to stop"),
            NothingToOutline => Notice::info("nothing to outline here"),
            NothingToReplace => Notice::info("nothing to replace"),
            NothingToResume => Notice::info("no picker to resume"),
            NothingToSave => Notice::info("no rows to save"),
            NotificationsCleared => Notice::info("notifications cleared").unlogged(),
            OnlyTab => {
                Notice::info("cannot detach the only tab: close the window or open another tab")
            }
            OutputReset(why) => Notice::error(format!(
                "a terminal's output hit an internal error and its reading was reset: {why}"
            )),
            Profiling(true) => Notice::info(
                "Profiling on: slow moments and freezes are recorded, warnings kept in winter.log",
            ),
            Profiling(false) => Notice::info("Profiling off"),
            RainbowParens(on) => Notice::info(format!("rainbow parens {}", toggle_word(on))),
            ReattachFailed(Failure { error, what }) => Notice::error(format!(
                "re-attaching to mux session '{what}' failed ({error})"
            )),
            ReattachRemoteFailed(RemoteFailure { error, session }) => Notice::error(format!(
                "re-attaching to '{}:{}' failed ({error})",
                session.host, session.session
            )),
            RecordingMacro(register) => Notice::info(format!("recording @{register}")),
            ReloadNoBinary => Notice::error("reload: the path of this binary is not known"),
            ReloadSaveFailed(error) => Notice::error(format!(
                "reload: could not save the session to hand over: {error}"
            )),
            ReloadStartFailed(Failure { error, what }) => {
                Notice::error(format!("reload: could not start {what}: {error}"))
            }
            ReloadWontRun(Failure { error, what }) => {
                Notice::error(format!("reload: {what} does not run: {error}"))
            }
            ReplaceBadPattern(why) => Notice::error(why),
            Replaced(self::Replaced {
                files,
                refused,
                written,
            }) => {
                let mut said = format!(
                    "replaced in {} of {}",
                    lines_count(written),
                    files_count(files)
                );
                if refused > 0 {
                    said.push_str(&format!(
                        ", {} changed since the search, left alone",
                        lines_count(refused)
                    ));
                }
                Notice::info(said)
            }
            RestoreFailed(error) => Notice::error(format!(
                "restoring a pane failed ({error}), trying the default shell"
            )),
            RingCleared => Notice::info("clipboard ring cleared"),
            SecretFailed(reason) => Notice::error(reason),
            SecretForgotten(SecretKey { extension, key }) => {
                Notice::info(format!("{extension}: {key} forgotten"))
            }
            SecretSaved(SecretKey { extension, key }) => {
                Notice::info(format!("{extension}: {key} saved"))
            }
            SentenceHighlight(on) => {
                Notice::info(format!("sentence highlight {}", toggle_word(on)))
            }
            SettingOverridden => {
                Notice::info("saved for every workspace; this one sets its own, kept in force")
            }
            SettingsNotSaved => Notice::error(
                "settings.kdl has errors: this change applies now, but is not saved until the \
                 file is fixed",
            ),
            SettingsSaveFailed(error) => {
                Notice::error(format!("could not save settings.kdl: {error}"))
            }
            ShellFailed(error) => Notice::error(format!(
                "shell failed to start ({error}), trying the default shell"
            )),
            SshFailed(Failure { error, what }) => {
                Notice::error(format!("could not reach '{what}' over ssh ({error})"))
            }
            StageFailed(said) => Notice::error(said),
            Stopping(names) => Notice::info(format!("stopping {}", names.join(", "))),
            TerminalGutter(true) => Notice::info("Terminal gutter: shown"),
            TerminalGutter(false) => Notice::info("Terminal gutter: hidden"),
            ThemeBadName => Notice::refusal("a theme name uses only letters, numbers, - or _"),
            ThemeCreateFailed(error) => Notice::error(error),
            ThemeCreated(name) => Notice::info(format!("created theme \"{name}\"")),
            ThemeProblem(problem) => Notice::error(problem),
            Undefined(keys) => Notice::refusal(format!("{keys} is undefined")),
            WorkspaceDone(said) => Notice::info(said),
            WorkspaceProblem(problem) => Notice::error(problem),
            WorkspaceSaveFailed(Failure { error, what }) => {
                Notice::error(format!("could not save workspace {what}: {error}"))
            }
            WriteFailed(why) => Notice::error(format!("could not write it: {why}")),
        }
    }
}
