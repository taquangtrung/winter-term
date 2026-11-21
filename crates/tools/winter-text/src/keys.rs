//! The text editor's key tables the Keys page lists for it: Winter's own
//! keys over the text (the inline suggestions'), as Cherry owns every
//! editing key; and the user's keys for Cherry's commands, read from
//! `keybindings.kdl` and put in force in the window thread's editor.

use std::cell::RefCell;
use std::collections::HashMap;

use cherry::keys::Key;
use cherry::misty::keys::to_cherry;
use cherry::{CommandError, Mode};
use winter_field::misty_key;
use winter_model::input::table::{self, Binding, KeyTable, Listed};

use super::session;

// ========================================================================
// Constants
// ========================================================================

/// Every key table of the text editor, for the Keys page to list; Cherry
/// binds every editing key.
pub const KEY_TABLES: &[&dyn Listed] = &[&INLINE_KEYS];

/// The blocks of `keybindings.kdl` that rebind Cherry's commands, each
/// with the mode it rebinds them in: `delete-char "F5" in="editor"` binds
/// in Normal mode, `in="editor.insert"` in Insert mode.
const CHERRY_BLOCKS: &[(&str, Mode)] = &[
    ("editor", Mode::Normal),
    ("editor.insert", Mode::Insert),
    ("editor.operator", Mode::OperatorPending),
    ("editor.prompt", Mode::CommandLine),
    ("editor.visual", Mode::Visual),
    ("editor.visual_block", Mode::VisualBlock),
    ("editor.visual_line", Mode::VisualLine),
];

thread_local! {
    /// The keys each command the user rebinds had before, by mode and id,
    /// in Cherry's notation: what it gets back once the user binds it no
    /// longer. Kept per thread, beside the window thread's editor.
    static REPLACED: RefCell<HashMap<(Mode, String), Vec<String>>> =
        RefCell::new(HashMap::new());
}

/// The keys over an inline suggestion, ahead of Cherry's: ghost text's
/// while typing, a next edit's while its card shows.
pub(crate) const INLINE_KEYS: KeyTable<InlineAct> = KeyTable {
    block: "inline",
    bindings: &[
        Binding {
            action: InlineAct::Ask,
            id: "ask",
            keys: &["C-\\"],
            label: "Ask for Ghost Text",
        },
        Binding {
            action: InlineAct::TakeAll,
            id: "take_all",
            keys: &["TAB", "C-RET"],
            label: "Take the Suggestion",
        },
        Binding {
            action: InlineAct::TakeWord,
            id: "take_word",
            keys: &["M-f", "M-<right>", "C-,"],
            label: "Take the Ghost Text's Next Word",
        },
        Binding {
            action: InlineAct::TakeLine,
            id: "take_line",
            keys: &["C-."],
            label: "Take the Ghost Text's Line",
        },
        Binding {
            action: InlineAct::Next,
            id: "next",
            keys: &["M-n"],
            label: "Next Ghost Text",
        },
        Binding {
            action: InlineAct::Previous,
            id: "previous",
            keys: &["M-p"],
            label: "Previous Ghost Text",
        },
        Binding {
            action: InlineAct::PutAway,
            id: "put_away",
            keys: &["C-g"],
            label: "Put the Suggestion Away",
        },
        Binding {
            action: InlineAct::Leave,
            id: "leave",
            keys: &["ESC"],
            label: "Put the Suggestion Away, then Escape",
        },
    ],
    configurable: true,
    title: "Text: Inline Suggestions",
};

// ========================================================================
// Functions
// ========================================================================

/// Whether the block `block` of `keybindings.kdl` rebinds Cherry's commands.
pub fn is_cherry_block(block: &str) -> bool {
    CHERRY_BLOCKS.iter().any(|(name, _)| *name == block)
}

