//! The terminals the Terminals view lists (see
//! `docs/design/agent-workspace.md`): every terminal open in the window, the
//! window's workspace's first, buried ones among them, then each workspace
//! put aside's, each by what it runs or last ran, so the view says what goes
//! on everywhere; one running an agent is marked as the agent stands. What
//! a terminal runs is read of its processes, which needs nothing of the
//! shell; the shell's own integration, where the user sources it, says
//! more: every command, and how each went. Showing one in a workspace put
//! aside goes to that workspace first.

use std::path::Path;

use winter_model::layout::{Layout, TabId};
use winter_model::path::abbreviate_home;
use winter_terminal::session::Terminal;

use super::super::App;
use super::super::notice::WindowNotice;
use super::super::panels::views::terminals::{Ran, TerminalEntry};
use crate::control::AgentState;

// ========================================================================
// App: the terminals
// ========================================================================

impl App {
    /// Every terminal open in the window: the window's workspace's, shown
    /// or buried, then each workspace put aside's, by name.
    pub(in crate::window) fn terminal_entries(&self) -> Vec<TerminalEntry> {
        let mut own = self.layout().members();
        for tab in self.open_tabs.buried() {
            if !own.contains(tab) {
                own.push(*tab);
            }
        }
        let mut entries: Vec<TerminalEntry> = own
            .into_iter()
            .filter_map(|tab| self.terminal_entry(tab, None))
            .collect();
        let mut parked: Vec<(&String, &Layout)> = self.workspace.parked.iter().collect();
        parked.sort_by(|a, b| a.0.cmp(b.0));
        for (name, layout) in parked {
            entries.extend(
                layout
                    .members()
                    .into_iter()
                    .filter_map(|tab| self.terminal_entry(tab, Some(name))),
            );
        }
        entries
    }

    /// Show `tab`, going first to the workspace put aside it is in.
    pub(in crate::window) fn reveal_tab(&mut self, tab: TabId) {
        let parked = self
            .workspace
            .parked
            .iter()
            .find(|(_, layout)| layout.contains(tab))
            .map(|(name, _)| name.clone());
        if let Some(name) = parked
            && let Err(why) = self.switch_workspace(&name)
        {
            self.notify(WindowNotice::AgentFailed(why));
            return;
        }
        self.show_tab(tab);
    }

    /// `tab`, of the workspace put aside `workspace` names or the window's
    /// own, as the view lists it: `None` for no terminal, or one running an
    /// agent, which the Agents section lists.
    fn terminal_entry(&self, tab: TabId, workspace: Option<&str>) -> Option<TerminalEntry> {
        let terminal = self.open_tabs.terminal(&tab)?;
        let facts = terminal.title_facts();
        // The last command the shell reported (OSC 133), by its first line.
        let last = terminal
            .scrollback()
            .blocks()
            .last()
            .and_then(|block| Some((block.command.lines().next()?.trim(), block.exit_code)))
            .filter(|(command, _)| !command.is_empty());
        let running = terminal.command_running() || facts.program.is_some();
        let (task, ran) = match (running, last) {
            // The processes say what runs exactly; the shell's own word, as
            // it echoed the line being edited, can hold its redraws.
            (true, last) => (
                terminal
                    .foreground_command_line()
                    .or_else(|| {
                        last.and_then(|(command, code)| code.is_none().then(|| command.to_string()))
                    })
                    .or_else(|| facts.program.clone())
                    .unwrap_or_else(|| self.tab_full_name(tab)),
                Ran::Running,
            ),
            (false, Some((command, code))) => (
                command.to_string(),
                match code {
                    Some(0) => Ran::Finished,
                    Some(code) => Ran::Failed(code),
                    None => Ran::Idle,
                },
            ),
            // What it last ran as its processes said, how it went unknown.
            (false, None) => (
                terminal
                    .last_command()
                    .or_else(|| shell_name(terminal))
                    .unwrap_or_else(|| self.tab_full_name(tab)),
                Ran::Idle,
            ),
        };
        Some(TerminalEntry {
            folder: facts.cwd.map(|cwd| abbreviate_home(Path::new(&cwd))),
            ran: self.agent_ran(tab).unwrap_or(ran),
            tab,
            task,
            workspace: workspace.map(str::to_string),
        })
    }

    /// How the agent in `tab` stands, as the Terminals view marks it: at
    /// work, or waiting for you; `None` for a terminal running none, or one
    /// whose agent has stopped, marked as any other.
    fn agent_ran(&self, tab: TabId) -> Option<Ran> {
        match self.tabs.agents.of(tab)? {
            AgentState::Working => Some(Ran::Running),
            AgentState::Waiting => Some(Ran::Waiting),
            AgentState::Stopped => None,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The name of the shell `terminal` started, `zsh` of `/usr/bin/zsh -l`:
/// what a terminal that has run nothing is called. A mux terminal's shell
/// is the server's to say, so it has none here.
fn shell_name(terminal: &Terminal) -> Option<String> {
    if terminal.mux_session().is_some() {
        return None;
    }
    let program = terminal.shell_command().split_whitespace().next()?;
    let name = Path::new(program).file_name()?;
    Some(name.to_string_lossy().into_owned())
}
