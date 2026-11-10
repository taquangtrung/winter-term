//! Key tables: the keys a surface answers, each written once beside what it
//! does, what it is called, and the id `keybindings.kdl` names it by. The
//! surface dispatches by looking a key up in its table, and the help list,
//! the right-click menu, and the Keys editor read the same table, so changing
//! a key in the table changes it everywhere it is shown.
//!
//! A table's keys are its defaults. The user's own, read from the lines of
//! `keybindings.kdl` whose `in` names the table's block, are put in force with
//! [`set_overrides`], and every lookup here resolves an entry's keys as the
//! user's where they bound it, else the table's: one rule, so what a key does
//! and what the help says it does cannot disagree.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Deref;

use super::chord::{format_key, is_chord_match, parse_chord};
use super::Key;

// ========================================================================
// Constants
// ========================================================================

/// Joins the chords of one entry that run the same thing, in a help row.
const KEY_SEPARATOR: &str = " / ";

// ========================================================================
// Data Structures
// ========================================================================

/// A surface's keys: its entries, and the block of `keybindings.kdl` that
/// rebinds them, under the title the Keys editor heads them with.
#[derive(Clone, Copy, Debug)]
pub struct KeyTable<A: 'static> {
    /// The block of `keybindings.kdl` that rebinds the table's keys, as a
    /// line's `in` names it: `toggle_archive "Z" in="dir"`.
    pub block: &'static str,
    /// The entries, in the order the help lists them.
    pub bindings: &'static [Binding<A>],
    /// Whether the user may rebind the table's keys: `false` for keys that
    /// are listed for reading only, such as Vim's motions.
    pub configurable: bool,
    /// What the Keys editor heads the table's entries with: `Dir`.
    pub title: &'static str,
}

/// One entry of a key table: the chords that run `action`, the id the user
/// rebinds it by, and what a reader is told it does.
#[derive(Clone, Copy, Debug)]
pub struct Binding<A: 'static> {
    /// What the surface does for it.
    pub action: A,
    /// The name `keybindings.kdl` rebinds it by, unique within its table:
    /// `toggle_archive`.
    pub id: &'static str,
    /// Every way of typing it, in `keybindings.kdl` notation (`"C-S-p"`, `"N"`,
    /// `"<down>"`). A spec of two chords separated by a space (`"z u"`) is a
    /// sequence. The first spec is the one a menu names.
    pub keys: &'static [&'static str],
    /// What it does, as the help list shows it.
    pub label: &'static str,
}

/// One entry of a table as the Keys editor lists it: what it is called and
/// rebound by, the keys it has by default, and the keys in force.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListedBinding {
    /// The table's defaults, as written.
    pub defaults: Vec<String>,
    /// The id it is rebound by.
    pub id: &'static str,
    /// The keys in force: the user's where they bound it, else the defaults.
    pub keys: Vec<String>,
    /// What it does.
    pub label: &'static str,
}

/// One row of a surface's help: the keys, as written, and what they do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyHelp {
    /// The keys, joined.
    pub keys: String,
    /// What they do.
    pub label: String,
}

/// What the keys typed so far amount to against a table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableMatch<A> {
    /// A whole entry: do this.
    Exact(A),
    /// No entry starts with them.
    Miss,
    /// They start a sequence: wait for the next key.
    Prefix,
}

// ========================================================================
// Traits
// ========================================================================

/// A key table with its action type left out, so tables of every surface can
/// be listed together, as the Keys editor lists them.
pub trait Listed: Sync {
    /// The block of `keybindings.kdl` that rebinds the table's keys.
    fn block(&self) -> &'static str;
    /// Whether the user may rebind the table's keys.
    fn configurable(&self) -> bool;
    /// The entries, each with its defaults and the keys in force.
    fn listed(&self) -> Vec<ListedBinding>;
    /// What the Keys editor heads the table's entries with.
    fn title(&self) -> &'static str;
}

// ========================================================================
// Overrides
// ========================================================================

thread_local! {
    /// The user's own keys, by block and then id, each a list of specs; an
    /// empty list unbinds the entry. Kept per thread: keys are resolved on
    /// the window's thread alone, and tests running side by side do not see
    /// each other's.
    static OVERRIDES: RefCell<HashMap<String, HashMap<String, Vec<String>>>> =
        RefCell::new(HashMap::new());
}

