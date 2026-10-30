//! Stepping through the problems [`winter_lsp::diagnostics`] holds, whichever
//! back-end found them, as magic-vscode steps through them: in the file or
//! the whole project, every problem or only the errors or the writing
//! issues, on from the cursor and round past the end.
//!
//! The navigator knows no back-end by name: those the editor runs itself
//! are brought up to date through [`BACKENDS`] before the problems are
//! gathered, and the language servers publish on their own.

use std::path::Path;

use winter_lsp::diagnostics::{self, Backend, Found};
use winter_lsp::protocol::{Position, Severity};
use winter_lsp::servers;
use crate::model::editor::{self, EditorOutcome, Notice};

use super::lsp::{describe, severity_word, Place, NO_SERVER};
use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// The back-ends the editor runs itself, each brought up to date before the
/// problems are gathered, given the project's root when the whole project
/// is looked at. The language servers publish on their own and are not
/// here. A new back-end the editor runs is a line here.
const BACKENDS: &[(Backend, Refresh)] = &[(Backend::Spelling, TextEditor::refresh_spelling)];

// ========================================================================
// Data Structures
// ========================================================================

/// How a back-end the editor runs is brought up to date: given the
/// project's root when the whole project is looked at, `None` for the file.
type Refresh = fn(&mut TextEditor, Option<&Path>);

/// Which problems a command steps through.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Filter {
    /// Every one.
    All,
    /// The errors alone.
    Errors,
    /// The writing issues alone.
    Writing,
}

/// Where a command looks for problems.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scope {
    /// The file in front.
    File,
    /// Every file of its project.
    Project,
}

/// Which problem a command goes to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    /// The first, an error first where there is one.
    First,
    /// The next after the cursor.
    Next,
    /// The one before it.
    Prev,
}

/// What a command steps through, where, and which way.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Walk {
    filter: Filter,
    scope: Scope,
    step: Step,
}

// ========================================================================
// Filter
// ========================================================================

impl Filter {
    /// Whether `found` is one of the problems stepped through.
    fn admits(self, found: &Found) -> bool {
        match self {
            Self::All => true,
            Self::Errors => found.diagnostic.severity == Severity::Error,
            Self::Writing => found.backend.is_writing(),
        }
    }

    /// Whether `backend` can find any of the problems stepped through, so
    /// it is brought up to date before they are gathered: a writing issue
    /// is never an error.
    fn wants(self, backend: Backend) -> bool {
        match self {
            Self::All => true,
            Self::Errors => !backend.is_writing(),
            Self::Writing => backend.is_writing(),
        }
    }

    /// What the problems stepped through are called, when there are none.
    fn plural(self) -> &'static str {
        match self {
            Self::All => "problems",
            Self::Errors => "errors",
            Self::Writing => "writing issues",
        }
    }
}

// ========================================================================
// TextEditor: problems
// ========================================================================

impl TextEditor {
    /// Run problem command `name`.
    pub(super) fn run_problem_command(&mut self, name: &str) -> EditorOutcome {
        if name == editor::PROBLEM_LIST {
            return self.list_problems();
        }
        match walk_of(name) {
            Some(walk) => self.walk_problems(walk),
            None => EditorOutcome::Ignored,
        }
    }

    /// Go to the problem `walk` steps to, and say what it is.
    fn walk_problems(&mut self, walk: Walk) -> EditorOutcome {
        let found = self.gather(walk.scope, walk.filter);
        let here = self.place_here();
        let now = (here.path.as_path(), here.at);
        let next = match walk.step {
            Step::First => found
                .iter()
                .find(|found| found.diagnostic.severity == Severity::Error)
                .or(found.first()),
            Step::Next => found
                .iter()
                .find(|found| key(found) > now)
                .or(found.first()),
            Step::Prev => found
                .iter()
                .rev()
                .find(|found| key(found) < now)
                .or(found.last()),
        };
        let Some(next) = next.cloned() else {
            return self.say_none(walk.scope, walk.filter);
        };
        let place = Place {
            at: next.diagnostic.range.start,
            path: next.path.clone(),
        };
        self.go_to(&place, next.path != here.path);
        self.show_problems_card(describe(std::slice::from_ref(&next.diagnostic)));
        EditorOutcome::Consumed
    }

