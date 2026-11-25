//! Managed agents (see `docs/design/agent-workspace.md`): `Agent: New…`
//! starts an agent on a branch and in a worktree of its own, in a mux
//! session that keeps it running when its tab closes and when Winter quits,
//! and `Agent: List` shows every agent, running or stopped, opening a stopped
//! one again with its resume line. `Agent: Manage` moves an agent started
//! by hand onto a branch of its own (see [`manage`]), and a mux server of
//! another version is restarted once the user says so (see [`server`]).
//!
//! Git and the mux server are asked on a thread of its own, since a worktree
//! can take seconds to check out: it hands back what it found
//! ([`Done`]) and wakes the window, which starts the session and opens its
//! tab. Nothing is kept between runs: the record is rebuilt from the
//! worktrees and the sessions each time (see [`record`]).

mod land;
mod manage;
mod record;
mod server;
mod status;
mod terminals;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use misty::picker::{Row, RowKey};
use winter_base::wake::wake_window;
use winter_model::layout::TabId;

pub(in crate::window) use land::{AgentAction, Allowed};
pub(in crate::window) use record::Agent;
use record::DEFAULT_WORKTREES;
use server::ensure_mux_server;
pub(in crate::window) use status::AgentStates;

use super::App;
use super::notice::WindowNotice;
use super::panels::views::agents::Standing;
use super::picker::sources::listing::ListingAction;
use super::picker::sources::{Entry, Listing};
use super::picker::{AgentRequest, WindowOutcome};
use crate::config::AgentKind;
use crate::control::AgentState;
use misty::picker::ActionId;

// ========================================================================
// Constants
// ========================================================================

/// The thread git and the mux server are asked on.
const THREAD_NAME: &str = "winter agents";

/// How long the mux server has to answer the handshake, or what sessions
/// it holds.
const MUX_WAIT: Duration = Duration::from_millis(500);

/// What git runs with here: never rewriting the index, never asking for a
/// password, and in plain words.
const GIT_ENV: [(&str, &str); 3] = [
    ("GIT_OPTIONAL_LOCKS", "0"),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("LC_ALL", "C"),
];

/// The pickers' ids and titles.
const NEW_PICKER: &str = "agent_new";
const NEW_TITLE: &str = "Agent: run which agent";
const LIST_PICKER: &str = "agent_list";
const LIST_TITLE: &str = "Agents: Enter shows or opens again";

/// What an agent's row says of it, and of a session whose worktree is
/// gone, which `Enter` ends.
const RUNNING: &str = "running";
const STOPPED: &str = "stopped";
const NO_WORKTREE: &str = "worktree gone: Enter ends it";

/// What an agent's row offers besides showing it, in this order.
const ROW_ACTIONS: [ListingAction; 4] = [
    ListingAction {
        button: None,
        id: ActionId("agent_review"),
        title: "Review",
    },
    ListingAction {
        button: None,
        id: ActionId("agent_merge"),
        title: "Merge",
    },
    ListingAction {
        button: None,
        id: ActionId("agent_discard"),
        title: "Discard",
    },
    ListingAction {
        button: None,
        id: ActionId("agent_stop"),
        title: "Stop",
    },
];

// ========================================================================
// Data Structures
// ========================================================================

/// What the agents' thread hands back.
pub(in crate::window) enum Done {
    /// Do the action to the agent found, as far as allowed.
    Act(AgentAction, Agent, Allowed),
    /// Ask the user this before the action goes on with the agent.
    Ask(AgentAction, Agent, String),
    /// What went wrong, to say.
    Failed(String),
    /// The agents found, for `Agent: List`, and the agents' sessions whose
    /// worktrees are gone, left to end.
    Listed(Vec<Agent>, Vec<String>),
    /// The agents found, for the Agents view.
    Viewed(Vec<Agent>),
    /// The mux server answering is of another version of Winter, as the
    /// handshake said: ask to restart it.
    Outdated(String),
    /// The mux server was restarted: open these agents again, typing their
    /// resume lines but in these sessions, which the server started again
    /// itself.
    Resume(Vec<Agent>, Vec<String>),
    /// Open this worktree's work against this commit.
    Review(PathBuf, String),
    /// What was done, to say.
    Said(String),
    /// A mux server the thread started, by its process id, to be reaped
    /// when it ends.
    ServerStarted(u32),
    /// An agent's worktree is ready: start it there.
    Start(Start),
}