/// Puts `keys`, the user's keys for Cherry's commands by block, command id,
/// and specs in Winter's notation (an empty list unbinding the command), in
/// force in the window thread's editor, each in place of the keys the
/// command had; a command the user bound before and binds no longer gets its
/// own keys back. Returns what could not be bound, one line each.
pub fn set_user_keys(keys: &HashMap<String, HashMap<String, Vec<String>>>) -> Vec<String> {
    let mut problems = Vec::new();
    let mut wanted: Vec<(&str, Mode, &str, Vec<String>)> = Vec::new();
    for (block, commands) in keys {
        let Some(mode) = mode_of(block) else {
            problems.push(format!("`{block}` is no block of Cherry's commands"));
            continue;
        };
        for (id, specs) in commands {
            let converted: Option<Vec<String>> =
                specs.iter().map(|spec| cherry_spec(spec)).collect();
            match converted {
                Some(specs) => wanted.push((block, mode, id, specs)),
                None => problems.push(format!("`{block}`: {specs:?} for \"{id}\" is no key")),
            }
        }
    }
    let is_wanted = |mode: Mode, id: &str| {
        wanted
            .iter()
            .any(|(_, wanted_mode, wanted_id, _)| *wanted_mode == mode && *wanted_id == id)
    };
    let forgotten: Vec<((Mode, String), Vec<String>)> = REPLACED.with(|cell| {
        let mut replaced = cell.borrow_mut();
        let gone: Vec<(Mode, String)> = replaced
            .keys()
            .filter(|(mode, id)| !is_wanted(*mode, id))
            .cloned()
            .collect();
        gone.into_iter()
            .filter_map(|key| replaced.remove_entry(&key))
            .collect()
    });
    if forgotten.is_empty() && wanted.is_empty() {
        return problems;
    }
    session::with(|session| {
        for ((mode, id), specs) in forgotten {
            let specs: Vec<&str> = specs.iter().map(String::as_str).collect();
            let restored = session.editor.override_keys(mode, &id, &specs);
            debug_assert!(restored.is_ok(), "{id}: {restored:?}");
        }
        wanted.sort_by(|a, b| (a.0, a.2).cmp(&(b.0, b.2)));
        for (block, mode, id, specs) in wanted {
            let before = REPLACED
                .with(|cell| cell.borrow().get(&(mode, id.to_string())).cloned())
                .unwrap_or_else(|| {
                    session
                        .editor
                        .keys_for(mode, id)
                        .iter()
                        .map(|keys| sequence_spec(keys))
                        .collect()
                });
            let refs: Vec<&str> = specs.iter().map(String::as_str).collect();
            match session.editor.override_keys(mode, id, &refs) {
                Ok(()) => REPLACED.with(|cell| {
                    cell.borrow_mut()
                        .entry((mode, id.to_string()))
                        .or_insert(before);
                }),
                Err(CommandError::UnknownId(_)) => {
                    problems.push(format!("`{block}` has no command named \"{id}\""));
                }
                Err(error) => problems.push(format!("`{block}`: {error}")),
            }
        }
    });
    problems
}

/// The mode the block `block` rebinds Cherry's commands in.
fn mode_of(block: &str) -> Option<Mode> {
    CHERRY_BLOCKS
        .iter()
        .find(|(name, _)| *name == block)
        .map(|(_, mode)| *mode)
}

/// `spec`, keys in Winter's notation (`F5`, `RET`, `SPC`), in Cherry's
/// (`<f5>`, `<cr>`, `<space>`): read as Winter's keys, then each converted
/// and written as Cherry writes it. `None` for a spec that is no keys, or
/// names a key Cherry has no code for.
fn cherry_spec(spec: &str) -> Option<String> {
    let keys = table::keys(spec).filter(|keys| !keys.is_empty())?;
    let keys: Option<Vec<Key>> = keys.iter().map(|key| to_cherry(misty_key(key))).collect();
    Some(sequence_spec(&keys?))
}

/// `keys`, a sequence of Cherry's keys, written in its notation.
fn sequence_spec(keys: &[Key]) -> String {
    keys.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

// ========================================================================
// Data Structures
// ========================================================================

/// What a key does to the inline suggestion on screen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InlineAct {
    /// Ask for ghost text where none came by itself.
    Ask,
    /// Put the suggestion away, the key going on to Cherry.
    Leave,
    /// Show the next ghost text suggestion.
    Next,
    /// Show the ghost text suggestion before.
    Previous,
    /// Put the suggestion away.
    PutAway,
    /// Take all of the suggestion.
    TakeAll,
    /// Take the rest of the ghost text's line.
    TakeLine,
    /// Take the ghost text's next word.
    TakeWord,
}
