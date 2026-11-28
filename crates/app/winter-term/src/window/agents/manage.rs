//! `Agent: Manage` (see `docs/design/agent-workspace.md`, "Moving a
//! detected agent"): an agent started by hand in a plain terminal moves onto
//! a branch, a worktree, and a mux session of its own. A running program
//! cannot move into the mux server, so the agent is ended and started again
//! there with its resume line; an agent that keeps its conversations by
//! folder, as Claude Code does, starts a fresh one. Uncommitted edits stay
//! in the checkout it ran in.

use std::path::PathBuf;

use winter_model::layout::TabId;

use super::super::App;
use super::super::notice::WindowNotice;
use super::{Begun, Done, ensure_mux_server, make_agent, record};
use crate::config::AgentKind;
use crate::control::AgentState;

// ========================================================================
// Data Structures
// ========================================================================

/// An agent started by hand, to move onto a branch of its own.
struct Moving {
    /// Where it runs, whose repository its worktree is made from.
    dir: PathBuf,
    /// Its process group, ended once the worktree is made.
    group: i32,
    /// Which of the agents `settings.kdl` offers it is.
    kind: AgentKind,
}

// ========================================================================
// App: moving an agent
// ========================================================================

impl App {
    /// `Agent: Manage`: move the agent the focused terminal runs onto a
    /// branch, a worktree, and a mux session of its own.
    pub(in crate::window) fn manage_focused_agent(&mut self) {
        let focused = self.layout().focused();
        let moving = match self.agent_to_manage(focused) {
            Ok(moving) => moving,
            Err(why) => {
                self.notify(WindowNotice::AgentFailed(why));
                return;
            }
        };
        let template = self.worktrees_template();
        self.on_agent_thread(move |inbox| {
            ensure_mux_server(inbox)?;
            // The worktree first, so an agent that cannot move runs on.
            let mut start = make_agent(&moving.dir, &moving.kind, &template)?;
            start.line = start.resume.clone();
            start.how = Begun::Moved;
            end_group(moving.group);
            Ok(Done::Start(start))
        });
    }

    /// What `Agent: Manage` would move from `tab`, or why it cannot: an
    /// agent at work is left to finish its turn, since ending it would cut
    /// the turn off.
    fn agent_to_manage(&self, tab: TabId) -> Result<Moving, String> {
        let terminal = self
            .open_tabs
            .terminal(&tab)
            .ok_or_else(|| "this tab is no terminal".to_string())?;
        if terminal
            .mux_session()
            .is_some_and(|session| record::branch_of_session(session).is_some())
        {
            return Err("this agent has a branch of its own already".to_string());
        }
        let programs = terminal.foreground_programs();
        let (group, kind) = terminal
            .foreground_group()
            .zip(self.agent_kind_in(&programs))
            .ok_or_else(|| "this tab runs no agent settings.kdl offers".to_string())?;
        if self.tabs.agents.of(tab) == Some(AgentState::Working) {
            return Err(format!(
                "{} is working: move it once it waits for you",
                kind.name
            ));
        }
        let dir = terminal
            .cwd()
            .map(PathBuf::from)
            .ok_or_else(|| "cannot tell which folder the agent runs in".to_string())?;
        Ok(Moving {
            dir,
            group,
            kind: kind.clone(),
        })
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Ask the agent's process group `group` to end, leaving its terminal at
/// the shell.
#[cfg(unix)]
fn end_group(group: i32) {
    winter_base::process::signal_group(group, libc::SIGTERM);
}

/// Only Linux names a terminal's foreground group, so nothing gets here.
#[cfg(not(unix))]
fn end_group(_group: i32) {}
