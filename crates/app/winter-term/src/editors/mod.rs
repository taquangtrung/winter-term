//! Editors: the tabs Winter paints itself, peers of the terminal in a pane's
//! strip.
//!
//! One module per kind of editor (with focused views such as [`keys`]), beside
//! the helpers they share. Everything in an editor stays pure and std-only
//! except the modules named for their side effects, so logic is testable
//! without a window, a filesystem, or a repository.
//!
//! The editors, one module per kind of tab Winter paints itself:
//!
//! - [`compile`]: a build, its output, and the places it points at.
//! - diff: texts compared and merged region by region, as Emacs' ediff
//!   does it, in the `winter_text` crate.
//! - dir: a directory listing, in the `winter_dir` crate.
//! - docker: what the docker daemon knows, in the `winter_docker` crate.
//! - [`extension`]: a tab an extension draws.
//! - git: the working tree's state, in the `winter_git` crate.
//! - [`grep`]: the lines under a directory holding some text.
//! - [`hex`]: a file's bytes, sixteen to a row, read and written.
//! - [`image`]: one picture, drawn by Winter itself.
//! - [`keys`]: the command list and the chord bound to each command.
//! - [`launcher`]: what a split opens, holding the place for what goes there.
//! - [`list`]: a picker's rows, saved as a buffer of their own.
//! - [`notifications`]: everything the window has said this session.
//! - pdf: one PDF document, drawn by Winter itself, in the `winter_pdf` crate.
//! - processes and system: the machine's monitors, in the `winter_monitor`
//!   crate.
//! - [`preview`]: a Markdown document as it reads rendered.
//! - text: one file as editable text, in the `winter_text` crate.
//!
//! The helpers more than one editor here shares (those an editor in a crate of
//! its own needs too live in `winter_model`):
//!
//! - highlight: coloring source with the TextMate grammars, in the
//!   `winter_syntax` crate.

pub mod compile;
pub mod extension;
#[cfg(feature = "wasm-extensions")]
pub mod extensions;
pub mod grep;
pub mod hex;
pub mod image;
pub mod keys;
pub mod launcher;
pub mod list;
pub mod notifications;
pub mod preview;
pub mod settings;
mod web_page;

use std::collections::{HashMap, HashSet};

use winter_model::editor::EditorCommand;
use winter_model::input::table::{self, Listed};
use winter_model::input::Blocks;

// ========================================================================
// Data Structures
// ========================================================================

/// One editor's block in `keybindings.kdl`: the commands it names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandBlock {
    /// The block's name, the editor kind's own (`git`).
    pub block: &'static str,
    /// The commands the editor runs by name.
    pub commands: &'static [EditorCommand],
    /// How the Keys editor heads the editor's commands.
    pub title: &'static str,
}

/// The user's keys from `keybindings.kdl`, split by what binds them (see
/// [`split_key_overrides`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SplitKeys {
    /// The blocks for the keymap: the window's commands and the editors'
    /// named ones, by block and then chord.
    pub keymap: Blocks,
    /// What is wrong with the key table bindings, in the keymap's words.
    pub problems: Vec<String>,
    /// What rebinds an entry of a key table, by block, id, and specs, for
    /// [`table::set_overrides`].
    pub tables: HashMap<String, HashMap<String, Vec<String>>>,
}

// ========================================================================
// Constants
// ========================================================================

/// Every editor that has commands to bind. An editor missing here has a block
/// that `keybindings.kdl` reports as unknown.
pub const COMMAND_BLOCKS: &[CommandBlock] = &[
    CommandBlock {
        block: "git",
        commands: winter_git::COMMANDS,
        title: "Git",
    },
    CommandBlock {
        block: "dir",
        commands: winter_dir::COMMANDS,
        title: "Dir",
    },
    CommandBlock {
        block: "editor",
        commands: winter_text::COMMANDS,
        title: "Editor",
    },
];

/// What `keybindings.kdl` binds an entry of a key table to, to take its
/// keys away: `toggle_archive "none" in="dir"`.
pub const UNBOUND: &str = "none";

/// The Markdown preview's keys.
const PREVIEW_KEY_TABLES: &[&dyn Listed] = preview::KEY_TABLES;

/// The Extensions page's keys, when extensions installed as files are built in.
#[cfg(feature = "wasm-extensions")]
const EXTENSIONS_KEY_TABLES: &[&dyn Listed] = extensions::KEY_TABLES;
#[cfg(not(feature = "wasm-extensions"))]
const EXTENSIONS_KEY_TABLES: &[&dyn Listed] = &[];