    /// Every problem in the project, to pick one to go to.
    fn list_problems(&mut self) -> EditorOutcome {
        let found = self.gather(Scope::Project, Filter::All);
        if found.is_empty() {
            return self.say_none(Scope::Project, Filter::All);
        }
        let labels = found
            .iter()
            .map(|found| {
                let d = &found.diagnostic;
                let first = d.message.lines().next().unwrap_or_default();
                format!(
                    "{} {}:{}:{}  {first}",
                    severity_word(d.severity),
                    self.shown_path(&found.path),
                    d.range.start.line + 1,
                    d.range.start.character + 1
                )
            })
            .collect();
        let places = found
            .into_iter()
            .map(|found| Place {
                at: found.diagnostic.range.start,
                path: found.path,
            })
            .collect();
        self.show_places("problems", labels, places)
    }

    /// The problems `filter` admits in `scope`, in file and then position
    /// order, every back-end's brought up to date first.
    fn gather(&mut self, scope: Scope, filter: Filter) -> Vec<Found> {
        let root = match scope {
            Scope::File => None,
            Scope::Project => Some(servers::root_of(&self.doc().path, &[], false)),
        };
        for (backend, refresh) in BACKENDS {
            if filter.wants(*backend) {
                refresh(self, root.as_deref());
            }
        }
        let found = match &root {
            None => diagnostics::in_file(&self.doc().path),
            Some(root) => diagnostics::under(root),
        };
        found
            .into_iter()
            .filter(|found| filter.admits(found))
            .collect()
    }

    /// Say there are no problems of `filter`'s kind in `scope`, and how to
    /// start a server where none runs to find them.
    fn say_none(&mut self, scope: Scope, filter: Filter) -> EditorOutcome {
        let said = match (scope, filter) {
            (Scope::Project, Filter::All | Filter::Errors) if !self.has_language_server() => {
                NO_SERVER.to_string()
            }
            (Scope::File, _) => format!("no {} in this file", filter.plural()),
            (Scope::Project, _) => format!("no {} in the project", filter.plural()),
        };
        self.modal.report(Notice::info(said).unlogged());
        EditorOutcome::Consumed
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Where `found` is, to order it against the cursor.
fn key(found: &Found) -> (&Path, Position) {
    (&found.path, found.diagnostic.range.start)
}

/// What problem command `name` steps through, where, and which way.
fn walk_of(name: &str) -> Option<Walk> {
    let (scope, filter, step) = match name {
        editor::PROBLEM_NEXT_IN_FILE => (Scope::File, Filter::All, Step::Next),
        editor::PROBLEM_PREV_IN_FILE => (Scope::File, Filter::All, Step::Prev),
        editor::PROBLEM_FIRST => (Scope::Project, Filter::All, Step::First),
        editor::PROBLEM_NEXT => (Scope::Project, Filter::All, Step::Next),
        editor::PROBLEM_PREV => (Scope::Project, Filter::All, Step::Prev),
        editor::PROBLEM_FIRST_ERROR => (Scope::Project, Filter::Errors, Step::First),
        editor::PROBLEM_NEXT_ERROR => (Scope::Project, Filter::Errors, Step::Next),
        editor::PROBLEM_PREV_ERROR => (Scope::Project, Filter::Errors, Step::Prev),
        editor::PROBLEM_FIRST_WRITING => (Scope::Project, Filter::Writing, Step::First),
        editor::PROBLEM_NEXT_WRITING => (Scope::Project, Filter::Writing, Step::Next),
        editor::PROBLEM_PREV_WRITING => (Scope::Project, Filter::Writing, Step::Prev),
        _ => return None,
    };
    Some(Walk {
        filter,
        scope,
        step,
    })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_every_problem_command_has_a_walk() {
        for name in editor::PROBLEM_COMMANDS {
            let known = *name == editor::PROBLEM_LIST || walk_of(name).is_some();
            assert!(known, "{name} runs nothing");
        }
    }
}