/// An agent to start in a session of its own.
pub(in crate::window) struct Start {
    /// Its branch, which names its session.
    branch: String,
    /// How it came to start, which says what to tell the user.
    how: Begun,
    /// The line typed into the session's shell.
    line: String,
    /// The line that starts it again: what the mux server types into the
    /// session's shell when it brings the session back after a restart.
    resume: String,
    /// Its checkout, where the session's shell starts.
    worktree: PathBuf,
}

/// How an agent came to start in its session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Begun {
    /// In a worktree just made for it, by `Agent: New…`.
    Made,
    /// In a worktree just made for an agent started by hand, by
    /// `Agent: Manage`.
    Moved,
    /// In its worktree again, with its resume line.
    Opened,
}

/// Where the agents' thread hands back what it found.
pub(in crate::window) struct Inbox {
    /// The action waiting on the user's answer, with its agent.
    asked: Option<land::Asked>,
    rx: Receiver<Done>,
    tx: Sender<Done>,
}

/// What stops the agents' thread short of its work.
pub(in crate::window) enum Halt {
    /// What went wrong, to say.
    Failed(String),
    /// The mux server answering is of another version of Winter, as the
    /// handshake said.
    Outdated(String),
}

impl From<String> for Halt {
    /// What went wrong, as the thread's work says it.
    fn from(why: String) -> Self {
        Halt::Failed(why)
    }
}

impl From<Halt> for Done {
    /// What the window is handed when the work stopped short.
    fn from(halt: Halt) -> Self {
        match halt {
            Halt::Failed(why) => Done::Failed(why),
            Halt::Outdated(why) => Done::Outdated(why),
        }
    }
}

impl Default for Inbox {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            asked: None,
            rx,
            tx,
        }
    }
}

// ========================================================================
// App: agents
// ========================================================================

impl App {
    /// `Agent: New…`: which agent to start, of those `settings.kdl` offers,
    /// in the repository the focused tab is in.
    pub(in crate::window) fn open_agent_new(&mut self) {
        let mut listing = Listing::new(NEW_PICKER, NEW_TITLE).placeholder("Agent");
        for kind in &self.config.agents.agents {
            let row = Row::new(RowKey(0), kind.name.as_str()).with_hint(kind.command.as_str());
            let request = AgentRequest::New(kind.name.clone());
            listing.push(Entry::new(row, WindowOutcome::Agent(request)));
        }
        self.open_picker(Box::new(listing));
    }

    /// `Agent: List`: every agent of the repository the focused tab is in,
    /// once git and the mux server have said.
    pub(in crate::window) fn open_agent_list(&mut self) {
        let dir = self.focused_start_dir();
        self.on_agent_thread(move |inbox| {
            ensure_mux_server(inbox)?;
            let sessions = mux_sessions().unwrap_or_default();
            let agents = list_agents(&dir, &sessions)?;
            let left = record::sessions_left(&agents, &sessions);
            Ok(Done::Listed(agents, left))
        });
    }

