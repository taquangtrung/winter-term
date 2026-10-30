//! Every open tab and what it holds: a terminal, or a view of an editor
//! Winter paints itself, whose editor is one of the window's buffers. The
//! layout says where a tab sits; this says what it is.

use std::collections::{HashMap, HashSet};

use crate::model::editor::Editor;
use crate::model::guard::Guarded;
use crate::model::layout::TabId;
use crate::model::mode::Mode;
use crate::terminal::session::Terminal;

use crate::window::editor::buffers::{Buffer, Buffers, EditorMut, EditorRef, EditorView};
use crate::window::editor::column::RowCursor;
use crate::window::editor::EditorKind;
use crate::window::terminal::prompt_edit::PromptShadow;

// ========================================================================
// Data Structures
// ========================================================================

/// The tabs of the window, by id. A tab is a terminal or an editor and never
/// both, so asking for one kind of a tab of the other answers `None`.
#[derive(Default)]
pub(in crate::window) struct OpenTabs {
    /// The editors the editor tabs show.
    buffers: Buffers,
    /// The editor tabs no pane shows, their buffers buried, the most recently
    /// buried last.
    buried: Vec<TabId>,
    tabs: HashMap<TabId, Tab>,
}

/// What one tab holds. Both kinds are boxed: each is several hundred bytes
/// or more, and a tab is only ever reached through the map.
pub(in crate::window) enum Tab {
    /// A view of an editor Winter paints itself: a listing, the git view,
    /// an editor.
    Editor(Box<EditorView>),
    /// A shell or another program on a pseudo-terminal.
    Terminal(Box<TerminalTab>),
}

/// A terminal tab: the terminal, and the state Winter keeps beside it, which
/// goes when the tab does.
pub(in crate::window) struct TerminalTab {
    /// The blocks folded down to their first line, by index.
    pub(in crate::window) folded_blocks: HashSet<usize>,
    /// Where the tab's keystrokes go.
    pub(in crate::window) mode: Mode,
    /// The Normal and Visual traversal cursor, while the tab has one.
    pub(in crate::window) nav_cursor: Option<(usize, usize)>,
    /// Whether the last key forwarded in Insert was Tab, so the shell may be
    /// mid its own completion (zsh's menu-select, say). Lets a bare Escape
    /// in Normal, with nothing of Winter's to cancel, reach the PTY and close
    /// it: see `escape_forwarded_to_pty`.
    pub(in crate::window) pending_tab_completion: bool,
    /// The model of the editable prompt line behind `C-/` undo and
    /// `C-\` redo, made on the first key typed at the prompt.
    pub(in crate::window) prompt_shadow: Option<PromptShadow>,
    pub(in crate::window) terminal: Terminal,
}

// ========================================================================
// OpenTabs Implementations
// ========================================================================

impl OpenTabs {
    // --------------------------------------------------------------------
    // Terminals
    // --------------------------------------------------------------------

