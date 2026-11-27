//! The menubar: the menus each title-bar style offers, built from the same
//! command names the palette runs, opened and closed, and the state the
//! menubar shares with the right-click context menu, which lives in
//! [`context`]. Misty's [`Menu`] draws both and follows the pointer and the
//! keys over them (see [`crate::window::frame::menus`]).

pub(in crate::window) mod context;

use crate::render::MenuStyle;
use misty::widgets::{Menu, MenuEntry, MenuItem, PopupSide};

use winter_model::input::{Keymap, format_key};

use crate::window::App;

use self::context::ContextAction;

// ========================================================================
// Constants
// ========================================================================

/// A horizontal separator line.
pub(in crate::window) const SEPARATOR: ItemDef = leaf("-", "-");

const LAYOUT_ITEMS: &[ItemDef] = &[
    leaf("split_vertical", "Split Right"),
    leaf("split_horizontal", "Split Down"),
    leaf("zoom_pane", "Zoom Pane"),
    leaf("toggle_fullscreen", "Full Screen"),
    leaf("close_pane", "Close Pane"),
];

const SEARCH_ITEMS: &[ItemDef] = &[
    leaf("search_blocks", "Search Blocks"),
    leaf("quick_select", "Quick Select"),
    leaf("toggle_fold", "Toggle Fold"),
];

pub(in crate::window) const MODERN_ITEMS: &[ItemDef] = &[
    leaf("new_terminal", "New Tab"),
    leaf("new_window", "New Window"),
    leaf("close_tab", "Close Tab"),
    leaf("close_other_tabs", "Close Other Tabs"),
    leaf("close_tabs_to_left", "Close Tabs to the Left"),
    leaf("close_tabs_to_right", "Close Tabs to the Right"),
    leaf("rename_tab", "Rename Tab"),
    SEPARATOR,
    parent("Layout", LAYOUT_ITEMS),
    parent("Search & Fold", SEARCH_ITEMS),
    SEPARATOR,
    leaf("open_settings", "Settings"),
];

const MODERN_MENUS: &[MenuDef] = &[MenuDef {
    items: MODERN_ITEMS,
    title: "Menu",
}];

