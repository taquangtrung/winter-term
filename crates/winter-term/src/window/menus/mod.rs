//! The menubar: the menus each title-bar style offers, built from the same
//! command names the palette runs, opened, hovered, and closed, and the state
//! the menubar shares with the right-click context menu, which lives in
//! [`context`].

pub(in crate::window) mod context;

use winter_render::{MenuItem, MenuStyle, TabbarHit};

use crate::model::input::format_key;

use crate::window::App;

use self::context::ContextAction;

// ========================================================================
// Constants
// ========================================================================

/// A horizontal separator line.
pub(in crate::window) const SEPARATOR: ItemDef = leaf("-", "-", "");

const LAYOUT_ITEMS: &[ItemDef] = &[
    leaf("split_vertical", "Split Vertical", ""),
    leaf("split_horizontal", "Split Horizontal", ""),
    leaf("toggle_pane_zoom", "Zoom Pane", ""),
    leaf("close_pane", "Close Pane", ""),
];

const SEARCH_ITEMS: &[ItemDef] = &[
    leaf("search", "Search Blocks", ""),
    leaf("quick_select", "Quick Select", ""),
    leaf("toggle_fold", "Toggle Fold", ""),
];

pub(in crate::window) const MODERN_ITEMS: &[ItemDef] = &[
    leaf("new_tab", "New Tab", "C-S-n"),
    leaf("new_window", "New Window", "C-M-S-n"),
    leaf("close_tab", "Close Tab", "C-S-w"),
    leaf("close_other_tabs", "Close Other Tabs", "M-S-q"),
    leaf("rename_tab", "Rename Tab", ""),
    SEPARATOR,
    parent("Layout", LAYOUT_ITEMS),
    parent("Search & Fold", SEARCH_ITEMS),
    SEPARATOR,
    leaf("open_settings", "Settings", "C-,"),
];

const MODERN_MENUS: &[MenuDef] = &[MenuDef {
    items: MODERN_ITEMS,
    title: "Menu",
}];

const CLASSIC_MENUS: &[MenuDef] = &[
    MenuDef {
        title: "File",
        items: &[
            leaf("new_tab", "New Tab", "C-S-n"),
            leaf("new_window", "New Window", "C-M-S-n"),
            leaf("close_tab", "Close Tab", "C-S-w"),
            leaf("close_other_tabs", "Close Other Tabs", "M-S-q"),
            leaf("rename_tab", "Rename Tab", ""),
            SEPARATOR,
            leaf("close_pane", "Close Pane", ""),
            SEPARATOR,
            leaf("open_settings", "Settings", "C-,"),
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
            SEPARATOR,
            leaf("move_tab_left", "Move Tab Left", ""),
            leaf("move_tab_down", "Move Tab Down", ""),
            leaf("move_tab_up", "Move Tab Up", ""),
            leaf("move_tab_right", "Move Tab Right", ""),
        ],
    },
];

// ========================================================================
// Data Structures
// ========================================================================

/// What the menubar and the right-click context menu currently have open.
/// Both are drawn by the same dropdown pass, so they share one struct.
#[derive(Default)]
pub(in crate::window) struct MenuState {
    /// The actions bound to each context-menu item, parallel to the rendered list.
    pub(in crate::window) context_actions: Vec<ContextAction>,
    /// Active right-click context menu: pixel position where it was opened.
    pub(in crate::window) context_pos: Option<(f32, f32)>,
    /// The currently hovered item in the context menu (drives the hover highlight).
    pub(in crate::window) context_selected: Option<usize>,
    /// The URL (if any) that was under the pointer when the context menu opened.
    pub(in crate::window) context_url: Option<String>,
    /// The open menu/dropdown (index into the tabbar's menu list), or `None`.
    pub(in crate::window) open: Option<usize>,
    /// Index of the open submenu's parent within the open menu's items, or `None`.
    pub(in crate::window) open_submenu: Option<usize>,
    /// The hovered dropdown item while a menu is open.
    pub(in crate::window) selected_item: Option<usize>,
    /// The hovered submenu child while a submenu is open.
    pub(in crate::window) selected_subitem: Option<usize>,
}

/// A menu line: its display `label`, an optional `shortcut` hint, and the
/// `command` dispatched by [`App::run_command`]. An item with `children` is a
/// submenu parent that opens a child panel on hover instead of running a command.
pub(in crate::window) struct ItemDef {
    pub(in crate::window) children: &'static [ItemDef],
    pub(in crate::window) command: &'static str,
    pub(in crate::window) label: &'static str,
    pub(in crate::window) shortcut: &'static str,
}

/// One menu: a `title` (shown only in the classic menubar) and its items.
pub(in crate::window) struct MenuDef {
    pub(in crate::window) items: &'static [ItemDef],
    pub(in crate::window) title: &'static str,
}

// ========================================================================
// Functions
// ========================================================================

/// A command leaf: no children.
pub(in crate::window) const fn leaf(
    command: &'static str,
    label: &'static str,
    shortcut: &'static str,
) -> ItemDef {
    ItemDef {
        children: &[],
        command,
        label,
        shortcut,
    }
}

