//! Setting one command's keys in `keybindings.kdl`, as the Keys view does
//! when a binding is assigned there. Only the command's own lines change:
//! every other line of the file, its comments and layout too, is kept as it
//! was written, since the file is the user's to edit by hand as well.

use std::io::ErrorKind;

use kdl::{KdlDocument, KdlError, KdlNode};
use winter_base::paths::config_dir;
use winter_base::persist::write_atomic;
use winter_render::kdl_string;

use super::schema::{commands_target, COMMANDS_NODE, WINDOW_BLOCK};

// ========================================================================
// Constants
// ========================================================================

/// The file the user's keybindings live in, in the config directory.
const KEYBINDINGS_FILE: &str = "keybindings.kdl";

/// The property a chord line of a block may name its command by, rather
/// than by its first argument.
const ACTION_PROPERTY: &str = "action";

/// How far a line within a block is indented, as `keybindings.kdl` is
/// documented and written.
const BLOCK_INDENT: &str = "    ";

// ========================================================================
// Functions
// ========================================================================

/// Bind `command` in `block` to `chords` in the user's `keybindings.kdl`,
/// replacing whatever the file bound it to there; with no `chords`, take the
/// file's binding of it away, so the built-in one is in force again. The
/// config watcher then puts the change in force.
pub(crate) fn save_command_binding(
    block: &str,
    command: &str,
    chords: &[String],
) -> Result<(), String> {
    let path = config_dir().join(KEYBINDINGS_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let written = bind_command(&text, block, command, chords)?;
    write_atomic(&path, written).map_err(|error| format!("{}: {error}", path.display()))
}

/// `text`, a `keybindings.kdl`, with `command` in `block` bound to `chords`
/// alone: every line binding it there before is gone, and, unless `chords`
/// is empty, one line in the block's `commands` node binds it now, the node
/// added at the end where the file has none. An error where `text` does not
/// parse, which leaves the file to the user to mend.
fn bind_command(
    text: &str,
    block: &str,
    command: &str,
    chords: &[String],
) -> Result<String, String> {
    let mut document: KdlDocument = text.parse().map_err(|error: KdlError| {
        format!("{KEYBINDINGS_FILE} does not parse, so it was left as it is: {error}")
    })?;
    for node in document.nodes_mut() {
        unbind_in(node, block, command);
    }
    if chords.is_empty() {
        return Ok(document.to_string());
    }
    let line = quoted(command, chords);
    let is_block =
        |node: &KdlNode| node.name().value() == COMMANDS_NODE && commands_target(node) == block;
    match document.nodes_mut().iter_mut().find(|node| is_block(node)) {
        Some(node) => {
            let line = parse_node(&format!("{BLOCK_INDENT}{line}\n"))?;
            node.ensure_children().nodes_mut().push(line);
        }
        None => {
            let target = match block == WINDOW_BLOCK {
                true => String::new(),
                false => format!(" {}", kdl_string(block)),
            };
            let node = parse_node(&format!(
                "\n{COMMANDS_NODE}{target} {{\n{BLOCK_INDENT}{line}\n}}\n"
            ))?;
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

/// Take away every line of `node` that binds `command` in `block`: a line
/// of a `commands` node for the block naming it, or a chord line of the
/// block's own node running it.
fn unbind_in(node: &mut KdlNode, block: &str, command: &str) {
    let by_name = node.name().value() == COMMANDS_NODE;
    let binds_here = match by_name {
        true => commands_target(node) == block,
        false => node.name().value() == block,
    };
    if !binds_here {
        return;
    }
    let Some(children) = node.children_mut().as_mut() else {
        return;
    };
    children.nodes_mut().retain(|line| match by_name {
        true => line.name().value() != command,
        false => !runs(line, command),
    });
}

/// Whether chord line `line` runs `command`: its first argument, or its
/// `action` property, names it.
fn runs(line: &KdlNode, command: &str) -> bool {
    line.entries()
        .iter()
        .find(|entry| {
            entry
                .name()
                .is_none_or(|name| name.value() == ACTION_PROPERTY)
        })
        .and_then(|entry| entry.value().as_string())
        == Some(command)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::parse_keys;

    const FILE: &str = r#"// My keys.
window {
    "M-S--" "split_horizontal" // the one I use most
}

commands {
    "magic.gtd" "C-S-t" "F10"
    "magic.calendar" "C-S-c"
}
"#;

    fn chords(chords: &[&str]) -> Vec<String> {
        chords.iter().map(|chord| chord.to_string()).collect()
    }

    fn bound(text: &str, block: &str, chord: &str) -> Option<String> {
        parse_keys(text).unwrap().get(block)?.get(chord).cloned()
    }

    #[test]
    fn test_a_new_binding_joins_the_commands_block_and_every_other_line_is_kept() {
        let written = bind_command(FILE, WINDOW_BLOCK, "new_tab", &chords(&["C-S-n"])).unwrap();
        assert_eq!(
            bound(&written, WINDOW_BLOCK, "C-S-n").as_deref(),
            Some("new_tab")
        );
        for kept in FILE.lines().filter(|line| *line != "}") {
            assert!(written.contains(kept), "{kept:?} kept in:\n{written}");
        }
        assert!(
            written.contains("    \"new_tab\" \"C-S-n\"\n"),
            "quoted and indented as the block's lines are:\n{written}"
        );
    }

    #[test]
    fn test_rebinding_a_command_replaces_every_line_that_bound_it() {
        let written = bind_command(FILE, WINDOW_BLOCK, "magic.gtd", &chords(&["C-S-g"])).unwrap();
        assert_eq!(
            bound(&written, WINDOW_BLOCK, "C-S-g").as_deref(),
            Some("magic.gtd")
        );
        assert_eq!(
            bound(&written, WINDOW_BLOCK, "C-S-t"),
            None,
            "its old chords are gone"
        );
        assert_eq!(bound(&written, WINDOW_BLOCK, "F10"), None);
        // A chord line of the window block running it goes too.
        let written =
            bind_command(FILE, WINDOW_BLOCK, "split_horizontal", &chords(&["M-|"])).unwrap();
        assert_eq!(bound(&written, WINDOW_BLOCK, "M-S--"), None);
        assert_eq!(
            bound(&written, WINDOW_BLOCK, "M-|").as_deref(),
            Some("split_horizontal")
        );
    }

    #[test]
    fn test_no_chords_takes_the_files_binding_away() {
        let written = bind_command(FILE, WINDOW_BLOCK, "magic.calendar", &[]).unwrap();
        assert_eq!(bound(&written, WINDOW_BLOCK, "C-S-c"), None);
        assert_eq!(
            bound(&written, WINDOW_BLOCK, "C-S-t").as_deref(),
            Some("magic.gtd"),
            "others stay"
        );
    }

    #[test]
    fn test_an_editors_command_is_bound_in_a_commands_node_of_its_own_block() {
        let written = bind_command(FILE, "git", "open_remote", &chords(&["M-g o"])).unwrap();
        assert_eq!(
            bound(&written, "git", "M-g o").as_deref(),
            Some("open_remote")
        );
        assert_eq!(
            bound(&written, WINDOW_BLOCK, "M-g o"),
            None,
            "not the window's"
        );
        assert!(
            written.starts_with(FILE),
            "added after what was there:\n{written}"
        );
    }

    #[test]
    fn test_an_empty_file_gets_a_commands_block() {
        let written = bind_command("", WINDOW_BLOCK, "new_tab", &chords(&["C-S-n"])).unwrap();
        assert_eq!(
            bound(&written, WINDOW_BLOCK, "C-S-n").as_deref(),
            Some("new_tab")
        );
    }

    #[test]
    fn test_a_file_that_does_not_parse_is_left_alone() {
        let error =
            bind_command("window {", WINDOW_BLOCK, "new_tab", &chords(&["C-S-n"])).unwrap_err();
        assert!(error.contains("left as it is"), "{error}");
    }
}