    /// Open `terminal` as tab `id`, in the default mode.
    pub(in crate::window) fn insert_terminal(&mut self, id: TabId, terminal: Terminal) {
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
    pub(in crate::window) fn terminal(&self, id: &TabId) -> Option<&Terminal> {
        self.terminal_tab(id).map(|tab| &tab.terminal)
    }

    /// The terminal of tab `id`, to change.
    pub(in crate::window) fn terminal_mut(&mut self, id: &TabId) -> Option<&mut Terminal> {
        self.terminal_tab_mut(id).map(|tab| &mut tab.terminal)
    }

    /// Tab `id`, when it is a terminal.
    pub(in crate::window) fn terminal_tab(&self, id: &TabId) -> Option<&TerminalTab> {
        match self.tabs.get(id) {
            Some(Tab::Terminal(tab)) => Some(tab.as_ref()),
            Some(Tab::Editor(_)) | None => None,
        }
    }

    /// Tab `id`, when it is a terminal, to change.
    pub(in crate::window) fn terminal_tab_mut(&mut self, id: &TabId) -> Option<&mut TerminalTab> {
        match self.tabs.get_mut(id) {
            Some(Tab::Terminal(tab)) => Some(tab.as_mut()),
            Some(Tab::Editor(_)) | None => None,
        }
    }

    /// Whether tab `id` is a terminal.
    pub(in crate::window) fn is_terminal(&self, id: &TabId) -> bool {
        self.terminal_tab(id).is_some()
    }

    /// Every terminal tab's terminal, in no particular order.
    pub(in crate::window) fn terminals(&self) -> impl Iterator<Item = (&TabId, &Terminal)> {
        self.tabs.iter().filter_map(|(id, tab)| match tab {
            Tab::Terminal(tab) => Some((id, &tab.terminal)),
            Tab::Editor(_) => None,
        })
    }

    /// Every terminal tab's terminal, to change.
    pub(in crate::window) fn terminals_mut(
        &mut self,
    ) -> impl Iterator<Item = (&TabId, &mut Terminal)> {
        self.tabs.iter_mut().filter_map(|(id, tab)| match tab {
            Tab::Terminal(tab) => Some((id, &mut tab.terminal)),
            Tab::Editor(_) => None,
        })
    }

    /// Every terminal tab's terminal by id, as a session snapshot takes them.
    pub(in crate::window) fn terminal_map(&self) -> HashMap<TabId, &Terminal> {
        self.terminals()
            .map(|(id, terminal)| (*id, terminal))
            .collect()
    }

    /// Close every terminal tab, leaving the editors.
    pub(in crate::window) fn clear_terminals(&mut self) {
        self.tabs.retain(|_, tab| matches!(tab, Tab::Editor(_)));
    }

    /// Where tab `id`'s keystrokes go, or `None` for an editor, which takes
    /// its own keys rather than being in a mode.
    pub(in crate::window) fn mode(&self, id: &TabId) -> Option<Mode> {
        self.terminal_tab(id).map(|tab| tab.mode)
    }

    /// Put terminal tab `id` in `mode`. An editor has no mode to set.
    pub(in crate::window) fn set_mode(&mut self, id: &TabId, mode: Mode) {
        if let Some(tab) = self.terminal_tab_mut(id) {
            tab.mode = mode;
        }
    }

    /// Terminal tab `id`'s traversal cursor, if it has one.
    pub(in crate::window) fn nav_cursor(&self, id: &TabId) -> Option<(usize, usize)> {
        self.terminal_tab(id).and_then(|tab| tab.nav_cursor)
    }

    /// Set or drop terminal tab `id`'s traversal cursor. An editor keeps a
    /// cursor of its own.
    pub(in crate::window) fn set_nav_cursor(&mut self, id: &TabId, cursor: Option<(usize, usize)>) {
        if let Some(tab) = self.terminal_tab_mut(id) {
            tab.nav_cursor = cursor;
        }
    }

    /// Terminal tab `id`'s prompt model, once a key has been typed there.
    pub(in crate::window) fn prompt_shadow_mut(&mut self, id: &TabId) -> Option<&mut PromptShadow> {
        self.terminal_tab_mut(id)
            .and_then(|tab| tab.prompt_shadow.as_mut())
    }

    // --------------------------------------------------------------------
    // Editors
    // --------------------------------------------------------------------

    /// Open `editor`, of `kind`, as tab `id`: a buffer of its own, and the
    /// tab a view of it.
    pub(in crate::window) fn insert_editor(
        &mut self,
        id: TabId,
        kind: EditorKind,
        editor: Box<dyn Editor>,
    ) {
        // Run under the guard, so a panic in the editor stops it, not the
        // window.
        let picks_rows = editor.picks_rows();
        let buffer = self.buffers.insert(kind, Guarded::wrap(editor));
        if let Some(held) = self.buffers.get_mut(buffer) {
            held.loaded = Some(id);
        }
        let view = match picks_rows {
            true => EditorView::new(buffer).with_column(RowCursor::leftmost()),
            false => EditorView::new(buffer),
        };
        self.tabs
            .insert(id, Tab::Editor(Box::new(view)));
    }

    /// The editor of tab `id`.
    pub(in crate::window) fn editor(&self, id: &TabId) -> Option<EditorRef<'_>> {
        let view = self.editor_view(id)?;
        let buffer = self.buffers.get(view.buffer)?;
        Some(EditorRef::new(view, buffer))
    }

