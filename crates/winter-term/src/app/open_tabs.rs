//! Every open tab and what it holds: a terminal, or a tool Winter paints
//! itself. The layout says where a tab sits; this says what it is.

use std::collections::{HashMap, HashSet};

use crate::model::layout::TabId;
use crate::model::mode::Mode;
use crate::terminal::session::Terminal;

use super::prompt_edit::PromptShadow;
use super::tool::ToolSlot;

// ========================================================================
// Data Structures
// ========================================================================

/// The tabs of the window, by id. A tab is a terminal or a tool and never
/// both, so asking for one kind of a tab of the other answers `None`.
#[derive(Default)]
pub(crate) struct OpenTabs {
    tabs: HashMap<TabId, Tab>,
}

/// What one tab holds. Both kinds are boxed: each is several hundred bytes
/// or more, and a tab is only ever reached through the map.
pub(crate) enum Tab {
    /// A tool Winter paints itself: a listing, the git view, an editor.
    Tool(Box<ToolSlot>),
    /// A shell or another program on a pseudo-terminal.
    Terminal(Box<TerminalTab>),
}

/// A terminal tab: the terminal, and the state Winter keeps beside it, which
/// goes when the tab does.
pub(crate) struct TerminalTab {
    /// The blocks folded down to their first line, by index.
    pub(crate) folded_blocks: HashSet<usize>,
    /// Where the tab's keystrokes go.
    pub(crate) mode: Mode,
    /// The Normal and Visual traversal cursor, while the tab has one.
    pub(crate) nav_cursor: Option<(usize, usize)>,
    /// Whether the last key forwarded in Insert was Tab, so the shell may be
    /// mid its own completion (zsh's menu-select, say). Lets a bare Escape
    /// in Normal, with nothing of Winter's to cancel, reach the PTY and close
    /// it: see `escape_forwarded_to_pty`.
    pub(crate) pending_tab_completion: bool,
    /// The model of the editable prompt line behind `Ctrl-/` undo and
    /// `Ctrl-\` redo, made on the first key typed at the prompt.
    pub(crate) prompt_shadow: Option<PromptShadow>,
    pub(crate) terminal: Terminal,
}

// ========================================================================
// OpenTabs Implementations
// ========================================================================

impl OpenTabs {
    // --------------------------------------------------------------------
    // Terminals
    // --------------------------------------------------------------------

    /// Open `terminal` as tab `id`, in the default mode.
    pub(crate) fn insert_terminal(&mut self, id: TabId, terminal: Terminal) {
        let tab = TerminalTab {
            folded_blocks: HashSet::new(),
            mode: Mode::default(),
            nav_cursor: None,
            pending_tab_completion: false,
            prompt_shadow: None,
            terminal,
        };
        self.tabs.insert(id, Tab::Terminal(Box::new(tab)));
    }

    /// The terminal of tab `id`.
    pub(crate) fn terminal(&self, id: &TabId) -> Option<&Terminal> {
        self.terminal_tab(id).map(|tab| &tab.terminal)
    }

    /// The terminal of tab `id`, to change.
    pub(crate) fn terminal_mut(&mut self, id: &TabId) -> Option<&mut Terminal> {
        self.terminal_tab_mut(id).map(|tab| &mut tab.terminal)
    }

    /// Tab `id`, when it is a terminal.
    pub(crate) fn terminal_tab(&self, id: &TabId) -> Option<&TerminalTab> {
        match self.tabs.get(id) {
            Some(Tab::Terminal(tab)) => Some(tab.as_ref()),
            Some(Tab::Tool(_)) | None => None,
        }
    }

    /// Tab `id`, when it is a terminal, to change.
    pub(crate) fn terminal_tab_mut(&mut self, id: &TabId) -> Option<&mut TerminalTab> {
        match self.tabs.get_mut(id) {
            Some(Tab::Terminal(tab)) => Some(tab.as_mut()),
            Some(Tab::Tool(_)) | None => None,
        }
    }

    /// Whether tab `id` is a terminal.
    pub(crate) fn is_terminal(&self, id: &TabId) -> bool {
        self.terminal_tab(id).is_some()
    }

    /// Every terminal tab's terminal, in no particular order.
    pub(crate) fn terminals(&self) -> impl Iterator<Item = (&TabId, &Terminal)> {
        self.tabs.iter().filter_map(|(id, tab)| match tab {
            Tab::Terminal(tab) => Some((id, &tab.terminal)),
            Tab::Tool(_) => None,
        })
    }

