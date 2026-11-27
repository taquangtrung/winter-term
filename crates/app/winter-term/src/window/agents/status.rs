//! How each agent stands (see `docs/design/agent-workspace.md`, "Agent
//! status"): working, waiting for you, or stopped. The agent's own hook says
//! so best, through `winter agent report`; a bell or a desktop-notification
//! sequence from a terminal already known to run an agent says it waits.
//! Without hooks, the terminal's title tells: an agent is known by the
//! program in its terminal's foreground (one `settings.kdl` names) or by its
//! mux session (`agent-…`), and a title led by a spinner glyph says it is
//! working, any other that it waits.
//! A working agent's tab spins, and one that starts waiting out of sight
//! (its tab not shown, or the window in the background) carries the attention
//! dot until its tab is seen.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use winter_emulator::Grid;
use winter_model::layout::TabId;
use winter_model::path::abbreviate_home;

use super::super::App;
use super::super::panels::views::sessions::{Detected, Standing};
use super::record;
use crate::config::AgentKind;
use crate::control::{AgentReport, AgentState};

// ========================================================================
// Constants
// ========================================================================

/// The braille patterns, from which command-line spinners draw their
/// frames (`⠋⠙⠹⠸…`).
const SPINNER_FIRST: char = '\u{2800}';
const SPINNER_LAST: char = '\u{28ff}';

/// The box-drawing characters, which a spinner set in a border (`── ⠋
/// Working ──`, as pi draws) comes after.
const BOX_FIRST: char = '\u{2500}';
const BOX_LAST: char = '\u{257f}';

/// The braille pattern with no dots, a blank some programs fill with, never
/// a spinner's frame on the screen.
const SPINNER_BLANK: char = '\u{2800}';

// ========================================================================
// Data Structures
// ========================================================================

/// How the agent in each terminal that runs one stands.
#[derive(Debug, Default)]
pub(in crate::window) struct AgentStates {
    /// The tabs whose agents report through their hooks, which the title
    /// then never overrules.
    reporting: HashSet<TabId>,
    states: HashMap<TabId, AgentState>,
    /// The tabs whose agents started waiting out of sight and have not been
    /// seen since, which carry the attention dot.
    unseen: HashSet<TabId>,
}

// ========================================================================
// AgentStates Implementations
// ========================================================================

impl AgentStates {
    /// How the agent in `tab` stands, if it runs one known to Winter.
    pub(in crate::window) fn of(&self, tab: TabId) -> Option<AgentState> {
        self.states.get(&tab).copied()
    }

    /// Whether the agent in `tab` waits for you and its tab has not been
    /// seen since it started.
    pub(in crate::window) fn unseen(&self, tab: TabId) -> bool {
        self.unseen.contains(&tab)
    }
}

// ========================================================================
// App: agent status
// ========================================================================

impl App {
    /// Take in `report`, from the hook of an agent in one of this window's
    /// terminals: the one whose shell is among the reporter's ancestors.
    pub(in crate::window) fn take_agent_report(&mut self, report: &AgentReport) {
        let tab = self
            .open_tabs
            .terminals()
            .find(|(_, terminal)| {
                terminal
                    .child_pid()
                    .is_some_and(|pid| report.ancestors.contains(&pid))
            })
            .map(|(tab, _)| *tab);
        if let Some(tab) = tab {
            self.take_hook_state(tab, report.state);
        }
    }

    /// `state`, as the hook of the agent in `tab` reports it: through
    /// `winter agent report`, or the mux server passing that on.
    pub(in crate::window) fn take_hook_state(&mut self, tab: TabId, state: AgentState) {
        self.tabs.agents.reporting.insert(tab);
        self.set_agent_state(tab, state);
    }

    /// What `tab`'s new `title` says of the agent in it, where its hooks do
    /// not report: a spinner leading it says it works, any other title that
    /// it waits, and a program no longer an agent's that it stopped. An
    /// agent first seen is taken in without a notice.
    pub(in crate::window) fn note_agent_title(&mut self, tab: TabId, title: &str) {
        if self.tabs.agents.reporting.contains(&tab) {
            return;
        }
        let Some(terminal) = self.open_tabs.terminal(&tab) else {
            return;
        };
        let managed = terminal
            .mux_session()
            .is_some_and(|session| session.starts_with(record::SESSION_PREFIX));
        let program = terminal.title_facts().program;
        let known = managed
            || program
                .as_deref()
                .is_some_and(|program| self.agent_kind_of_program(program).is_some());
        let spinning = screen_spinning(terminal.grid());
        let before = self.tabs.agents.of(tab);
        let state = match (known, before) {
            (false, None | Some(AgentState::Stopped)) => return,
            (false, Some(_)) => AgentState::Stopped,
            (true, _) if spinning => AgentState::Working,
            (true, _) => title_state(title),
        };
        match before {
            Some(_) => self.set_agent_state(tab, state),
            None => {
                self.tabs.agents.states.insert(tab, state);
                self.dirty = true;
                self.sync_live_views();
            }
        }
    }