const CLASSIC_MENUS: &[MenuDef] = &[
    MenuDef {
        title: "File",
        items: &[
            leaf("new_terminal", "New Tab"),
            leaf("new_window", "New Window"),
            leaf("close_tab", "Close Tab"),
            leaf("close_other_tabs", "Close Other Tabs"),
            leaf("close_tabs_to_left", "Close Tabs to the Left"),
            leaf("close_tabs_to_right", "Close Tabs to the Right"),
            leaf("rename_tab", "Rename Tab"),
            SEPARATOR,
            leaf("close_pane", "Close Pane"),
            SEPARATOR,
            leaf("open_settings", "Settings"),
        ],
    },
    MenuDef {
        title: "Edit",
        items: &[
            leaf("search_blocks", "Search Blocks"),
            leaf("quick_select", "Quick Select"),
            SEPARATOR,
            leaf("yank_block", "Yank Block Source"),
        ],
    },
    MenuDef {
        title: "View",
        items: &[
            leaf("split_vertical", "Split Right"),
            leaf("split_horizontal", "Split Down"),
            leaf("toggle_fullscreen", "Full Screen"),
            SEPARATOR,
            leaf("toggle_fold", "Toggle Fold"),
        ],
    },
    MenuDef {
        title: "Window",
        items: &[
            leaf("focus_left", "Focus Left"),
            leaf("focus_down", "Focus Down"),
            leaf("focus_up", "Focus Up"),
            leaf("focus_right", "Focus Right"),
            SEPARATOR,
            leaf("move_tab_left", "Move Tab Left"),
            leaf("move_tab_down", "Move Tab Down"),
            leaf("move_tab_up", "Move Tab Up"),
            leaf("move_tab_right", "Move Tab Right"),
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

/// A menu line: its display `label` and the `command` dispatched by
/// [`App::run_command`], shown with the keys the keymap binds that command
/// to, if any. An item with `children` is a submenu parent that opens a child
/// panel on hover instead of running a command.
pub(in crate::window) struct ItemDef {
    pub(in crate::window) children: &'static [ItemDef],
    pub(in crate::window) command: &'static str,
    pub(in crate::window) label: &'static str,
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
pub(in crate::window) const fn leaf(command: &'static str, label: &'static str) -> ItemDef {
    ItemDef {
        children: &[],
        command,
        label,
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
/// and an item naming its command for a leaf, beside the keys `keymap` binds
/// that command to (none for a command it leaves unbound).
fn entries_of(items: &'static [ItemDef], keymap: &Keymap) -> Vec<MenuEntry<&'static str>> {
    items
        .iter()
        .map(|item| {
            if item.label == SEPARATOR.label {
                return MenuEntry::Separator;
            }
            let entry = MenuItem::new(item.command, item.label);
            let entry = match item.children.is_empty() {
                true => entry,
                false => entry.with_submenu(entries_of(item.children, keymap)),
            };
            let shortcut = match item.children.is_empty() {
                true => keymap.chord_hint(item.command),
                false => String::new(),
            };
            match shortcut.is_empty() {
                true => entry.into(),
                false => entry.with_accelerator(shortcut).into(),
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
        if self.parts.menus.open == Some(index) {
            self.close_menu();
            return;
        }
        let Some(def) = menu_defs(self.config.menu_style).get(index) else {
            return;
        };
        self.close_context_menu();
        self.parts.menus.open = Some(index);
        let anchor = self.menu_anchor(index).unwrap_or_default();
        // Built from the keymap as it opens, so it names the keys in force.
        let mut menu = Menu::new(entries_of(def.items, &self.keymap));
        menu.open(anchor, PopupSide::Below);
        self.parts.menus.dropdown = Some(menu);
        self.dirty = true;
    }

    /// Rebuild the open dropdown, if one is, so the keys it shows follow a
    /// keymap just changed.
    pub(in crate::window) fn refresh_menu(&mut self) {
        if let Some(index) = self.parts.menus.open {
            self.close_menu();
            self.toggle_menu(index);
        }
    }

    /// Close any open dropdown (and its submenu).
    pub(in crate::window) fn close_menu(&mut self) {
        if self.parts.menus.open.is_some() {
            self.parts.menus.open = None;
            self.parts.menus.dropdown = None;
            self.dirty = true;
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::render::MenuStyle;

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
        let menu = app.parts.menus.dropdown.as_mut().expect("open");
        menu.move_highlight(1);
        assert_eq!(menu.highlighted(), Some("new_terminal"));
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
    fn test_a_shortcut_names_the_keys_the_user_bound_and_none_when_unbound() {
        let mut bindings = HashMap::new();
        bindings.insert("F5".to_string(), "new_terminal".to_string());
        let keymap = Keymap::from_config(Some(&bindings), None);
        let entries = entries_of(MODERN_ITEMS, &keymap);
        let entry = |command: &'static str, label: &str| {
            let at = MODERN_ITEMS
                .iter()
                .position(|item| item.command == command)
                .expect("in the menu");
            (entries[at].clone(), MenuItem::new(command, label))
        };
        let (new_tab, item) = entry("new_terminal", "New Tab");
        assert_eq!(new_tab, item.with_accelerator("F5").into());
        let (rename, item) = entry("rename_tab", "Rename Tab");
        assert_eq!(rename, item.into(), "an unbound command shows no keys");
    }

    #[test]
    fn test_an_open_menu_is_rebuilt_when_the_keymap_changes() {
        let mut app = App::new();
        app.toggle_menu(0);
        app.refresh_menu();
        assert_eq!(app.parts.menus.open, Some(0), "still open, rebuilt");
        assert!(app.parts.menus.dropdown.is_some());
    }

    #[test]
    fn test_toggle_menu_opens_then_closes() {
        let mut app = App::new();
        app.toggle_menu(0);
        assert_eq!(app.parts.menus.open, Some(0));
        app.toggle_menu(0);
        assert_eq!(app.parts.menus.open, None);
    }
}
