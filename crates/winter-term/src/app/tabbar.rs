//! Top-tabbar glue: build the [`TopTabbar`] the renderer draws from the app's
//! tab/menu state, and dispatch clicks the renderer's [`hit_test`] resolves.
//!
//! The menu commands reuse the same command names as the command palette (see
//! [`App::run_command`]), so a menu item and a palette entry that do the same
//! thing share one dispatch path.

use winter_render::{ContextMenu, Menu, MenuItem, MenuStyle, TabLabel, TabbarHit, TopTabbar};

use super::App;
use crate::config::TitleBarStyle;

// ========================================================================
// Data Structures
// ========================================================================

/// A menu line: its display `label`, an optional `shortcut` hint, and the
/// `command` dispatched by [`App::run_command`]. An item with `children` is a
/// submenu parent that opens a child panel on hover instead of running a command.
struct ItemDef {
    children: &'static [ItemDef],
    command: &'static str,
    label: &'static str,
    shortcut: &'static str,
}

/// One menu: a `title` (shown only in the classic menubar) and its items.
struct MenuDef {
    items: &'static [ItemDef],
    title: &'static str,
}

/// A command leaf: no children.
const fn leaf(command: &'static str, label: &'static str, shortcut: &'static str) -> ItemDef {
    ItemDef {
        children: &[],
        command,
        label,
        shortcut,
    }
}

/// A submenu parent: a label and its children, with no command of its own.
const fn parent(label: &'static str, children: &'static [ItemDef]) -> ItemDef {
    ItemDef {
        children,
        command: "",
        label,
        shortcut: "",
    }
}

/// A horizontal separator line.
const SEPARATOR: ItemDef = leaf("-", "-", "");

// ========================================================================
// Menu definitions
// ========================================================================

const LAYOUT_ITEMS: &[ItemDef] = &[
    leaf("split_vertical", "Split Vertical", ""),
    leaf("split_horizontal", "Split Horizontal", ""),
    leaf("toggle_pane_zoom", "Zoom Pane", "Ctrl-Shift-M"),
    leaf("close_pane", "Close Pane", ""),
];

const SEARCH_ITEMS: &[ItemDef] = &[
    leaf("search", "Search Blocks", ""),
    leaf("quick_select", "Quick Select", ""),
    leaf("toggle_fold", "Toggle Fold", ""),
];

const MODERN_ITEMS: &[ItemDef] = &[
    leaf("new_tab", "New Tab", "Ctrl-Shift-T"),
    leaf("close_tab", "Close Tab", "Ctrl-Shift-W"),
    leaf("rename_tab", "Rename Tab", ""),
    SEPARATOR,
    parent("Layout", LAYOUT_ITEMS),
    parent("Search & Fold", SEARCH_ITEMS),
    SEPARATOR,
    leaf("open_settings", "Settings", "Ctrl-,"),
];

const MODERN_MENUS: &[MenuDef] = &[MenuDef {
    items: MODERN_ITEMS,
    title: "Menu",
}];

const CLASSIC_MENUS: &[MenuDef] = &[
    MenuDef {
        title: "File",
        items: &[
            leaf("new_tab", "New Tab", "Ctrl-Shift-T"),
            leaf("close_tab", "Close Tab", "Ctrl-Shift-W"),
            leaf("rename_tab", "Rename Tab", ""),
            SEPARATOR,
            leaf("close_pane", "Close Pane", ""),
            SEPARATOR,
            leaf("open_settings", "Settings", "Ctrl-,"),
        ],
    },
    MenuDef {
        title: "Edit",
        items: &[
            leaf("search", "Search Blocks", ""),
            leaf("quick_select", "Quick Select", ""),
            SEPARATOR,
            leaf("yank_block", "Yank Block Source", ""),
        ],
    },
    MenuDef {
        title: "View",
        items: &[
            leaf("split_vertical", "Split Vertical", ""),
            leaf("split_horizontal", "Split Horizontal", ""),
            SEPARATOR,
            leaf("toggle_fold", "Toggle Fold", ""),
        ],
    },
    MenuDef {
        title: "Window",
        items: &[
            leaf("focus_left", "Focus Left", ""),
            leaf("focus_down", "Focus Down", ""),
            leaf("focus_up", "Focus Up", ""),
            leaf("focus_right", "Focus Right", ""),
        ],
    },
];