    /// Take in the agents running in the window's terminals that nothing
    /// has said of yet: one waiting at its prompt sets no new title, and
    /// after a reload the window knows of none. Each is taken in as its
    /// terminal's title says, without a notice, as one first seen is.
    pub(in crate::window) fn take_in_running_agents(&mut self) {
        let unknown: Vec<(TabId, AgentState)> = self
            .open_tabs
            .terminals()
            .filter(|(tab, _)| self.tabs.agents.of(**tab).is_none())
            .filter(|(_, terminal)| {
                terminal
                    .title_facts()
                    .program
                    .is_some_and(|program| self.agent_kind_of_program(&program).is_some())
            })
            .map(|(tab, _)| {
                let title = self.tabs.program_title(*tab).unwrap_or_default();
                (*tab, title_state(title))
            })
            .collect();
        for (tab, state) in unknown {
            self.tabs.agents.states.insert(tab, state);
            self.dirty = true;
        }
    }

    /// What the agent in `tab` is doing, as its terminal's title says.
    pub(in crate::window) fn agent_task(&self, tab: TabId) -> Option<String> {
        task_of(self.tabs.program_title(tab)?)
    }

    /// Terminals running an agent started by hand, with how each stands.
    /// Managed agents have their own rows, and stopped agents leave plain
    /// shells behind for the Terminals section.
    pub(in crate::window) fn detected_agents(&self) -> Vec<Detected> {
        let mut detected: Vec<Detected> = self
            .tabs
            .agents
            .states
            .iter()
            .filter(|(tab, _)| {
                self.open_tabs.terminal(tab).is_some_and(|terminal| {
                    !terminal
                        .mux_session()
                        .is_some_and(|session| record::branch_of_session(session).is_some())
                })
            })
            .filter_map(|(tab, state)| {
                let standing = match state {
                    AgentState::Stopped => return None,
                    AgentState::Waiting => Standing::Waiting,
                    AgentState::Working => Standing::Working,
                };
                let facts = self.open_tabs.terminal(tab)?.title_facts();
                let folder = facts.cwd.map(|cwd| abbreviate_home(Path::new(&cwd)));
                let agent = facts
                    .program
                    .and_then(|program| self.agent_kind_of_program(&program))
                    .map_or_else(|| self.tab_full_name(*tab), |kind| kind.name.clone());
                Some(Detected {
                    agent,
                    folder,
                    standing,
                    tab: *tab,
                    task: self.agent_task(*tab),
                })
            })
            .collect();
        detected.sort_by_key(|detected| detected.tab.0);
        detected
    }

    /// What `tab`'s screen, new output on it, says of the agent in it, where
    /// its hooks do not report: working while a line is led by a spinner,
    /// as an agent that sets no title (Antigravity) draws one, else as its
    /// title says. A terminal known to run no agent is left alone.
    pub(in crate::window) fn note_agent_screen(&mut self, tab: TabId) {
        if self.tabs.agents.reporting.contains(&tab) {
            return;
        }
        let Some(terminal) = self.open_tabs.terminal(&tab) else {
            return;
        };
        let known = self
            .tabs
            .agents
            .of(tab)
            .is_some_and(|state| state != AgentState::Stopped)
            || terminal
                .title_facts()
                .program
                .is_some_and(|program| self.agent_kind_of_program(&program).is_some());
        if !known {
            return;
        }
        let state = match screen_spinning(terminal.grid()) {
            true => AgentState::Working,
            false => title_state(self.tabs.program_title(tab).unwrap_or_default()),
        };
        match self.tabs.agents.of(tab) {
            Some(_) => self.set_agent_state(tab, state),
            // First seen, as a title first seen is: no notice.
            None => {
                self.tabs.agents.states.insert(tab, state);
                self.dirty = true;
                self.sync_live_views();
            }
        }
    }

    /// Which of the agents `settings.kdl` offers `program`, a terminal's
    /// foreground program by name, is: by its name, or its command's first
    /// word.
    pub(super) fn agent_kind_of_program(&self, program: &str) -> Option<&AgentKind> {
        self.config.agents.agents.iter().find(|kind| {
            kind.name == program || kind.command.split_whitespace().next() == Some(program)
        })
    }