/// Put `overrides`, the user's own keys by block and then id, in force for
/// every table, in place of any put in force before.
pub fn set_overrides(overrides: HashMap<String, HashMap<String, Vec<String>>>) {
    OVERRIDES.with(|cell| *cell.borrow_mut() = overrides);
}

/// The specs entry `binding` of `table` is typed by: the user's where they
/// bound it, else the table's own.
fn specs_of<A>(table: &KeyTable<A>, binding: &Binding<A>) -> Vec<String> {
    let user = match table.configurable {
        true => OVERRIDES.with(|cell| {
            cell.borrow()
                .get(table.block)
                .and_then(|ids| ids.get(binding.id))
                .cloned()
        }),
        false => None,
    };
    user.unwrap_or_else(|| binding.keys.iter().map(|spec| spec.to_string()).collect())
}

// ========================================================================
// KeyTable
// ========================================================================

impl<A> Deref for KeyTable<A> {
    type Target = [Binding<A>];

    fn deref(&self) -> &[Binding<A>] {
        self.bindings
    }
}

impl<A: Sync> Listed for KeyTable<A> {
    fn block(&self) -> &'static str {
        self.block
    }

    fn configurable(&self) -> bool {
        self.configurable
    }

    fn listed(&self) -> Vec<ListedBinding> {
        self.bindings
            .iter()
            .map(|binding| ListedBinding {
                defaults: binding.keys.iter().map(|spec| spec.to_string()).collect(),
                id: binding.id,
                keys: specs_of(self, binding),
                label: binding.label,
            })
            .collect()
    }

    fn title(&self) -> &'static str {
        self.title
    }
}

// ========================================================================
// Lookups
// ========================================================================

/// The action `key` runs on its own, or `None` for a key the table does not
/// bind as a single chord.
pub fn find<A: Copy>(table: &KeyTable<A>, key: &Key) -> Option<A> {
    match lookup(table, std::slice::from_ref(key)) {
        TableMatch::Exact(action) => Some(action),
        TableMatch::Miss | TableMatch::Prefix => None,
    }
}

/// What `typed` amounts to: a whole entry, the start of a sequence, or
/// nothing. A whole entry wins over a longer sequence that shares its keys.
pub fn lookup<A: Copy>(table: &KeyTable<A>, typed: &[Key]) -> TableMatch<A> {
    let mut is_prefix = false;
    for binding in table.bindings {
        for spec in specs_of(table, binding) {
            let Some(chords) = parse_spec(&spec) else {
                continue;
            };
            if !starts_with(&chords, typed) {
                continue;
            }
            if chords.len() == typed.len() {
                return TableMatch::Exact(binding.action);
            }
            is_prefix = true;
        }
    }
    match is_prefix {
        true => TableMatch::Prefix,
        false => TableMatch::Miss,
    }
}

/// What `typed` amounts to against `help`, the rows a surface read from its
/// own tables: the label of a whole entry, the start of a longer one, or
/// nothing.
pub fn lookup_help<'a>(help: &'a [KeyHelp], typed: &[Key]) -> TableMatch<&'a str> {
    let mut is_prefix = false;
    for row in help {
        for spec in row.keys.split(KEY_SEPARATOR) {
            let Some(chords) = parse_spec(spec) else {
                continue;
            };
            if !starts_with(&chords, typed) {
                continue;
            }
            if chords.len() == typed.len() {
                return TableMatch::Exact(row.label.as_str());
            }
            is_prefix = true;
        }
    }
    match is_prefix {
        true => TableMatch::Prefix,
        false => TableMatch::Miss,
    }
}

/// The keys that can follow `typed`, each with what it runs: the which-key
/// card for a sequence part-way typed.
pub fn continuations<A>(table: &KeyTable<A>, typed: &[Key]) -> Vec<(String, String)> {
    let mut items = Vec::new();
    for binding in table.bindings {
        for spec in specs_of(table, binding) {
            let Some(chords) = parse_spec(&spec) else {
                continue;
            };
            if chords.len() == typed.len() + 1 && starts_with(&chords, typed) {
                items.push((format_key(&chords[typed.len()]), binding.label.to_string()));
            }
        }
    }
    items
}

/// The keys of `action`'s first entry, one chord or a sequence, and what it
/// is called, for a menu entry typing them. `None` for an action the table
/// does not hold.
pub fn menu_typed<A: PartialEq>(
    table: &KeyTable<A>,
    action: A,
) -> Option<(Vec<Key>, &'static str)> {
    let binding = table.iter().find(|binding| binding.action == action)?;
    Some((
        parse_spec(specs_of(table, binding).first()?)?,
        binding.label,
    ))
}

