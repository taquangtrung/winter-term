//! Closing editor tabs, panes, and the window, and killing buffers: asking
//! first when something would be lost (unsaved edits, a running program,
//! unfinished jobs), and answering that question.
//!
//! Closing an editor's tab buries its buffer, which loses nothing and so asks
//! nothing; killing the buffer is what lets it go. A terminal's tab is its
//! shell, and closing it ends the shell.

use crate::model::button::ButtonGroup;
use crate::model::input::Key;
use crate::model::layout::TabId;

use crate::window::input::overlay::{ClosePrompt, Overlay};
use crate::window::App;

use super::named;
use crate::window::EditorHost;

// ========================================================================
// Constants
// ========================================================================

/// How a running program is named when the system does not say.
const RUNNING_PROGRAM: &str = "a program";

// ========================================================================
// Data Structures
// ========================================================================

/// What the host is waiting to close once its question is answered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) enum PendingClose {
    /// Every pane but the one holding this tab.
    OtherPanes(TabId),
    /// Every tab but this one, in its pane.
    OtherTabs(TabId),
    /// The pane holding this tab, and every tab in it.
    Terminal(TabId),
    /// This one tab.
    Tab(TabId),
    /// The buffer this tab shows, and every tab on it.
    Kill(TabId),
    /// The whole window.
    Quit,
    /// The window, replaced by the binary now on disk.
    Reload,
    /// Everything in the window, for the workspace a switch goes to (see
    /// [`crate::window::workspace::WorkspaceState::switch_to`]).
    Switch,
}

/// Whether an editor tab's buffer outlives the tab.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) enum Closing {
    /// The buffer is buried: an editor closed with `q`, a pane merged away, a
    /// tab closed. A terminal's shell ends all the same.
    Keep,
    /// The buffer is killed with its tab: a commit message's companion diff,
    /// a terminal whose shell exited on its own.
    Forget,
}

// ========================================================================
// PendingClose Implementations
// ========================================================================

impl PendingClose {
    /// The tab the close is about, whose own closing makes the question moot.
    /// `None` for the whole window, which no single tab stands for.
    pub(in crate::window) fn subject(self) -> Option<TabId> {
        match self {
            Self::OtherPanes(tab)
            | Self::OtherTabs(tab)
            | Self::Terminal(tab)
            | Self::Tab(tab)
            | Self::Kill(tab) => Some(tab),
            Self::Quit | Self::Reload | Self::Switch => None,
        }
    }
}

// ========================================================================
// App: closing
// ========================================================================

impl App {
    /// Close the editor tab `tab_id`, burying its buffer.
    pub(in crate::window) fn close_editor(&mut self, tab_id: TabId) {
        self.close_tab_now(tab_id, Closing::Keep);
    }

    /// Kill the buffer tab `tab` shows, asking first about unsaved text or a
    /// build or search still running. On a terminal's tab, close the tab, which
    /// asks about a program running in it.
    pub(in crate::window) fn kill_buffer(&mut self, tab: TabId) {
        let Some(slot) = self.open_tabs.editor(&tab) else {
            self.close_tab(tab);
            return;
        };
        let title = slot.editor.title();
        let mut lost = Vec::new();
        if slot.editor.is_dirty() {
            lost.push(format!("unsaved edits in {title}"));
        }
        if slot.editor.is_busy() {
            lost.push(format!("{title} still running"));
        }
        if self.ask_close(lost, "buffer", PendingClose::Kill(tab)) {
            return;
        }
        self.kill_buffer_now(tab);
    }

    /// Kill the buffer tab `tab` shows without asking: every other tab on it
    /// closes, then `tab`, whose editor hears it is over; a buried buffer's
    /// tab is let go.
    pub(in crate::window) fn kill_buffer_now(&mut self, tab: TabId) {
        let others = self.open_tabs.views_of(&tab).into_iter().filter(|view| *view != tab);
        for view in others.collect::<Vec<_>>() {
            self.close_tab_now(view, Closing::Forget);
        }
        match self.layout().contains(tab) {
            true => self.close_tab_now(tab, Closing::Forget),
            false => self.drop_tab_state(tab),
        }
    }

    /// Whether closing tab `tab` with `closing` buries its buffer rather than
    /// letting go of what it holds: an editor's last view, when the editor
    /// says it is worth keeping (see [`crate::model::editor::Editor::is_kept_when_closed`]).
    /// Closing one of several views of a buffer only closes the view.
    pub(in crate::window) fn buries(&self, tab: TabId, closing: Closing) -> bool {
        closing == Closing::Keep
            && self.open_tabs.views_of(&tab).len() == 1
            && self.open_tabs.editor(&tab).is_some_and(|slot| slot.editor.is_kept_when_closed())
    }