    /// A bell or a desktop-notification sequence from `tab`: an agent known
    /// to run there now waits for you. A terminal not known to run one is
    /// left alone, since any program rings the bell.
    pub(in crate::window) fn note_agent_attention(&mut self, tab: TabId) {
        if self.tabs.agents.of(tab).is_some() {
            self.set_agent_state(tab, AgentState::Waiting);
        }
    }

    /// The agent in `tab` now stands at `state`; the attention dot marks it
    /// when it starts waiting where the user cannot see it.
    pub(in crate::window) fn set_agent_state(&mut self, tab: TabId, state: AgentState) {
        let open: Vec<TabId> = self.open_tabs.terminals().map(|(tab, _)| *tab).collect();
        let agents = &mut self.tabs.agents;
        // A tab gone takes its agent's state with it.
        agents.states.retain(|tab, _| open.contains(tab));
        agents.unseen.retain(|tab| open.contains(tab));
        let before = agents.states.insert(tab, state);
        if before == Some(state) {
            return;
        }
        self.dirty = true;
        // In front of its pane, buried or zoomed out of sight being neither.
        let in_front = self.layout().main_shown_tabs().contains(&tab);
        let out_of_sight = !self.input.focus.window || !in_front;
        match state == AgentState::Waiting && out_of_sight {
            true => self.tabs.agents.unseen.insert(tab),
            false => self.tabs.agents.unseen.remove(&tab),
        };
        self.sync_live_views();
    }