/// The chords `spec` names, space-separated the way a table's keys are:
/// `None` for one naming an unknown key.
pub fn keys(spec: &str) -> Option<Vec<Key>> {
    parse_spec(spec)
}

/// The first key of `action`'s entry and what it is called, for a menu
/// entry standing for it. `None` for an action the table does not hold, or
/// one only a sequence runs.
pub fn menu_entry<A: PartialEq>(table: &KeyTable<A>, action: A) -> Option<(Key, &'static str)> {
    let binding = table.iter().find(|binding| binding.action == action)?;
    let chords = parse_spec(specs_of(table, binding).first()?)?;
    match chords.as_slice() {
        [only] => Some((*only, binding.label)),
        _ => None,
    }
}

/// The table as help rows, in its own order, each with the keys in force.
pub fn help<A>(table: &KeyTable<A>) -> Vec<KeyHelp> {
    table
        .iter()
        .map(|binding| KeyHelp {
            keys: specs_of(table, binding).join(KEY_SEPARATOR),
            label: binding.label.to_string(),
        })
        .collect()
}

/// Every spec in `table` that does not parse, for a test to insist on none:
/// a misspelled key would otherwise never match and nothing would say so.
pub fn invalid_specs<A>(table: &KeyTable<A>) -> Vec<&'static str> {
    table
        .iter()
        .flat_map(|binding| binding.keys.iter().copied())
        .filter(|spec| parse_spec(spec).is_none())
        .collect()
}

/// Every id `table` holds more than once, for a test to insist on none: the
/// user could not rebind one of them without the other.
pub fn repeated_ids<A>(table: &KeyTable<A>) -> Vec<&'static str> {
    let mut seen = std::collections::HashSet::new();
    table
        .iter()
        .map(|binding| binding.id)
        .filter(|id| !seen.insert(*id))
        .collect()
}

// ========================================================================
// Helpers
// ========================================================================

/// A spec's chords, or `None` for one that names an unknown key.
fn parse_spec(spec: &str) -> Option<Vec<Key>> {
    spec.split_whitespace().map(parse_chord).collect()
}

