//! Code lenses: what a language server offers to do with a piece of code,
//! shown dim at the end of its first line (`▶ Run | 3 references`) and
//! offered among the fixes `M-\` lists there. Asked for with the inlay
//! hints, once typing pauses; a lens the server sends without its command
//! is resolved on its own.
//!
//! A lens runs as its server says: through `workspace/executeCommand` where
//! the server runs the command itself. Some leave running it to the editor:
//! rust-analyzer's Run and Debug, which run as a task in a Compile tab, and
//! the references lenses, which list the places they count.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{json, Value};
use winter_lsp::protocol::{self, Position};
use winter_lsp::{Port, ServerId};
use winter_model::editor::{BuildStart, EditorOutcome, Notice};

use super::fixes::{Apply, Fix};
use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// What a server says it can do, for code lenses.
pub(super) const PROVIDER: &str = "codeLensProvider";

/// Where a server says it resolves a lens sent without its command.
const RESOLVES: &str = "/codeLensProvider/resolveProvider";

/// The most lenses resolved at once, so a long file asks no flood.
const MAX_RESOLVED: usize = 200;

/// What stands between the lenses at a line's end, and before them.
const LENS_SEPARATOR: &str = " | ";
const LENS_GAP: &str = "    ";

/// The commands servers leave to their editor that Winter runs: a test or
/// a binary run as a task, and the places a count of references counts.
const RUN_COMMANDS: [&str; 2] = ["rust-analyzer.runSingle", "rust-analyzer.debugSingle"];
const REFERENCE_COMMANDS: [&str; 2] = [
    "rust-analyzer.showReferences",
    "editor.action.showReferences",
];

// ========================================================================
// Data Structures
// ========================================================================

/// One document's code lenses.
#[derive(Debug, Default)]
pub(super) struct Lenses {
    /// The text's line count and revision the lenses were answered for.
    answered: Option<(usize, usize)>,
    /// The lenses, by the line each is on.
    held: BTreeMap<usize, Vec<Lens>>,
}

/// One code lens: the lens as the server sent it, for resolving, and its
/// command once it has one.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Lens {
    command: Option<Value>,
    sent: Value,
    server: ServerId,
}

// ========================================================================
// TextEditor: code lenses
// ========================================================================

impl TextEditor {
    /// Ask `port` for the code lenses of the text at `revision`.
    pub(super) fn ask_lenses(&mut self, port: &Port, revision: usize) {
        let params = json!({"textDocument": {"uri": protocol::uri_of(&self.doc().path)}});
        if let Some(id) = port.request("textDocument/codeLens", params) {
            self.wait_for_lenses(port.id(), id, revision);
        }
    }

    /// Take the code lenses `server` answered about the text at `revision`,
    /// resolving those sent without their command.
    pub(super) fn take_lenses(
        &mut self,
        server: ServerId,
        revision: usize,
        value: &Value,
    ) -> EditorOutcome {
        let lines = self.buffer().lines();
        let mut held: BTreeMap<usize, Vec<Lens>> = BTreeMap::new();
        for sent in value.as_array().into_iter().flatten() {
            let Some(at) = sent.pointer("/range/start").and_then(position_of) else {
                continue;
            };
            if at.line as usize >= lines.len() {
                continue;
            }
            held.entry(at.line as usize).or_default().push(Lens {
                command: sent
                    .get("command")
                    .filter(|command| !command.is_null())
                    .cloned(),
                sent: sent.clone(),
                server,
            });
        }
        let count = lines.len();
        let lenses = &mut self.doc_mut().lenses;
        lenses.held = held;
        lenses.answered = Some((count, revision));
        self.resolve_lenses(server, revision);
        EditorOutcome::Consumed
    }

    /// Ask `server` for the commands of the lenses sent without one.
    fn resolve_lenses(&mut self, server: ServerId, revision: usize) {
        let Some(port) = self
            .serving_port()
            .filter(|port| port.id() == server)
            .filter(|port| port.capability(RESOLVES).and_then(|on| on.as_bool()) == Some(true))
            .cloned()
        else {
            return;
        };
        let unresolved: Vec<(usize, usize, Value)> = self
            .doc()
            .lenses
            .held
            .iter()
            .flat_map(|(row, lenses)| {
                lenses
                    .iter()
                    .enumerate()
                    .filter(|(_, lens)| lens.command.is_none())
                    .map(|(at, lens)| (*row, at, lens.sent.clone()))
            })
            .take(MAX_RESOLVED)
            .collect();
        for (row, at, sent) in unresolved {
            if let Some(id) = port.request("codeLens/resolve", sent) {
                self.wait_for_lens(server, id, (revision, row, at));
            }
        }
    }

    /// Take the command the server resolved for the lens at `at` on `row`,
    /// where the lenses are still the ones of the text at `revision`.
    pub(super) fn take_resolved_lens(
        &mut self,
        (revision, row, at): (usize, usize, usize),
        value: &Value,
    ) -> EditorOutcome {
        let lenses = &mut self.doc_mut().lenses;
        if lenses.answered.map(|(_, answered)| answered) != Some(revision) {
            return EditorOutcome::Consumed;
        }
        let command = value.get("command").filter(|command| !command.is_null());
        if let (Some(lens), Some(command)) = (
            lenses.held.get_mut(&row).and_then(|row| row.get_mut(at)),
            command,
        ) {
            lens.command = Some(command.clone());
        }
        EditorOutcome::Consumed
    }

