//! Setting one command's keys in `keybindings.kdl`, as the Keys view does
//! when a binding is assigned there. Only the command's own lines change:
//! every other line of the file, its comments and layout too, is kept as it
//! was written, since the file is the user's to edit by hand as well.

use std::io::ErrorKind;

use crate::render::kdl_string;
use kdl::{KdlDocument, KdlError, KdlNode};
use winter_base::paths::config_dir;
use winter_base::persist::write_atomic;
use winter_model::input::{
    block_of, platform_of, BLOCK_PROPERTY, COMMANDS_NODE, PLATFORM_PROPERTY,
};

// ========================================================================
// Constants
// ========================================================================

/// The file the user's keybindings live in, in the config directory.
const KEYBINDINGS_FILE: &str = "keybindings.kdl";

/// How far a line within the list is indented, as `keybindings.kdl` is
/// documented and written.
const BLOCK_INDENT: &str = "    ";

// ========================================================================
// Functions
// ========================================================================

/// Bind `command` in `block` to `chords` in the user's `keybindings.kdl`,
/// replacing whatever the file bound it to there, on `platform` alone where
/// one is named; with no `chords`, take the file's binding of it away, so the
/// built-in one is in force again. The config watcher then puts the change in
/// force.
pub(crate) fn save_command_binding(
    block: &str,
    command: &str,
    chords: &[String],
    platform: Option<&str>,
) -> Result<(), String> {
    let path = config_dir().join(KEYBINDINGS_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let written = bind_command(&text, block, command, chords, platform)?;
    write_atomic(&path, written).map_err(|error| format!("{}: {error}", path.display()))
}

/// `text`, a `keybindings.kdl`, with `command` in `block` bound to `chords`
/// alone on `platform` (every platform where `None`): every line binding it
/// there on that platform, or on every platform, is gone, and, unless
/// `chords` is empty, one line at the end of the `commands` list binds it
/// now, the list added where the file has none. A line kept to another
/// platform stays. An error where `text` does not parse, which leaves the
/// file to the user to mend.
fn bind_command(
    text: &str,
    block: &str,
    command: &str,
    chords: &[String],
    platform: Option<&str>,
) -> Result<String, String> {
    let mut document: KdlDocument = text.parse().map_err(|error: KdlError| {
        format!("{KEYBINDINGS_FILE} does not parse, so it was left as it is: {error}")
    })?;
    for node in document.nodes_mut() {
        unbind_in(node, block, command, platform);
    }
    if chords.is_empty() {
        return Ok(document.to_string());
    }
    let mut text = quoted(command, chords);
    if block_of(&parse_node(&text)?) != block {
        text.push_str(&format!(" {BLOCK_PROPERTY}={}", kdl_string(block)));
    }
    if let Some(platform) = platform {
        text.push_str(&format!(" {PLATFORM_PROPERTY}={}", kdl_string(platform)));
    }
    let line = parse_node(&format!("{BLOCK_INDENT}{text}\n"))?;
    let list = document
        .nodes_mut()
        .iter_mut()
        .find(|node| node.name().value() == COMMANDS_NODE);
    match list {
        Some(node) => node.ensure_children().nodes_mut().push(line),
        None => {
            let mut node = parse_node(&format!("\n{COMMANDS_NODE} {{\n}}\n"))?;
            node.ensure_children().nodes_mut().push(line);
            document.nodes_mut().push(node);
        }
    }
    Ok(document.to_string())
}

/// `command` and then `chords`, each quoted, as the file's own lines are
/// written.
fn quoted(command: &str, chords: &[String]) -> String {
    let words: Vec<String> = std::iter::once(command)
        .chain(chords.iter().map(String::as_str))
        .map(kdl_string)
        .collect();
    words.join(" ")
}

/// The one node `text` holds, laid out as `text` lays it out.
fn parse_node(text: &str) -> Result<KdlNode, String> {
    let mut parsed: KdlDocument = text.parse().map_err(|error: KdlError| error.to_string())?;
    parsed
        .nodes_mut()
        .pop()
        .ok_or_else(|| format!("{text:?} holds no line"))
}

/// Take away every line of the `commands` list `node` that binds `command`
/// in `block` on `platform` alone, or on every platform where `None`.
fn unbind_in(node: &mut KdlNode, block: &str, command: &str, platform: Option<&str>) {
    if node.name().value() != COMMANDS_NODE {
        return;
    }
    let Some(children) = node.children_mut().as_mut() else {
        return;
    };
    children.nodes_mut().retain(|line| {
        line.name().value() != command || block_of(line) != block || platform_of(line) != platform
    });
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{parse_keys, WINDOW_BLOCK};

    const FILE: &str = r#"// My keys.
commands {
    split_horizontal "M-S--" // the one I use most
    "example.word_count" "C-S-t" "F10"
    "example.flag_todos" "C-S-c"
    new_tab "s-t" platform="macos"
}
"#;

    fn chords(chords: &[&str]) -> Vec<String> {
        chords.iter().map(|chord| chord.to_string()).collect()
    }

    fn bound(text: &str, block: &str, chord: &str) -> Option<String> {
        parse_keys(text).unwrap().get(block)?.get(chord).cloned()
    }

    #[test]
    fn test_a_platform_s_binding_replaces_only_that_platform_s_line() {
        let written = bind_command(
            FILE,
            WINDOW_BLOCK,
            "new_tab",
            &chords(&["s-n"]),
            Some("macos"),
        )
        .unwrap();
        assert!(written.contains("\"s-n\" platform=\"macos\""), "{written}");
        assert!(
            !written.contains("\"s-t\""),
            "the old macOS line is gone: {written}"
        );
        let everywhere =
            bind_command(&written, WINDOW_BLOCK, "new_tab", &chords(&["C-S-n"]), None).unwrap();
        assert!(
            everywhere.contains("platform=\"macos\""),
            "kept by an every-platform binding"
        );
    }

    #[test]
    fn test_a_new_binding_joins_the_list_and_every_other_line_is_kept() {
        let written =
            bind_command(FILE, WINDOW_BLOCK, "new_tab", &chords(&["C-S-n"]), None).unwrap();
        assert_eq!(
            bound(&written, WINDOW_BLOCK, "C-S-n").as_deref(),
            Some("new_tab")
        );
        for kept in FILE.lines().filter(|line| *line != "}") {
            assert!(written.contains(kept), "{kept:?} kept in:\n{written}");
        }
        assert!(
            written.contains("    \"new_tab\" \"C-S-n\"\n"),
            "quoted and indented as the list's lines are:\n{written}"
        );
    }

    #[test]
    fn test_rebinding_a_command_replaces_every_line_that_bound_it() {
        let written = bind_command(
            FILE,
            WINDOW_BLOCK,
            "example.word_count",
            &chords(&["C-S-g"]),
            None,
        )
        .unwrap();
        assert_eq!(
            bound(&written, WINDOW_BLOCK, "C-S-g").as_deref(),
            Some("example.word_count")
        );
        assert_eq!(
            bound(&written, WINDOW_BLOCK, "C-S-t"),
            None,
            "its old chords are gone"
        );
        assert_eq!(bound(&written, WINDOW_BLOCK, "F10"), None);
    }

    #[test]
    fn test_no_chords_takes_the_files_binding_away() {
        let written = bind_command(FILE, WINDOW_BLOCK, "example.flag_todos", &[], None).unwrap();
        assert_eq!(bound(&written, WINDOW_BLOCK, "C-S-c"), None);
        assert_eq!(
            bound(&written, WINDOW_BLOCK, "C-S-t").as_deref(),
            Some("example.word_count"),
            "others stay"
        );
    }

    #[test]
    fn test_a_platform_s_line_survives_a_rebinding() {
        let written =
            bind_command(FILE, WINDOW_BLOCK, "new_tab", &chords(&["C-S-n"]), None).unwrap();
        assert!(
            written.contains("new_tab \"s-t\" platform=\"macos\""),
            "{written}"
        );
    }

    #[test]
    fn test_an_editors_command_is_bound_with_its_block_named() {
        let written = bind_command(FILE, "git", "open_remote", &chords(&["M-g o"]), None).unwrap();
        assert_eq!(
            bound(&written, "git", "M-g o").as_deref(),
            Some("open_remote")
        );
        assert_eq!(
            bound(&written, WINDOW_BLOCK, "M-g o"),
            None,
            "not the window's"
        );
        assert!(written.contains("in=\"git\""), "{written}");
        // Rebinding it takes that line away.
        let rebound =
            bind_command(&written, "git", "open_remote", &chords(&["M-o"]), None).unwrap();
        assert_eq!(bound(&rebound, "git", "M-g o"), None);
    }

    #[test]
    fn test_an_empty_file_gets_a_commands_list() {
        let written = bind_command("", WINDOW_BLOCK, "new_tab", &chords(&["C-S-n"]), None).unwrap();
        assert_eq!(
            bound(&written, WINDOW_BLOCK, "C-S-n").as_deref(),
            Some("new_tab")
        );
    }

    #[test]
    fn test_a_file_that_does_not_parse_is_left_alone() {
        let error = bind_command(
            "commands {",
            WINDOW_BLOCK,
            "new_tab",
            &chords(&["C-S-n"]),
            None,
        )
        .unwrap_err();
        assert!(error.contains("left as it is"), "{error}");
    }
}
