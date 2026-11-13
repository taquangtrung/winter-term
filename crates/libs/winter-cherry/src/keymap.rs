//! The keymap adapter: for an app that resolves its keys with Cherry's
//! keymap, [`bind`] binds a [`CommandRegistry`]'s keys into a
//! `cherry::keys::Keymap`, [`which_key_entries`] turns the continuations of
//! a pending prefix into [`WhichKeyEntry`] rows, and [`to_keys`] turns
//! Cherry keys (a resolver's pending prefix) into misty keys. Single keys
//! convert with [`crate::keys`].

use std::hash::Hash;

use cherry::keys::{BindError, Continuation, Keymap};
use misty::key::Key;
use misty::{CommandRegistry, KeyScope, WhichKeyEntry};

use super::keys::{from_cherry, to_cherry};

// ========================================================================
// Constants
// ========================================================================

/// What an entry that leads to more keys, and runs nothing itself, says.
const MORE_KEYS_TITLE: &str = "more keys";

// ========================================================================
// Functions
// ========================================================================

/// Binds the keys in force for the commands `scope` takes into `layer` of
/// `keymap`, each to its command id: defaults as defaults, overridden keys
/// as user bindings, so the user's win.
pub fn bind<L: Copy + Eq + Hash>(
    registry: &CommandRegistry,
    keymap: &mut Keymap<L, &'static str>,
    layer: L,
    scope: KeyScope,
) -> Result<(), BindError> {
    for row in registry.bindings(scope) {
        for sequence in &row.keys {
            let sequence: Vec<cherry::keys::Key> =
                sequence.iter().map(|&key| to_cherry(key)).collect();
            match row.overridden {
                true => keymap.bind_user(layer, &sequence, row.id)?,
                false => keymap.bind_default(layer, &sequence, row.id)?,
            }
        }
    }
    Ok(())
}

/// The which-key rows of `continuations`, as a Cherry keymap's help data
/// lists them, each titled by `title` of its command; a key that only
/// leads to more keys is titled "more keys".
pub fn which_key_entries<C>(
    continuations: &[Continuation<'_, C>],
    title: impl Fn(&C) -> String,
) -> Vec<WhichKeyEntry> {
    continuations
        .iter()
        .map(|continuation| {
            let title = continuation
                .command
                .map_or_else(|| MORE_KEYS_TITLE.to_owned(), &title);
            WhichKeyEntry::new(from_cherry(continuation.key), title, continuation.continues)
        })
        .collect()
}

/// `keys`, Cherry keys such as a resolver's pending prefix, as misty keys.
pub fn to_keys(keys: &[cherry::keys::Key]) -> Vec<Key> {
    keys.iter().map(|&key| from_cherry(key)).collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use misty::Command;
    use misty::key::parse_sequence;

    use super::*;

    /// `tab.new` on `C-t`, a local-only `copy` on `C-c`, and `top` on
    /// `g g` with `deep` on `g z a`.
    const COMMANDS: &[Command] = &[
        Command::new("tab.new", "New Tab", "Tabs").with_keys(&["C-t"]),
        Command::new("copy", "Copy", "Edit")
            .with_keys(&["C-c"])
            .local_only(),
        Command::new("top", "Top", "Go").with_keys(&["g g"]),
        Command::new("deep", "Deep", "Go").with_keys(&["g z a"]),
    ];

    fn theirs(spec: &str) -> Vec<cherry::keys::Key> {
        parse_sequence(spec)
            .expect("a valid sequence")
            .into_iter()
            .map(to_cherry)
            .collect()
    }

    fn keymap(registry: &CommandRegistry, scope: KeyScope) -> Keymap<(), &'static str> {
        let mut keymap = Keymap::new();
        bind(registry, &mut keymap, (), scope).expect("bindable keys");
        keymap
    }

    #[test]
    fn test_an_override_binds_over_the_default_and_text_focus_leaves_out_local_only_keys() {
        let mut registry = CommandRegistry::new(COMMANDS).expect("a valid table");
        registry
            .override_keys("tab.new", &["M-t"])
            .expect("a known id");
        let all = keymap(&registry, KeyScope::All);
        let active = all.active(&[()]);
        assert_eq!(active.command_for(&theirs("M-t")), Some(&"tab.new"));
        assert_eq!(
            active.command_for(&theirs("C-t")),
            None,
            "the default is gone"
        );
        assert_eq!(active.command_for(&theirs("C-c")), Some(&"copy"));
        let text = keymap(&registry, KeyScope::TextFocused);
        assert_eq!(text.active(&[()]).command_for(&theirs("C-c")), None);
    }

    #[test]
    fn test_which_key_rows_title_commands_and_mark_prefixes() {
        let registry = CommandRegistry::new(COMMANDS).expect("a valid table");
        let keymap = keymap(&registry, KeyScope::All);
        let layers = [()];
        let active = keymap.active(&layers);
        let entries = which_key_entries(&active.continuations(&theirs("g")), |id| {
            registry
                .command(id)
                .map_or(*id, |command| command.title)
                .to_owned()
        });
        let key = |spec| parse_sequence(spec).expect("a key")[0];
        assert_eq!(
            entries,
            [
                WhichKeyEntry::new(key("g"), "Top", false),
                WhichKeyEntry::new(key("z"), MORE_KEYS_TITLE, true),
            ]
        );
        assert_eq!(
            to_keys(&theirs("g z")),
            parse_sequence("g z").expect("keys")
        );
    }
}
