//! Acting on what is at the cursor, as Emacs' Embark does: `M-a` lists
//! everything that can be done with the thing the cursor is on, whatever
//! kind of thing it is, and choosing one does it.
//!
//! - A web address: open it in the browser, or copy it.
//! - A file's path, with a line where it says one (`src/main.rs:12`): open
//!   it here or beside, open it with the system's own app, or copy it.
//! - A commit's hash, in a git repository: show the commit, or copy it.
//! - A name: go to where it is defined, peek at it, find where it is used,
//!   rename it, or describe it (where a language server serves the file);
//!   change every occurrence of it at once; copy it.

use std::path::{Path, PathBuf};

use winter_model::editor::{
    EditorOutcome, GitViewRequest, Notice, OpenTarget, PickPlacement, PickRequest,
    EDITOR_CHANGE_OCCURRENCES, LSP_DEFINITION, LSP_HOVER, LSP_PEEK_DEFINITION, LSP_PEEK_REFERENCES,
    LSP_REFERENCES, LSP_RENAME,
};
use winter_model::paint::IconKind;

use super::complete::is_word_char;
use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// The question the list of actions answers.
pub(super) const ASK_POINT: &str = "point";

/// What the list is headed with, and each action wears.
const POINT_LABEL: &str = "Act On";
const POINT_ICON: IconKind = IconKind::Symbol { name: "zap" };

/// What a thing ends at, besides a space: what wraps a path or an address
/// in prose and code.
const WRAPPERS: [char; 10] = ['"', '\'', '`', '(', ')', '<', '>', '[', ']', ','];

/// What a web address or a path written at a sentence's end leaves out.
const TRAILING: [char; 4] = ['.', ';', ':', '!'];

/// How long a commit's hash written short is at the least, and at the most.
const HASH_LENGTHS: std::ops::RangeInclusive<usize> = 7..=40;

/// What is said where there is nothing at the cursor to act on.
const NOTHING_HERE: &str = "nothing here to act on";

// ========================================================================
// Data Structures
// ========================================================================

/// One thing to do with what is at the cursor: what the list calls it, and
/// what doing it is.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PointAction {
    act: PointAct,
    title: String,
}

/// What an action at the cursor does.
#[derive(Clone, Debug, PartialEq)]
enum PointAct {
    /// Run one of the editor's own commands.
    Command(&'static str),
    Copy(String),
    /// Open a file, or a web address, with the system's own app.
    External(PathBuf),
    Open(OpenTarget),
    OpenBeside(OpenTarget),
    /// Show a commit, in the repository at the path.
    ShowCommit(PathBuf, String),
}

// ========================================================================
// TextEditor: acting at the cursor
// ========================================================================

impl TextEditor {
    /// Offer everything that can be done with what is at the cursor.
    pub(super) fn ask_point(&mut self) -> EditorOutcome {
        let actions = self.point_actions();
        if actions.is_empty() {
            self.modal.report(Notice::info(NOTHING_HERE).unlogged());
            return EditorOutcome::Consumed;
        }
        let items: Vec<String> = actions.iter().map(|action| action.title.clone()).collect();
        self.offered_points = actions;
        EditorOutcome::Pick(PickRequest {
            icons: vec![POINT_ICON; items.len()],
            items,
            label: POINT_LABEL.to_string(),
            placement: PickPlacement::Cursor,
            tag: ASK_POINT,
        })
    }

    /// Do the action the list came back with, named by its title.
    pub(super) fn point_chosen(&mut self, chosen: &str) -> EditorOutcome {
        let offered = std::mem::take(&mut self.offered_points);
        let Some(action) = offered.into_iter().find(|action| action.title == chosen) else {
            return EditorOutcome::Consumed;
        };
        match action.act {
            PointAct::Command(name) => {
                <Self as winter_model::editor::Page>::run_command(self, name)
                    .unwrap_or(EditorOutcome::Consumed)
            }
            PointAct::Copy(text) => EditorOutcome::Yank(text),
            PointAct::External(path) => EditorOutcome::OpenExternal(path),
            PointAct::Open(target) => EditorOutcome::OpenPath(target),
            PointAct::OpenBeside(target) => EditorOutcome::OpenBeside(target),
            PointAct::ShowCommit(root, rev) => match winter_vcs::exec::Rev::parse(&rev) {
                Ok(rev) => EditorOutcome::GitView(GitViewRequest {
                    git_dir: None,
                    request: winter_vcs::exec::show_commit(&root, &rev),
                    root,
                }),
                Err(why) => {
                    self.modal.report(Notice::error(why));
                    EditorOutcome::Consumed
                }
            },
        }
    }

