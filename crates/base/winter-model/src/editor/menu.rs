//! The menu an editor offers over its own rows, the right-click one.
//!
//! An entry runs in one of two ways. Most name keys the editor already binds
//! ([`MenuRun::Keys`]): a menu built that way cannot offer something the
//! keyboard cannot do, cannot drift from what the keys mean, and teaches them,
//! since the host shows the keys beside the label. An action that lives in
//! the window's command list instead (a language server's, say) names that
//! command ([`MenuRun::Command`]), which the host runs on the editor's tab.
//!
//! A menu is declared, not assembled: groups of an editor's table actions
//! ([`menu_of`], [`EditorMenuItem::bound`]), set apart by separators
//! ([`menu_groups`]). The host adds what every tab shares after it (the
//! file's path, the pane), so an editor lists only its own.

use crate::input::table::{self, KeyTable};
use crate::input::Key;

// ========================================================================
// Data Structures
// ========================================================================

/// How choosing an entry runs it.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuRun {
    /// The window's command `id`, as the palette lists it, run on the
    /// editor's tab.
    Command(&'static str),
    /// Keys the editor binds, typed to it in order.
    Keys(Vec<Key>),
    /// A line between two groups of entries, which runs nothing.
    Separator,
}

/// One entry an editor offers in the menu opened over its own rows.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorMenuItem {
    /// What the host shows for it.
    pub label: String,
    /// What choosing it does.
    pub run: MenuRun,
}

// ========================================================================
// EditorMenuItem
// ========================================================================

impl EditorMenuItem {
    /// An entry called `label`, run by `key`.
    pub fn new(key: Key, label: impl Into<String>) -> Self {
        Self::typed(vec![key], label)
    }

    /// An entry called `label`, run by typing `keys` in order: `gg`, or the
    /// `"+y` that copies to the clipboard rather than a register.
    pub fn typed(keys: Vec<Key>, label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            run: MenuRun::Keys(keys),
        }
    }

    /// An entry called `label`, running the window's command `id`.
    pub fn command(id: &'static str, label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            run: MenuRun::Command(id),
        }
    }

    /// The entry for `action` in `table`: its first keys, one chord or a
    /// sequence, under the table's label. `None` for an action the table
    /// does not hold.
    pub fn bound<A: PartialEq>(table: &KeyTable<A>, action: A) -> Option<Self> {
        let (keys, label) = table::menu_typed(table, action)?;
        Some(Self::typed(keys, label))
    }

    /// The same, under `label`: a menu's name is often shorter than the help
    /// list's sentence.
    pub fn bound_as<A: PartialEq>(table: &KeyTable<A>, action: A, label: &str) -> Option<Self> {
        let (keys, _) = table::menu_typed(table, action)?;
        Some(Self::typed(keys, label))
    }

    /// The entry for keys written as a table writes them (`"g d"`, `"C-r"`),
    /// called `label`. `None` for a spec naming an unknown key.
    pub fn spec(spec: &str, label: &str) -> Option<Self> {
        Some(Self::typed(table::keys(spec)?, label))
    }

    /// A line between two groups of entries.
    pub fn separator() -> Self {
        Self {
            label: String::new(),
            run: MenuRun::Separator,
        }
    }

    /// Whether this is a line between groups rather than an entry.
    pub fn is_separator(&self) -> bool {
        self.run == MenuRun::Separator
    }

    /// The keys it types, for the host to show beside it; none for a
    /// command or a separator.
    pub fn keys(&self) -> &[Key] {
        match &self.run {
            MenuRun::Keys(keys) => keys,
            MenuRun::Command(_) | MenuRun::Separator => &[],
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `groups` of entries, each set off from the next by a separator, with any
/// group left empty dropped rather than leaving two lines together.
pub fn menu_groups(groups: impl IntoIterator<Item = Vec<EditorMenuItem>>) -> Vec<EditorMenuItem> {
    let mut items = Vec::new();
    for group in groups.into_iter().filter(|group| !group.is_empty()) {
        if !items.is_empty() {
            items.push(EditorMenuItem::separator());
        }
        items.extend(group);
    }
    items
}

/// A menu declared as `groups` of `table`'s actions, each entry its first
/// keys under the table's label; an action the table does not hold is left
/// out.
pub fn menu_of<A: PartialEq + Copy>(table: &KeyTable<A>, groups: &[&[A]]) -> Vec<EditorMenuItem> {
    menu_groups(groups.iter().map(|group| {
        group
            .iter()
            .filter_map(|&action| EditorMenuItem::bound(table, action))
            .collect()
    }))
}

/// The entries of `entries` that are there, as one group: an entry offered
/// only in some states is `None` in the others.
pub fn present(entries: impl IntoIterator<Item = Option<EditorMenuItem>>) -> Vec<EditorMenuItem> {
    entries.into_iter().flatten().collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::table::Binding;

    use crate::input::KeyCode;

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Act {
        Open,
        Stage,
        Unbound,
    }

    const KEYS: KeyTable<Act> = KeyTable {
        block: "test",
        bindings: &[
            Binding {
                action: Act::Open,
                id: "open",
                keys: &["RET", "o"],
                label: "Open",
            },
            Binding {
                action: Act::Stage,
                id: "stage",
                keys: &["g s"],
                label: "Stage",
            },
        ],
        configurable: true,
        title: "Test",
    };

    /// The menu's shape: labels, `-` for a separator.
    fn shape(items: &[EditorMenuItem]) -> Vec<&str> {
        items
            .iter()
            .map(|item| match item.is_separator() {
                true => "-",
                false => item.label.as_str(),
            })
            .collect()
    }

    #[test]
    fn test_menu_groups_set_groups_apart_and_drop_the_empty_ones() {
        let entry = |label: &str| EditorMenuItem::new(Key::plain(KeyCode::Char('x')), label);
        let items = menu_groups([vec![entry("a"), entry("b")], Vec::new(), vec![entry("c")]]);
        assert_eq!(shape(&items), ["a", "b", "-", "c"]);
        assert!(menu_groups([Vec::new(), Vec::new()]).is_empty());
    }

    #[test]
    fn test_a_declared_menu_takes_each_action_s_first_keys_and_leaves_out_the_unbound() {
        let items = menu_of(&KEYS, &[&[Act::Open], &[Act::Unbound], &[Act::Stage]]);
        assert_eq!(shape(&items), ["Open", "-", "Stage"]);
        assert_eq!(
            items[0].keys(),
            [Key::plain(KeyCode::Enter)],
            "the first spec"
        );
        assert_eq!(items[2].keys().len(), 2, "a sequence is typed whole");
    }

    #[test]
    fn test_a_command_entry_types_no_keys() {
        let item = EditorMenuItem::command("lsp_rename", "Rename Symbol");
        assert!(item.keys().is_empty());
        assert!(!item.is_separator());
    }
}