    /// Every buffer no pane shows, by the tab it was last shown in.
    pub(in crate::window) fn buried_tabs(&self) -> Vec<TabId> {
        self.open_tabs.buried().to_vec()
    }

    /// Ask before closing a pane whose editor tabs hold edits on no disk, or
    /// whose terminal has a program running in it.
    ///
    /// Returns whether the question was asked, which is the caller's cue to
    /// wait for the answer rather than close now. A shell sitting at its
    /// prompt has nothing to lose, but a program in the way (an agent, an
    /// editor, a build) is ended with the pane.
    pub(in crate::window) fn ask_before_closing_pane(&mut self, tab_id: TabId) -> bool {
        let tabs = self.let_go(self.layout().tabs_in_pane(tab_id));
        self.ask_before_closing(&tabs, "pane", PendingClose::Terminal(tab_id))
    }

    /// The same, for every pane but the one holding `keep`.
    pub(in crate::window) fn ask_before_closing_others(&mut self, keep: TabId) -> bool {
        let others = self.let_go(self.other_panes_tabs(keep));
        self.ask_before_closing(&others, "other panes", PendingClose::OtherPanes(keep))
    }

    /// The same, for every tab in `keep`'s pane but `keep`.
    pub(in crate::window) fn ask_before_closing_other_tabs(&mut self, keep: TabId) -> bool {
        let others: Vec<TabId> = self
            .layout()
            .tabs_in_pane(keep)
            .into_iter()
            .filter(|tab| *tab != keep)
            .collect();
        let others = self.let_go(others);
        self.ask_before_closing(&others, "other tabs", PendingClose::OtherTabs(keep))
    }

    /// The same, for one tab.
    pub(in crate::window) fn ask_before_closing_tab(&mut self, tab_id: TabId) -> bool {
        let tabs = self.let_go(vec![tab_id]);
        self.ask_before_closing(&tabs, "tab", PendingClose::Tab(tab_id))
    }

    /// The same, for the whole window, with the workspaces put aside in it
    /// and the buried buffers.
    pub(in crate::window) fn ask_before_quitting(&mut self) -> bool {
        let lost = self.quit_losses();
        self.ask_close(lost, "window", PendingClose::Quit)
    }

    /// Whether quitting now would lose something worth asking about.
    pub(in crate::window) fn would_quit_lose_anything(&self) -> bool {
        !self.quit_losses().is_empty()
    }

    /// What quitting would lose: edits on no disk anywhere in the window,
    /// buried buffers included, and programs still running.
    fn quit_losses(&self) -> Vec<String> {
        let mut everything = self.every_open_tab();
        everything.extend(self.buried_tabs());
        let mut lost = self.unsaved_reasons(&everything);
        lost.extend(self.running_reason(&everything));
        lost
    }

    /// The same, for everything in the window, before a switch to another
    /// workspace closes it.
    pub(in crate::window) fn ask_before_switching(&mut self) -> bool {
        let everything = self.layout().members();
        self.ask_before_closing(&everything, "window's tabs", PendingClose::Switch)
    }

    /// Ask before replacing the window with the binary on disk, when that
    /// would lose something: edits on no disk, which a reload does not carry
    /// over, and tasks still running, which it cuts off. Where the reload
    /// cannot hand the programs in the panes over to the new binary, those
    /// are asked about too, since they end with it.
    pub(in crate::window) fn ask_before_reloading(&mut self) -> bool {
        let everything = self.every_open_tab();
        let mut buffers = everything.clone();
        buffers.extend(self.buried_tabs());
        let mut lost = self.unsaved_reasons(&buffers);
        lost.extend(self.host.jobs_reason());
        // The programs of the workspaces put aside are not handed over.
        let ended = match cfg!(unix) {
            true => self.parked_tabs(),
            false => everything,
        };
        lost.extend(self.running_reason(&ended));
        self.ask_close(lost, "window and reload", PendingClose::Reload)
    }

    /// Put the question, if there is anything to ask about.
    fn ask_before_closing(&mut self, panes: &[TabId], what: &str, pending: PendingClose) -> bool {
        let mut lost = self.unsaved_reasons(panes);
        lost.extend(self.running_reason(panes));
        self.ask_close(lost, what, pending)
    }

    /// The tabs among `tabs` that closing lets go of: an editor tab whose
    /// buffer is buried, or another tab still shows, loses nothing, so is
    /// nothing to ask about.
    fn let_go(&self, tabs: Vec<TabId>) -> Vec<TabId> {
        tabs.into_iter()
            .filter(|tab| {
                !self.buries(*tab, Closing::Keep) && self.open_tabs.views_of(tab).len() <= 1
            })
            .collect()
    }

