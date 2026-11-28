//! Reviewing and landing a managed agent's work, and ending it (see
//! `docs/design/agent-workspace.md`, "Review and land" and "Lifecycle"):
//!
//! - Review opens the agent's work against the branch it started from: its
//!   commits and what its worktree has not committed.
//! - Merge rebases the agent's branch onto that branch inside its worktree,
//!   so a conflict is met there and never in the main checkout, fast-forwards
//!   the branch to it, and removes the worktree, the branch, and the session.
//! - Discard removes them without landing anything.
//! - Stop ends the agent's session and keeps its worktree, so it can be
//!   opened again; Stop All ends every agent's.
//!
//! What may lose something asks first: ending an agent still at work, a
//! merge while the main checkout has changes of its own, a discard of work
//! not on the base branch. Git runs on the agents' thread; the questions are
//! the window's.

use std::path::{Path, PathBuf};

use misty::Confirmation;
use winter_model::editor::GitViewRequest;
use winter_vcs::exec::{self, Rev};

use super::super::App;
use super::super::editor::closing::PendingClose;
use super::super::notice::WindowNotice;
use super::record::{self, Agent};
use super::{Done, git, mux_sessions, repo_root};
use crate::control::AgentState;

// ========================================================================
// Data Structures
// ========================================================================

/// What can be done to a managed agent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) enum AgentAction {
    /// Remove its worktree, branch, and session, landing nothing.
    Discard,
    /// Land its branch on the one it started from, then remove it.
    Merge,
    /// Open its work against the branch it started from.
    Review,
    /// End its session, keeping its worktree.
    Stop,
}

/// What waits on the user's yes.
#[derive(Clone, Debug)]
pub(in crate::window) enum Asked {
    /// An action, with the agent it is for.
    Act(AgentAction, Agent),
    /// Restarting a mux server of another version, then opening again the
    /// agents of the repository this directory is in.
    Restart(PathBuf),
}

/// How far an action has been allowed: what has been asked and answered
/// yes, so it is not asked again on the way.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::window) struct Allowed {
    /// Its changes may be thrown away, or the main checkout's left as they
    /// are under a merge.
    changes: bool,
    /// The agent may be ended though it is at work.
    working: bool,
}

// ========================================================================
// App: landing
// ========================================================================

impl App {
    /// Do `action` to the agent the focused tab runs, found by its session
    /// once git says where it works.
    pub(in crate::window) fn act_on_focused_agent(&mut self, action: AgentAction) {
        let focused = self.layout().focused();
        let session = self
            .open_tabs
            .terminal(&focused)
            .and_then(|terminal| terminal.mux_session().map(str::to_string));
        let Some(branch) = session.as_deref().and_then(record::branch_of_session) else {
            self.notify(WindowNotice::AgentFailed(
                "this tab runs no agent Agent: New… started: Agent: List shows them".to_string(),
            ));
            return;
        };
        let dir = self.focused_start_dir();
        self.on_agent_thread(move |_| {
            let sessions = mux_sessions().unwrap_or_default();
            let agent = super::list_agents(&dir, &sessions)?
                .into_iter()
                .find(|agent| agent.branch == branch)
                .ok_or_else(|| format!("no worktree is on {branch} any more"))?;
            Ok(Done::Act(action, agent, Allowed::default()))
        });
    }

    /// Do `action` to `agent`, as far as `allowed` lets it without asking.
    pub(in crate::window) fn act_on_agent(
        &mut self,
        action: AgentAction,
        agent: Agent,
        allowed: Allowed,
    ) {
        let ends = matches!(action, AgentAction::Merge | AgentAction::Discard);
        if ends && !allowed.working && self.agent_working(&agent) {
            let question = format!("{} is still working. End it?", agent.branch);
            self.ask_about_agent(action, agent, question);
            return;
        }
        match action {
            AgentAction::Review => self.on_agent_thread(move |_| Ok(review(&agent)?)),
            AgentAction::Stop => {
                let session = agent.session();
                self.on_agent_thread(move |_| {
                    end_sessions(&[session])?;
                    Ok(Done::Said(format!(
                        "stopped {}; its worktree stays",
                        agent.branch
                    )))
                });
            }
            AgentAction::Merge => self.on_agent_thread(move |_| Ok(merge(agent, allowed)?)),
            AgentAction::Discard => self.on_agent_thread(move |_| Ok(discard(agent, allowed)?)),
        }
    }