/// Whether `typed` is how `chords` start, each pressed key matched the way
/// a binding matches: canonically, then unshifted.
fn starts_with(chords: &[Key], typed: &[Key]) -> bool {
    typed.len() <= chords.len()
        && chords
            .iter()
            .zip(typed)
            .all(|(bound, pressed)| is_chord_match(bound, &pressed.canonical()))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KeyCode;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Act {
        Down,
        Next,
        Previous,
        Unfold,
    }

    const TABLE: KeyTable<Act> = KeyTable {
        block: "test",
        bindings: &[
            Binding {
                action: Act::Down,
                id: "down",
                keys: &["j", "<down>"],
                label: "Move down",
            },
            Binding {
                action: Act::Next,
                id: "next",
                keys: &["n"],
                label: "Next match",
            },
            Binding {
                action: Act::Previous,
                id: "previous",
                keys: &["N"],
                label: "Previous match",
            },
            Binding {
                action: Act::Unfold,
                id: "unfold",
                keys: &["z u"],
                label: "Open the subtree",
            },
        ],
        configurable: true,
        title: "Test",
    };

    fn key(code: KeyCode, shift: bool) -> Key {
        Key {
            alt: false,
            code,
            ctrl: false,
            shift,
            sup: false,
        }
    }

    #[test]
    fn test_a_capital_matches_however_the_shifted_letter_arrives() {
        assert_eq!(
            find(&TABLE, &key(KeyCode::Char('N'), true)),
            Some(Act::Previous)
        );
        assert_eq!(
            find(&TABLE, &key(KeyCode::Char('n'), true)),
            Some(Act::Previous)
        );
        assert_eq!(
            find(&TABLE, &key(KeyCode::Char('n'), false)),
            Some(Act::Next)
        );
    }

    #[test]
    fn test_every_spelling_of_an_entry_runs_it() {
        assert_eq!(find(&TABLE, &key(KeyCode::Down, false)), Some(Act::Down));
        assert_eq!(
            find(&TABLE, &key(KeyCode::Char('j'), false)),
            Some(Act::Down)
        );
    }

    #[test]
    fn test_an_extra_modifier_is_a_different_key() {
        let ctrl_j = Key {
            ctrl: true,
            ..key(KeyCode::Char('j'), false)
        };
        assert_eq!(find(&TABLE, &ctrl_j), None);
    }

    #[test]
    fn test_a_sequence_waits_then_completes() {
        let z = key(KeyCode::Char('z'), false);
        let u = key(KeyCode::Char('u'), false);
        assert_eq!(lookup(&TABLE, &[z]), TableMatch::Prefix);
        assert_eq!(lookup(&TABLE, &[z, u]), TableMatch::Exact(Act::Unfold));
        assert_eq!(find(&TABLE, &z), None);
        assert_eq!(
            continuations(&TABLE, &[z]),
            [("u".to_string(), "Open the subtree".to_string())]
        );
    }

    #[test]
    fn test_a_menu_names_the_first_single_chord() {
        assert_eq!(
            menu_entry(&TABLE, Act::Down),
            Some((key(KeyCode::Char('j'), false), "Move down"))
        );
        assert_eq!(menu_entry(&TABLE, Act::Unfold), None);
    }

    #[test]
    fn test_a_misspelled_key_is_reported() {
        const BAD: KeyTable<Act> = KeyTable {
            bindings: &[Binding {
                action: Act::Down,
                id: "down",
                keys: &["<dwn>"],
                label: "",
            }],
            ..TABLE
        };
        assert_eq!(invalid_specs(&BAD), ["<dwn>"]);
        assert!(invalid_specs(&TABLE).is_empty());
    }

    fn user_keys(block: &str, id: &str, specs: &[&str]) {
        let ids = HashMap::from([(
            id.to_string(),
            specs.iter().map(|spec| spec.to_string()).collect(),
        )]);
        set_overrides(HashMap::from([(block.to_string(), ids)]));
    }

    #[test]
    fn test_the_users_keys_replace_the_tables_own() {
        user_keys("test", "next", &["C-n"]);
        let ctrl_n = Key {
            ctrl: true,
            ..key(KeyCode::Char('n'), false)
        };
        assert_eq!(find(&TABLE, &ctrl_n), Some(Act::Next));
        assert_eq!(
            find(&TABLE, &key(KeyCode::Char('n'), false)),
            None,
            "the default is gone"
        );
        assert_eq!(help(&TABLE)[1].keys, "C-n", "and the help says so");
        let listed = TABLE.listed();
        assert_eq!(
            (listed[1].defaults.clone(), listed[1].keys.clone()),
            (vec!["n".to_string()], vec!["C-n".to_string()])
        );
        set_overrides(HashMap::new());
        assert_eq!(
            find(&TABLE, &key(KeyCode::Char('n'), false)),
            Some(Act::Next),
            "back to the default"
        );
    }

    #[test]
    fn test_no_keys_unbinds_an_entry() {
        user_keys("test", "down", &[]);
        assert_eq!(find(&TABLE, &key(KeyCode::Char('j'), false)), None);
        assert_eq!(find(&TABLE, &key(KeyCode::Down, false)), None);
        set_overrides(HashMap::new());
    }

    #[test]
    fn test_a_table_for_reading_only_keeps_its_keys() {
        const READ_ONLY: KeyTable<Act> = KeyTable {
            configurable: false,
            ..TABLE
        };
        user_keys("test", "next", &["C-n"]);
        assert_eq!(
            find(&READ_ONLY, &key(KeyCode::Char('n'), false)),
            Some(Act::Next)
        );
        set_overrides(HashMap::new());
    }

    #[test]
    fn test_another_blocks_keys_change_nothing() {
        user_keys("elsewhere", "next", &["C-n"]);
        assert_eq!(
            find(&TABLE, &key(KeyCode::Char('n'), false)),
            Some(Act::Next)
        );
        set_overrides(HashMap::new());
    }

    #[test]
    fn test_a_repeated_id_is_reported() {
        const TWICE: KeyTable<Act> = KeyTable {
            bindings: &[
                Binding {
                    action: Act::Down,
                    id: "down",
                    keys: &["j"],
                    label: "",
                },
                Binding {
                    action: Act::Next,
                    id: "down",
                    keys: &["n"],
                    label: "",
                },
            ],
            ..TABLE
        };
        assert_eq!(repeated_ids(&TWICE), ["down"]);
        assert!(repeated_ids(&TABLE).is_empty());
    }
}
