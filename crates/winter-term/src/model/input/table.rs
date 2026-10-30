//! Key tables: the keys a surface answers, each written once beside what it
//! does and what it is called. The surface dispatches by looking a key up in
//! its table, and the help list and the right-click menu read the same table,
//! so changing a key in the table changes it everywhere it is shown.

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

/// One entry of a key table: the chords that run `action`, and what a reader
/// is told it does.
#[derive(Clone, Copy, Debug)]
pub struct Binding<A: 'static> {
    /// What the surface does for it.
    pub action: A,
    /// Every way of typing it, in `keybindings.kdl` notation (`"C-S-p"`, `"N"`,
    /// `"<down>"`). A spec of two chords separated by a space (`"z u"`) is a
    /// sequence. The first spec is the one a menu names.
    pub keys: &'static [&'static str],
    /// What it does, as the help list shows it.
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
// Lookups
// ========================================================================

/// The action `key` runs on its own, or `None` for a key the table does not
/// bind as a single chord.
pub fn find<A: Copy>(table: &[Binding<A>], key: &Key) -> Option<A> {
    match lookup(table, std::slice::from_ref(key)) {
        TableMatch::Exact(action) => Some(action),
        TableMatch::Miss | TableMatch::Prefix => None,
    }
}

/// What `typed` amounts to: a whole entry, the start of a sequence, or
/// nothing. A whole entry wins over a longer sequence that shares its keys.
pub fn lookup<A: Copy>(table: &[Binding<A>], typed: &[Key]) -> TableMatch<A> {
    let mut is_prefix = false;
    for binding in table {
        for spec in binding.keys {
            let Some(chords) = parse_spec(spec) else {
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

/// The keys that can follow `typed`, each with what it runs: the which-key
/// card for a sequence part-way typed.
pub fn continuations<A>(table: &[Binding<A>], typed: &[Key]) -> Vec<(String, String)> {
    let mut items = Vec::new();
    for binding in table {
        for spec in binding.keys {
            let Some(chords) = parse_spec(spec) else {
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
    table: &[Binding<A>],
    action: A,
) -> Option<(Vec<Key>, &'static str)> {
    let binding = table.iter().find(|binding| binding.action == action)?;
    Some((parse_spec(binding.keys.first()?)?, binding.label))
}

/// The chords `spec` names, space-separated the way a table's keys are:
/// `None` for one naming an unknown key.
pub fn keys(spec: &str) -> Option<Vec<Key>> {
    parse_spec(spec)
}

/// The first key of `action`'s entry and what it is called, for a menu
/// entry standing for it. `None` for an action the table does not hold, or
/// one only a sequence runs.
pub fn menu_entry<A: PartialEq>(table: &[Binding<A>], action: A) -> Option<(Key, &'static str)> {
    let binding = table.iter().find(|binding| binding.action == action)?;
    let chords = parse_spec(binding.keys.first()?)?;
    match chords.as_slice() {
        [only] => Some((*only, binding.label)),
        _ => None,
    }
}

/// The table as help rows, in its own order.
pub fn help<A>(table: &[Binding<A>]) -> Vec<KeyHelp> {
    table
        .iter()
        .map(|binding| KeyHelp {
            keys: binding.keys.join(KEY_SEPARATOR),
            label: binding.label.to_string(),
        })
        .collect()
}

/// Every spec in `table` that does not parse, for a test to insist on none:
/// a misspelled key would otherwise never match and nothing would say so.
pub fn invalid_specs<A>(table: &[Binding<A>]) -> Vec<&'static str> {
    table
        .iter()
        .flat_map(|binding| binding.keys.iter().copied())
        .filter(|spec| parse_spec(spec).is_none())
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
    use crate::model::input::KeyCode;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Act {
        Down,
        Next,
        Previous,
        Unfold,
    }

    const TABLE: &[Binding<Act>] = &[
        Binding {
            action: Act::Down,
            keys: &["j", "<down>"],
            label: "Move down",
        },
        Binding {
            action: Act::Next,
            keys: &["n"],
            label: "Next match",
        },
        Binding {
            action: Act::Previous,
            keys: &["N"],
            label: "Previous match",
        },
        Binding {
            action: Act::Unfold,
            keys: &["z u"],
            label: "Open the subtree",
        },
    ];

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
            find(TABLE, &key(KeyCode::Char('N'), true)),
            Some(Act::Previous)
        );
        assert_eq!(
            find(TABLE, &key(KeyCode::Char('n'), true)),
            Some(Act::Previous)
        );
        assert_eq!(
            find(TABLE, &key(KeyCode::Char('n'), false)),
            Some(Act::Next)
        );
    }

    #[test]
    fn test_every_spelling_of_an_entry_runs_it() {
        assert_eq!(find(TABLE, &key(KeyCode::Down, false)), Some(Act::Down));
        assert_eq!(
            find(TABLE, &key(KeyCode::Char('j'), false)),
            Some(Act::Down)
        );
    }

    #[test]
    fn test_an_extra_modifier_is_a_different_key() {
        let ctrl_j = Key {
            ctrl: true,
            ..key(KeyCode::Char('j'), false)
        };
        assert_eq!(find(TABLE, &ctrl_j), None);
    }

    #[test]
    fn test_a_sequence_waits_then_completes() {
        let z = key(KeyCode::Char('z'), false);
        let u = key(KeyCode::Char('u'), false);
        assert_eq!(lookup(TABLE, &[z]), TableMatch::Prefix);
        assert_eq!(lookup(TABLE, &[z, u]), TableMatch::Exact(Act::Unfold));
        assert_eq!(find(TABLE, &z), None);
        assert_eq!(
            continuations(TABLE, &[z]),
            [("u".to_string(), "Open the subtree".to_string())]
        );
    }

    #[test]
    fn test_a_menu_names_the_first_single_chord() {
        assert_eq!(
            menu_entry(TABLE, Act::Down),
            Some((key(KeyCode::Char('j'), false), "Move down"))
        );
        assert_eq!(menu_entry(TABLE, Act::Unfold), None);
    }

    #[test]
    fn test_a_misspelled_key_is_reported() {
        const BAD: &[Binding<Act>] = &[Binding {
            action: Act::Down,
            keys: &["<dwn>"],
            label: "",
        }];
        assert_eq!(invalid_specs(BAD), ["<dwn>"]);
        assert!(invalid_specs(TABLE).is_empty());
    }
}