    /// `Agent: Stop All`: end every agent's session, keeping the worktrees.
    pub(in crate::window) fn stop_all_agents(&mut self) {
        self.on_agent_thread(|_| {
            let agents: Vec<String> = mux_sessions()?
                .into_iter()
                .filter(|session| record::branch_of_session(session).is_some())
                .collect();
            end_sessions(&agents)?;
            Ok(Done::Said(match agents.len() {
                0 => "no agent was running".to_string(),
                1 => "stopped 1 agent; its worktree stays".to_string(),
                n => format!("stopped {n} agents; their worktrees stay"),
            }))
        });
    }

    /// The window is quitting: end the agents running in its tabs where
    /// `settings.kdl` says so, else say how many keep running, on the
    /// desktop, since the window is going.
    pub(in crate::window) fn leave_agents(
        &mut self,
        cx: &misty::AppContext<'_, crate::window::WindowMessage>,
    ) {
        let sessions: Vec<String> = self
            .open_tabs
            .terminals()
            .filter_map(|(_, terminal)| terminal.mux_session())
            .filter(|session| record::branch_of_session(session).is_some())
            .map(str::to_string)
            .collect();
        if sessions.is_empty() {
            return;
        }
        if self.config.agents.stop_on_quit {
            let _ = end_sessions(&sessions);
            return;
        }
        let body = match sessions.len() {
            1 => "1 agent keeps running: Agent: List shows it".to_string(),
            n => format!("{n} agents keep running: Agent: List shows them"),
        };
        let _ = misty::Notification::new("Winter")
            .with_body(body)
            .show(cx, |_| crate::window::WindowMessage::Nothing);
    }

    /// End the mux session `session`, whose agent's worktree is gone: what
    /// `Agent: List` offers for a session left behind.
    pub(in crate::window) fn end_agent_session(&mut self, session: String) {
        self.on_agent_thread(move |_| {
            end_sessions(std::slice::from_ref(&session))?;
            Ok(Done::Said(format!("ended {session}")))
        });
    }

    /// Ask the user `question` before `action` goes on with `agent`.
    pub(in crate::window) fn ask_about_agent(
        &mut self,
        action: AgentAction,
        agent: Agent,
        question: String,
    ) {
        let asked = match action {
            AgentAction::Discard => {
                Confirmation::destructive("Discard Agent?", question, "Discard")
            }
            AgentAction::Merge => Confirmation::new("Merge Agent?", question, "Merge"),
            AgentAction::Review | AgentAction::Stop => {
                Confirmation::new("Go On with Agent?", question, "Go On")
            }
        };
        self.runtime.agents.asked = Some(Asked::Act(action, agent));
        self.confirm_for_window(asked, PendingClose::Agent);
    }

    /// The user said yes to the question about an agent: go on, allowed
    /// what was asked, which is everything asked so far.
    pub(in crate::window) fn answer_agent_question(&mut self) {
        match self.runtime.agents.asked.take() {
            Some(Asked::Act(action, agent)) => {
                let allowed = Allowed {
                    changes: true,
                    working: true,
                };
                self.act_on_agent(action, agent, allowed);
            }
            Some(Asked::Restart(dir)) => self.restart_mux_server(dir),
            None => {}
        }
    }

    /// Open `root`'s work against `commit` in a tab of its own.
    pub(in crate::window) fn show_agent_review(&mut self, root: PathBuf, commit: String) {
        let Ok(rev) = Rev::parse(&commit) else {
            return;
        };
        self.open_git_view(GitViewRequest {
            git_dir: None,
            request: exec::diff_all(&root, false, Some(&rev)),
            root,
        });
    }