    /// Do what an agents picker asked.
    pub(in crate::window) fn carry_out_agent(&mut self, request: AgentRequest, focused: TabId) {
        match request {
            AgentRequest::New(name) => {
                let Some(kind) = self.agent_kind(&name) else {
                    return;
                };
                let dir = self.focused_start_dir();
                let template = self.worktrees_template();
                self.on_agent_thread(move |inbox| {
                    ensure_mux_server(inbox)?;
                    Ok(Done::Start(make_agent(&dir, &kind, &template)?))
                });
            }
            AgentRequest::Open(agent) => self.open_agent(agent, focused),
            AgentRequest::Act(action, agent) => {
                self.act_on_agent(action, agent, Allowed::default())
            }
            AgentRequest::EndSession(session) => self.end_agent_session(session),
            AgentRequest::ShowTab(tab) => self.reveal_tab(tab),
        }
    }

    /// Act on what the agents' thread handed back since the last look;
    /// whether anything changed on screen.
    pub(in crate::window) fn take_agent_work(&mut self) -> bool {
        let done: Vec<Done> = self.runtime.agents.rx.try_iter().collect();
        let any = !done.is_empty();
        for done in done {
            let focused = self.layout().focused();
            match done {
                Done::Act(action, agent, allowed) => self.act_on_agent(action, agent, allowed),
                Done::Ask(action, agent, question) => self.ask_about_agent(action, agent, question),
                Done::Failed(why) => self.notify(WindowNotice::AgentFailed(why)),
                Done::Review(root, commit) => self.show_agent_review(root, commit),
                Done::Said(what) => {
                    self.notify(WindowNotice::AgentDone(what));
                    self.refresh_agents_view();
                }
                Done::Listed(agents, left) => self.show_agent_list(agents, left),
                Done::Outdated(why) => self.ask_to_restart_server(why),
                Done::Resume(agents, resumed) => self.resume_agents(agents, &resumed, focused),
                Done::Viewed(agents) => self.fill_agents_view(agents),
                Done::ServerStarted(pid) => self.runtime.strays.push(pid),
                Done::Start(start) => self.start_agent(start, focused),
            }
        }
        any
    }

    /// Name the tab attached to `session` after the agent's `branch`, which
    /// says both which agent it is and where it works; the tab, if any.
    fn name_agent_tab(&mut self, session: &str, branch: &str) -> Option<TabId> {
        let tab = self
            .open_tabs
            .terminals()
            .find(|(_, terminal)| terminal.mux_session() == Some(session))
            .map(|(tab, _)| *tab)?;
        self.tabs.set_name(tab, Some(branch.to_string()));
        Some(tab)
    }

    /// Look for the agents of the project the window works in, for the
    /// Agents view.
    pub(in crate::window) fn look_for_agents_view(&mut self) {
        let dir = self.focused_start_dir();
        self.on_agent_thread(move |_| {
            let sessions = mux_sessions().unwrap_or_default();
            // Outside a repository there are no managed agents, which is
            // nothing to say: the view still lists the terminals.
            Ok(Done::Viewed(
                list_agents(&dir, &sessions).unwrap_or_default(),
            ))
        });
    }

    /// How `agent` stands, as the Agents view shows it: as its tab last
    /// said, else as its session is.
    pub(in crate::window) fn agent_standing(&self, agent: &Agent) -> Standing {
        let session = agent.session();
        let state = self
            .open_tabs
            .terminals()
            .filter(|(_, terminal)| terminal.mux_session() == Some(session.as_str()))
            .find_map(|(tab, _)| self.tabs.agents.of(*tab));
        match (state, agent.running) {
            (Some(AgentState::Working), _) => Standing::Working,
            (Some(AgentState::Waiting), _) => Standing::Waiting,
            (_, true) => Standing::Running,
            (_, false) => Standing::Stopped,
        }
    }

    /// The tab attached to `agent`'s session, shown or buried.
    pub(in crate::window) fn agent_tab(&self, agent: &Agent) -> Option<TabId> {
        let session = agent.session();
        self.open_tabs
            .terminals()
            .find(|(_, terminal)| terminal.mux_session() == Some(session.as_str()))
            .map(|(tab, _)| *tab)
    }

    /// The agent `settings.kdl` calls `name`.
    fn agent_kind(&self, name: &str) -> Option<AgentKind> {
        self.config
            .agents
            .agents
            .iter()
            .find(|kind| kind.name == name)
            .cloned()
    }

