//! Multi-key sequences: bindings of two or more chords typed one after another
//! (`M-g M-h`, `M-g r`), looked up as a prefix grows one key at a time.

use super::chord::{format_key, is_chord_match};
use super::Key;

// ========================================================================
// Constants
// ========================================================================

/// The longest sequence a binding may name, in chords.
pub(crate) const MAX_SEQUENCE_LEN: usize = 4;

/// Marks a continuation that opens more choices rather than running one,
/// after the categories it leads to, as magic-vscode's key hints do.
const GROUP_ARROW: &str = " \u{2192}";

// ========================================================================
// Data Structures
// ========================================================================

/// Bindings from chord sequences to the command names they run.
///
/// No sequence is a prefix of another: binding one drops every binding it
/// would shadow or be shadowed by, so a prefix is never also a command and a
/// lookup can always tell whether to run, wait, or give up.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SequenceMap {
    entries: Vec<SequenceEntry>,
}

/// One sequence binding.
#[derive(Clone, Debug, Eq, PartialEq)]
struct SequenceEntry {
    keys: Vec<Key>,
    name: String,
}

/// What the keys typed so far amount to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SequenceMatch<'a> {
    /// They are a whole binding: run the command named here.
    Exact(&'a str),
    /// No binding starts with them.
    Miss,
    /// They start one or more bindings: wait for the next key.
    Prefix,
}

// ========================================================================
// SequenceMap
// ========================================================================

impl SequenceMap {
    /// Bind `keys` to `name`, dropping every binding that shares a prefix
    /// with it: the same keys, a sequence it starts, or one that starts it.
    pub(crate) fn bind(&mut self, keys: Vec<Key>, name: String) {
        self.entries.retain(|entry| {
            !entry
                .keys
                .iter()
                .zip(&keys)
                .all(|(bound, binding)| is_same_chord(bound, binding))
        });
        self.entries.push(SequenceEntry { keys, name });
    }

    /// Drop every binding whose first chord is `key`, for a chord being bound
    /// on its own: a key that runs a command cannot also open a sequence.
    pub(crate) fn unbind_leader(&mut self, key: &Key) {
        self.entries.retain(|entry| {
            !entry
                .keys
                .first()
                .is_some_and(|first| is_same_chord(first, key))
        });
    }

    /// Drop every binding whose command `is_rebound` claims, for a command
    /// being rebound (under any of its names).
    pub(crate) fn unbind_commands(&mut self, is_rebound: impl Fn(&str) -> bool) {
        self.entries.retain(|entry| !is_rebound(&entry.name));
    }

    /// Whether any binding opens with `key`, as typed.
    pub(crate) fn has_leader(&self, key: &Key) -> bool {
        self.entries.iter().any(|entry| {
            entry
                .keys
                .first()
                .is_some_and(|first| is_chord_match(first, key))
        })
    }

    /// The keys bound to `name`, if any.
    pub(crate) fn keys_for(&self, name: &str) -> Option<&[Key]> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.keys.as_slice())
    }

    /// What `typed` amounts to against these bindings.
    pub(crate) fn lookup(&self, typed: &[Key]) -> SequenceMatch<'_> {
        let mut is_prefix = false;
        for entry in self
            .entries
            .iter()
            .filter(|entry| starts_with(&entry.keys, typed))
        {
            if entry.keys.len() == typed.len() {
                return SequenceMatch::Exact(&entry.name);
            }
            is_prefix = true;
        }
        match is_prefix {
            true => SequenceMatch::Prefix,
            false => SequenceMatch::Miss,
        }
    }

    /// The keys that can follow `typed`, sorted, each with what it does: the
    /// command it completes, as `label` words its name, or for a key that
    /// opens a longer sequence, the categories of the commands it leads to
    /// and [`GROUP_ARROW`] (`"Git, Window \u{2192}"`).
    pub(crate) fn continuations(
        &self,
        typed: &[Key],
        label: impl Fn(&str) -> String,
    ) -> Vec<(String, String)> {
        // Each next key with the commands under it; no prefix is also a
        // binding, so a key leads either to one command or to a group.
        let mut next_keys: Vec<(String, bool, Vec<&str>)> = Vec::new();
        for entry in self
            .entries
            .iter()
            .filter(|entry| entry.keys.len() > typed.len() && starts_with(&entry.keys, typed))
        {
            let next = format_key(&entry.keys[typed.len()]);
            let is_command = entry.keys.len() == typed.len() + 1;
            match next_keys.iter_mut().find(|(key, _, _)| *key == next) {
                Some((_, _, names)) => names.push(&entry.name),
                None => next_keys.push((next, is_command, vec![&entry.name])),
            }
        }
        let mut items: Vec<(String, String)> = next_keys
            .into_iter()
            .map(|(key, is_command, names)| {
                let what = match is_command {
                    true => label(names[0]),
                    false => group_label(names.iter().map(|name| label(name))),
                };
                (key, what)
            })
            .collect();
        items.sort();
        items
    }
}

// ========================================================================
// Labels
// ========================================================================

/// What a key opening a group reads as: the distinct categories of the
/// `labels` it leads to, in order, then [`GROUP_ARROW`].
fn group_label(labels: impl Iterator<Item = String>) -> String {
    let mut categories: Vec<String> = Vec::new();
    for label in labels {
        let category = category(&label).to_string();
        if !categories.contains(&category) {
            categories.push(category);
        }
    }
    categories.join(", ") + GROUP_ARROW
}