/// The menus for a given style.
fn menu_defs(style: MenuStyle) -> &'static [MenuDef] {
    match style {
        MenuStyle::Classic => CLASSIC_MENUS,
        MenuStyle::Modern => MODERN_MENUS,
    }
}

/// Recursively convert a static [`ItemDef`] into the renderer's [`MenuItem`].
fn menu_item(item: &ItemDef) -> MenuItem {
    MenuItem {
        children: item.children.iter().map(menu_item).collect(),
        label: item.label.to_string(),
        shortcut: item.shortcut.to_string(),
    }
}

// ========================================================================
// App — tabbar model and interaction
// ========================================================================

impl App {
    pub(crate) fn tab_title(&self, tab_index: usize) -> String {
        if let Some(name) = self.tab_names.get(&tab_index) {
            return name.clone();
        }
        let focused = self.tabs[tab_index].focused();
        // An OSC 0/2 title is the app's own presentation of itself (butterfly,
        // for one, puts the open file's name there), so it wins over the
        // process-name/cwd heuristic below.
        if let Some(title) = self.pane_titles.get(&focused).filter(|t| !t.is_empty()) {
            return clean_title(title);
        }
        if let Some(pane) = self.panes.get(&focused) {
            if let Some(session) = pane.mux_session() {
                return format!("mux: {session}");
            }
            if let Some(proc_name) = pane.foreground_process_name() {
                if let Some(cwd) = pane.cwd() {
                    return format!("{}: {}", proc_name, clean_cwd(&cwd));
                }
                return proc_name;
            }
            if let Some(cwd) = pane.cwd() {
                return clean_cwd(&cwd);
            }
        }
        format!("Terminal {}", tab_index + 1)
    }
}

pub(crate) fn clean_cwd(cwd: &str) -> String {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return cwd.to_string(),
    };
    if cwd == home {
        return "~".to_string();
    }
    let suffix = match cwd.strip_prefix(&format!("{}/", home)) {
        Some(s) => s,
        None => return cwd.to_string(),
    };
    // Abbreviate every intermediate directory component to its first character,
    // keeping only the leaf component in full so a long project path stays
    // compact in the tab title while still being recognizable:
    //   ~/Workspace/apps/winter-term -> ~/W/a/winter-term
    let parts: Vec<&str> = suffix.split('/').collect();
    if parts.len() <= 1 {
        return format!("~/{}", suffix);
    }
    let (head, tail) = parts.split_at(parts.len() - 1);
    let abbrev: Vec<String> = head
        .iter()
        .map(|c| {
            c.chars()
                .next()
                .map(|ch| ch.to_string())
                .unwrap_or_default()
        })
        .collect();
    format!("~/{}/{}", abbrev.join("/"), tail[0])
}

pub(crate) fn clean_title(title: &str) -> String {
    if let Some(idx) = title.find('@') {
        if let Some(colon_idx) = title[idx..].find(':') {
            return title[idx + colon_idx + 1..].trim().to_string();
        }
    }
    title.to_string()
}