    /// Where `settings.kdl` places agents' worktrees.
    fn worktrees_template(&self) -> String {
        self.config
            .agents
            .worktrees
            .clone()
            .unwrap_or_else(|| DEFAULT_WORKTREES.to_string())
    }

    /// Run `work` on the agents' thread, handing back what it comes to, or
    /// what stopped it: what went wrong, or a question to ask.
    fn on_agent_thread(
        &self,
        work: impl FnOnce(&Sender<Done>) -> Result<Done, Halt> + Send + 'static,
    ) {
        let inbox = self.runtime.agents.tx.clone();
        let spawned = thread::Builder::new()
            .name(THREAD_NAME.to_string())
            .spawn(move || {
                let done = work(&inbox).unwrap_or_else(Done::from);
                let _ = inbox.send(done);
                wake_window();
            });
        if let Err(error) = spawned {
            let _ = self
                .runtime
                .agents
                .tx
                .send(Done::Failed(format!("could not start its thread: {error}")));
        }
    }

    /// The picker of `agents`, each with its state at the right.
    fn show_agent_list(&mut self, agents: Vec<Agent>, left: Vec<String>) {
        let mut listing = Listing::new(LIST_PICKER, LIST_TITLE)
            .placeholder("Agent")
            .resume(misty::picker::Resume::Never)
            .extra_actions(&ROW_ACTIONS);
        if agents.is_empty() && left.is_empty() {
            let row = Row::new(RowKey(0), "(no agents: start one with Agent: New…)");
            listing.push(Entry::read_only(row));
        }
        for agent in agents {
            let state = match agent.running {
                true => RUNNING,
                false => STOPPED,
            };
            let row = Row::new(RowKey(0), agent.branch.as_str()).with_hint(state);
            let act = |action| {
                Some(WindowOutcome::Agent(AgentRequest::Act(
                    action,
                    agent.clone(),
                )))
            };
            let actions = vec![
                act(AgentAction::Review),
                act(AgentAction::Merge),
                act(AgentAction::Discard),
                agent.running.then(|| act(AgentAction::Stop)).flatten(),
            ];
            listing.push(
                Entry::new(row, WindowOutcome::Agent(AgentRequest::Open(agent)))
                    .with_actions(actions),
            );
        }
        // A session whose worktree was removed outside Winter is offered for
        // ending, rather than left running unseen.
        for session in left {
            let row = Row::new(RowKey(0), session.as_str()).with_hint(NO_WORKTREE);
            let end = WindowOutcome::Agent(AgentRequest::EndSession(session));
            listing.push(Entry::new(row, end));
        }
        self.open_picker(Box::new(listing));
    }

    /// Show `agent`: the tab attached to its session, buried or not; a new
    /// tab on its session while it runs; else a session started again in
    /// its worktree, with its resume line.
    fn open_agent(&mut self, agent: Agent, focused: TabId) {
        let session = agent.session();
        let attached = self
            .open_tabs
            .terminals()
            .find(|(_, terminal)| terminal.mux_session() == Some(session.as_str()))
            .map(|(tab, _)| *tab);
        if let Some(tab) = attached {
            self.show_tab(tab);
            return;
        }
        if agent.running {
            self.new_mux_tab(&session);
            self.name_agent_tab(&session, &agent.branch);
            return;
        }
        let Some(kind) = self.agent_kind(&agent.kind) else {
            self.notify(WindowNotice::AgentFailed(format!(
                "settings.kdl offers no agent called {}",
                agent.kind
            )));
            return;
        };
        let line = kind.resume.unwrap_or(kind.command);
        let start = Start {
            branch: agent.branch,
            how: Begun::Opened,
            line: line.clone(),
            resume: line,
            worktree: agent.worktree,
        };
        self.start_agent(start, focused);
    }