    /// Whether `agent` is at work, as the tab on its session last said.
    fn agent_working(&self, agent: &Agent) -> bool {
        let session = agent.session();
        self.open_tabs
            .terminals()
            .filter(|(_, terminal)| terminal.mux_session() == Some(session.as_str()))
            .any(|(tab, _)| self.tabs.agents.of(*tab) == Some(AgentState::Working))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What `agent` did, from where its branch left the one it started from,
/// for the window to open.
fn review(agent: &Agent) -> Result<Done, String> {
    let base = base_of(agent)?;
    let fork = git(&agent.worktree, &["merge-base", base, "HEAD"])?;
    Ok(Done::Review(
        agent.worktree.clone(),
        fork.trim().to_string(),
    ))
}

/// Land `agent`'s branch on the one it started from, then remove it: asked
/// back first when the main checkout has changes of its own.
fn merge(agent: Agent, allowed: Allowed) -> Result<Done, String> {
    let base = base_of(&agent)?.to_string();
    if is_dirty(&agent.worktree)? {
        return Err(format!(
            "{} has changes it has not committed: have the agent commit them, or Discard it",
            agent.branch
        ));
    }
    let repo = repo_root(&agent.worktree)?;
    let checked_out = git(&repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let lands_here = checked_out.trim() == base;
    if lands_here && !allowed.changes && is_dirty(&repo)? {
        let question = format!(
            "{} has changes of its own. Merge {} into {base} under them?",
            repo.display(),
            agent.branch
        );
        return Ok(Done::Ask(AgentAction::Merge, agent, question));
    }
    // A conflict stops here, in the agent's worktree, where it or the user
    // resolves it; the main checkout is never left half-merged.
    if let Err(why) = git(&agent.worktree, &["rebase", &base]) {
        return Err(format!(
            "rebasing {} onto {base} stopped ({why}): resolve it in {}, then Merge again",
            agent.branch,
            agent.worktree.display()
        ));
    }
    match lands_here {
        true => git(&repo, &["merge", "--ff-only", &agent.branch])?,
        false => git(&repo, &["fetch", ".", &format!("{}:{base}", agent.branch)])?,
    };
    remove(&repo, &agent, false)?;
    Ok(Done::Said(format!("merged {} into {base}", agent.branch)))
}

/// Remove `agent` without landing it: asked back first when it holds work
/// not on the branch it started from.
fn discard(agent: Agent, allowed: Allowed) -> Result<Done, String> {
    let repo = repo_root(&agent.worktree)?;
    if !allowed.changes && holds_work(&agent)? {
        let question = format!(
            "{} holds work that is on no other branch. Throw it away?",
            agent.branch
        );
        return Ok(Done::Ask(AgentAction::Discard, agent, question));
    }
    remove(&repo, &agent, true)?;
    Ok(Done::Said(format!("discarded {}", agent.branch)))
}

/// End `agent`'s session, then remove its worktree and branch from `repo`,
/// with what is in them when `force`.
fn remove(repo: &Path, agent: &Agent, force: bool) -> Result<(), String> {
    end_sessions(&[agent.session()]).ok();
    let place = agent.worktree.to_string_lossy();
    let mut args = vec!["worktree", "remove", &place];
    if force {
        args.push("--force");
    }
    git(repo, &args)?;
    git(repo, &["branch", "-D", &agent.branch])?;
    Ok(())
}

/// The branch `agent` started from, or why it is not known.
fn base_of(agent: &Agent) -> Result<&str, String> {
    agent.base.as_deref().ok_or_else(|| {
        format!(
            "{} does not say which branch it started from: merge it with git",
            agent.branch
        )
    })
}

/// Whether the checkout at `dir` has changes it has not committed.
fn is_dirty(dir: &Path) -> Result<bool, String> {
    Ok(!git(dir, &["status", "--porcelain"])?.trim().is_empty())
}

/// Whether `agent` holds work a discard would lose: changes it has not
/// committed, or commits its base does not have.
fn holds_work(agent: &Agent) -> Result<bool, String> {
    if is_dirty(&agent.worktree)? {
        return Ok(true);
    }
    let Some(base) = agent.base.as_deref() else {
        return Ok(true);
    };
    let ahead = git(
        &agent.worktree,
        &["rev-list", "--count", &format!("{base}..HEAD")],
    )?;
    Ok(ahead.trim() != "0")
}

/// End the mux sessions `sessions`.
fn end_sessions(sessions: &[String]) -> Result<(), String> {
    if sessions.is_empty() {
        return Ok(());
    }
    let socket = winter_mux::server::default_socket_path();
    let mut client =
        winter_mux::client::MuxClient::connect(&socket).map_err(|error| error.to_string())?;
    for session in sessions {
        client.kill(session).map_err(|error| error.to_string())?;
    }
    // The server acts on them before it answers this, so they are not lost
    // when the connection closes at once.
    client
        .query_sessions(super::MUX_WAIT)
        .map_err(|error| error.to_string())?;
    Ok(())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::tests::{claude, repository};
    use super::super::{list_agents, make_agent};
    use super::*;

    /// The agent `make_agent` made from `repo`, as the list finds it.
    fn found(repo: &Path) -> Agent {
        list_agents(repo, &[]).expect("listed").remove(0)
    }

    /// Commit everything in `dir` as `message`.
    fn commit(dir: &Path, message: &str) {
        for args in [
            &["add", "-A"][..],
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                message,
            ],
        ] {
            git(dir, args).expect("git runs");
        }
    }

    #[test]
    fn test_a_merge_lands_the_agent_s_commits_and_removes_it() {
        let repo = repository("merge");
        let start = make_agent(&repo, &claude(), record::DEFAULT_WORKTREES).expect("made");
        std::fs::write(start.worktree.join("work.txt"), "done\n").expect("write");
        commit(&start.worktree, "the agent's work");
        let agent = found(&repo);
        assert_eq!(agent.base.as_deref(), Some("main"));
        let said = merge(agent, Allowed::default());
        let landed = repo.join("work.txt").exists();
        let left = list_agents(&repo, &[]).expect("listed");
        let _ = std::fs::remove_dir_all(repo.with_extension("worktrees"));
        let _ = std::fs::remove_dir_all(&repo);
        assert!(matches!(said, Ok(Done::Said(_))), "merged");
        assert!(landed, "on main, in the main checkout");
        assert!(left.is_empty(), "worktree and branch gone");
    }

    #[test]
    fn test_a_merge_refuses_uncommitted_work_and_asks_over_the_main_checkout_s() {
        let repo = repository("dirty");
        let start = make_agent(&repo, &claude(), record::DEFAULT_WORKTREES).expect("made");
        std::fs::write(start.worktree.join("draft.txt"), "wip\n").expect("write");
        let refused = merge(found(&repo), Allowed::default());
        commit(&start.worktree, "the agent's work");
        std::fs::write(repo.join("mine.txt"), "mine\n").expect("write");
        git(&repo, &["add", "mine.txt"]).expect("git runs");
        let asked = merge(found(&repo), Allowed::default());
        let _ = std::fs::remove_dir_all(repo.with_extension("worktrees"));
        let _ = std::fs::remove_dir_all(&repo);
        assert!(refused.is_err_and(|why| why.contains("not committed")));
        assert!(
            matches!(asked, Ok(Done::Ask(AgentAction::Merge, ..))),
            "the user's own changes"
        );
    }

    #[test]
    fn test_a_discard_asks_over_work_on_no_other_branch_and_then_removes_it() {
        let repo = repository("discard");
        let start = make_agent(&repo, &claude(), record::DEFAULT_WORKTREES).expect("made");
        let quiet = discard(found(&repo), Allowed::default());
        let again = make_agent(&repo, &claude(), record::DEFAULT_WORKTREES).expect("made");
        std::fs::write(again.worktree.join("work.txt"), "done\n").expect("write");
        let asked = discard(
            list_agents(&repo, &[]).expect("listed").remove(0),
            Allowed::default(),
        );
        let forced = discard(
            list_agents(&repo, &[]).expect("listed").remove(0),
            Allowed {
                changes: true,
                working: true,
            },
        );
        let left = list_agents(&repo, &[]).expect("listed");
        let _ = std::fs::remove_dir_all(repo.with_extension("worktrees"));
        let _ = std::fs::remove_dir_all(&repo);
        assert!(!start.worktree.exists(), "nothing of its own: gone at once");
        assert!(matches!(quiet, Ok(Done::Said(_))));
        assert!(matches!(asked, Ok(Done::Ask(AgentAction::Discard, ..))));
        assert!(matches!(forced, Ok(Done::Said(_))));
        assert!(left.is_empty());
    }

    #[test]
    fn test_a_review_compares_from_where_the_agent_s_branch_left_its_base() {
        let repo = repository("review");
        let start = make_agent(&repo, &claude(), record::DEFAULT_WORKTREES).expect("made");
        let fork = git(&repo, &["rev-parse", "HEAD"]).expect("git runs");
        let shown = review(&found(&repo));
        let _ = std::fs::remove_dir_all(repo.with_extension("worktrees"));
        let _ = std::fs::remove_dir_all(&repo);
        assert!(
            matches!(shown, Ok(Done::Review(root, commit)) if root == start.worktree && commit == fork.trim())
        );
    }
}