    /// Every terminal tab's terminal, to change.
    pub(crate) fn terminals_mut(&mut self) -> impl Iterator<Item = (&TabId, &mut Terminal)> {
        self.tabs.iter_mut().filter_map(|(id, tab)| match tab {
            Tab::Terminal(tab) => Some((id, &mut tab.terminal)),
            Tab::Tool(_) => None,
        })
    }

    /// Every terminal tab's terminal by id, as a session snapshot takes them.
    pub(crate) fn terminal_map(&self) -> HashMap<TabId, &Terminal> {
        self.terminals()
            .map(|(id, terminal)| (*id, terminal))
            .collect()
    }

    /// Close every terminal tab, leaving the tools.
    pub(crate) fn clear_terminals(&mut self) {
        self.tabs.retain(|_, tab| matches!(tab, Tab::Tool(_)));
    }

    /// Where tab `id`'s keystrokes go, or `None` for a tool, which takes
    /// its own keys rather than being in a mode.
    pub(crate) fn mode(&self, id: &TabId) -> Option<Mode> {
        self.terminal_tab(id).map(|tab| tab.mode)
    }

    /// Put terminal tab `id` in `mode`. A tool has no mode to set.
    pub(crate) fn set_mode(&mut self, id: &TabId, mode: Mode) {
        if let Some(tab) = self.terminal_tab_mut(id) {
            tab.mode = mode;
        }
    }

    /// Terminal tab `id`'s traversal cursor, if it has one.
    pub(crate) fn nav_cursor(&self, id: &TabId) -> Option<(usize, usize)> {
        self.terminal_tab(id).and_then(|tab| tab.nav_cursor)
    }

    /// Set or drop terminal tab `id`'s traversal cursor. A tool keeps a
    /// cursor of its own.
    pub(crate) fn set_nav_cursor(&mut self, id: &TabId, cursor: Option<(usize, usize)>) {
        if let Some(tab) = self.terminal_tab_mut(id) {
            tab.nav_cursor = cursor;
        }
    }

    /// Terminal tab `id`'s prompt model, once a key has been typed there.
    pub(crate) fn prompt_shadow_mut(&mut self, id: &TabId) -> Option<&mut PromptShadow> {
        self.terminal_tab_mut(id)
            .and_then(|tab| tab.prompt_shadow.as_mut())
    }

    // --------------------------------------------------------------------
    // Tools
    // --------------------------------------------------------------------

    /// Open `slot` as tab `id`.
    pub(crate) fn insert_tool(&mut self, id: TabId, slot: ToolSlot) {
        self.tabs.insert(id, Tab::Tool(Box::new(slot)));
    }

    /// The tool of tab `id`.
    pub(crate) fn tool(&self, id: &TabId) -> Option<&ToolSlot> {
        match self.tabs.get(id) {
            Some(Tab::Tool(slot)) => Some(slot.as_ref()),
            Some(Tab::Terminal(_)) | None => None,
        }
    }

    /// The tool of tab `id`, to change.
    pub(crate) fn tool_mut(&mut self, id: &TabId) -> Option<&mut ToolSlot> {
        match self.tabs.get_mut(id) {
            Some(Tab::Tool(slot)) => Some(slot.as_mut()),
            Some(Tab::Terminal(_)) | None => None,
        }
    }

    /// Whether tab `id` is a tool.
    pub(crate) fn is_tool(&self, id: &TabId) -> bool {
        self.tool(id).is_some()
    }

    /// Take tab `id`'s tool out, leaving a terminal tab where it is.
    pub(crate) fn remove_tool(&mut self, id: &TabId) -> Option<ToolSlot> {
        if !self.is_tool(id) {
            return None;
        }
        match self.tabs.remove(id) {
            Some(Tab::Tool(slot)) => Some(*slot),
            Some(Tab::Terminal(_)) | None => None,
        }
    }

    /// Every tool tab's tool, in no particular order.
    pub(crate) fn tools(&self) -> impl Iterator<Item = (&TabId, &ToolSlot)> {
        self.tabs.iter().filter_map(|(id, tab)| match tab {
            Tab::Tool(slot) => Some((id, slot.as_ref())),
            Tab::Terminal(_) => None,
        })
    }

    /// Every tool tab's tool, to change.
    pub(crate) fn tools_mut(&mut self) -> impl Iterator<Item = (&TabId, &mut ToolSlot)> {
        self.tabs.iter_mut().filter_map(|(id, tab)| match tab {
            Tab::Tool(slot) => Some((id, slot.as_mut())),
            Tab::Terminal(_) => None,
        })
    }

    // --------------------------------------------------------------------
    // Either kind
    // --------------------------------------------------------------------

    /// Close tab `id`, whichever kind it is.
    pub(crate) fn remove(&mut self, id: &TabId) -> Option<Tab> {
        self.tabs.remove(id)
    }
}