    /// Start a session for `start` in its worktree, attach a tab to it, and
    /// type its line into the session's shell, so the shell and the
    /// agent's last words outlive the agent.
    fn start_agent(&mut self, start: Start, focused: TabId) {
        let session = record::session_name(&start.branch);
        let worktree = start.worktree.to_string_lossy();
        self.spawn_mux_tab_at(
            &winter_mux::server::default_socket_path(),
            &session,
            None,
            Some(&worktree),
            focused,
        );
        let Some(tab) = self.name_agent_tab(&session, &start.branch) else {
            return;
        };
        self.type_agent_line(tab, &start.line);
        keep_resume_line(&session, &start.resume);
        match start.how {
            Begun::Made => self.notify(WindowNotice::AgentStarted(start.branch)),
            Begun::Moved => self.notify(WindowNotice::AgentMoved(start.branch)),
            Begun::Opened => {}
        }
        self.refresh_agents_view();
    }

    /// Type `line` into the shell of the agent's terminal `tab`, starting
    /// the agent there.
    fn type_agent_line(&mut self, tab: TabId, line: &str) {
        if let Some(terminal) = self.open_tabs.terminal_mut(&tab) {
            terminal.write(format!("{line}\r").as_bytes());
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Make a worktree for a new `kind` agent in the repository `dir` is in, on
/// its own branch from the current one, where `template` places worktrees.
fn make_agent(dir: &Path, kind: &AgentKind, template: &str) -> Result<Start, String> {
    let repo = repo_root(dir)?;
    let listed = git(&repo, &["branch", "--format=%(refname:short)", "--list"])?;
    let taken: Vec<String> = listed.lines().map(str::to_string).collect();
    let branch = record::next_branch(&kind.name, &taken);
    let worktree = record::worktrees_dir(&repo, template).join(&branch);
    let place = worktree.to_string_lossy();
    // From where the user is: the branch checked out in `dir`, or its
    // commit when none is, which the agent lands on later.
    let start = git(dir, &["rev-parse", "HEAD"])?.trim().to_string();
    let base = match git(dir, &["rev-parse", "--abbrev-ref", "HEAD"])?.trim() {
        "HEAD" => start.clone(),
        branch => branch.to_string(),
    };
    git(&repo, &["worktree", "add", "-b", &branch, &place, &start])?;
    let key = format!("branch.{branch}.{}", record::BASE_KEY);
    git(&repo, &["config", &key, &base])?;
    Ok(Start {
        branch,
        how: Begun::Made,
        line: kind.command.clone(),
        resume: kind.resume.clone().unwrap_or_else(|| kind.command.clone()),
        worktree,
    })
}

/// The agents of the repository `dir` is in, each running when `sessions`
/// (the mux server's session names) holds its session.
fn list_agents(dir: &Path, sessions: &[String]) -> Result<Vec<Agent>, String> {
    let repo = repo_root(dir)?;
    let porcelain = git(&repo, &["worktree", "list", "--porcelain"])?;
    let worktrees = record::parse_worktrees(&porcelain);
    // None kept is an empty answer, not a failure.
    let pattern = format!("^branch\\..*\\.{}$", record::BASE_KEY.to_ascii_lowercase());
    let bases = git(&repo, &["config", "--get-regexp", &pattern]).unwrap_or_default();
    Ok(record::agents_of(
        &worktrees,
        sessions,
        &record::parse_bases(&bases),
    ))
}

/// The top of the repository `dir` is in: the main checkout's, when `dir`
/// is in one of its worktrees, so agents are made beside the repository
/// rather than inside another agent's.
fn repo_root(dir: &Path) -> Result<PathBuf, String> {
    let common = git(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .map_err(|_| format!("{} is in no git repository", dir.display()))?;
    let common = PathBuf::from(common.trim());
    match common.file_name().is_some_and(|name| name == ".git") {
        true => common
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| format!("{} has no repository above it", common.display())),
        false => git(dir, &["rev-parse", "--show-toplevel"]).map(|top| PathBuf::from(top.trim())),
    }
}

/// What `git` with `args` prints in `dir`, or what it said went wrong.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = winter_base::process::command("git")
        .args(args)
        .current_dir(dir)
        .envs(GIT_ENV)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("git could not run: {error}"))?;
    match output.status.success() {
        true => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
        false => {
            let said = String::from_utf8_lossy(&output.stderr);
            let said = said.trim();
            Err(match said.is_empty() {
                true => format!("git {} failed", args.first().unwrap_or(&"")),
                false => said.to_string(),
            })
        }
    }
}

/// Have the mux server type `line` into `session`'s shell whenever it brings
/// the session back after a restart, so the agent comes back with it rather
/// than a bare shell. A server that cannot be reached keeps nothing, and the
/// agent opens again from `Agent: List` as before.
fn keep_resume_line(session: &str, line: &str) {
    let socket = winter_mux::server::default_socket_path();
    let Ok(mut client) = winter_mux::client::MuxClient::connect(&socket) else {
        return;
    };
    if client.set_resume(session, line).is_ok() {
        // The server acts on it before it answers this, so it is not lost
        // when the connection closes at once.
        let _ = client.query_sessions(MUX_WAIT);
    }
}

/// The names of the mux server's sessions.
fn mux_sessions() -> Result<Vec<String>, String> {
    let socket = winter_mux::server::default_socket_path();
    let mut client =
        winter_mux::client::MuxClient::connect(&socket).map_err(|error| error.to_string())?;
    let sessions = client
        .query_sessions(MUX_WAIT)
        .map_err(|error| error.to_string())?;
    Ok(sessions.into_iter().map(|session| session.name).collect())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository at a fresh temporary `name`, with one commit.
    pub(super) fn repository(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("winter-agents-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(dir.with_extension("worktrees"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        for args in [
            &["init", "-q", "-b", "main"][..],
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "base",
            ],
        ] {
            git(&dir, args).expect("git runs");
        }
        dir
    }

