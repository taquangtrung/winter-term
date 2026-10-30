//! The fixes on offer at the cursor, gathered from every source that has
//! any into the one list `M-\` shows: the editor's own sources, which
//! answer at once, and the language server serving the file, which answers
//! later and is waited for.
//!
//! A new source is a function in [`LOCAL_SOURCES`] saying what it offers
//! where the cursor is, each fix with the [`Apply`] that makes it.

use winter_lsp::protocol::CodeAction;
use crate::editors::spell::WordList;
use winter_lsp::ServerId;
use crate::model::editor::{EditorOutcome, Notice, PickPlacement, PickRequest};
use crate::model::paint::IconKind;

use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// The question a list of fixes answers.
pub(super) const ASK_FIX: &str = "fix";

/// What the list is headed with.
const FIXES_LABEL: &str = "Quick Fix";

/// The symbol each fix on offer wears, as VS Code's do.
const FIX_ICON: IconKind = IconKind::Symbol { name: "lightbulb" };

/// The sources that answer at once, asked in this order: their fixes lead
/// the list, a server's following them.
const LOCAL_SOURCES: &[fn(&TextEditor) -> Vec<Fix>] = &[TextEditor::spelling_fixes];

// ========================================================================
// Data Structures
// ========================================================================

/// One fix on offer: what the list calls it, and what choosing it does.
#[derive(Debug)]
pub(super) struct Fix {
    pub(super) apply: Apply,
    pub(super) title: String,
}

/// What choosing a fix does.
#[derive(Debug)]
pub(super) enum Apply {
    /// A language server's code action, made as that server says.
    Action(ServerId, CodeAction),
    /// Put a text in place of some columns of a line.
    Replace(Replace),
    /// Take a word as spelled right, in one of the word lists.
    Accept(Accept),
}

/// A word to take as spelled right, and the list it goes in.
#[derive(Debug)]
pub(super) struct Accept {
    pub(super) list: WordList,
    pub(super) word: String,
}

/// A text to put in place of columns `from` to `to` of line `row`.
#[derive(Debug)]
pub(super) struct Replace {
    pub(super) from: usize,
    pub(super) row: usize,
    pub(super) text: String,
    pub(super) to: usize,
}

/// The fixes last offered, and the ones waiting on a server's.
#[derive(Debug, Default)]
pub(super) struct Fixes {
    /// What the list last showed, in its order.
    offered: Vec<Fix>,
    /// The editor's own, held while a server's are waited for.
    waiting: Option<Vec<Fix>>,
}

// ========================================================================
// TextEditor: fixes
// ========================================================================

impl TextEditor {
    /// Gather every source's fixes at the cursor, or over the selection,
    /// and offer them; a server that can be asked is waited for first.
    pub(super) fn ask_fixes(&mut self) -> EditorOutcome {
        if let Some(refused) = self.refuse_edits() {
            return refused;
        }
        let local: Vec<Fix> = LOCAL_SOURCES.iter().flat_map(|source| source(self)).collect();
        if self.ask_actions() {
            self.fixes.waiting = Some(local);
            return EditorOutcome::Consumed;
        }
        self.offer_fixes(FIXES_LABEL.to_string(), local)
    }

    /// The server's code actions came back: offered after the editor's own
    /// fixes.
    pub(super) fn server_fixes(&mut self, server: ServerId, actions: Vec<CodeAction>) -> EditorOutcome {
        let mut fixes = self.fixes.waiting.take().unwrap_or_default();
        fixes.extend(actions.into_iter().map(|action| Fix {
            title: action.title.clone(),
            apply: Apply::Action(server, action),
        }));
        self.offer_fixes(FIXES_LABEL.to_string(), fixes)
    }

    /// The server could not say: the editor's own fixes are offered alone.
    pub(super) fn server_fixes_failed(&mut self) -> EditorOutcome {
        match self.fixes.waiting.take() {
            Some(local) if !local.is_empty() => self.offer_fixes(FIXES_LABEL.to_string(), local),
            _ => EditorOutcome::Consumed,
        }
    }

    /// Offer `fixes` as a list headed `label`, or say there are none.
    pub(super) fn offer_fixes(&mut self, label: String, fixes: Vec<Fix>) -> EditorOutcome {
        if fixes.is_empty() {
            self.modal.report(Notice::info("no fixes or refactorings here").unlogged());
            return EditorOutcome::Consumed;
        }
        let items = labels_of(&fixes);
        let icons = vec![FIX_ICON; items.len()];
        self.fixes.offered = fixes;
        EditorOutcome::Pick(PickRequest {
            icons,
            items,
            label,
            placement: PickPlacement::Cursor,
            tag: ASK_FIX,
        })
    }

    /// Make the fix the list came back with, named by its label.
    pub(super) fn fix_chosen(&mut self, answer: &str) -> EditorOutcome {
        let index = labels_of(&self.fixes.offered)
            .iter()
            .position(|label| label == answer);
        let Some(index) = index else {
            return EditorOutcome::Consumed;
        };
        let fix = self.fixes.offered.swap_remove(index);
        self.fixes.offered.clear();
        match fix.apply {
            Apply::Action(server, action) => self.take_action(server, action),
            Apply::Replace(replace) => self.replace_columns(replace),
            Apply::Accept(accept) => self.accept_word(&accept.word, &accept.list),
        }
        EditorOutcome::Consumed
    }

    /// Put `replace`'s text in place of its columns, as one undo step, the
    /// cursor at the start of what was put there.
    fn replace_columns(&mut self, replace: Replace) {
        let Some(line) = self.buffer().lines().get(replace.row) else {
            return;
        };
        let mut chars: Vec<char> = line.chars().collect();
        let to = replace.to.min(chars.len());
        let from = replace.from.min(to);
        chars.splice(from..to, replace.text.chars());
        self.buffer_mut()
            .replace_rows(replace.row, 1, vec![chars.into_iter().collect()]);
        self.buffer_mut().move_to(replace.row, from);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What the list calls each of `fixes`: its title, and where a title comes
/// again, its place among those of that title, so every choice is its own.
fn labels_of(fixes: &[Fix]) -> Vec<String> {
    fixes
        .iter()
        .enumerate()
        .map(|(at, fix)| {
            let before = fixes[..at].iter().filter(|other| other.title == fix.title).count();
            match before {
                0 => fix.title.clone(),
                _ => format!("{} ({})", fix.title, before + 1),
            }
        })
        .collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A fix titled `title`.
    fn titled(title: &str) -> Fix {
        Fix {
            apply: Apply::Accept(Accept {
                list: WordList::Session,
                word: title.to_string(),
            }),
            title: title.to_string(),
        }
    }

    #[test]
    fn test_a_title_offered_twice_is_told_apart_by_its_place() {
        let fixes = [titled("Fix it"), titled("Other"), titled("Fix it")];
        assert_eq!(labels_of(&fixes), ["Fix it", "Other", "Fix it (2)"]);
    }
}
