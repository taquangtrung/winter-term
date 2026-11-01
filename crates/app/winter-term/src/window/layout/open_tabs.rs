//! Every open tab and what it holds: a terminal, a page Winter paints
//! itself, or a view of an editor, a page whose contents are one of the
//! window's buffers. The layout says where a tab sits; this says what it is.

use std::collections::{HashMap, HashSet};

use std::time::Instant;

use winter_model::editor::{Editor, Page};
use winter_model::guard::{guard_editor, guard_page};
use winter_model::layout::TabId;
use winter_model::mode::Mode;
use winter_terminal::session::Terminal;

use crate::window::editor::buffers::{Buffer, Buffers, EditorTab, PageMut, PageRef, TabView};
use crate::window::editor::column::RowCursor;
use crate::window::editor::EditorKind;
use crate::window::terminal::prompt_edit::PromptShadow;

// ========================================================================
// Data Structures
// ========================================================================

/// The tabs of the window, by id. A tab is a terminal, a page, or an editor,
/// so asking for one kind of a tab of another answers `None`; an editor is a
/// page too, and answers as one.
#[derive(Default)]
pub(in crate::window) struct OpenTabs {
    /// The editors the editor tabs show.
    buffers: Buffers,
    /// The editor tabs no pane shows, their buffers buried, the most recently
    /// buried last.
    buried: Vec<TabId>,
    tabs: HashMap<TabId, Tab>,
}

/// What one tab holds. Every kind is boxed: each is several hundred bytes
/// or more, and a tab is only ever reached through the map.
pub(in crate::window) enum Tab {
    /// A view of an editor's buffer: a file, a listing, the git view.
    Editor(Box<EditorTab>),
    /// A page Winter paints of its own state: its settings, its keys.
    Page(Box<PageTab>),
    /// A shell or another program on a pseudo-terminal.
    Terminal(Box<TerminalTab>),
}

