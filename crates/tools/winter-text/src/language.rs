//! What a file is written in: the grammar it is colored with, by its name
//! or its first line, and the language settings name it by (the key of a
//! completion back-end's list, of a language server's settings).

use std::collections::BTreeMap;
use std::path::Path;

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
pub(super) const MESSAGE_FILES: [&str; 3] = [COMMIT_MESSAGE, "MERGE_MSG", "TAG_EDITMSG"];

/// The file git opens an editor on for an interactive rebase's plan.
pub(super) const REBASE_TODO: &str = "git-rebase-todo";

// ========================================================================
// Functions
// ========================================================================

/// Whether the file at `path` is the message of a commit being made, as
/// opposed to a merge's or a tag's.
pub(super) fn is_commit_message(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some(COMMIT_MESSAGE)
}

/// The id of the language the file at `path` is in, by its name (git's
/// message and rebase files by theirs) or its extension; `None` for one
/// iris knows nothing of.
pub(super) fn language_id(path: &Path) -> Option<String> {
    crate::syntax::grammars()
        .language_id(path, "")
        .map(str::to_string)
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
    detected: Option<String>,
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
        .or(detected)
        .unwrap_or_else(|| "text".to_string())
}
