//! Next-error: the `path:line` references a command printed (a compiler's
//! errors, a test runner's failures, a linter's findings), opened one after
//! another in the editor, the way Emacs' `next-error` walks a compilation.
//!
//! Winter already records each command's output and where it ran, so nothing
//! has to be re-run under a special runner: the walk reads the last command
//! that printed any reference to a file that exists.

use std::path::PathBuf;

use crate::model::layout::TabId;
use crate::model::page::OpenTarget;

use super::navigation::resolve_existing_path;
use super::App;

// ========================================================================
// Constants
// ========================================================================

/// A file reference as tools print one: a path, a line, and maybe a column,
/// with no blanks, quotes, or brackets in the path.
const REFERENCE: &str = r#"([^\s:'"()\[\]<>]+):(\d+)(?::\d+)?"#;

// ========================================================================
// Data Structures
// ========================================================================

/// Where a walk through one command's references has got to.
#[derive(Debug)]
pub(crate) struct ErrorWalk {
    /// Which reference was opened last.
    at: usize,
    /// The command block the references came from.
    block: usize,
    /// The terminal that printed them, which the walk keeps reading from while
    /// the editor it opens has the focus.
    tab: TabId,
    targets: Vec<OpenTarget>,
}

// ========================================================================
// App: next-error
// ========================================================================

impl App {
    /// Open the next reference the last command printed (`forward`), or the
    /// one before, starting a fresh walk whenever there is newer output.
    pub(crate) fn step_error(&mut self, focused: TabId, forward: bool) {
        let tab = match self.terminals.contains_key(&focused) {
            true => focused,
            false => match &self.error_walk {
                Some(walk) => walk.tab,
                None => {
                    self.set_error("no terminal output to read errors from");
                    return;
                }
            },
        };
        let Some((block, targets)) = self.latest_references(tab) else {
            self.set_error("the last commands printed no file:line references");
            return;
        };
        let at = match self.error_walk.take() {
            Some(walk) if walk.tab == tab && walk.block == block && walk.targets == targets => {
                let count = targets.len();
                match forward {
                    true => (walk.at + 1) % count,
                    false => (walk.at + count - 1) % count,
                }
            }
            _ if forward => 0,
            _ => targets.len() - 1,
        };
        let target = targets[at].clone();
        let note = format!(
            "{} of {}: {}{}",
            at + 1,
            targets.len(),
            target.path.display(),
            target
                .line
                .map(|line| format!(":{line}"))
                .unwrap_or_default()
        );
        self.error_walk = Some(ErrorWalk {
            at,
            block,
            tab,
            targets,
        });
        self.open_path_page(target);
        self.set_notice(note);
    }

    /// The newest command block in `tab` that printed a reference to a file
    /// that exists, with every such reference in it.
    fn latest_references(&self, tab: TabId) -> Option<(usize, Vec<OpenTarget>)> {
        let terminal = self.terminals.get(&tab)?;
        let blocks = terminal.scrollback().blocks();
        blocks.iter().enumerate().rev().find_map(|(index, block)| {
            let targets = references(&block.plain_text(), block.cwd.as_deref());
            (!targets.is_empty()).then_some((index, targets))
        })
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Every `path:line[:col]` in `text` naming a file that exists, resolved
/// against `cwd`, each once, in the order printed.
pub(crate) fn references(text: &str, cwd: Option<&str>) -> Vec<OpenTarget> {
    let Ok(pattern) = regex::Regex::new(REFERENCE) else {
        return Vec::new();
    };
    let mut found: Vec<OpenTarget> = Vec::new();
    for caps in pattern.captures_iter(text) {
        let (Some(path), Some(line)) = (caps.get(1), caps.get(2)) else {
            continue;
        };
        let Some(path): Option<PathBuf> = resolve_existing_path(path.as_str(), cwd) else {
            continue;
        };
        if path.is_dir() {
            continue;
        }
        let target = OpenTarget {
            line: line.as_str().parse().ok(),
            path,
        };
        if !found.contains(&target) {
            found.push(target);
        }
    }
    found
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_references_are_the_existing_files_a_tool_printed_each_once() {
        let dir = std::env::temp_dir().join(format!("winter-errors-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).expect("temp dir");
        std::fs::write(dir.join("src/lib.rs"), "x").expect("write");
        let output = "\
error[E0425]: cannot find value `x`
  --> src/lib.rs:12:5
warning: unused at src/lib.rs:3
note: see src/lib.rs:12:5 again, and gone.rs:4 which does not exist
";
        let found = references(output, dir.to_str());
        let lines: Vec<Option<usize>> = found.iter().map(|target| target.line).collect();
        assert_eq!(
            lines,
            [Some(12), Some(3)],
            "in order, each once, only what exists"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