/// A page tab: the page, which goes when the tab does, and its view.
pub(in crate::window) struct PageTab {
    kind: EditorKind,
    page: Box<dyn Page>,
    /// The earliest tick the page has asked for since its last one, kept as
    /// a buffer's is.
    tick_at: Option<Instant>,
    view: TabView,
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
            Some(Tab::Editor(_) | Tab::Page(_)) | None => None,
        }
    }

    /// Tab `id`, when it is a terminal, to change.
    pub(in crate::window) fn terminal_tab_mut(&mut self, id: &TabId) -> Option<&mut TerminalTab> {
        match self.tabs.get_mut(id) {
            Some(Tab::Terminal(tab)) => Some(tab.as_mut()),
            Some(Tab::Editor(_) | Tab::Page(_)) | None => None,
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
            Tab::Editor(_) | Tab::Page(_) => None,
        })
    }

    /// Every terminal tab's terminal, to change.
    pub(in crate::window) fn terminals_mut(
        &mut self,
    ) -> impl Iterator<Item = (&TabId, &mut Terminal)> {
        self.tabs.iter_mut().filter_map(|(id, tab)| match tab {
            Tab::Terminal(tab) => Some((id, &mut tab.terminal)),
            Tab::Editor(_) | Tab::Page(_) => None,
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
        self.tabs
            .retain(|_, tab| matches!(tab, Tab::Editor(_) | Tab::Page(_)));
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
        let view = first_view(editor.picks_rows());
        let buffer = self.buffers.insert(kind, guard_editor(editor));
        if let Some(held) = self.buffers.get_mut(buffer) {
            held.loaded = Some(id);
        }
        self.tabs
            .insert(id, Tab::Editor(Box::new(EditorTab::new(buffer, view))));
    }

    /// Open `page`, of `kind`, as tab `id`.
    pub(in crate::window) fn insert_page(
        &mut self,
        id: TabId,
        kind: EditorKind,
        page: Box<dyn Page>,
    ) {
        let view = first_view(page.picks_rows());
        let tab = PageTab {
            kind,
            page: guard_page(page),
            tick_at: None,
            view,
        };
        self.tabs.insert(id, Tab::Page(Box::new(tab)));
    }

    /// The page tab `id` shows: its own, or its editor's.
    pub(in crate::window) fn page(&self, id: &TabId) -> Option<PageRef<'_>> {
        match self.tabs.get(id)? {
            Tab::Editor(tab) => Some(PageRef::of_editor(tab, self.buffers.get(tab.buffer)?)),
            Tab::Page(tab) => Some(PageRef::of_page(
                tab.kind,
                tab.page.as_ref(),
                tab.tick_at,
                &tab.view,
            )),
            Tab::Terminal(_) => None,
        }
    }

    /// The same, to change, with an editor tab's view loaded in its editor.
    pub(in crate::window) fn page_mut(&mut self, id: &TabId) -> Option<PageMut<'_>> {
        self.load_view(*id);
        match self.tabs.get_mut(id)? {
            Tab::Editor(tab) => {
                let buffer = self.buffers.get_mut(tab.buffer)?;
                Some(PageMut::of_editor(tab, buffer))
            }
            Tab::Page(tab) => Some(PageMut::of_page(
                tab.kind,
                tab.page.as_mut(),
                &mut tab.tick_at,
                &mut tab.view,
            )),
            Tab::Terminal(_) => None,
        }
    }

    /// Make editor tab `id`'s view the one its buffer's editor holds, keeping
    /// a copy of the view held before in that view's own tab.
    pub(in crate::window) fn load_view(&mut self, id: TabId) {
        let Some(Tab::Editor(tab)) = self.tabs.get(&id) else {
            return;
        };
        let Some(buffer) = self.buffers.get_mut(tab.buffer) else {
            return;
        };
        if buffer.loaded == Some(id) {
            return;
        }
        if let Some(Tab::Editor(before)) = buffer.loaded.and_then(|tab| self.tabs.get_mut(&tab)) {
            before.state = buffer.editor.save_view();
        }
        if let Some(Tab::Editor(tab)) = self.tabs.get_mut(&id) {
            if let Some(state) = tab.state.take() {
                buffer.editor.restore_view(state);
            }
        }
        buffer.loaded = Some(id);
    }

    /// Open tab `id` as another view of the buffer editor tab `of` shows,
    /// starting where that view is. Returns whether `of` was an editor tab.
    pub(in crate::window) fn add_view(&mut self, id: TabId, of: &TabId) -> bool {
        let Some(tab) = self.editor_tab(of) else {
            return false;
        };
        let tab = tab.another();
        self.tabs.insert(id, Tab::Editor(Box::new(tab)));
        true
    }

    /// Every editor tab showing the buffer tab `id` shows, `id` among them.
    pub(in crate::window) fn views_of(&self, id: &TabId) -> Vec<TabId> {
        let Some(buffer) = self.editor_tab(id).map(|tab| tab.buffer) else {
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

    /// Whether tab `id` is one Winter paints: a page or an editor.
    pub(in crate::window) fn is_page(&self, id: &TabId) -> bool {
        matches!(self.tabs.get(id), Some(Tab::Editor(_) | Tab::Page(_)))
    }

    /// Take editor tab `id` out, and its buffer with it when no other tab
    /// shows it, leaving a terminal tab where it is.
    pub(in crate::window) fn remove_editor(&mut self, id: &TabId) -> Option<Buffer> {
        self.editor_tab(id)?;
        self.remove_view(id)
    }

    /// Every tab Winter paints, page or editor, in no particular order.
    pub(in crate::window) fn pages(&self) -> impl Iterator<Item = (&TabId, PageRef<'_>)> {
        self.tabs.iter().filter_map(|(id, tab)| match tab {
            Tab::Editor(tab) => self
                .buffers
                .get(tab.buffer)
                .map(|buffer| (id, PageRef::of_editor(tab, buffer))),
            Tab::Page(tab) => Some((
                id,
                PageRef::of_page(tab.kind, tab.page.as_ref(), tab.tick_at, &tab.view),
            )),
            Tab::Terminal(_) => None,
        })
    }

    /// Every page tab's page, to change: those holding no buffer, which
    /// [`Self::buffers_mut`] does not reach.
    pub(in crate::window) fn own_pages_mut(&mut self) -> impl Iterator<Item = &mut Box<dyn Page>> {
        self.page_ticks_mut().map(|(page, _)| page)
    }

    /// The same, each with the tick it is kept to, as a buffer's is.
    pub(in crate::window) fn page_ticks_mut(
        &mut self,
    ) -> impl Iterator<Item = (&mut Box<dyn Page>, &mut Option<Instant>)> {
        self.tabs.values_mut().filter_map(|tab| match tab {
            Tab::Page(tab) => Some((&mut tab.page, &mut tab.tick_at)),
            Tab::Editor(_) | Tab::Terminal(_) => None,
        })
    }

    /// Every buffer, to change, whichever tabs show it.
    pub(in crate::window) fn buffers_mut(&mut self) -> impl Iterator<Item = &mut Buffer> {
        self.buffers.iter_mut().map(|(_, buffer)| buffer)
    }

    /// Bury editor tab `id`'s buffer: no pane shows it now, so its editor
    /// stops refreshing and lets go of what it can build again.
    pub(in crate::window) fn bury(&mut self, id: TabId) {
        let Some(editor) = self.buffer_editor_mut(&id) else {
            return;
        };
        editor.set_shown(false);
        editor.shed_caches();
        self.buried.retain(|tab| *tab != id);
        self.buried.push(id);
    }

    /// Take tab `id` off the buried, as a pane shows it again. Returns whether
    /// it was buried.
    pub(in crate::window) fn unbury(&mut self, id: TabId) -> bool {
        let before = self.buried.len();
        self.buried.retain(|tab| *tab != id);
        let was_buried = self.buried.len() != before;
        if was_buried {
            if let Some(editor) = self.buffer_editor_mut(&id) {
                editor.set_shown(true);
            }
        }
        was_buried
    }

    /// The editor tabs whose buffers are buried, the most recently buried last.
    pub(in crate::window) fn buried(&self) -> &[TabId] {
        &self.buried
    }

    /// Tab `id`, when it is an editor.
    fn editor_tab(&self, id: &TabId) -> Option<&EditorTab> {
        match self.tabs.get(id) {
            Some(Tab::Editor(tab)) => Some(tab.as_ref()),
            Some(Tab::Page(_) | Tab::Terminal(_)) | None => None,
        }
    }

    /// The editor of the buffer editor tab `id` shows, with its view loaded.
    fn buffer_editor_mut(&mut self, id: &TabId) -> Option<&mut Box<dyn Editor>> {
        self.load_view(*id);
        let buffer = self.editor_tab(id)?.buffer;
        Some(&mut self.buffers.get_mut(buffer)?.editor)
    }

    // --------------------------------------------------------------------
    // Either kind
    // --------------------------------------------------------------------

    /// Close tab `id`, whichever kind it is, letting an editor's buffer go
    /// with it.
    pub(in crate::window) fn remove(&mut self, id: &TabId) -> Option<Tab> {
        if self.editor_tab(id).is_some() {
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

// ========================================================================
// Functions
// ========================================================================

/// The view a page or an editor is first shown in: a listing's cursor at
/// the start of its rows.
fn first_view(picks_rows: bool) -> TabView {
    match picks_rows {
        true => TabView::new().with_column(RowCursor::leftmost()),
        false => TabView::new(),
    }
}
