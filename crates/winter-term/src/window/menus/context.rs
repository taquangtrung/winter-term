//! The right-click context menu: what it offers where it was opened (copy,
//! paste, a link, the focused editor's own actions), hovering it, and running
//! the item picked.

use winter_render::TabbarHit;

use crate::model::paint::Point;

use crate::model::input::Key;
use crate::model::layout::TabId;
use crate::window::App;
use crate::window::Chrome;

// ========================================================================
// Data Structures
// ========================================================================

/// An action dispatched from a right-click context menu item.
#[derive(Clone)]
pub(in crate::window) enum ContextAction {
    Copy,
    OpenLink(String),
    Paste,
    /// A line between two groups of entries, which does nothing.
    Separator,
    /// An entry an editor offered, standing for a key that editor binds.
    EditorKey(EditorKeyAction),
}

/// A menu entry an editor offered: what it is called, and the keys of its
/// own the editor is typed when it is chosen.
#[derive(Clone)]
pub(in crate::window) struct EditorKeyAction {
    pub(in crate::window) keys: Vec<Key>,
    pub(in crate::window) label: String,
    /// The pane whose editor offered it, since the menu outlives the click and
    /// the focus could move under it.
    pub(in crate::window) tab: TabId,
}

// ========================================================================
// App: context menu
// ========================================================================

impl App {
    /// Close the right-click context menu.
    pub(in crate::window) fn close_context_menu(&mut self) {
        if self.chrome.menus.context_pos.is_some() {
            self.chrome.menus.context_pos = None;
            self.chrome.menus.context_url = None;
            self.chrome.menus.context_actions.clear();
            self.chrome.menus.context_selected = None;
            self.dirty = true;
        }
    }

    /// Open a right-click context menu at `(x, y)`.
    ///
    /// Over an editor the menu is the editor's own, because the terminal's
    /// entries mean nothing there: a paste would go to the shell running
    /// underneath the listing, which is not what was clicked on.
    pub(in crate::window) fn open_context_menu(&mut self, x: f32, y: f32) {
        use ContextAction;
        let over_page = self
            .pane_at_pixel(x, y)
            .is_some_and(|(tab_id, _)| self.open_tabs.is_editor(&tab_id));
        let mut actions: Vec<ContextAction> = match over_page {
            true => self.editor_context_actions(x, y),
            false => Vec::new(),
        };
        let mut window = Vec::new();
        if self.selection.span.is_some() {
            window.push(ContextAction::Copy);
        }
        if !over_page {
            window.push(ContextAction::Paste);
            if let Some(url) = &self.pointer.hovered_url {
                window.push(ContextAction::OpenLink(url.clone()));
            }
        }
        // The window's own entries are a group apart from the editor's.
        if !actions.is_empty() && !window.is_empty() {
            actions.push(ContextAction::Separator);
        }
        actions.extend(window);
        // An editor with nothing to offer over this row gets no empty card.
        if actions.is_empty() {
            self.close_context_menu();
            return;
        }
        self.chrome.menus.context_pos = Some((x, y));
        self.chrome.menus.context_url = self.pointer.hovered_url.clone();
        self.chrome.menus.context_actions = actions;
        self.chrome.menus.context_selected = None;
        self.close_menu();
        self.dirty = true;
    }

    /// What the editor under `(x, y)` offers over the row that was
    /// clicked.
    ///
    /// The pointer moves the editor's cursor first, so the menu acts on the row
    /// it was opened over rather than wherever the keyboard last left the
    /// cursor: right-clicking a file in Git and choosing "stage it" stages
    /// that file, not the one that happened to be under the cursor.
    fn editor_context_actions(&mut self, x: f32, y: f32) -> Vec<ContextAction> {
        let Some((tab_id, pane_rect)) = self.pane_at_pixel(x, y) else {
            return Vec::new();
        };
        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
        let at = Point {
            col,
            drag: false,
            row,
        };
        let outcome = match self.open_tabs.editor_mut(&tab_id) {
            Some(slot) => slot.editor.on_context_mouse(slot.view.skipped.editor_point(at)),
            None => return Vec::new(),
        };
        self.act_on_editor_outcome(tab_id, outcome);
        let Some(slot) = self.open_tabs.editor(&tab_id) else {
            return Vec::new();
        };
        slot.editor
            .context_items()
            .into_iter()
            .map(|item| match item.is_separator() {
                true => ContextAction::Separator,
                false => ContextAction::EditorKey(EditorKeyAction {
                    keys: item.keys,
                    label: item.label,
                    tab: tab_id,
                }),
            })
            .collect()
    }


    /// Carry out a context-menu entry. Both the tabbar's click handling and
    /// the window's own reach this, so an entry means one thing wherever the
    /// click that chose it was resolved.
    pub(in crate::window) fn run_context_action(&mut self, action: ContextAction) {
        match action {
            ContextAction::Copy => self.copy_selection(),
            ContextAction::Paste => self.paste_from_clipboard(),
            ContextAction::OpenLink(url) => self.open_link(&url),
            ContextAction::Separator => {}
            ContextAction::EditorKey(action) => {
                for key in &action.keys {
                    self.offer_key_to_editor(action.tab, key);
                }
            }
        }
    }

    /// Update the hovered context menu item from the pointer position while the
    /// context menu is open.
    pub(in crate::window) fn update_context_menu_hover(&mut self, x: f32, y: f32) {
        if self.chrome.menus.context_pos.is_none() {
            return;
        }
        let Some((cw, ch)) = self.renderer.as_ref().map(|r| r.cell_size()) else {
            return;
        };
        let surface_w = self.viewport_rect().width;
        let tabbar = self.build_top_tabbar();
        let hit = winter_render::hit_test(&tabbar, surface_w, cw, ch, x, y);
        // A separator is no entry to light.
        let new_sel = match hit {
            TabbarHit::ContextMenuItem(i) => Some(i).filter(|i| {
                !matches!(
                    self.chrome.menus.context_actions.get(*i),
                    Some(ContextAction::Separator)
                )
            }),
            _ => None,
        };
        if new_sel != self.chrome.menus.context_selected {
            self.chrome.menus.context_selected = new_sel;
            self.dirty = true;
        }
    }
}

// ========================================================================
// Chrome
// ========================================================================

impl Chrome {
    /// The context menu's entry at `index`, when it is one a click runs: a
    /// separator is not, and a click on it leaves the menu open.
    pub(in crate::window) fn context_entry(&self, index: usize) -> Option<ContextAction> {
        self.menus
            .context_actions
            .get(index)
            .filter(|action| !matches!(action, ContextAction::Separator))
            .cloned()
    }
}