impl App {
    /// The static def of item `i` in the currently open menu, if any.
    fn open_menu_item(&self, i: usize) -> Option<&'static ItemDef> {
        let open = self.open_menu?;
        menu_defs(self.config.menu_style).get(open)?.items.get(i)
    }

    /// The static def of submenu child `child` under the open submenu parent.
    fn open_submenu_item(&self, child: usize) -> Option<&'static ItemDef> {
        let open = self.open_menu?;
        let parent = self.open_submenu?;
        let item = menu_defs(self.config.menu_style)
            .get(open)?
            .items
            .get(parent)?;
        item.children.get(child)
    }

    /// Build the tabbar model for this frame from the current tab/menu state.
    pub(crate) fn build_top_tabbar(&self) -> TopTabbar {
        let tabs = (0..self.tabs.len())
            .map(|i| TabLabel {
                title: if i == self.active_tab {
                    if let Some(input) = &self.tab_rename_input {
                        format!("{input}\u{2502}")
                    } else {
                        self.tab_title(i)
                    }
                } else {
                    self.tab_title(i)
                },
                zoomed: self.tabs[i].is_zoomed(),
            })
            .collect();
        let menus = menu_defs(self.config.menu_style)
            .iter()
            .map(|menu| Menu {
                items: menu.items.iter().map(menu_item).collect(),
                title: menu.title.to_string(),
            })
            .collect();
        let context_menu = self.context_menu_pos.map(|(x, y)| ContextMenu {
            items: self
                .context_menu_actions
                .iter()
                .map(|a| MenuItem {
                    children: vec![],
                    label: match a {
                        super::ContextAction::Copy => "Copy".into(),
                        super::ContextAction::Paste => "Paste".into(),
                        super::ContextAction::OpenLink(_) => "Open Link".into(),
                    },
                    shortcut: String::new(),
                })
                .collect(),
            selected: self.context_menu_selected,
            x,
            y,
        });
        let url_tooltip = if let Some(url) = &self.hovered_url {
            let (cx, cy) = self.cursor_pos;
            Some((url.clone(), cx, cy))
        } else if let TabbarHit::Tab(idx) = self.tabbar_hover {
            let (cx, cy) = self.tab_hover_pos.unwrap_or(self.cursor_pos);
            let full_title = self.tab_title(idx);
            Some((full_title, cx, cy))
        } else {
            None
        };
        TopTabbar {
            active_tab: self.active_tab,
            tabbar_hover: self.tabbar_hover,
            context_menu,
            controls_side: self.config.window_controls_side,
            menu_style: self.config.menu_style,
            menus,
            open_menu: self.open_menu,
            open_submenu: self.open_submenu,
            selected_item: self.selected_item,
            selected_subitem: self.selected_subitem,
            tabs,
            url_tooltip,
            window_controls: self.config.title_bar_style == TitleBarStyle::Modern,
        }
    }

    /// Close the right-click context menu.
    pub(crate) fn close_context_menu(&mut self) {
        if self.context_menu_pos.is_some() {
            self.context_menu_pos = None;
            self.context_menu_url = None;
            self.context_menu_actions.clear();
            self.context_menu_selected = None;
            self.dirty = true;
        }
    }

    /// Open a right-click context menu at `(x, y)`.
    pub(crate) fn open_context_menu(&mut self, x: f32, y: f32) {
        use super::ContextAction;
        let mut actions: Vec<ContextAction> = Vec::new();
        if self.selection.is_some() {
            actions.push(ContextAction::Copy);
        }
        actions.push(ContextAction::Paste);
        if let Some(url) = &self.hovered_url {
            actions.push(ContextAction::OpenLink(url.clone()));
        }
        self.context_menu_pos = Some((x, y));
        self.context_menu_url = self.hovered_url.clone();
        self.context_menu_actions = actions;
        self.context_menu_selected = None;
        self.close_menu();
        self.dirty = true;
    }

    /// Update the hovered context menu item from the pointer position while the
    /// context menu is open.
    pub(crate) fn update_context_menu_hover(&mut self, x: f32, y: f32) {
        if self.context_menu_pos.is_none() {
            return;
        }
        let Some((cw, ch)) = self.renderer.as_ref().map(|r| r.cell_size()) else {
            return;
        };
        let surface_w = self.viewport_rect().width;
        let tabbar = self.build_top_tabbar();
        let hit = winter_render::hit_test(&tabbar, surface_w, cw, ch, x, y);
        let new_sel = match hit {
            TabbarHit::ContextMenuItem(i) => Some(i),
            _ => None,
        };
        if new_sel != self.context_menu_selected {
            self.context_menu_selected = new_sel;
            self.dirty = true;
        }
    }

    /// Pixel height of the reserved top-tabbar band.
    pub(crate) fn top_chrome_height(&self) -> f32 {
        let ch = self
            .renderer
            .as_ref()
            .map(|r| r.cell_size().1)
            .unwrap_or(0.0);
        ch * self.top_chrome_rows() as f32
    }

    /// Handle a left-click at `(x, y)` against the tabbar. Returns `true` when the
    /// click was consumed (a tabbar element, or dismissing an open menu) and the
    /// caller should not treat it as a pane click.
    pub(crate) fn handle_tabbar_click(&mut self, x: f32, y: f32) -> bool {
        let Some((cw, ch)) = self.renderer.as_ref().map(|r| r.cell_size()) else {
            return false;
        };
        let surface_w = self.viewport_rect().width;
        let tabbar = self.build_top_tabbar();
        let hit = winter_render::hit_test(&tabbar, surface_w, cw, ch, x, y);
        let menu_was_open = self.open_menu.is_some();
        let focused = self.tab().focused();

        match hit {
            TabbarHit::Tab(i) => {
                self.close_menu();
                self.switch_tab(i);
                self.tab_drag_start = Some((i, x));
            }
            TabbarHit::CloseTab(i) => {
                self.close_menu();
                self.close_tab(i);
            }
            TabbarHit::NewTab => {
                self.close_menu();
                self.new_tab();
            }
            TabbarHit::ScrollTabsLeft => {
                self.close_menu();
                if self.active_tab > 0 {
                    self.switch_tab(self.active_tab - 1);
                }
            }
            TabbarHit::ScrollTabsRight => {
                self.close_menu();
                if self.active_tab + 1 < self.tabs.len() {
                    self.switch_tab(self.active_tab + 1);
                }
            }
            TabbarHit::Hamburger => {
                if self.open_menu.is_some() {
                    self.close_menu();
                } else {
                    self.toggle_menu(0);
                }
            }
            TabbarHit::Minimize => {
                self.close_menu();
                if let Some(window) = &self.window {
                    window.set_minimized(true);
                }
            }
            TabbarHit::Maximize => {
                self.close_menu();
                if let Some(window) = &self.window {
                    window.set_maximized(!window.is_maximized());
                }
            }
            TabbarHit::Close => {
                // Drained by the mouse handler into the shared quit path.
                self.exit_requested = true;
            }
            TabbarHit::MenuTitle(i) => self.toggle_menu(i),
            TabbarHit::DropdownItem(i) => {
                if let Some(item) = self.open_menu_item(i) {
                    if item.label == "-" {
                        return true;
                    }
                    if item.children.is_empty() {
                        let command = item.command;
                        self.close_menu();
                        self.run_command(command, focused);
                    } else {
                        // A submenu parent: keep it open and expand its children.
                        self.open_submenu = Some(i);
                        self.selected_item = Some(i);
                        self.selected_subitem = None;
                    }
                }
            }
            TabbarHit::SubmenuItem(child) => {
                if let Some(item) = self.open_submenu_item(child) {
                    if item.label != "-" {
                        let command = item.command;
                        self.close_menu();
                        self.run_command(command, focused);
                    }
                }
            }
            TabbarHit::ContextMenuItem(i) => {
                if let Some(action) = self.context_menu_actions.get(i).cloned() {
                    self.close_context_menu();
                    match action {
                        super::ContextAction::Copy => self.copy_selection(),
                        super::ContextAction::Paste => self.paste_from_clipboard(),
                        super::ContextAction::OpenLink(url) => {
                            let scheme = url.split(':').next().unwrap_or("").to_ascii_lowercase();
                            if matches!(scheme.as_str(), "http" | "https" | "mailto") {
                                let _ = open::that(&url);
                            }
                        }
                    }
                }
            }
            TabbarHit::None => {
                if self.context_menu_pos.is_some() {
                    self.close_context_menu();
                    return true;
                }
                if menu_was_open {
                    self.close_menu();
                } else if y < self.top_chrome_height() {
                    // Empty title-bar space. In the borderless modern style this is
                    // the drag handle to move the window; either way, swallow the
                    // click so it does not select pane text.
                    if self.config.title_bar_style == TitleBarStyle::Modern {
                        if let Some(window) = &self.window {
                            let _ = window.drag_window();
                        }
                    }
                    return true;
                } else {
                    return false;
                }
            }
        }
        self.dirty = true;
        true
    }

    /// Update the hovered menu/submenu rows from the pointer position while a menu
    /// is open, requesting a redraw when the highlight changes. Hovering a submenu
    /// parent opens its child panel; the parent stays open throughout.
    pub(crate) fn update_menu_hover(&mut self, x: f32, y: f32) {
        if self.open_menu.is_none() {
            return;
        }
        let Some((cw, ch)) = self.renderer.as_ref().map(|r| r.cell_size()) else {
            return;
        };
        let surface_w = self.viewport_rect().width;
        let tabbar = self.build_top_tabbar();
        let hit = winter_render::hit_test(&tabbar, surface_w, cw, ch, x, y);

        // (selected_item, open_submenu, selected_subitem) for this hover.
        let next = match hit {
            TabbarHit::DropdownItem(i) => match self.open_menu_item(i) {
                Some(item) if item.label == "-" => (None, self.open_submenu, None),
                Some(item) if !item.children.is_empty() => (Some(i), Some(i), None),
                Some(_) => (Some(i), None, None),
                None => (None, self.open_submenu, None),
            },
            // Keep the parent highlighted and its submenu open over its children.
            TabbarHit::SubmenuItem(child) => (self.open_submenu, self.open_submenu, Some(child)),
            // Off the items: leave the submenu open so the cursor can reach it.
            _ => (None, self.open_submenu, None),
        };

        let current = (self.selected_item, self.open_submenu, self.selected_subitem);
        if next != current {
            (self.selected_item, self.open_submenu, self.selected_subitem) = next;
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    /// Toggle the dropdown for menu `index` open or closed.
    pub(crate) fn toggle_menu(&mut self, index: usize) {
        self.open_menu = if self.open_menu == Some(index) {
            None
        } else {
            Some(index)
        };
        self.open_submenu = None;
        self.selected_item = None;
        self.selected_subitem = None;
        self.dirty = true;
    }

    /// Close any open dropdown (and its submenu).
    pub(crate) fn close_menu(&mut self) {
        if self.open_menu.is_some() {
            self.open_menu = None;
            self.open_submenu = None;
            self.selected_item = None;
            self.selected_subitem = None;
            self.dirty = true;
        }
    }

    /// On mouse release, check whether the pointer moved far enough from the
    /// drag start to count as a reorder (>= 10 px). If so, hit-test the release
    /// position and swap the source tab with the target tab.
    pub(crate) fn finalize_tab_drag(&mut self) {
        let Some((src, start_x)) = self.tab_drag_start.take() else {
            return;
        };
        let (x, y) = self.cursor_pos;
        if (x - start_x).abs() < 10.0 {
            return;
        }
        let Some((cw, ch)) = self.renderer.as_ref().map(|r| r.cell_size()) else {
            return;
        };
        let surface_w = self.viewport_rect().width;
        let tabbar = self.build_top_tabbar();
        let hit = winter_render::hit_test(&tabbar, surface_w, cw, ch, x, y);
        if let TabbarHit::Tab(dst) = hit {
            if dst != src {
                self.swap_tabs(src, dst);
            }
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_modern_has_one_menu_classic_has_several() {
        assert_eq!(menu_defs(MenuStyle::Modern).len(), 1);
        assert!(menu_defs(MenuStyle::Classic).len() >= 4);
    }

    #[test]
    fn test_leaves_dispatch_a_command_and_parents_open_submenus() {
        fn check(items: &[ItemDef]) {
            for item in items {
                if item.children.is_empty() {
                    if item.label != "-" {
                        assert!(!item.command.is_empty(), "{} has empty command", item.label);
                    }
                } else {
                    // A submenu parent opens a child panel; it has no command.
                    assert!(
                        item.command.is_empty(),
                        "parent {} should not dispatch",
                        item.label
                    );
                    check(item.children);
                }
            }
        }
        for style in [MenuStyle::Modern, MenuStyle::Classic] {
            for menu in menu_defs(style) {
                assert!(!menu.items.is_empty(), "{} has no items", menu.title);
                check(menu.items);
            }
        }
    }

    #[test]
    fn test_modern_menu_has_a_layout_submenu() {
        let layout = MODERN_ITEMS
            .iter()
            .find(|it| it.label == "Layout")
            .expect("Layout parent");
        assert_eq!(layout.children.len(), 4);
        assert_eq!(layout.children[0].command, "split_vertical");
    }

    #[test]
    fn test_open_submenu_item_resolves_the_child_command() {
        let mut app = App::new();
        app.open_menu = Some(0);
        app.open_submenu = MODERN_ITEMS.iter().position(|it| it.label == "Layout");
        assert_eq!(
            app.open_submenu_item(0).map(|it| it.command),
            Some("split_vertical")
        );
        assert!(app.open_submenu_item(99).is_none());
    }

    #[test]
    fn test_build_top_tabbar_carries_submenu_children_and_state() {
        let mut app = App::new();
        app.open_menu = Some(0);
        let layout_idx = MODERN_ITEMS
            .iter()
            .position(|it| it.label == "Layout")
            .unwrap();
        app.open_submenu = Some(layout_idx);
        app.selected_subitem = Some(1);
        let tabbar = app.build_top_tabbar();
        assert_eq!(tabbar.open_submenu, Some(layout_idx));
        assert_eq!(tabbar.selected_subitem, Some(1));
        assert!(tabbar.menus[0].items[layout_idx].has_children());
        assert_eq!(tabbar.menus[0].items[layout_idx].children.len(), 4);
    }

    #[test]
    fn test_tab_title_falls_back_to_terminal_number() {
        let app = App::new();
        assert_eq!(app.tab_title(0), "Terminal 1");
    }

    #[test]
    fn test_osc_title_is_used_before_process_and_cwd_fallbacks() {
        let mut app = App::new();
        let focused = app.tabs[0].focused();
        // An app-set title (e.g. butterfly naming the open file) shows as-is,
        // even though a foreground process may also be running in the pane.
        app.pane_titles
            .insert(focused, "butterfly — Winter Term.md".into());
        assert_eq!(app.tab_title(0), "butterfly — Winter Term.md");

        // A shell-style "user@host: cwd" title is cleaned to its cwd part.
        app.pane_titles.insert(focused, "user@host:~/notes".into());
        assert_eq!(app.tab_title(0), "~/notes");

        // An empty title falls through to the remaining fallbacks.
        app.pane_titles.insert(focused, String::new());
        assert_eq!(app.tab_title(0), "Terminal 1");
    }

    #[test]
    fn test_build_top_tabbar_reflects_active_tab_and_menu_state() {
        let mut app = App::new();
        app.open_menu = Some(0);
        let tabbar = app.build_top_tabbar();
        assert_eq!(tabbar.tabs.len(), 1);
        assert_eq!(tabbar.active_tab, 0);
        assert_eq!(tabbar.open_menu, Some(0));
        assert_eq!(tabbar.menu_style, MenuStyle::Modern);
    }

    #[test]
    fn test_toggle_menu_opens_then_closes() {
        let mut app = App::new();
        app.toggle_menu(0);
        assert_eq!(app.open_menu, Some(0));
        app.toggle_menu(0);
        assert_eq!(app.open_menu, None);
    }

    #[test]
    fn test_clean_title_and_cwd() {
        assert_eq!(clean_title("user@host: /home/user"), "/home/user");
        assert_eq!(clean_title("user@host:~/workspace"), "~/workspace");
        assert_eq!(clean_title("user@host"), "user@host");
        assert_eq!(clean_title("some other title"), "some other title");

        if let Ok(home) = std::env::var("HOME") {
            assert_eq!(clean_cwd(&home), "~");
            assert_eq!(clean_cwd(&format!("{}/workspace", home)), "~/workspace");
            // Intermediate directory components are abbreviated to their first
            // character; only the leaf component is kept in full.
            assert_eq!(
                clean_cwd(&format!("{}/Workspace/apps/winter-term", home)),
                "~/W/a/winter-term"
            );
        }
    }
}