/// A submenu parent: a label and its children, with no command of its own.
pub(in crate::window) const fn parent(
    label: &'static str,
    children: &'static [ItemDef],
) -> ItemDef {
    ItemDef {
        children,
        command: "",
        label,
        shortcut: "",
    }
}

/// The menus for a given style.
pub(in crate::window) fn menu_defs(style: MenuStyle) -> &'static [MenuDef] {
    match style {
        MenuStyle::Classic => CLASSIC_MENUS,
        MenuStyle::Modern => MODERN_MENUS,
    }
}

/// Recursively convert a static [`ItemDef`] into the renderer's [`MenuItem`].
/// How a menu shows the keys an entry types: run together when each is one
/// character (`gg`, `"+y`), apart when any is a chord (`C-c C-k`).
pub(in crate::window) fn keys_label(keys: &[crate::model::input::Key]) -> String {
    let names: Vec<String> = keys.iter().map(format_key).collect();
    let separator = match names.iter().all(|name| name.chars().count() == 1) {
        true => "",
        false => " ",
    };
    names.join(separator)
}

pub(in crate::window) fn menu_item(item: &ItemDef) -> MenuItem {
    MenuItem {
        children: item.children.iter().map(menu_item).collect(),
        label: item.label.to_string(),
        shortcut: item.shortcut.to_string(),
    }
}

// ========================================================================
// App: menus
// ========================================================================

impl App {
    /// The static def of item `i` in the currently open menu, if any.
    pub(in crate::window) fn open_menu_item(&self, i: usize) -> Option<&'static ItemDef> {
        let open = self.chrome.menus.open?;
        menu_defs(self.config.menu_style).get(open)?.items.get(i)
    }

    /// The static def of submenu child `child` under the open submenu parent.
    pub(in crate::window) fn open_submenu_item(&self, child: usize) -> Option<&'static ItemDef> {
        let open = self.chrome.menus.open?;
        let parent = self.chrome.menus.open_submenu?;
        let item = menu_defs(self.config.menu_style)
            .get(open)?
            .items
            .get(parent)?;
        item.children.get(child)
    }

    /// Update the hovered menu/submenu rows from the pointer position while a menu
    /// is open, requesting a redraw when the highlight changes. Hovering a submenu
    /// parent opens its child panel; the parent stays open throughout.
    pub(in crate::window) fn update_menu_hover(&mut self, x: f32, y: f32) {
        if self.chrome.menus.open.is_none() {
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
                Some(item) if item.label == "-" => (None, self.chrome.menus.open_submenu, None),
                Some(item) if !item.children.is_empty() => (Some(i), Some(i), None),
                Some(_) => (Some(i), None, None),
                None => (None, self.chrome.menus.open_submenu, None),
            },
            // Keep the parent highlighted and its submenu open over its children.
            TabbarHit::SubmenuItem(child) => (
                self.chrome.menus.open_submenu,
                self.chrome.menus.open_submenu,
                Some(child),
            ),
            // Off the items: leave the submenu open so the cursor can reach it.
            _ => (None, self.chrome.menus.open_submenu, None),
        };

        let current = (
            self.chrome.menus.selected_item,
            self.chrome.menus.open_submenu,
            self.chrome.menus.selected_subitem,
        );
        if next != current {
            (
                self.chrome.menus.selected_item,
                self.chrome.menus.open_submenu,
                self.chrome.menus.selected_subitem,
            ) = next;
            self.dirty = true;
        }
    }

    /// Toggle the dropdown for menu `index` open or closed.
    pub(in crate::window) fn toggle_menu(&mut self, index: usize) {
        self.chrome.menus.open = if self.chrome.menus.open == Some(index) {
            None
        } else {
            Some(index)
        };
        self.chrome.menus.open_submenu = None;
        self.chrome.menus.selected_item = None;
        self.chrome.menus.selected_subitem = None;
        self.dirty = true;
    }

    /// Close any open dropdown (and its submenu).
    pub(in crate::window) fn close_menu(&mut self) {
        if self.chrome.menus.open.is_some() {
            self.chrome.menus.open = None;
            self.chrome.menus.open_submenu = None;
            self.chrome.menus.selected_item = None;
            self.chrome.menus.selected_subitem = None;
            self.dirty = true;
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_render::MenuStyle;

    use crate::window::App;

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
    fn test_open_submenu_item_resolves_the_child_command() {
        let mut app = App::new();
        app.chrome.menus.open = Some(0);
        app.chrome.menus.open_submenu = MODERN_ITEMS.iter().position(|it| it.label == "Layout");
        assert_eq!(
            app.open_submenu_item(0).map(|it| it.command),
            Some("split_vertical")
        );
        assert!(app.open_submenu_item(99).is_none());
    }

    #[test]
    fn test_toggle_menu_opens_then_closes() {
        let mut app = App::new();
        app.toggle_menu(0);
        assert_eq!(app.chrome.menus.open, Some(0));
        app.toggle_menu(0);
        assert_eq!(app.chrome.menus.open, None);
    }
}
