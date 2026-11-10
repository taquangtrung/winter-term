//! The keys extensions' views answer, as key tables of Winter's own.
//!
//! A view never sees a key: its kind declares the actions it answers and
//! the keys each starts on ([`winter_ext_api::ViewKey`]), and those become
//! a [`KeyTable`] like any editor's, under a block of `keybindings.kdl`
//! named for the view. So the user rebinds them there and finds them in the
//! Keys editor, and a key the view did not declare stays Winter's. Chords
//! held with Ctrl, Alt, or Super are Winter's too: a view may not start on
//! one, though the user may bind one to its action.
//!
//! The tables are built once, from the registry, and kept as long as
//! Winter runs, as the registry is.

use std::collections::HashSet;
use std::sync::OnceLock;

use winter_ext_api::ViewKind;
use winter_model::input::table::{self, Binding, KeyTable, Listed};

use super::registry;

// ========================================================================
// Constants
// ========================================================================

/// The extensions' views' key tables, built the first time asked for.
static TABLES: OnceLock<Tables> = OnceLock::new();

/// Why a view may not start on a chord held with Ctrl, Alt, or Super.
const RESERVED: &str =
    "is held with Ctrl, Alt, or Super, which are Winter's; bind it in keybindings.kdl instead";

// ========================================================================
// Data Structures
// ========================================================================

/// Every view's key table, and what was refused building them.
struct Tables {
    /// Each table as the Keys editor lists it, in the views' order.
    listed: Vec<&'static dyn Listed>,
    /// What was refused, said once a window is up.
    problems: Vec<String>,
    /// Each view's table, by the view's full id.
    views: Vec<(&'static str, &'static KeyTable<&'static str>)>,
}

/// A table built for one view, and what was refused building it.
struct ViewTable {
    problems: Vec<String>,
    table: KeyTable<&'static str>,
}

// ========================================================================
// Functions
// ========================================================================

/// The key table of the view whose full id is `view`, if an extension
/// added it.
pub(crate) fn view_table(view: &str) -> Option<&'static KeyTable<&'static str>> {
    tables()
        .views
        .iter()
        .find(|(id, _)| *id == view)
        .map(|(_, table)| *table)
}

/// Every extension view's key table, as the Keys editor lists them.
pub(crate) fn listed() -> &'static [&'static dyn Listed] {
    &tables().listed
}

/// What was refused building the tables: a key that does not parse, one
/// held with a modifier that is Winter's, an action declared twice.
pub(crate) fn problems() -> &'static [String] {
    &tables().problems
}

/// The tables, built from the registry the first time.
fn tables() -> &'static Tables {
    TABLES.get_or_init(|| {
        let mut built = Tables {
            listed: Vec::new(),
            problems: Vec::new(),
            views: Vec::new(),
        };
        for kind in registry().views() {
            let view = table_of(kind);
            let table: &'static KeyTable<&'static str> = Box::leak(Box::new(view.table));
            built.listed.push(table);
            built.views.push((table.block, table));
            built.problems.extend(view.problems);
        }
        built
    })
}

/// `kind`'s declared keys as a table, each name kept as long as Winter runs,
/// as the registry keeps the kind. An action declared twice keeps its first
/// declaration; a key that does not parse, or is held with Ctrl, Alt, or
/// Super, is refused, and its action kept on the keys left.
fn table_of(kind: &ViewKind) -> ViewTable {
    let mut problems = Vec::new();
    let mut seen = HashSet::new();
    let mut bindings = Vec::new();
    for key in &kind.keys {
        if key.id.is_empty() || !seen.insert(key.id.as_str()) {
            problems.push(format!(
                "{}: the action \"{}\" is declared twice, or has no name",
                kind.id, key.id
            ));
            continue;
        }
        let mut specs: Vec<&'static str> = Vec::new();
        for spec in &key.keys {
            match refusal(spec) {
                Some(why) => {
                    problems.push(format!("{}: \"{spec}\" for {} {why}", kind.id, key.label))
                }
                None => specs.push(leak(spec)),
            }
        }
        bindings.push(Binding {
            action: leak(&key.id),
            id: leak(&key.id),
            keys: Box::leak(specs.into_boxed_slice()),
            label: leak(&key.label),
        });
    }
    ViewTable {
        problems,
        table: KeyTable {
            block: leak(&kind.id),
            bindings: Box::leak(bindings.into_boxed_slice()),
            configurable: true,
            title: leak(&kind.title),
        },
    }
}

/// Why a view may not start on `spec`, or `None` when it may.
fn refusal(spec: &str) -> Option<&'static str> {
    let Some(chords) = table::keys(spec) else {
        return Some("is not a key Winter knows");
    };
    let reserved = chords.iter().any(|key| key.ctrl || key.alt || key.sup); // key-exact: owned mods
    reserved.then_some(RESERVED)
}

/// `text`, kept as long as Winter runs.
fn leak(text: &str) -> &'static str {
    Box::leak(text.to_string().into_boxed_str())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_ext_api::{Extension, Host, Line, Registry, View, ViewKey};
    use winter_model::input::table::find;
    use winter_model::input::{Key, KeyCode};

    use super::*;

    struct Blank;

    impl View for Blank {
        fn render(&mut self, _rows: usize, _columns: usize) -> Vec<Line> {
            Vec::new()
        }

        fn on_action(&mut self, _host: &mut dyn Host, _action: &str) {}
    }

    /// An extension adding one view, `board`, answering the keys it holds.
    struct Declaring(Vec<ViewKey>);

    impl Extension for Declaring {
        fn id(&self) -> &'static str {
            "declaring"
        }

        fn activate(&self, registry: &mut Registry) {
            registry.view("board", "Board", self.0.clone(), || Box::new(Blank));
        }
    }

    /// The table built for a view declaring `keys`, and what was refused.
    fn built(keys: Vec<ViewKey>) -> ViewTable {
        let extension = Declaring(keys);
        let registry = Registry::activate_all([&extension as &dyn Extension]);
        table_of(&registry.views()[0])
    }

    #[test]
    fn test_a_declared_key_runs_its_action_and_modifiers_count() {
        let view = built(vec![ViewKey::new("next", "Next day", &["l", "<right>"])]);
        assert!(view.problems.is_empty(), "{:?}", view.problems);
        let plain = Key::plain(KeyCode::Char('l'));
        assert_eq!(find(&view.table, &plain), Some("next"));
        let alt = Key { alt: true, ..plain };
        assert_eq!(find(&view.table, &alt), None, "Alt+l stays Winter's");
    }

    #[test]
    fn test_a_view_may_not_start_on_a_chord_winter_owns() {
        let view = built(vec![ViewKey::new("today", "Today", &["t", "C-t", "M-t"])]);
        assert_eq!(view.problems.len(), 2, "{:?}", view.problems);
        assert_eq!(view.table.bindings[0].keys, ["t"], "the plain key is kept");
    }

    #[test]
    fn test_an_unknown_key_and_a_repeated_action_are_refused() {
        let view = built(vec![
            ViewKey::new("next", "Next", &["<nonsense>"]),
            ViewKey::new("next", "Next again", &["n"]),
        ]);
        assert_eq!(view.problems.len(), 2, "{:?}", view.problems);
        assert_eq!(view.table.bindings.len(), 1, "the first declaration stands");
    }

    #[test]
    fn test_a_view_s_keys_are_rebound_in_a_block_named_for_it() {
        let view = built(vec![ViewKey::new("next", "Next", &["l"])]);
        assert_eq!(view.table.block, "declaring.board", "the view's full id");
        assert!(view.table.configurable);
    }
}
