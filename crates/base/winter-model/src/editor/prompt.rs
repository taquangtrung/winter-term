//! The questions an editor asks: a line typed in, or a pick from a list.

use crate::paint::IconKind;

// ========================================================================
// Data Structures
// ========================================================================

/// A choice an editor asks the host to collect from a list it already knows: the
/// branches to check out, the tags to delete, the remotes to prune.
///
/// What the host shows is a filter over `items`, not a text field: a name that
/// is not in the list is not an answer, which is the difference between this
/// and a [`PromptRequest`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PickRequest {
    /// The icon drawn before each choice, in the same order: a list of files
    /// shows their types. Empty for a list drawn without icons.
    pub icons: Vec<IconKind>,
    /// The choices, in the order they should read.
    pub items: Vec<String>,
    /// What the list is of, shown where a prompt shows its question.
    pub label: String,
    /// Where the list is shown.
    pub placement: PickPlacement,
    /// Which question this answers, so the editor knows what came back.
    pub tag: &'static str,
}

/// Where a list an editor asks from is shown.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PickPlacement {
    /// Centred on the window under a title bar naming what it is of: a
    /// question about the file or the project.
    #[default]
    Centered,
    /// Floating at the cursor with no title bar: what can be done right
    /// there, as the fixes and the spellings of a word.
    Cursor,
    /// Anchored at the bottom of the editor pane so the editor content above is visible.
    EditorBottom,
}

/// A question an editor answers from a list rather than from typing, held while
/// the list itself is gathered.
///
/// The gathering differs per editor: a git branch list is a command away, a
/// directory's own rows are already in hand — but what happens to the answer
/// does not: it arrives under [`PickQuestion::tag`] as the reply to the same
/// question a [`PromptRequest`] would have asked. A list that comes back
/// empty is no list at all, and every constructor here says so with `None`,
/// which is the caller's cue to fall back to asking for it typed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PickQuestion {
    /// What the list is of, shown as the picker's heading.
    pub label: String,
    /// The question the choice answers.
    pub tag: &'static str,
}

impl PickQuestion {
    /// A question to be answered from a list.
    pub fn new(tag: &'static str, label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            tag,
        }
    }

    /// The list an editor already holds, in the order it should read.
    pub fn over(&self, items: Vec<String>) -> Option<PickRequest> {
        (!items.is_empty()).then(|| PickRequest {
            icons: Vec::new(),
            items,
            label: self.label.clone(),
            placement: PickPlacement::Centered,
            tag: self.tag,
        })
    }

    /// The list an editor already holds, shown at the bottom of the editor pane.
    pub fn over_editor(&self, items: Vec<String>) -> Option<PickRequest> {
        (!items.is_empty()).then(|| PickRequest {
            icons: Vec::new(),
            items,
            label: self.label.clone(),
            placement: PickPlacement::EditorBottom,
            tag: self.tag,
        })
    }

    /// The list a program wrote, one candidate per line: blank lines are
    /// dropped, each line is trimmed, and `keep` has the last word on what
    /// belongs (a remote's own `HEAD`, say, which stands for a name already
    /// listed).
    pub fn over_lines(&self, text: &str, keep: impl Fn(&str) -> bool) -> Option<PickRequest> {
        self.over(
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && keep(line))
                .map(str::to_string)
                .collect(),
        )
    }
}

/// What an editor is asking the host to read from the user.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromptRequest {
    /// Text the input starts out holding, for an answer that is usually an edit
    /// of what is already there.
    pub initial: String,
    /// What is being asked, shown before the input.
    pub label: String,
    /// Whether the answer is typed or a single yes-or-no key.
    pub mode: PromptMode,
    /// Which question this is, so the editor knows what the answer belongs to.
    pub tag: &'static str,
    /// The dialog's own words: its title and what its yes button says;
    /// `None` for one the host names by the question's kind alone.
    pub title: Option<ConfirmText>,
}

/// A confirmation's own words, both naming what a yes does: the title asks
/// it ("Move to Trash?") and the yes button says it ("Move to Trash"), so
/// the answer reads without the message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfirmText {
    /// Whether a yes loses something (edits, files, a running program), so
    /// `Enter` alone must not give it: the focus starts on `Cancel`.
    pub destructive: bool,
    pub title: String,
    pub yes: String,
}

impl PromptRequest {
    /// A yes-or-no question for what cannot be undone: `question` with its
    /// first letter a capital, and the keys that answer it after.
    pub fn confirm(tag: &'static str, question: &str) -> Self {
        let mut chars = question.chars();
        let label = match chars.next() {
            Some(first) => format!("{}{} (y/n) ", first.to_uppercase(), chars.as_str()),
            None => "(y/n) ".to_string(),
        };
        Self {
            initial: String::new(),
            label,
            mode: PromptMode::Confirm,
            tag,
            title: None,
        }
    }

    /// The question titled `title`, which asks in a few words what a yes
    /// does, its yes button saying `yes`, so the dialog reads before its
    /// message is.
    pub fn titled(mut self, title: &str, yes: &str) -> Self {
        self.title = Some(ConfirmText {
            destructive: false,
            title: title.to_string(),
            yes: yes.to_string(),
        });
        self
    }

    /// The question, its yes losing something: the dialog's focus starts on
    /// `Cancel`, so only a deliberate `y` or click gives it.
    pub fn destructive(mut self) -> Self {
        if let Some(text) = self.title.as_mut() {
            text.destructive = true;
        }
        self
    }

    /// The `/` prompt, searching forward through what the editor shows.
    pub fn search(tag: &'static str) -> Self {
        Self {
            initial: String::new(),
            label: "/".to_string(),
            mode: PromptMode::Search { reverse: false },
            tag,
            title: None,
        }
    }
}

/// The answer to a [`PromptRequest`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromptReply {
    /// What the user typed, or `None` when the prompt was cancelled.
    pub answer: Option<String>,
    /// The tag of the question being answered.
    pub tag: &'static str,
}

/// How a prompt collects its answer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PromptMode {
    /// One key: `y` confirms, anything else cancels. For a question whose wrong
    /// answer cannot be undone.
    Confirm,
    /// Single-key query-replace choice: `y` (yes), `n` (no), `!` (all), `.` (replace & exit), `q` (quit).
    QueryReplace,
    /// A line of text to search for, ending at `Enter`: the host highlights
    /// what it matches in the tab while it is typed.
    Search {
        /// Whether the search runs backwards (`?`).
        reverse: bool,
    },
    /// A line of text drawn as dots, ending at `Enter`: a password. The
    /// host neither recalls nor remembers what is typed into it.
    Secret,
    /// A line of text, ending at `Enter`.
    #[default]
    Text,
}