/// A command label's category: what comes before its colon (`"Git: Status"`
/// is `Git`), or else its first word.
fn category(label: &str) -> &str {
    match label.split_once(':') {
        Some((category, _)) => category.trim(),
        None => label.split_whitespace().next().unwrap_or(label),
    }
}

// ========================================================================
// Matching
// ========================================================================

/// Whether two bound chords are typed the same way: one written with the
/// shifted glyph (`|`), the other with Shift and the physical key (`S-\\`).
fn is_same_chord(one: &Key, other: &Key) -> bool {
    is_chord_match(one, other) || is_chord_match(other, one)
}

/// Whether `bound` starts with the chords in `typed`.
fn starts_with(bound: &[Key], typed: &[Key]) -> bool {
    bound.len() >= typed.len()
        && bound
            .iter()
            .zip(typed)
            .all(|(bound, pressed)| is_chord_match(bound, pressed))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::KeyCode;
    use super::*;

    fn alt(c: char) -> Key {
        Key::with_alt(KeyCode::Char(c))
    }

    fn plain(c: char) -> Key {
        Key::plain(KeyCode::Char(c))
    }

    fn sample() -> SequenceMap {
        let mut map = SequenceMap::default();
        map.bind(vec![alt('g'), alt('h')], "move_tab_left".to_string());
        map.bind(vec![alt('g'), plain('r')], "open_remote".to_string());
        map
    }

    #[test]
    fn test_lookup_waits_on_a_prefix_and_runs_the_whole_sequence() {
        let map = sample();
        assert_eq!(map.lookup(&[alt('g')]), SequenceMatch::Prefix);
        assert_eq!(
            map.lookup(&[alt('g'), plain('r')]),
            SequenceMatch::Exact("open_remote")
        );
        assert_eq!(map.lookup(&[alt('g'), plain('x')]), SequenceMatch::Miss);
    }

    #[test]
    fn test_a_follow_key_only_matches_under_its_own_leader() {
        // The old window keymap matched the follow key under any leader, so
        // `C-w M-h` ran `move_tab_left`.
        let map = sample();
        let ctrl_w = Key {
            ctrl: true,
            ..plain('w')
        };
        assert_eq!(map.lookup(&[ctrl_w, alt('h')]), SequenceMatch::Miss);
    }

    #[test]
    fn test_binding_a_prefix_drops_the_sequences_it_would_shadow() {
        let mut map = sample();
        map.bind(vec![alt('g'), alt('h'), plain('x')], "deeper".to_string());
        assert_eq!(map.keys_for("move_tab_left"), None);
        map.bind(vec![alt('g')], "short".to_string());
        assert_eq!(map.keys_for("deeper"), None);
        assert_eq!(map.keys_for("open_remote"), None);
        assert_eq!(map.lookup(&[alt('g')]), SequenceMatch::Exact("short"));
    }

    #[test]
    fn test_a_shifted_follow_key_matches_the_physical_key_it_names() {
        let mut map = SequenceMap::default();
        let bound_shift_backslash = Key {
            shift: true,
            ..plain('\\')
        };
        map.bind(vec![alt('g'), bound_shift_backslash], "split".to_string());
        let typed_pipe = Key {
            shift: true,
            ..plain('|')
        };
        assert_eq!(
            map.lookup(&[alt('g'), typed_pipe]),
            SequenceMatch::Exact("split")
        );
    }

    #[test]
    fn test_continuations_name_commands_and_mark_longer_prefixes() {
        let mut map = sample();
        map.bind(
            vec![alt('g'), plain('b'), plain('c')],
            "branch_create".to_string(),
        );
        map.bind(
            vec![alt('g'), plain('b'), plain('t')],
            "tab_new".to_string(),
        );
        let label = |name: &str| match name {
            "branch_create" => "Git: Create Branch".to_string(),
            "tab_new" => "New Tab".to_string(),
            other => other.to_uppercase(),
        };
        assert_eq!(
            map.continuations(&[alt('g')], label),
            vec![
                ("M-h".to_string(), "MOVE_TAB_LEFT".to_string()),
                ("b".to_string(), "Git, New \u{2192}".to_string()),
                ("r".to_string(), "OPEN_REMOTE".to_string()),
            ]
        );
    }

    #[test]
    fn test_a_shifted_symbol_leader_opens_its_sequence_and_shadows_its_twin() {
        // Regression: the leader check compared chords exactly, so a sequence
        // whose leader was written `S-\\` never opened when `|` was typed,
        // and binding the same keys spelled the other way kept both.
        let shift_backslash = Key {
            shift: true,
            ..plain('\\')
        };
        let typed_pipe = Key {
            shift: true,
            ..plain('|')
        };
        let mut map = SequenceMap::default();
        map.bind(vec![shift_backslash, plain('x')], "first".to_string());
        assert!(map.has_leader(&typed_pipe));
        map.bind(vec![typed_pipe, plain('x')], "second".to_string());
        assert_eq!(
            map.keys_for("first"),
            None,
            "the same keys typed are one binding"
        );
        map.unbind_leader(&shift_backslash);
        assert!(!map.has_leader(&typed_pipe));
    }
}