    /// Claude Code, as `settings.kdl` offers it.
    pub(super) fn claude() -> AgentKind {
        AgentKind {
            command: "claude".to_string(),
            name: "claude".to_string(),
            resume: Some("claude --continue".to_string()),
        }
    }

    #[test]
    fn test_a_new_agent_gets_its_own_branch_and_worktree_beside_the_repository() {
        let repo = repository("new");
        let kind = claude();
        let first = make_agent(&repo, &kind, DEFAULT_WORKTREES).expect("made");
        let second = make_agent(&first.worktree, &kind, DEFAULT_WORKTREES).expect("made");
        let found = list_agents(&repo, &["agent-claude-2".to_string()]).expect("listed");
        let _ = std::fs::remove_dir_all(repo.with_extension("worktrees"));
        let _ = std::fs::remove_dir_all(&repo);
        assert_eq!(
            (first.branch.as_str(), first.line.as_str()),
            ("agent/claude-1", "claude")
        );
        assert_eq!(
            first.worktree,
            repo.with_extension("worktrees").join("agent/claude-1"),
            "beside the repository"
        );
        assert_eq!(
            second.branch, "agent/claude-2",
            "from inside an agent, still beside the repository"
        );
        let seen: Vec<(&str, bool)> = found
            .iter()
            .map(|agent| (agent.branch.as_str(), agent.running))
            .collect();
        assert_eq!(seen, [("agent/claude-1", false), ("agent/claude-2", true)]);
        assert!(found.iter().all(|agent| agent.kind == "claude"));
    }

    #[test]
    fn test_outside_a_repository_it_says_so() {
        let dir = std::env::temp_dir().join(format!("winter-agents-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let said = make_agent(&dir, &AgentKind::default(), DEFAULT_WORKTREES).err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(said.is_some_and(|why| why.contains("no git repository")));
    }
}
