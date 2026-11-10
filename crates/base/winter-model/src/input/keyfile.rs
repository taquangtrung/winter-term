//! Reading a `keybindings.kdl` into [`Blocks`]: the built-in file and the
//! user's go through the same reading, so what one may say the other may too.
//!
//! A file is one list, a `commands` node whose every line names a command and
//! then its chords:
//!
//! ```kdl
//! commands {
//!     new_tab "C-S-t"
//!     new_tab "s-t" platform="macos"
//!     open_remote "M-g r" in="git"
//! }
//! ```
//!
//! A line binds in the window's block, or the line edits' for a line edit's
//! name, unless `in` names an editor's block. One with `platform="macos"` (or
//! `"linux"`, `"windows"`) binds only on that platform, and there after every
//! line that names none, so its chords win where both bind one.

use kdl::{KdlDocument, KdlNode};

use super::actions::EditBinding;
use super::keymap::{Blocks, EDITING_BLOCK, WINDOW_BLOCK};
use super::when::{when_of, WHEN_PROPERTY};

// ========================================================================
// Constants
// ========================================================================

/// The node whose lines name a command and then its chords.
pub const COMMANDS_NODE: &str = "commands";

/// The property naming the editor's block a line binds in.
pub const BLOCK_PROPERTY: &str = "in";

/// The property naming the one platform a line binds on, as
/// [`std::env::consts::OS`] names it.
pub const PLATFORM_PROPERTY: &str = "platform";

// ========================================================================
// Functions
// ========================================================================

/// What `document` binds on the platform this build runs on, or what in it
/// is no binding.
pub fn blocks_from_document(document: &KdlDocument) -> Result<Blocks, String> {
    blocks_for_platform(document, std::env::consts::OS)
}

/// What `document` binds on `platform`: every line naming no platform, then
/// every line naming this one, each over what came before. A node that is no
/// `commands` list, or a line with a property it does not know, is an error,
/// which the file is rejected for rather than read in part.
pub fn blocks_for_platform(document: &KdlDocument, platform: &str) -> Result<Blocks, String> {
    let mut lines = Vec::new();
    for node in document.nodes() {
        if node.name().value() != COMMANDS_NODE {
            return Err(format!(
                "`{}` is no list of keys: every binding is a line of `{COMMANDS_NODE} {{ }}`",
                node.name().value()
            ));
        }
        if let Some(children) = node.children() {
            for line in children.nodes() {
                check_properties(line)?;
                // A line with a condition binds only in its context, which
                // the blocks know nothing of: it is read as a `WhenBinding`.
                if when_of(line).is_none() {
                    lines.push(line);
                }
            }
        }
    }
    let shared = lines.iter().filter(|line| platform_of(line).is_none());
    let scoped = lines
        .iter()
        .filter(|line| platform_of(line) == Some(platform));
    let mut blocks = Blocks::new();
    for line in shared.chain(scoped) {
        let command = line.name().value();
        let block = blocks.entry(block_of(line).to_string()).or_default();
        for chord in line.entries().iter().filter(|entry| entry.name().is_none()) {
            if let Some(chord) = chord.value().as_string() {
                block.insert(chord.to_string(), command.to_string());
            }
        }
    }
    Ok(blocks)
}

/// Fail on a property of `line` that is none of [`BLOCK_PROPERTY`],
/// [`PLATFORM_PROPERTY`], and [`WHEN_PROPERTY`]: a misspelt one would
/// otherwise bind everywhere.
fn check_properties(line: &KdlNode) -> Result<(), String> {
    let unknown = line
        .entries()
        .iter()
        .filter_map(|entry| entry.name())
        .find(|name| ![BLOCK_PROPERTY, PLATFORM_PROPERTY, WHEN_PROPERTY].contains(&name.value()));
    match unknown {
        Some(name) => Err(format!(
            "`{}` has `{}`, which is none of `{BLOCK_PROPERTY}`, `{PLATFORM_PROPERTY}`, and `{WHEN_PROPERTY}`",
            line.name().value(),
            name.value()
        )),
        None => Ok(()),
    }
}

/// The block line `line` binds in: the editor's its `in` names, else the line
/// edits' for a line edit, else the window's.
pub fn block_of(line: &KdlNode) -> &str {
    if let Some(block) = line.get(BLOCK_PROPERTY).and_then(|value| value.as_string()) {
        return block;
    }
    match EditBinding::from_name(line.name().value()) {
        Some(_) => EDITING_BLOCK,
        None => WINDOW_BLOCK,
    }
}

/// The one platform `line` binds on, where it names one.
pub fn platform_of(line: &KdlNode) -> Option<&str> {
    line.get(PLATFORM_PROPERTY)
        .and_then(|value| value.as_string())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"
commands {
    close_pane "C-S-q" platform="macos"
    new_tab "C-S-n"
    new_tab "s-t" platform="macos"
    close_tab "C-S-q" "C-S-w"
    reload "F5" platform="windows"
    open_remote "M-g r" in="git"
    prompt_undo "C-/"
}
"#;

    fn bound(platform: &str, block: &str, chord: &str) -> Option<String> {
        let document: KdlDocument = FILE.parse().unwrap();
        blocks_for_platform(&document, platform)
            .unwrap()
            .get(block)?
            .get(chord)
            .cloned()
    }

    #[test]
    fn test_a_platform_s_lines_bind_only_there_and_over_the_shared_ones() {
        assert_eq!(bound("macos", "window", "s-t").as_deref(), Some("new_tab"));
        assert_eq!(
            bound("macos", "window", "C-S-n").as_deref(),
            Some("new_tab")
        );
        assert_eq!(
            bound("macos", "window", "C-S-q").as_deref(),
            Some("close_pane"),
            "the platform's line wins, though written first"
        );
        assert_eq!(bound("linux", "window", "s-t"), None);
        assert_eq!(
            bound("linux", "window", "C-S-q").as_deref(),
            Some("close_tab")
        );
        assert_eq!(bound("windows", "window", "F5").as_deref(), Some("reload"));
        assert_eq!(bound("linux", "window", "F5"), None);
    }

    #[test]
    fn test_a_line_binds_in_the_block_its_in_names_or_the_line_edits_for_one() {
        assert_eq!(
            bound("linux", "git", "M-g r").as_deref(),
            Some("open_remote")
        );
        assert_eq!(bound("linux", "window", "M-g r"), None);
        assert_eq!(
            bound("linux", "editing", "C-/").as_deref(),
            Some("prompt_undo")
        );
    }

    #[test]
    fn test_a_block_of_chords_or_a_misspelt_property_rejects_the_file() {
        for text in [
            "window {\n    \"C-S-n\" \"new_tab\"\n}",
            "commands {\n    new_tab \"C-S-n\" platfrom=\"macos\"\n}",
        ] {
            let document: KdlDocument = text.parse().unwrap();
            assert!(blocks_for_platform(&document, "linux").is_err(), "{text}");
        }
    }
}