    /// Ask whether to close `what`, naming what would be `lost`; nothing is
    /// asked when nothing would be. Returns whether the question was put.
    fn ask_close(&mut self, lost: Vec<String>, what: &str, pending: PendingClose) -> bool {
        if lost.is_empty() {
            return false;
        }
        self.open_overlay(Overlay::Close(ClosePrompt {
            buttons: ButtonGroup::confirm(),
            label: format!("{}. Close the {what}? (y/n) ", lost.join("; ")),
            pending,
        }));
        true
    }

    /// What closing `panes` would lose in unwritten edits, as a reason.
    fn unsaved_reasons(&self, panes: &[TabId]) -> Vec<String> {
        let unsaved = self.unsaved_in(panes);
        match unsaved.is_empty() {
            true => Vec::new(),
            false => vec![format!("unsaved edits in {}", named(&unsaved))],
        }
    }

    /// What closing `panes` would end of the programs running in them.
    fn running_reason(&self, panes: &[TabId]) -> Option<String> {
        let running = self.running_in(panes);
        (!running.is_empty()).then(|| format!("{} still running", named(&running)))
    }


    /// The program running in each terminal among `panes` that has one beyond
    /// its shell, named as the system names it.
    fn running_in(&self, panes: &[TabId]) -> Vec<String> {
        panes
            .iter()
            .filter_map(|pane| self.open_tabs.terminal(pane))
            .filter(|terminal| terminal.has_foreground_process())
            .map(|terminal| {
                terminal
                    .foreground_process_name()
                    .unwrap_or_else(|| RUNNING_PROGRAM.to_string())
            })
            .collect()
    }

    /// What every editor tab among `panes` is holding that is on no disk,
    /// named as the editors name themselves.
    fn unsaved_in(&self, panes: &[TabId]) -> Vec<String> {
        panes
            .iter()
            .filter_map(|pane| self.open_tabs.editor(pane))
            .filter(|slot| slot.editor.is_dirty())
            .map(|slot| slot.editor.title())
            .collect()
    }

    /// Close what the question was about, or nothing when the answer was not
    /// yes.
    pub(in crate::window) fn answer_close_prompt(&mut self, pending: PendingClose, yes: bool) {
        // A switch turned down is off, not waiting for another answer.
        let switch_to = match pending {
            PendingClose::Switch => self.workspace.switch_to.take(),
            _ => None,
        };
        if !yes {
            return;
        }
        match pending {
            PendingClose::OtherPanes(keep) => self.close_other_panes_now(keep),
            PendingClose::OtherTabs(keep) => self.close_other_tabs_now(keep),
            PendingClose::Terminal(pane) => self.close_pane_now(pane),
            PendingClose::Tab(pane) => self.close_tab_now(pane, Closing::Keep),
            PendingClose::Kill(tab) => self.kill_buffer_now(tab),
            PendingClose::Quit => self.runtime.exit.requested = true,
            PendingClose::Reload => {
                self.runtime.exit.reload_confirmed = true;
                self.runtime.exit.reload_pending = true;
            }
            PendingClose::Switch => {
                if let Some(name) = switch_to {
                    let done = self.switch_workspace_now(&name);
                    self.report(done);
                }
            }
        }
        // Closing the last tab quits. The answer just given covers that tab;
        // it covers the quit only when nothing else would go with the window:
        // edits buried out of sight, or programs in a workspace put aside,
        // still have to be asked about.
        if self.runtime.exit.requested {
            let elsewhere_lost = match pending {
                PendingClose::Quit => false,
                _ => {
                    let mut elsewhere = self.parked_tabs();
                    elsewhere.extend(self.buried_tabs());
                    let mut lost = self.unsaved_reasons(&elsewhere);
                    lost.extend(self.running_reason(&elsewhere));
                    !lost.is_empty()
                }
            };
            self.runtime.exit.quit_confirmed = !elsewhere_lost;
        }
    }

    /// Route one key into the host's close question: `Tab` cycles between buttons,
    /// `Enter` confirms the focused button, direct shortcuts (`y`, `n`, `Esc`) activate immediately.
    pub(in crate::window) fn handle_close_prompt_key(&mut self, key: &Key) {
        let Some(Overlay::Close(prompt)) = &mut self.chrome.overlay else {
            return;
        };
        if let Some(yes) = prompt.buttons.on_key(key) {
            let Some(Overlay::Close(prompt)) =
                self.chrome.overlay.take_if(|o| matches!(o, Overlay::Close(_)))
            else {
                return;
            };
            self.dirty = true;
            self.answer_close_prompt(prompt.pending, yes);
        } else {
            self.dirty = true;
        }
    }
}

// ========================================================================
// EditorHost
// ========================================================================

impl EditorHost {
    /// The tasks running for any editor, which a reload cuts off.
    fn jobs_reason(&self) -> Option<String> {
        let labels = self.jobs.labels();
        (!labels.is_empty()).then(|| format!("{} still running", named(&labels)))
    }
}
