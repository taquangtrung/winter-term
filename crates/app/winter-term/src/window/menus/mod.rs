//! The menubar: the menus each title-bar style offers, built from the same
//! command names the palette runs, opened and closed, and the state the
//! menubar shares with the right-click context menu, which lives in
//! [`context`]. Misty's [`Menu`] draws both and follows the pointer and the
//! keys over them (see [`crate::window::frame::menus`]).

pub(in crate::window) mod context;

use misty::{Menu, MenuEntry, MenuItem, PopupSide, Rect};
use winter_render::MenuStyle;

use winter_model::input::format_key;

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
    leaf("close_tabs_to_left", "Close Tabs to the Left", ""),
    leaf("close_tabs_to_right", "Close Tabs to the Right", ""),
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
            leaf("close_tabs_to_left", "Close Tabs to the Left", ""),
            leaf("close_tabs_to_right", "Close Tabs to the Right", ""),
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
#[derive(Default)]
pub(in crate::window) struct MenuState {
    /// The right-click context menu, its items numbered by their place in
    /// `context_actions`, or `None` while closed.
    pub(in crate::window) context: Option<Menu<usize>>,
    /// What each context-menu item runs, parallel to its entries.
    pub(in crate::window) context_actions: Vec<ContextAction>,
    /// The open menubar menu, its items naming the commands they run, or
    /// `None` while closed.
    pub(in crate::window) dropdown: Option<Menu<&'static str>>,
    /// The open menu (index into the tabbar's menu list), whose title is lit,
    /// or `None`.
    pub(in crate::window) open: Option<usize>,
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

/// How a menu shows the keys an entry types: run together when each is one
/// character (`gg`, `"+y`), apart when any is a chord (`C-c C-k`).
pub(in crate::window) fn keys_label(keys: &[winter_model::input::Key]) -> String {
    let names: Vec<String> = keys.iter().map(format_key).collect();
    let separator = match names.iter().all(|name| name.chars().count() == 1) {
        true => "",
        false => " ",
    };
    names.join(separator)
}

/// Misty's entries for `items`: a separator for `-`, a submenu for a parent,
/// and an item naming its command for a leaf.
fn entries_of(items: &'static [ItemDef]) -> Vec<MenuEntry<&'static str>> {
    items
        .iter()
        .map(|item| {
            if item.label == SEPARATOR.label {
                return MenuEntry::Separator;
            }
            let entry = MenuItem::new(item.command, item.label);
            let entry = match item.children.is_empty() {
                true => entry,
                false => entry.with_submenu(entries_of(item.children)),
            };
            match item.shortcut.is_empty() {
                true => entry.into(),
                false => entry.with_accelerator(item.shortcut).into(),
            }
        })
        .collect()
}

// ========================================================================
// App: menus
// ========================================================================

impl App {
    /// Toggle the dropdown for menu `index` open or closed, dropping it from
    /// its title (or the hamburger button).
    pub(in crate::window) fn toggle_menu(&mut self, index: usize) {
        if self.chrome.menus.open == Some(index) {
            self.close_menu();
            return;
        }
        let Some(def) = menu_defs(self.config.menu_style).get(index) else {
            return;
        };
        self.close_context_menu();
        self.chrome.menus.open = Some(index);
        let anchor = self.renderer.as_ref().and_then(|renderer| {
            let (cell_w, cell_h) = renderer.cell_size();
            let surface_w = self.viewport_rect().width;
            winter_render::menu_anchor(&self.build_top_tabbar(), surface_w, cell_w, cell_h)
        });
        let anchor = anchor.map_or_else(Rect::default, |at| Rect::new(at.x, at.y, at.w, at.h));
        let mut menu = Menu::new(entries_of(def.items));
        menu.open(anchor, PopupSide::Below);
        self.chrome.menus.dropdown = Some(menu);
        self.dirty = true;
    }

    /// Close any open dropdown (and its submenu).
    pub(in crate::window) fn close_menu(&mut self) {
        if self.chrome.menus.open.is_some() {
            self.chrome.menus.open = None;
            self.chrome.menus.dropdown = None;
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
    fn test_a_parent_opens_its_children_and_a_leaf_runs_its_command() {
        let mut app = App::new();
        app.toggle_menu(0);
        let menu = app.chrome.menus.dropdown.as_mut().expect("open");
        menu.move_highlight(1);
        assert_eq!(menu.highlighted(), Some("new_tab"));
        // The highlight passes over the separators.
        let layout = MODERN_ITEMS
            .iter()
            .filter(|item| item.label != SEPARATOR.label)
            .position(|item| item.label == "Layout")
            .expect("Layout");
        menu.move_highlight(layout as isize);
        assert!(menu.open_submenu(), "Layout holds a submenu");
        assert_eq!(menu.choose(), Some("split_vertical"));
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