    /// The editor of tab `id`, to change, with tab `id`'s view loaded in it.
    pub(in crate::window) fn editor_mut(&mut self, id: &TabId) -> Option<EditorMut<'_>> {
        self.load_view(*id);
        let view = match self.tabs.get_mut(id) {
            Some(Tab::Editor(view)) => view.as_mut(),
            Some(Tab::Terminal(_)) | None => return None,
        };
        let buffer = self.buffers.get_mut(view.buffer)?;
        Some(EditorMut::new(view, buffer))
    }

    /// Make editor tab `id`'s view the one its buffer's editor holds, keeping
    /// a copy of the view held before in that view's own tab.
    pub(in crate::window) fn load_view(&mut self, id: TabId) {
        let Some(Tab::Editor(view)) = self.tabs.get(&id) else {
            return;
        };
        let Some(buffer) = self.buffers.get_mut(view.buffer) else {
            return;
        };
        if buffer.loaded == Some(id) {
            return;
        }
        if let Some(Tab::Editor(before)) = buffer.loaded.and_then(|tab| self.tabs.get_mut(&tab)) {
            before.state = buffer.editor.save_view();
        }
        if let Some(Tab::Editor(view)) = self.tabs.get_mut(&id) {
            if let Some(state) = view.state.take() {
                buffer.editor.restore_view(state);
            }
        }
        buffer.loaded = Some(id);
    }

    /// Open tab `id` as another view of the buffer editor tab `of` shows,
    /// starting where that view is. Returns whether `of` was an editor tab.
    pub(in crate::window) fn add_view(&mut self, id: TabId, of: &TabId) -> bool {
        let Some(view) = self.editor_view(of) else {
            return false;
        };
        let view = view.another();
        self.tabs.insert(id, Tab::Editor(Box::new(view)));
        true
    }

    /// Every editor tab showing the buffer tab `id` shows, `id` among them.
    pub(in crate::window) fn views_of(&self, id: &TabId) -> Vec<TabId> {
        let Some(buffer) = self.editor_view(id).map(|view| view.buffer) else {
            return Vec::new();
        };
        let mut views: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|(_, tab)| matches!(tab, Tab::Editor(view) if view.buffer == buffer))
            .map(|(tab, _)| *tab)
            .collect();
        views.sort_by_key(|tab| tab.0);
        views
    }

    /// Whether tab `id` is an editor.
    pub(in crate::window) fn is_editor(&self, id: &TabId) -> bool {
        self.editor_view(id).is_some()
    }

    /// Take editor tab `id` out, and its buffer with it when no other tab
    /// shows it, leaving a terminal tab where it is.
    pub(in crate::window) fn remove_editor(&mut self, id: &TabId) -> Option<Buffer> {
        self.editor_view(id)?;
        self.remove_view(id)
    }

    /// Every editor tab's editor, in no particular order.
    pub(in crate::window) fn editors(&self) -> impl Iterator<Item = (&TabId, EditorRef<'_>)> {
        self.tabs.iter().filter_map(|(id, tab)| match tab {
            Tab::Editor(view) => self
                .buffers
                .get(view.buffer)
                .map(|buffer| (id, EditorRef::new(view, buffer))),
            Tab::Terminal(_) => None,
        })
    }

    /// Every buffer, to change, whichever tabs show it.
    pub(in crate::window) fn buffers_mut(&mut self) -> impl Iterator<Item = &mut Buffer> {
        self.buffers.iter_mut().map(|(_, buffer)| buffer)
    }

    /// Bury editor tab `id`'s buffer: no pane shows it now, so its editor
    /// stops refreshing and lets go of what it can build again.
    pub(in crate::window) fn bury(&mut self, id: TabId) {
        self.buried.retain(|tab| *tab != id);
        self.buried.push(id);
        if let Some(slot) = self.editor_mut(&id) {
            slot.editor.set_shown(false);
            slot.editor.shed_caches();
        }
    }

    /// Take tab `id` off the buried, as a pane shows it again. Returns whether
    /// it was buried.
    pub(in crate::window) fn unbury(&mut self, id: TabId) -> bool {
        let before = self.buried.len();
        self.buried.retain(|tab| *tab != id);
        let was_buried = self.buried.len() != before;
        if was_buried {
            if let Some(slot) = self.editor_mut(&id) {
                slot.editor.set_shown(true);
            }
        }
        was_buried
    }

    /// The editor tabs whose buffers are buried, the most recently buried last.
    pub(in crate::window) fn buried(&self) -> &[TabId] {
        &self.buried
    }

    /// Tab `id`'s view, when it is an editor.
    fn editor_view(&self, id: &TabId) -> Option<&EditorView> {
        match self.tabs.get(id) {
            Some(Tab::Editor(view)) => Some(view.as_ref()),
            Some(Tab::Terminal(_)) | None => None,
        }
    }

    // --------------------------------------------------------------------
    // Either kind
    // --------------------------------------------------------------------

    /// Close tab `id`, whichever kind it is, letting an editor's buffer go
    /// with it.
    pub(in crate::window) fn remove(&mut self, id: &TabId) -> Option<Tab> {
        if self.editor_view(id).is_some() {
            self.remove_view(id);
            return None;
        }
        self.tabs.remove(id)
    }

    /// Close editor tab `id`'s view, letting its buffer go when no other
    /// view is left, and handing that buffer back.
    fn remove_view(&mut self, id: &TabId) -> Option<Buffer> {
        let Some(Tab::Editor(view)) = self.tabs.remove(id) else {
            return None;
        };
        self.buried.retain(|tab| tab != id);
        let shown_elsewhere = self
            .tabs
            .values()
            .any(|tab| matches!(tab, Tab::Editor(other) if other.buffer == view.buffer));
        if shown_elsewhere {
            if let Some(buffer) = self.buffers.get_mut(view.buffer) {
                if buffer.loaded == Some(*id) {
                    buffer.loaded = None;
                }
            }
            return None;
        }
        self.buffers.remove(view.buffer)
    }
}
