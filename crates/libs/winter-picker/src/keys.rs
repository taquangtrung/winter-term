//! The keys every picker answers, as a key table like every editor's: listed
//! in the Keys editor, described by `F1 k`, and rebound by lines with
//! `in="picker"` in `keybindings.kdl`.
//!
//! A key no table binds goes to the source (the browser's `Tab`) and then to
//! the query, which takes the Emacs keys of every input field.

use winter_model::input::table::{Binding, KeyTable, Listed};

// ========================================================================
// Constants
// ========================================================================

/// The picker's keys, block `picker`.
pub const KEYS: KeyTable<PickerAct> = KeyTable {
    block: "picker",
    bindings: &[
        Binding {
            action: PickerAct::Accept,
            id: "accept",
            keys: &["RET"],
            label: "Run the Row's First Action",
        },
        Binding {
            action: PickerAct::ActionMenu,
            id: "action_menu",
            keys: &["F4", "C-RET"],
            label: "Show the Row's Actions",
        },
        Binding {
            action: PickerAct::TogglePanel,
            id: "toggle_panel",
            keys: &["F6"],
            label: "Toggle Preview",
        },
        Binding {
            action: PickerAct::Save,
            id: "save",
            keys: &["F3", "C-S-s"],
            label: "Save the Rows to a Buffer",
        },
        Binding {
            action: PickerAct::Next,
            id: "next",
            keys: &["C-n", "<down>"],
            label: "Next Row",
        },
        Binding {
            action: PickerAct::Previous,
            id: "previous",
            keys: &["C-p", "<up>"],
            label: "Previous Row",
        },
        Binding {
            action: PickerAct::PageDown,
            id: "page_down",
            keys: &["<next>"],
            label: "A Page Down",
        },
        Binding {
            action: PickerAct::PageUp,
            id: "page_up",
            keys: &["<prior>"],
            label: "A Page Up",
        },
        Binding {
            action: PickerAct::HistoryPrevious,
            id: "history_previous",
            keys: &["M-p"],
            label: "Earlier Query",
        },
        Binding {
            action: PickerAct::HistoryNext,
            id: "history_next",
            keys: &["M-n"],
            label: "Later Query",
        },
        Binding {
            action: PickerAct::Close,
            id: "close",
            keys: &["ESC", "C-g"],
            label: "Close",
        },
    ],
    configurable: true,
    title: "Picker",
};

/// The picker's key tables, for the one list the Keys editor shows and
/// `keybindings.kdl` rebinds by.
pub const KEY_TABLES: &[&dyn Listed] = &[&KEYS];

// ========================================================================
// Data Structures
// ========================================================================

/// What a picker key does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PickerAct {
    /// Run the selected row's first action, or the answer typed.
    Accept,
    /// Show the selected row's actions.
    ActionMenu,
    /// Close the picker, or the action menu over it.
    Close,
    /// Bring back a later query.
    HistoryNext,
    /// Bring back an earlier query.
    HistoryPrevious,
    /// Select the next row.
    Next,
    /// Move the selection a page down.
    PageDown,
    /// Move the selection a page up.
    PageUp,
    /// Select the previous row.
    Previous,
    /// Save the rows to a buffer.
    Save,
    /// Show or hide the side panel.
    TogglePanel,
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_model::input::table::{find, invalid_specs, repeated_ids};
    use winter_model::input::{Key, KeyCode};

    use super::*;

    #[test]
    fn test_every_picker_key_parses_and_has_an_id_of_its_own() {
        assert!(
            invalid_specs(&KEYS).is_empty(),
            "{:?}",
            invalid_specs(&KEYS)
        );
        assert!(repeated_ids(&KEYS).is_empty());
    }

    #[test]
    fn test_the_designs_keys_reach_their_acts() {
        let act = |key: Key| find(&KEYS, &key);
        assert_eq!(act(Key::plain(KeyCode::F(4))), Some(PickerAct::ActionMenu));
        assert_eq!(
            act(Key::with_ctrl(KeyCode::Enter)),
            Some(PickerAct::ActionMenu)
        );
        assert_eq!(act(Key::plain(KeyCode::F(6))), Some(PickerAct::TogglePanel));
        assert_eq!(act(Key::plain(KeyCode::Enter)), Some(PickerAct::Accept));
    }
}