    /// Everything that can be done with what is at the cursor, the most
    /// particular kind of thing first.
    fn point_actions(&self) -> Vec<PointAction> {
        let line: Vec<char> = self.buffer().lines()[self.buffer().row()].chars().collect();
        let col = self.buffer().col();
        let mut actions = Vec::new();
        if let Some(token) = token_at(&line, col) {
            if token.starts_with("https://") || token.starts_with("http://") {
                actions.push(action(
                    format!("Open {token} in the Browser"),
                    PointAct::External(token.clone().into()),
                ));
                actions.push(action(
                    format!("Copy {token}"),
                    PointAct::Copy(token.clone()),
                ));
            } else if let Some((path, line)) = self.path_at(&token) {
                let shown = split_line(&token).0.to_string();
                let target = OpenTarget {
                    column: None,
                    line,
                    path: path.clone(),
                };
                actions.push(action(
                    format!("Open {shown}"),
                    PointAct::Open(target.clone()),
                ));
                actions.push(action(
                    format!("Open {shown} Beside"),
                    PointAct::OpenBeside(target),
                ));
                actions.push(action(
                    format!("Open {shown} with Its App"),
                    PointAct::External(path.clone()),
                ));
                actions.push(action(format!("Copy {shown}"), PointAct::Copy(shown)));
            } else if let Some(root) = self.commit_root(&token) {
                actions.push(action(
                    format!("Show Commit {token}"),
                    PointAct::ShowCommit(root, token.clone()),
                ));
                actions.push(action(
                    format!("Copy {token}"),
                    PointAct::Copy(token.clone()),
                ));
            }
        }
        if let Some(word) = word_at(&line, col) {
            if self.is_served() {
                let served = [
                    ("Go to the Definition", LSP_DEFINITION),
                    ("Peek at the Definition", LSP_PEEK_DEFINITION),
                    ("Find Where It Is Used", LSP_REFERENCES),
                    ("Peek at Where It Is Used", LSP_PEEK_REFERENCES),
                    ("Rename It", LSP_RENAME),
                    ("Describe It", LSP_HOVER),
                ];
                actions.extend(served.into_iter().map(|(what, command)| {
                    action(format!("{word}: {what}"), PointAct::Command(command))
                }));
            }
            actions.push(action(
                format!("{word}: Change Every Occurrence"),
                PointAct::Command(EDITOR_CHANGE_OCCURRENCES),
            ));
            let copy = format!("Copy {word}");
            if !actions.iter().any(|offered| offered.title == copy) {
                actions.push(action(copy, PointAct::Copy(word)));
            }
        }
        actions
    }

    /// The file `token` names, with the line it says where it says one:
    /// beside this file, under its project, or where it says outright.
    fn path_at(&self, token: &str) -> Option<(PathBuf, Option<usize>)> {
        let (path, line) = split_line(token);
        let named = Path::new(path);
        let here = self.doc().path.parent().map(Path::to_path_buf);
        let project = winter_base::project::marked_root(&self.doc().path);
        let candidates = match named.is_absolute() {
            true => vec![named.to_path_buf()],
            false => [here, project]
                .into_iter()
                .flatten()
                .map(|dir| dir.join(named))
                .collect(),
        };
        let found = candidates
            .into_iter()
            .find(|candidate| candidate.is_file())?;
        Some((found, line))
    }

    /// The repository a commit's hash `token` would be in, where it looks
    /// like one and this file is in a repository.
    fn commit_root(&self, token: &str) -> Option<PathBuf> {
        let hex = token.chars().all(|c| c.is_ascii_hexdigit());
        let mixed = token.chars().any(|c| c.is_ascii_digit())
            && token.chars().any(|c| c.is_ascii_alphabetic());
        if !(hex && mixed && HASH_LENGTHS.contains(&token.len())) {
            return None;
        }
        let root = winter_base::project::marked_root(&self.doc().path)?;
        root.join(".git").exists().then_some(root)
    }
}

// ========================================================================
// Functions
// ========================================================================

fn action(title: String, act: PointAct) -> PointAction {
    PointAction { act, title }
}

/// The run of text `col` is in, up to a space or what wraps it, without
/// the punctuation a sentence ends it with.
fn token_at(line: &[char], col: usize) -> Option<String> {
    let ends = |c: &char| c.is_whitespace() || WRAPPERS.contains(c);
    if line.get(col).is_none_or(ends) {
        return None;
    }
    let start = line[..col].iter().rposition(ends).map_or(0, |at| at + 1);
    let end = line[col..]
        .iter()
        .position(ends)
        .map_or(line.len(), |at| col + at);
    let token: String = line[start..end].iter().collect();
    let token = token.trim_end_matches(TRAILING);
    (!token.is_empty()).then(|| token.to_string())
}

/// The name `col` is in.
fn word_at(line: &[char], col: usize) -> Option<String> {
    if !line.get(col).is_some_and(|c| is_word_char(*c)) {
        return None;
    }
    let start = line[..col]
        .iter()
        .rposition(|c| !is_word_char(*c))
        .map_or(0, |at| at + 1);
    let end = line[col..]
        .iter()
        .position(|c| !is_word_char(*c))
        .map_or(line.len(), |at| col + at);
    Some(line[start..end].iter().collect())
}

/// A path and the line it names after a colon, where it names one:
/// `src/main.rs:12:4` is `src/main.rs` at line 12.
fn split_line(token: &str) -> (&str, Option<usize>) {
    let mut parts = token.splitn(3, ':');
    let path = parts.next().unwrap_or(token);
    let line = parts.next().and_then(|line| line.parse().ok());
    match line {
        Some(line) => (path, Some(line)),
        None => (token, None),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    #[test]
    fn test_a_thing_is_read_up_to_what_wraps_it_and_a_sentence_s_end() {
        let line = chars("see (https://example.org/a?b=1). Then");
        assert_eq!(
            token_at(&line, 10).as_deref(),
            Some("https://example.org/a?b=1")
        );
        let error = chars("  --> src/main.rs:12:4");
        assert_eq!(token_at(&error, 8).as_deref(), Some("src/main.rs:12:4"));
        assert_eq!(token_at(&line, 3), None, "on a space");
    }

    #[test]
    fn test_a_path_s_line_is_read_after_its_colon() {
        assert_eq!(split_line("src/main.rs:12:4"), ("src/main.rs", Some(12)));
        assert_eq!(split_line("notes.md"), ("notes.md", None));
        assert_eq!(split_line("C:x"), ("C:x", None));
    }
}