    /// What the lenses of line `index` say at its end, where they still fit
    /// the text: their titles, after a gap.
    pub(super) fn lens_label(&self, index: usize) -> Option<String> {
        let titles: Vec<&str> = self
            .fitting_lenses(index)
            .iter()
            .filter_map(|lens| title_of(lens.command.as_ref()?))
            .collect();
        (!titles.is_empty()).then(|| format!("{LENS_GAP}{}", titles.join(LENS_SEPARATOR)))
    }

    /// The lenses of line `index`, where they still fit the text: answered
    /// for it as it is, or for as many lines, the line not being typed on.
    fn fitting_lenses(&self, index: usize) -> &[Lens] {
        let lenses = &self.doc().lenses;
        let Some((lines, revision)) = lenses.answered else {
            return &[];
        };
        let fits = revision == self.buffer().revision()
            || (lines == self.buffer().lines().len() && index != self.buffer().row());
        match fits {
            true => lenses.held.get(&index).map_or(&[], Vec::as_slice),
            false => &[],
        }
    }

    /// The lenses on the cursor's line, as fixes `M-\` offers.
    pub(super) fn lens_fixes(&self) -> Vec<Fix> {
        self.fitting_lenses(self.buffer().row())
            .iter()
            .filter_map(|lens| {
                let command = lens.command.clone()?;
                Some(Fix {
                    title: title_of(&command)?.to_string(),
                    apply: Apply::Lens(lens.server, command),
                })
            })
            .collect()
    }

    /// Run the lens command `command`, from `server`.
    pub(super) fn run_lens(&mut self, server: ServerId, command: &Value) -> EditorOutcome {
        let name = command
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let arguments = command.get("arguments").cloned().unwrap_or(json!([]));
        let port = self
            .serving_port()
            .filter(|port| port.id() == server)
            .cloned();
        if let Some(port) = port.filter(|port| port.executes(name)) {
            let params = json!({"command": name, "arguments": arguments});
            if let Some(id) = port.request("workspace/executeCommand", params) {
                self.wait_for_command(server, id);
            }
            return EditorOutcome::Consumed;
        }
        if RUN_COMMANDS.contains(&name) {
            if let Some(build) = cargo_run_of(&arguments) {
                return EditorOutcome::RunTask(build);
            }
        }
        if REFERENCE_COMMANDS.contains(&name) {
            let found = arguments
                .get(2)
                .map(protocol::locations)
                .unwrap_or_default();
            if !found.is_empty() {
                return self.list_lens_places(found);
            }
        }
        let title = title_of(command).unwrap_or(name);
        self.modal.report(Notice::info(format!(
            "{title}: the server leaves running this to its editor, and Winter does not know how"
        )));
        EditorOutcome::Consumed
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What a lens's command is called, its codicons (`$(play)`) left out.
fn title_of(command: &Value) -> Option<&str> {
    let title = command.get("title")?.as_str()?;
    let title = match title.strip_prefix("$(") {
        Some(rest) => rest.split_once(')').map_or(title, |(_, after)| after),
        None => title,
    };
    let title = title.trim();
    (!title.is_empty()).then_some(title)
}

/// A position from the wire.
fn position_of(value: &Value) -> Option<Position> {
    Some(Position {
        character: u32::try_from(value.get("character")?.as_u64()?).ok()?,
        line: u32::try_from(value.get("line")?.as_u64()?).ok()?,
    })
}

/// The task a rust-analyzer Run asks for: `cargo` with its arguments, then
/// the program's after `--`, in the workspace's directory.
fn cargo_run_of(arguments: &Value) -> Option<BuildStart> {
    let runnable = arguments.get(0)?;
    if runnable.get("kind").and_then(Value::as_str) != Some("cargo") {
        return None;
    }
    let args = runnable.get("args")?;
    let words = |key: &str| -> Vec<String> {
        args.get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|word| word.as_str().map(shell_quote))
            .collect()
    };
    let (cargo, program) = (words("cargoArgs"), words("executableArgs"));
    let dir = args
        .get("workspaceRoot")
        .or_else(|| args.get("cwd"))
        .and_then(Value::as_str)
        .map(PathBuf::from)?;
    let mut command = format!("cargo {}", cargo.join(" "));
    if !program.is_empty() {
        command.push_str(&format!(" -- {}", program.join(" ")));
    }
    Some(BuildStart {
        command,
        dir: dir.clone(),
        root: dir,
    })
}

/// `word` as the shell reads it back the same: as it is where it holds only
/// what the shell leaves alone, else in single quotes.
fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@+,".contains(c));
    match plain {
        true => word.to_string(),
        false => format!("'{}'", word.replace('\'', r"'\''")),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_lens_title_leaves_out_its_icon() {
        assert_eq!(title_of(&json!({"title": "$(play) Run"})), Some("Run"));
        assert_eq!(
            title_of(&json!({"title": "3 references"})),
            Some("3 references")
        );
        assert_eq!(title_of(&json!({"title": "$(gear)"})), None);
    }

    #[test]
    fn test_a_rust_analyzer_run_is_a_cargo_task_in_its_workspace() {
        let arguments = json!([{
            "kind": "cargo",
            "args": {
                "workspaceRoot": "/src/app",
                "cargoArgs": ["test", "--package", "app", "--lib"],
                "executableArgs": ["tests::it works", "--exact"],
            },
        }]);
        let build = cargo_run_of(&arguments).expect("a task");
        assert_eq!(
            build.command,
            "cargo test --package app --lib -- 'tests::it works' --exact"
        );
        assert_eq!(build.dir, PathBuf::from("/src/app"));
        assert_eq!(cargo_run_of(&json!([{"kind": "shell"}])), None);
    }

    #[test]
    fn test_a_word_with_a_quote_is_quoted_whole() {
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote("--exact"), "--exact");
    }
}
