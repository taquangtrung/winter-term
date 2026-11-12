//! What a file is written in: the grammar it is colored with, by its name
//! or its first line, and the language settings name it by (the key of a
//! completion back-end's list, of a language server's settings).

use std::collections::BTreeMap;
use std::path::Path;

use winter_syntax::Grammar;

// ========================================================================
// Constants
// ========================================================================

/// Each file extension and the language settings name it by.
const LANGUAGES: &[(&str, &str)] = &[
    ("bash", "shell"),
    ("c", "c"),
    ("cc", "cpp"),
    ("cjs", "javascript"),
    ("cls", "latex"),
    ("cpp", "cpp"),
    ("cxx", "cpp"),
    ("go", "go"),
    ("h", "c"),
    ("hpp", "cpp"),
    ("js", "javascript"),
    ("json", "json"),
    ("jsx", "javascript"),
    ("kdl", "kdl"),
    ("ltx", "latex"),
    ("markdown", "markdown"),
    ("md", "markdown"),
    ("mjs", "javascript"),
    ("py", "python"),
    ("rs", "rust"),
    ("sh", "shell"),
    ("sty", "latex"),
    ("tex", "latex"),
    ("toml", "toml"),
    ("ts", "typescript"),
    ("tsx", "typescript"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
    ("zsh", "shell"),
    ("ml", "ocaml"),
    ("mli", "ocaml"),
    ("lua", "lua"),
    ("hs", "haskell"),
    ("zig", "zig"),
    ("nix", "nix"),
    ("wit", "wit"),
];

/// The file git opens an editor on for a commit's message.
const COMMIT_MESSAGE: &str = "COMMIT_EDITMSG";

/// Files git opens an editor on for a message, read with the commit
/// message grammar.
const MESSAGE_FILES: [&str; 3] = [COMMIT_MESSAGE, "MERGE_MSG", "TAG_EDITMSG"];

/// The file git opens an editor on for an interactive rebase's plan.
const REBASE_TODO: &str = "git-rebase-todo";

/// The grammars git's files are read with.
const COMMIT_GRAMMAR: &str = "Git Commit";
const REBASE_GRAMMAR: &str = "Git Rebase Todo";

// ========================================================================
// Functions
// ========================================================================

/// Whether the file at `path` is the message of a commit being made, as
/// opposed to a merge's or a tag's.
pub(super) fn is_commit_message(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some(COMMIT_MESSAGE)
}

/// The grammar the file at `path` is read with, by its name: git's
/// message and rebase files by theirs, the rest by their extension.
pub(super) fn grammar_of(path: &Path) -> Option<Grammar> {
    let name = path.file_name().and_then(|name| name.to_str());
    if name.is_some_and(|name| MESSAGE_FILES.contains(&name)) {
        return winter_syntax::named(COMMIT_GRAMMAR);
    }
    if name == Some(REBASE_TODO) {
        return winter_syntax::named(REBASE_GRAMMAR);
    }
    winter_syntax::of_path(path)
}

/// Every language the editor knows by its files, as settings name them, in
/// alphabetical order.
pub fn known_languages() -> Vec<&'static str> {
    let mut languages: Vec<&'static str> =
        LANGUAGES.iter().map(|(_, language)| *language).collect();
    languages.sort_unstable();
    languages.dedup();
    languages
}

/// The language a file at `path`, read with `grammar`, is written in, as
/// settings name it: by its extension, the ones settings add (`added`)
/// first, else by its grammar's name.
pub(super) fn language_of(
    path: &Path,
    grammar: Option<Grammar>,
    added: &BTreeMap<String, String>,
) -> String {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    extension
        .and_then(|extension| {
            if let Some(language) = added.get(&extension) {
                return Some(language.clone());
            }
            LANGUAGES
                .iter()
                .find(|(known, _)| *known == extension)
                .map(|(_, language)| language.to_string())
        })
        .or_else(|| grammar.map(|grammar| grammar.name().to_ascii_lowercase()))
        .unwrap_or_else(|| "text".to_string())
}