    /// Take the attention dot off the waiting agents whose tabs are in
    /// front of their panes while the window has the focus: seen now.
    /// Called once a frame, before the strips take their marks.
    pub(in crate::window) fn mark_agents_seen(&mut self) {
        if !self.input.focus.window || self.tabs.agents.unseen.is_empty() {
            return;
        }
        let in_front = self.layout().main_shown_tabs();
        self.tabs.agents.unseen.retain(|tab| !in_front.contains(tab));
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Whether `grid`'s screen has a line led by a spinner: a braille pattern
/// first, past blanks and a border's lines, then a space and a word
/// (`⣽  Generating...`, `── ⠋ Working ──`), as agents draw while they work.
/// Braille art and graphs, with no word after, are not one.
fn screen_spinning(grid: &Grid) -> bool {
    (0..grid.rows()).any(|row| {
        let mut chars = (0..grid.cols())
            .filter_map(|col| grid.cell(row, col))
            .map(|cell| cell.ch)
            .skip_while(|ch| *ch == ' ' || (BOX_FIRST..=BOX_LAST).contains(ch));
        let Some(glyph) = chars.next() else {
            return false;
        };
        let spinner = glyph != SPINNER_BLANK && (SPINNER_FIRST..=SPINNER_LAST).contains(&glyph);
        spinner
            && chars.next() == Some(' ')
            && chars.find(|ch| *ch != ' ').is_some_and(char::is_alphabetic)
    })
}

/// What an agent's terminal `title` says it is doing, past the glyph it
/// leads the title with to show how it stands (a spinner, `✳`).
fn task_of(title: &str) -> Option<String> {
    let title = title.trim();
    let mut chars = title.chars();
    let glyph = chars.next()?;
    let rest = chars.as_str();
    let task = match !glyph.is_ascii()
        && !glyph.is_alphanumeric()
        && (rest.is_empty() || rest.starts_with(' '))
    {
        true => rest.trim_start(),
        false => title,
    };
    (!task.is_empty()).then(|| task.to_string())
}

/// What an agent's terminal `title` says of it: working while a spinner
/// glyph (a braille pattern, as command-line spinners draw) leads it,
/// waiting otherwise.
fn title_state(title: &str) -> AgentState {
    let spinning = title
        .trim_start()
        .chars()
        .next()
        .is_some_and(|glyph| (SPINNER_FIRST..=SPINNER_LAST).contains(&glyph));
    match spinning {
        true => AgentState::Working,
        false => AgentState::Waiting,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_report_reaches_the_terminal_its_shell_is_an_ancestor_of() {
        let mut app = App::new();
        let tab = app.layout().focused();
        let Some(shell) = app.open_tabs.terminal(&tab).and_then(|t| t.child_pid()) else {
            return;
        };
        let report = |state| AgentReport {
            ancestors: vec![999_999, shell, 1],
            state,
        };
        app.take_agent_report(&report(AgentState::Working));
        assert_eq!(app.tabs.agents.of(tab), Some(AgentState::Working));
        app.take_agent_report(&AgentReport {
            ancestors: vec![999_999],
            state: AgentState::Stopped,
        });
        assert_eq!(
            app.tabs.agents.of(tab),
            Some(AgentState::Working),
            "no shell of ours"
        );
    }

    #[test]
    fn test_a_task_is_the_title_past_the_glyph_that_shows_how_it_stands() {
        assert_eq!(
            task_of("⠋ Refactoring the parser").as_deref(),
            Some("Refactoring the parser")
        );
        assert_eq!(task_of("✳ Claude Code").as_deref(), Some("Claude Code"));
        assert_eq!(task_of("vim main.rs").as_deref(), Some("vim main.rs"));
        assert_eq!(task_of("(1) build").as_deref(), Some("(1) build"));
        assert_eq!(task_of("✳ ").as_deref(), None);
    }

    #[test]
    fn test_a_line_led_by_a_spinner_and_a_word_says_the_screen_is_working() {
        let screen = |text: &str| {
            let mut grid = Grid::new(30, 3);
            for (row, line) in text.lines().enumerate() {
                grid.move_to(row, 0);
                line.chars().for_each(|ch| grid.print(ch));
            }
            screen_spinning(&grid)
        };
        assert!(screen("> Reply with just: hi\n⣽  Generating..."));
        assert!(!screen("> Reply with just: hi\n  hi"), "done");
        assert!(!screen("⣿⣿⣷⣄ ⣿⣿"), "braille art, no word");
        assert!(!screen("\u{2800} Blank"), "the blank pattern");
        assert!(screen("── ⠋ Working ──────"), "set in a border, as pi does");
        assert!(!screen("── ⠋ ──────"), "a border's spinner alone, no word");
    }

    #[test]
    fn test_a_spinner_leading_the_title_says_working_and_any_other_waiting() {
        assert_eq!(title_state("⠋ Refactoring the parser"), AgentState::Working);
        assert_eq!(title_state("  ⠹ claude"), AgentState::Working);
        assert_eq!(title_state("✳ Claude Code"), AgentState::Waiting);
        assert_eq!(title_state(""), AgentState::Waiting);
    }

    #[test]
    fn test_an_agent_is_known_by_its_name_or_its_command_s_program() {
        let app = App::new();
        let name = |program| {
            app.agent_kind_of_program(program)
                .map(|kind| kind.name.as_str())
        };
        assert_eq!(name("claude"), Some("claude"));
        assert_eq!(name("codex"), Some("codex"));
        assert_eq!(name("agy"), Some("antigravity"), "by its command's program");
        assert_eq!(name("kiro-cli"), Some("kiro"));
        assert_eq!(name("pi"), Some("pi"));
        assert_eq!(name("vim"), None);
    }

    #[test]
    fn test_a_title_never_overrules_an_agent_whose_hooks_report() {
        let mut app = App::new();
        let tab = app.layout().focused();
        app.take_hook_state(tab, AgentState::Waiting);
        app.note_agent_title(tab, "⠋ working away");
        assert_eq!(app.tabs.agents.of(tab), Some(AgentState::Waiting));
    }

    #[test]
    fn test_a_bell_means_waiting_only_where_an_agent_is_known_to_run() {
        let mut app = App::new();
        let tab = app.layout().focused();
        app.note_agent_attention(tab);
        assert_eq!(app.tabs.agents.of(tab), None, "any program rings the bell");
        app.set_agent_state(tab, AgentState::Working);
        app.note_agent_attention(tab);
        assert_eq!(app.tabs.agents.of(tab), Some(AgentState::Waiting));
    }

    #[test]
    fn test_the_attention_dot_lasts_from_waiting_out_of_sight_until_seen() {
        // An idle agent you have looked at is no news, however long it idles.
        let mut app = App::new();
        let tab = app.layout().focused();
        app.input.focus.window = true;
        app.set_agent_state(tab, AgentState::Waiting);
        assert!(!app.tabs.agents.unseen(tab), "waiting in sight");
        app.input.focus.window = false;
        app.set_agent_state(tab, AgentState::Working);
        app.set_agent_state(tab, AgentState::Waiting);
        app.mark_agents_seen();
        assert!(app.tabs.agents.unseen(tab), "the window is in the background");
        app.input.focus.window = true;
        app.mark_agents_seen();
        assert!(!app.tabs.agents.unseen(tab), "seen now");
        app.input.focus.window = false;
        app.set_agent_state(tab, AgentState::Working);
        app.set_agent_state(tab, AgentState::Waiting);
        app.set_agent_state(tab, AgentState::Working);
        assert!(!app.tabs.agents.unseen(tab), "back at work");
    }
}