/// Every key table Winter has, by the surface that answers it: the Vim
/// motions every editor shares first, for reading only, then each editor's
/// own. The one list the Keys editor lists and `keybindings.kdl` rebinds by,
/// so a key cannot be bound where it is not listed, or listed where it cannot
/// be found.
const KEY_TABLE_GROUPS: &[&[&dyn Listed]] = &[
    winter_model::vim::nav::KEY_TABLES,
    winter_text::KEY_TABLES,
    winter_dir::KEY_TABLES,
    winter_text::diff::KEY_TABLES,
    winter_git::KEY_TABLES,
    grep::KEY_TABLES,
    compile::KEY_TABLES,
    winter_pdf::KEY_TABLES,
    hex::KEY_TABLES,
    image::KEY_TABLES,
    PREVIEW_KEY_TABLES,
    winter_monitor::proc::KEY_TABLES,
    winter_monitor::sys::KEY_TABLES,
    winter_docker::KEY_TABLES,
    notifications::KEY_TABLES,
    list::KEY_TABLES,
    EXTENSIONS_KEY_TABLES,
    launcher::KEY_TABLES,
    keys::KEY_TABLES,
    crate::window::PICKER_FRAMEWORK_KEY_TABLES,
    crate::window::PICKER_KEY_TABLES,
];

// ========================================================================
// Lookups
// ========================================================================

/// Every key table, as [`KEY_TABLE_GROUPS`] lists them, then those of the
/// extensions' views, built as they activate.
pub fn key_tables() -> impl Iterator<Item = &'static dyn Listed> {
    KEY_TABLE_GROUPS
        .iter()
        .flat_map(|group| group.iter().copied())
        .chain(crate::extensions::keys::listed().iter().copied())
}

/// The key table `block` of `keybindings.kdl` rebinds, if any does.
pub fn key_table(block: &str) -> Option<&'static dyn Listed> {
    key_tables().find(|table| table.block() == block)
}

/// The user's keys from `blocks`, `keybindings.kdl` by block and then chord,
/// split in two: what rebinds an entry of a key table, and the rest, for the
/// keymap, which binds the window's commands and the editors' named ones.
pub fn split_key_overrides(blocks: &Blocks) -> SplitKeys {
    let mut rest: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut overrides: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
    let mut problems = Vec::new();
    for (block, chords) in blocks {
        let Some(keys) = key_table(block) else {
            rest.insert(block.clone(), chords.clone());
            continue;
        };
        let ids: HashSet<&str> = keys.listed().iter().map(|entry| entry.id).collect();
        let has_named = block_commands(block).is_some();
        let mut sorted: Vec<(&String, &String)> = chords.iter().collect();
        sorted.sort();
        for (chord, name) in sorted {
            if !ids.contains(name.as_str()) {
                match has_named {
                    true => {
                        rest.entry(block.clone())
                            .or_default()
                            .insert(chord.clone(), name.clone());
                    }
                    false => problems.push(format!("`{block}` has no key named \"{name}\"")),
                }
                continue;
            }
            if !keys.configurable() {
                problems.push(format!(
                    "`{block}` is listed for reading only, so \"{name}\" keeps its keys"
                ));
                continue;
            }
            let specs = overrides
                .entry(block.clone())
                .or_default()
                .entry(name.clone())
                .or_default();
            match chord.as_str() {
                UNBOUND => {}
                spec if table::keys(spec).is_some() => specs.push(spec.to_string()),
                spec => problems.push(format!("`{block}`: \"{spec}\" for \"{name}\" is no key")),
            }
        }
    }
    SplitKeys {
        keymap: rest,
        problems,
        tables: overrides,
    }
}

/// The commands an editor's block may bind, or `None` for no editor's block.
pub fn block_commands(block: &str) -> Option<&'static [EditorCommand]> {
    COMMAND_BLOCKS
        .iter()
        .find(|entry| entry.block == block)
        .map(|entry| entry.commands)
}

/// What an editor's command is called, or `None` for no command of that editor's.
pub fn command_label(block: &str, name: &str) -> Option<&'static str> {
    block_commands(block)?
        .iter()
        .find(|command| command.name == name)
        .map(|command| command.label)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use winter_model::input::table;

    #[test]
    fn test_every_table_has_a_block_of_its_own() {
        let mut seen = HashSet::new();
        let repeated: Vec<&str> = key_tables()
            .map(|table| table.block())
            .filter(|block| !seen.insert(*block))
            .collect();
        assert!(repeated.is_empty(), "blocks named twice: {repeated:?}");
    }

    #[test]
    fn test_every_entry_has_an_id_of_its_own_within_its_table() {
        let mut repeated = Vec::new();
        for table in key_tables() {
            let mut seen = HashSet::new();
            for entry in table.listed() {
                if !seen.insert(entry.id) {
                    repeated.push(format!("{}.{}", table.block(), entry.id));
                }
            }
        }
        assert!(repeated.is_empty(), "ids named twice: {repeated:?}");
    }

    #[test]
    fn test_every_id_is_a_name_one_types_in_keybindings_kdl() {
        for table in key_tables() {
            for entry in table.listed() {
                let is_name = !entry.id.is_empty()
                    && entry
                        .id
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
                assert!(is_name, "{}: {:?}", table.block(), entry.id);
            }
        }
    }

    #[test]
    fn test_every_default_key_is_spelled_right() {
        for table in key_tables() {
            for entry in table.listed() {
                for spec in &entry.defaults {
                    assert!(
                        table::keys(spec).is_some(),
                        "{}.{}: {spec:?}",
                        table.block(),
                        entry.id
                    );
                }
            }
        }
    }

    fn blocks(entries: &[(&str, &str, &str)]) -> HashMap<String, HashMap<String, String>> {
        let mut blocks: HashMap<String, HashMap<String, String>> = HashMap::new();
        for (block, chord, name) in entries {
            blocks
                .entry(block.to_string())
                .or_default()
                .insert(chord.to_string(), name.to_string());
        }
        blocks
    }

    #[test]
    fn test_a_tables_keys_become_overrides_and_the_rest_goes_to_the_keymap() {
        let SplitKeys {
            keymap: rest,
            problems,
            tables: overrides,
        } = split_key_overrides(&blocks(&[
            ("dir", "M-z", "toggle_archive"),
            ("dir", "M-S-z", "toggle_archive"),
            ("dir", "C-x C-q", "toggle_edit"),
            ("window", "C-S-n", "new_tab"),
            ("dir.edit", "none", "throw_away"),
        ]));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(overrides["dir"]["toggle_archive"], ["M-S-z", "M-z"]);
        assert_eq!(
            overrides["dir.edit"]["throw_away"],
            Vec::<String>::new(),
            "unbound"
        );
        assert_eq!(
            rest["dir"].get("C-x C-q").map(String::as_str),
            Some("toggle_edit"),
            "a named command stays"
        );
        assert_eq!(
            rest["window"].get("C-S-n").map(String::as_str),
            Some("new_tab")
        );
        assert!(
            !rest.contains_key("dir.edit"),
            "a block that is only a table never reaches the keymap"
        );
    }

    #[test]
    fn test_what_a_table_cannot_take_is_reported() {
        let SplitKeys {
            problems,
            tables: overrides,
            ..
        } = split_key_overrides(&blocks(&[
            ("dir.edit", "C-c", "no_such_key"),
            ("dir", "C-<bogus>", "toggle_archive"),
            ("vim", "J", "down"),
        ]));
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(
            !overrides.contains_key("vim"),
            "Vim's motions keep their keys"
        );
    }

    /// The files that match keys written out for good, each a part of Winter
    /// that defines no command of its own: the Vim and Emacs engine every
    /// editor's text is typed through, whose keys are built in and listed for
    /// reading only, and the plumbing that parses, routes, records, and edits
    /// keys. Paths under `crates/`.
    const KEY_ENGINE_FILES: &[&str] = &[
        "app/winter-term/src/window/editor/column.rs",
        "app/winter-term/src/window/editor/cursor.rs",
        "app/winter-term/src/window/editor/keys.rs",
        "app/winter-term/src/window/editor/prompt.rs",
        "app/winter-term/src/window/input/keys.rs",
        "app/winter-term/src/window/input/misty.rs",
        "app/winter-term/src/window/input/sequence.rs",
        "app/winter-term/src/window/macro_record.rs",
        "app/winter-term/src/window/terminal/prompt_edit.rs",
        "base/winter-keys/src/chord.rs",
        "base/winter-keys/src/key.rs",
        "base/winter-model/src/button.rs",
        "base/winter-model/src/input/insert.rs",
        "base/winter-model/src/input/mod.rs",
        "base/winter-model/src/input/motion.rs",
        "base/winter-model/src/input/normal.rs",
        "base/winter-model/src/input/visual.rs",
        "base/winter-textedit/src/modal/insert.rs",
        "base/winter-textedit/src/modal/mod.rs",
        "base/winter-textedit/src/modal/normal.rs",
        "base/winter-textedit/src/modal/operators.rs",
        "base/winter-textedit/src/modal/pending.rs",
        "base/winter-textedit/src/modal/visual.rs",
        "base/winter-textedit/src/vim/grammar.rs",
        "base/winter-textedit/src/vim/motion.rs",
        "base/winter-textedit/src/vim/nav.rs",
        "tools/winter-text/src/keys.rs",
    ];

    /// The editors' keys still written out rather than declared in a key table,
    /// each file with how many lines match them: a list that only shrinks. A
    /// file moved onto a table comes off it, and a new key goes in a table, so
    /// a file here may not match more keys than it does now. Paths under
    /// `crates/`.
    const KEY_TABLE_DEBT: &[(&str, usize)] = &[
        ("app/winter-term/src/editors/image/mod.rs", 1),
        ("tools/winter-dir/src/edit.rs", 1),
        ("tools/winter-git/src/keys.rs", 7),
        ("tools/winter-git/src/push.rs", 2),
        ("tools/winter-pdf/src/chrome.rs", 8),
        ("tools/winter-pdf/src/editor.rs", 4),
        ("tools/winter-pdf/src/keys.rs", 1),
        ("tools/winter-pdf/src/navigate.rs", 2),
    ];

    /// The tables declared but not listed, each holding no key: the Docker
    /// sections with none of their own.
    const UNLISTED_KEY_TABLES: &[&str] = &["docker.section"];

    /// The `crates/` directory, every crate's source under it, in the folder
    /// of its layer.
    fn crates_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("the crates directory")
    }

    #[test]
    fn test_every_key_is_declared_in_a_key_table() {
        // A key matched as written could be rebound by no one, listed in
        // no Keys editor, and drift from the help that says what it does.
        let found =
            winter_model::input::test_support::literal_key_matches(&crates_dir(), KEY_ENGINE_FILES);
        let mut by_file: HashMap<String, usize> = HashMap::new();
        for line in &found {
            let file = line.split(':').next().unwrap_or_default().to_string();
            *by_file.entry(file).or_default() += 1;
        }
        let mut wrong = Vec::new();
        for (file, count) in &by_file {
            match KEY_TABLE_DEBT.iter().find(|(debt, _)| debt == file) {
                None => wrong.push(format!("{file}: {count} key(s) written out; declare them in a key table")),
                Some((_, allowed)) if count > allowed => {
                    wrong.push(format!("{file}: {count} keys written out, {allowed} allowed; declare the new ones in a key table"))
                }
                Some(_) => {}
            }
        }
        for (file, allowed) in KEY_TABLE_DEBT {
            let count = by_file.get(*file).copied().unwrap_or(0);
            if count < *allowed {
                wrong.push(format!(
                    "{file}: down to {count} from {allowed}: lower KEY_TABLE_DEBT to match"
                ));
            }
        }
        wrong.sort();
        let lines: Vec<&String> = found
            .iter()
            .filter(|line| {
                wrong
                    .iter()
                    .any(|w| w.starts_with(line.split(':').next().unwrap_or_default()))
            })
            .collect();
        assert!(
            wrong.is_empty(),
            "{}\n{}",
            wrong.join("\n"),
            lines
                .iter()
                .map(|line| line.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[test]
    fn test_every_key_table_is_listed() {
        // A table no list names is a surface's keys the Keys editor never
        // shows and `keybindings.kdl` can never rebind.
        let declared =
            regex::Regex::new(r#"const \w+: KeyTable<\w+> = KeyTable \{\s*block: "([^"]+)""#)
                .expect("a pattern");
        let listed: HashSet<&str> = key_tables().map(|table| table.block()).collect();
        let mut missing = Vec::new();
        let mut dirs = vec![crates_dir()];
        while let Some(dir) = dirs.pop() {
            for path in std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
            {
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let source = text.split("#[cfg(test)]").next().unwrap_or_default();
                for found in declared.captures_iter(source) {
                    let block = &found[1];
                    if !listed.contains(block) && !UNLISTED_KEY_TABLES.contains(&block) {
                        missing.push(format!("{}: `{block}`", path.display()));
                    }
                }
            }
        }
        assert!(
            missing.is_empty(),
            "key tables no KEY_TABLES list names: {missing:?}"
        );
    }

    #[test]
    fn test_the_vim_motions_are_listed_for_reading_only() {
        let vim: Vec<&dyn Listed> = winter_model::vim::nav::KEY_TABLES.to_vec();
        assert!(!vim.is_empty() && vim.iter().all(|table| !table.configurable()));
        assert!(key_tables()
            .filter(|table| table.block().starts_with("dir"))
            .all(|table| table.configurable()));
    }
}
