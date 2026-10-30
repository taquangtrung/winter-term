//! What a key does in the editor: the chords that belong to the editor rather
//! than to the text (stepping between buffers, finishing a commit message,
//! resolving a merge conflict), then the modal keys every text shares, and
//! what the editor does with what those ask of it.

use crate::model::editor::{
    menu_groups, EditorMenuItem, EditorOutcome, Notice, PickPlacement, PickRequest, PromptMode, PromptRequest,
    LSP_CODE_ACTION, LSP_DECLARATION, LSP_DEFINITION, LSP_FORMAT, LSP_HOVER, LSP_IMPLEMENTATION,
    LSP_REFERENCES, LSP_RENAME, LSP_SIGNATURE, LSP_TYPE_DEFINITION, NAVIGATE_BACK, NAVIGATE_FORWARD,
    PROBLEM_NEXT_IN_FILE, PROBLEM_PREV_IN_FILE,
};
use crate::model::editor::{self as commands, Editor};
use crate::model::input::table::{self, Binding, KeyHelp, TableMatch};
use crate::model::input::{Key, KeyCode, Mods};
use crate::model::textedit::modal::{BracketWait, EditMode, EditOutcome, Pending};
use crate::model::vim::nav::VimNav;

use super::comment;
use super::commit;
use super::conflict::{self, Resolution, RESOLUTIONS};
use super::rebase;
use super::writing::Prose;
use super::{TextEditor, ASK_CONFLICT, ASK_SEARCH, ASK_SEARCH_BACK};

// ========================================================================
// Constants
// ========================================================================

/// The window commands a file's buffer keys hand on: a tab holds one file, so
/// going through files is going through tabs and buffers.
const NEXT_TAB: &str = "next_tab";
const PREV_TAB: &str = "prev_tab";
const RECENT_TAB_BACK: &str = "recent_tab_back";
const RECENT_TAB_FORWARD: &str = "recent_tab_forward";
const SWITCH_TO_BUFFER: &str = "switch_to_buffer";

/// The editor's own chords in any file. Stepping between buffers is no part
/// of a text command, so it is taken ahead of the modal keys: the chord means
/// the same whether a file is being read or typed into. Commenting is the
/// same chord in every mode: the line, or the lines a selection covers.
const EDITOR_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::RecentBuffer(false),
        keys: &["C-M-["],
        label: "Back through the buffers by recent use",
    },
    Binding {
        action: Act::RecentBuffer(true),
        keys: &["C-M-]"],
        label: "Forward through the buffers by recent use",
    },
    Binding {
        action: Act::Comment,
        keys: &["C-/"],
        label: "Comment or uncomment the line or selection",
    },
    // The fixes here from every source, a language server's among them, so
    // with or without one serving the file (see `fixes`).
    Binding {
        action: Act::Lsp(LSP_CODE_ACTION),
        keys: &["M-\\", "C-."],
        label: "Fixes and refactorings here",
    },
    // Vim's `]s`, `[s`, `z=`, and `zg` work through the words found (see
    // [`TextEditor::vim_spelling`]).
    Binding {
        action: Act::Spelling,
        keys: &["M-S-i"],
        label: "Check the spelling, or stop",
    },
    Binding {
        action: Act::QueryReplace,
        keys: &["M-%"],
        label: "Query replace: search and interactively replace matches",
    },
    // `C-s` is the modal keys' own; `C-S-s` saves too, ahead of the window's
    // Grep on the same chord, since a hand reaching to save from an editor
    // reaches for either.
    Binding {
        action: Act::Save,
        keys: &["C-S-s"],
        label: "Write the file",
    },
    // Back over a jump to a definition or a problem, and forward again, on
    // VS Code's chords and magic-vscode's.
    // Every occurrence of the word under the cursor, as magic-vscode's
    // occurrence keys have it, and the definition the cursor is in.
    Binding {
        action: Act::Run(commands::EDITOR_CHANGE_OCCURRENCES),
        keys: &["C-;"],
        label: "Change every occurrence of the word at once",
    },
    Binding {
        action: Act::Run(commands::EDITOR_NEXT_OCCURRENCE),
        keys: &["M-n"],
        label: "Next occurrence of the word",
    },
    Binding {
        action: Act::Run(commands::EDITOR_PREV_OCCURRENCE),
        keys: &["M-p"],
        label: "Previous occurrence of the word",
    },
    Binding {
        action: Act::Run(commands::EDITOR_RECENTER),
        keys: &["C-l"],
        label: "The line to the middle of the pane, then its top, then its bottom",
    },
    Binding {
        action: Act::Run(commands::RUST_CARGO_FIX),
        keys: &["C-S-M-o"],
        label: "Rust: run cargo fix",
    },
    Binding {
        action: Act::Run(commands::EDITOR_SELECT_DEFINITION),
        keys: &["C-M-i"],
        label: "Select the function or module, then the one around it",
    },
    Binding {
        action: Act::Lsp(NAVIGATE_BACK),
        keys: &["M-<left>", "S-M-b"],
        label: "Back to where the last jump left from",
    },
    Binding {
        action: Act::Lsp(NAVIGATE_FORWARD),
        keys: &["M-<right>", "S-M-f"],
        label: "Forward again",
    },
];

/// Chords in every file, of more than one key: through the code's
/// definitions (`C-c C-a`, `C-c C-e`, as Emacs has them). Taken last, so a kind of file's own `C-c` chords
/// keep their meaning.
const STRUCTURE_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Run(commands::EDITOR_DEFINITION_START),
        keys: &["C-c C-a"],
        label: "To the start of the function or module",
    },
    Binding {
        action: Act::Run(commands::EDITOR_DEFINITION_END),
        keys: &["C-c C-e"],
        label: "To the end of the function or module",
    },
    // The writing commands, on magic-vscode's `M-s` chords.
    Binding {
        action: Act::Run(commands::WRITING_INSERT_SYMBOL),
        keys: &["M-s i"],
        label: "Insert a math or Greek symbol",
    },
    Binding {
        action: Act::Run(commands::PROBLEM_FIRST_WRITING),
        keys: &["M-s e"],
        label: "First writing issue in the project",
    },
    Binding {
        action: Act::Run(commands::PROBLEM_NEXT_WRITING),
        keys: &["M-s ]"],
        label: "Next writing issue in the project",
    },
    Binding {
        action: Act::Run(commands::PROBLEM_PREV_WRITING),
        keys: &["M-s ["],
        label: "Previous writing issue in the project",
    },
    // The problems of the file, whichever back-end found them.
    Binding {
        action: Act::Run(PROBLEM_NEXT_IN_FILE),
        keys: &["S-M-n"],
        label: "Next problem in the file",
    },
    Binding {
        action: Act::Run(PROBLEM_PREV_IN_FILE),
        keys: &["S-M-p"],
        label: "Previous problem in the file",
    },
    Binding {
        action: Act::Run(commands::DICTIONARY_LOOKUP),
        keys: &["S-M-t"],
        label: "Look the word up in the dictionary",
    },
    Binding {
        action: Act::Run(commands::BIBTEX_SORT_YEAR_ASC),
        keys: &["C-S-M-s"],
        label: "BibTeX: sort the entries by year",
    },
];

/// A file's chords while a language server serves it, in every mode, as
/// magic-vscode binds them: `M-.` to the definition, `M-,` for what a name
/// is, and `C-c` and a control key for the other places a name leads. In
/// Normal mode Vim's `gd`, `gr`, `K`, `]d`, and `[d` do the same (see
/// [`TextEditor::vim_lsp`]).
const LSP_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Lsp(LSP_DEFINITION),
        keys: &["M-.", "C-c C-.", "<f12>"],
        label: "Go to the definition",
    },
    Binding {
        action: Act::Lsp(LSP_HOVER),
        keys: &["M-,", "M-0", "C-c t"],
        label: "What the name is, and the problems there",
    },
    Binding {
        action: Act::Lsp(LSP_REFERENCES),
        keys: &["C-c C-/", "S-<f12>"],
        label: "Every place the name is used",
    },
    Binding {
        action: Act::Lsp(LSP_TYPE_DEFINITION),
        keys: &["C-c C-["],
        label: "Go to the type's definition",
    },
    Binding {
        action: Act::Lsp(LSP_IMPLEMENTATION),
        keys: &["C-c C-]"],
        label: "Go to the implementation",
    },
    Binding {
        action: Act::Lsp(LSP_DECLARATION),
        keys: &["C-c C-\\"],
        label: "Go to the declaration",
    },
    Binding {
        action: Act::Lsp(LSP_RENAME),
        keys: &["C-c C-r", "<f2>"],
        label: "Rename the name everywhere",
    },
    Binding {
        action: Act::Lsp(LSP_FORMAT),
        keys: &["C-S-M-i"],
        label: "Format the file",
    },
    Binding {
        action: Act::Lsp(LSP_SIGNATURE),
        keys: &["C-S-SPC"],
        label: "The signature of the call being typed",
    },
];

/// The menu's entries typed as Vim keys, the clipboard ones through the `+`
/// register so they reach other windows rather than an internal register.
const MENU_COPY: (&str, &str) = ("\" + y", "Copy");
const MENU_CUT: (&str, &str) = ("\" + d", "Cut");
const MENU_PASTE: (&str, &str) = ("\" + p", "Paste");
const MENU_UNDO: (&str, &str) = ("u", "Undo");
const MENU_REDO: (&str, &str) = ("C-r", "Redo");
const MENU_SEARCH: (&str, &str) = ("/", "Find");
const MENU_ESCALATE: (&str, &str) = ("C-o", "Open in $EDITOR");

/// The menu's names for the editor's own chords, shorter than the help list's.
const MENU_COMMENT: &str = "Toggle Comment";
const MENU_SAVE: &str = "Save";

/// A commit message's chords: save and close to commit, or empty it and close
/// to cancel, and earlier messages to start from. `C-S-c` and `C-S-k` are
/// the same in every git editor, as Magit's `C-c C-c` and `C-c C-k` are, and
/// win over the window's copy and kill-buffer chords while one is in front.
const MESSAGE_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Finish,
        keys: &["C-c C-c", "C-S-c"],
        label: "Commit with this message",
    },
    Binding {
        action: Act::Cancel,
        keys: &["C-c C-k", "C-S-k"],
        label: "Cancel the commit",
    },
    Binding {
        action: Act::Recall(true),
        keys: &["C-M-p"],
        label: "An earlier commit message",
    },
    Binding {
        action: Act::Recall(false),
        keys: &["C-M-n"],
        label: "A later commit message",
    },
];

/// A rebase plan's chords: a letter sets the action of the line under the
/// cursor and steps on to the next, `j` and `k` move the line down or up.
const TODO_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Finish,
        keys: &["C-c C-c", "C-S-c"],
        label: "Carry out the rebase",
    },
    Binding {
        action: Act::Cancel,
        keys: &["C-c C-k", "C-S-k"],
        label: "Abort the rebase",
    },
    Binding {
        action: Act::SetAction("pick"),
        keys: &["C-c p"],
        label: "Pick the commit",
    },
    Binding {
        action: Act::SetAction("reword"),
        keys: &["C-c r"],
        label: "Reword the commit",
    },
    Binding {
        action: Act::SetAction("edit"),
        keys: &["C-c e"],
        label: "Stop to edit the commit",
    },
    Binding {
        action: Act::SetAction("squash"),
        keys: &["C-c s"],
        label: "Squash into the commit before",
    },
    Binding {
        action: Act::SetAction("fixup"),
        keys: &["C-c f"],
        label: "Fix up the commit before",
    },
    Binding {
        action: Act::SetAction("drop"),
        keys: &["C-c d"],
        label: "Drop the commit",
    },
    Binding {
        action: Act::MoveLine(true),
        keys: &["C-c j"],
        label: "Move the commit down",
    },
    Binding {
        action: Act::MoveLine(false),
        keys: &["C-c k"],
        label: "Move the commit up",
    },
];

/// A rebase plan's bare keys in Normal mode, as magit's plan buffer has them:
/// git's own one-letter name for each action sets it on the line under the
/// cursor and steps on to the next. They stand in for the Vim keys of the
/// same letters only here, where a plan is read and reordered far more than
/// it is typed into; Insert mode still types them.
const TODO_LETTER_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::SetAction("pick"),
        keys: &["p"],
        label: "Pick the commit",
    },
    Binding {
        action: Act::SetAction("reword"),
        keys: &["r"],
        label: "Reword the commit",
    },
    Binding {
        action: Act::SetAction("edit"),
        keys: &["e"],
        label: "Stop to edit the commit",
    },
    Binding {
        action: Act::SetAction("squash"),
        keys: &["s"],
        label: "Squash into the commit before",
    },
    Binding {
        action: Act::SetAction("fixup"),
        keys: &["f"],
        label: "Fix up the commit before",
    },
    Binding {
        action: Act::SetAction("drop"),
        keys: &["d"],
        label: "Drop the commit",
    },
];

/// A file's chords while it holds merge conflicts, as magic-vscode has them:
/// `M-n` and `M-p` step between the conflicts in every mode, and a `C-c`
/// letter keeps one side of the conflict under the cursor.
const CONFLICT_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::StepConflict(true),
        keys: &["M-n"],
        label: "Next merge conflict",
    },
    Binding {
        action: Act::StepConflict(false),
        keys: &["M-p"],
        label: "Previous merge conflict",
    },
    Binding {
        action: Act::ConflictMenu,
        keys: &["C-c m"],
        label: "Resolve the conflict: choose what to keep",
    },
    Binding {
        action: Act::Resolve(Resolution::Ours),
        keys: &["C-c o"],
        label: "Keep ours (the checked-out side)",
    },
    Binding {
        action: Act::Resolve(Resolution::Theirs),
        keys: &["C-c t"],
        label: "Keep theirs (the incoming side)",
    },
    Binding {
        action: Act::Resolve(Resolution::Both),
        keys: &["C-c b"],
        label: "Keep both, ours first",
    },
    Binding {
        action: Act::Resolve(Resolution::BothReversed),
        keys: &["C-c r"],
        label: "Keep both, theirs first",
    },
    Binding {
        action: Act::Resolve(Resolution::Base),
        keys: &["C-c a"],
        label: "Keep the ancestor's version",
    },
    Binding {
        action: Act::Resolve(Resolution::Neither),
        keys: &["C-c d"],
        label: "Keep neither: delete the conflict",
    },
];

/// A Markdown file's chords, as Emacs' markdown-mode has most of them:
/// `C-c` then a control key moves between headings, `C-c -` and `C-c =`
/// promote and demote, and `C-c` then a letter or a mark wraps the word under
/// the cursor (or the selection) in emphasis.
const MARKDOWN_KEYS: &[Binding<Act>] = &[
    #[cfg(feature = "preview")]
    Binding {
        action: Act::Window("markdown_preview"),
        keys: &["<f7>", "S-<f6>", "<f6>"],
        label: "Preview the document beside it",
    },
    Binding {
        action: Act::Heading(true, false),
        keys: &["C-c C-n"],
        label: "Next heading",
    },
    Binding {
        action: Act::Heading(false, false),
        keys: &["C-c C-p"],
        label: "Previous heading",
    },
    Binding {
        action: Act::Heading(true, true),
        keys: &["C-c C-f"],
        label: "Next heading at the same level",
    },
    Binding {
        action: Act::Heading(false, true),
        keys: &["C-c C-b"],
        label: "Previous heading at the same level",
    },
    Binding {
        action: Act::HeadingUp,
        keys: &["C-c C-u"],
        label: "Up to the parent heading",
    },
    Binding {
        action: Act::Shift(false, false),
        keys: &["C-c -"],
        label: "Promote the heading or list item",
    },
    Binding {
        action: Act::Shift(true, false),
        keys: &["C-c ="],
        label: "Demote the heading or list item",
    },
    Binding {
        action: Act::Shift(false, true),
        keys: &["C-c ["],
        label: "Promote the heading and everything under it",
    },
    Binding {
        action: Act::Shift(true, true),
        keys: &["C-c ]"],
        label: "Demote the heading and everything under it",
    },
    Binding {
        action: Act::Mark("**"),
        keys: &["C-c b"],
        label: "Toggle bold",
    },
    Binding {
        action: Act::Mark("*"),
        keys: &["C-c i"],
        label: "Toggle italic",
    },
    Binding {
        action: Act::Mark("`"),
        keys: &["C-c `"],
        label: "Toggle inline code",
    },
    Binding {
        action: Act::Mark("$"),
        keys: &["C-c $"],
        label: "Toggle inline math",
    },
    Binding {
        action: Act::Mark("~~"),
        keys: &["C-c ~"],
        label: "Toggle strikethrough",
    },
    Binding {
        action: Act::Task,
        keys: &["C-c x"],
        label: "Toggle the task box",
    },
    Binding {
        action: Act::Renumber,
        keys: &["C-c n"],
        label: "Number the ordered list again",
    },
    Binding {
        action: Act::NewItem,
        keys: &["M-RET"],
        label: "New list item or heading below",
    },
    Binding {
        action: Act::WordCount,
        keys: &["C-c w"],
        label: "Count the words",
    },
];

/// A LaTeX file's chords, as AUCTeX has most of them: `C-c C-e` starts an
/// environment, `C-c C-f` then a key sets the font of the word under the
/// cursor (or the selection), and `C-c` then a letter does the rest.
const LATEX_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Heading(true, false),
        keys: &["C-c C-n"],
        label: "Next sectioning command",
    },
    Binding {
        action: Act::Heading(false, false),
        keys: &["C-c C-p"],
        label: "Previous sectioning command",
    },
    Binding {
        action: Act::NewEnvironment,
        keys: &["C-c C-e"],
        label: "Start an environment...",
    },
    Binding {
        action: Act::RenameEnvironment,
        keys: &["C-c e"],
        label: "Rename the enclosing environment...",
    },
    Binding {
        action: Act::ToggleDisplay,
        keys: &["C-c d"],
        label: "Toggle the display between \\[...\\] and equation",
    },
    Binding {
        action: Act::Font("textbf"),
        keys: &["C-c b", "C-c C-f b", "C-c C-f C-b"],
        label: "Toggle bold (\\textbf)",
    },
    Binding {
        action: Act::Font("textit"),
        keys: &["C-c i", "C-c C-f i", "C-c C-f C-i"],
        label: "Toggle italic (\\textit)",
    },
    Binding {
        action: Act::Font("emph"),
        keys: &["C-c C-f e", "C-c C-f C-e"],
        label: "Toggle emphasis (\\emph)",
    },
    Binding {
        action: Act::Font("texttt"),
        keys: &["C-c C-f t", "C-c C-f C-t"],
        label: "Toggle typewriter (\\texttt)",
    },
    Binding {
        action: Act::Font("textsf"),
        keys: &["C-c C-f f", "C-c C-f C-f"],
        label: "Toggle sans serif (\\textsf)",
    },
    Binding {
        action: Act::Font("textsc"),
        keys: &["C-c C-f c", "C-c C-f C-c"],
        label: "Toggle small caps (\\textsc)",
    },
    Binding {
        action: Act::WordCount,
        keys: &["C-c w"],
        label: "Count the words (texcount)",
    },
];

/// The writing commands the palette runs by name, each with the prose it is
/// for and what it does: the same as its chord.
const WRITING: &[(&str, Prose, Act)] = &[
    ("markdown_next_heading", Prose::Markdown, Act::Heading(true, false)),
    ("markdown_prev_heading", Prose::Markdown, Act::Heading(false, false)),
    ("markdown_promote", Prose::Markdown, Act::Shift(false, false)),
    ("markdown_demote", Prose::Markdown, Act::Shift(true, false)),
    ("markdown_promote_subtree", Prose::Markdown, Act::Shift(false, true)),
    ("markdown_demote_subtree", Prose::Markdown, Act::Shift(true, true)),
    ("markdown_bold", Prose::Markdown, Act::Mark("**")),
    ("markdown_italic", Prose::Markdown, Act::Mark("*")),
    ("markdown_code", Prose::Markdown, Act::Mark("`")),
    ("markdown_math", Prose::Markdown, Act::Mark("$")),
    ("markdown_task", Prose::Markdown, Act::Task),
    ("markdown_renumber", Prose::Markdown, Act::Renumber),
    ("markdown_word_count", Prose::Markdown, Act::WordCount),
    ("latex_new_environment", Prose::Latex, Act::NewEnvironment),
    ("latex_rename_environment", Prose::Latex, Act::RenameEnvironment),
    ("latex_toggle_display", Prose::Latex, Act::ToggleDisplay),
    ("latex_bold", Prose::Latex, Act::Font("textbf")),
    ("latex_italic", Prose::Latex, Act::Font("textit")),
    ("latex_emph", Prose::Latex, Act::Font("emph")),
    ("latex_typewriter", Prose::Latex, Act::Font("texttt")),
    ("latex_sans", Prose::Latex, Act::Font("textsf")),
    ("latex_small_caps", Prose::Latex, Act::Font("textsc")),
    ("latex_word_count", Prose::Latex, Act::WordCount),
];

/// The menu's entries past the resolutions of one conflict: one side kept
/// in every conflict of the file.
const ALL_OURS: &str = "Keep ours in every conflict of the file";
const ALL_THEIRS: &str = "Keep theirs in every conflict of the file";

// ========================================================================
// Data Structures
// ========================================================================

/// What a chord of the editor's own does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Act {
    /// Close, leaving nothing for git to act on.
    Cancel,
    /// Comment the line or selection, or uncomment it.
    Comment,
    /// Offer every way of resolving the conflict under the cursor.
    ConflictMenu,
    /// Wrap the word or the selection in a LaTeX font command, or unwrap it.
    Font(&'static str),
    /// To the next heading when the first is set, else the one before; at
    /// the cursor's own level when the second is.
    Heading(bool, bool),
    /// Up to the heading over the cursor's section.
    HeadingUp,
    /// Wrap the word or the selection in a Markdown mark, or unwrap it.
    Mark(&'static str),
    /// Start a LaTeX environment.
    NewEnvironment,
    /// Start the next list item or heading.
    NewItem,
    /// Rename the LaTeX environment the cursor is in.
    RenameEnvironment,
    /// Number the ordered list the cursor is in again.
    Renumber,
    /// Write and close, for git to carry on.
    Finish,
    /// Move the plan line down when set, else up.
    MoveLine(bool),
    /// Step back through the buffers by recent use, forward when set.
    RecentBuffer(bool),
    /// An earlier commit message when set, else a later one.
    Recall(bool),
    /// Keep this of the conflict under the cursor.
    Resolve(Resolution),
    /// Demote when the first is set, else promote; the whole subtree when
    /// the second is.
    Shift(bool, bool),
    /// Tick or clear the task box.
    Task,
    /// Turn a `\[...\]` display into an equation, or back.
    ToggleDisplay,
    /// Count the words.
    WordCount,
    /// Run a language server command, or step over the jumps, by name.
    Lsp(&'static str),
    /// Run one of the editor's own commands by name.
    Run(&'static str),
    /// Show a window command's result: the palette's name for it. Only
    /// the preview is one.
    #[cfg(feature = "preview")]
    Window(&'static str),
    /// Write the file, as `C-s` does.
    Save,
    /// Check the spelling, or stop.
    Spelling,
    /// Interactive query replace (`M-%`).
    QueryReplace,
    /// Give the plan line this action.
    SetAction(&'static str),
    /// On to the next merge conflict when set, else back to the one before.
    StepConflict(bool),
}

// ========================================================================
// TextEditor: dispatch
// ========================================================================

impl TextEditor {
    /// One key: the editor's own chords first, then the modal keys.
    pub(super) fn dispatch(&mut self, key: &Key) -> EditorOutcome {
        if self.inspect_scopes && key.chord() == (Mods::NONE, KeyCode::Escape) {
            self.inspect_scopes = false;
            return EditorOutcome::Consumed;
        }
        // A buffer standing in for a file not yet read takes no edit, which
        // a save would write over the file: any key but a step to another
        // buffer reads it, asking for the password where sudo wants one.
        if self.doc().unread {
            return match table::find(EDITOR_KEYS, key) {
                Some(act @ Act::RecentBuffer(_)) => self.on_act(act),
                _ => self.read_in(),
            };
        }
        if let Some(outcome) = self.occur_key(key) {
            return outcome;
        }
        // Over folded lines, the vertical motions and the row's ends go by
        // screen rows; a run of vertical moves keeps to one column.
        let goal = self.wrap_goal.take();
        if self.completion.is_none() {
            if let Some(outcome) = self.wrap_key(key, goal) {
                return outcome;
            }
        }
        // A count typed before the next move keeps the column it keeps to.
        let counting = !self.modal.is_typing() && matches!(key.chord(), (Mods::NONE, KeyCode::Char('1'..='9')));
        if counting {
            self.wrap_goal = goal;
        }
        // `M-y` steps back through what was yanked right after a yank, and
        // otherwise opens the clipboard ring, as Emacs' yank-pop does.
        let yanking = self.modal.is_typing() && key.chord() == (Mods::ALT, KeyCode::Char('y'));
        let after_yank = std::mem::replace(
            &mut self.yanked,
            self.modal.is_typing() && matches!(key.chord(), (Mods::CTRL | Mods::ALT, KeyCode::Char('y'))),
        );
        if yanking && !after_yank {
            self.yanked = false;
            return EditorOutcome::RunActionHere("clipboard_ring".to_string());
        }
        if let Some(outcome) = self.completion_key(key) {
            return outcome;
        }
        if let Some(outcome) = self.context_key(key) {
            self.after_typed(key);
            return outcome;
        }
        if let Some(outcome) = self.vim_spelling(key) {
            return outcome;
        }
        if let Some(outcome) = self.vim_lsp(key) {
            return outcome;
        }
        if let Some(outcome) = self.file_chord(key) {
            return outcome;
        }
        if let Some(act) = table::find(EDITOR_KEYS, key) {
            return self.on_act(act);
        }
        let outcome = self.with_modal(|modal, view| modal.on_key(view, key));
        let outcome = self.carry_out(outcome);
        self.after_typed(key);
        outcome
    }

    /// The right-click menu, for the mode the text is in: what a selection
    /// can have done to it, what Normal mode offers, and in Insert mode only
    /// the chords that do not type. A commit message or a rebase plan adds
    /// what finishes it, and a plan in Normal mode the action of its line.
    pub(super) fn menu_items(&self) -> Vec<EditorMenuItem> {
        let vim = |(spec, label): (&str, &str)| {
            table::keys(spec).map(|keys| EditorMenuItem::typed(keys, label))
        };
        let own = |act: Act, label: &str| {
            table::menu_typed(EDITOR_KEYS, act).map(|(keys, _)| EditorMenuItem::typed(keys, label))
        };
        let file = |act: Act| {
            let (keys, label) = self
                .file_keys()
                .into_iter()
                .find_map(|keys| table::menu_typed(keys, act))?;
            Some(EditorMenuItem::typed(keys, label))
        };
        let writable = !self.modal.is_read_only();
        let finish: Vec<EditorMenuItem> = [file(Act::Finish), file(Act::Cancel)]
            .into_iter()
            .flatten()
            .collect();
        let conflicts: Vec<EditorMenuItem> = [file(Act::ConflictMenu)]
            .into_iter()
            .flatten()
            .collect();
        let mode = self.modal.mode();
        let letters: Vec<EditorMenuItem> =
            match mode == EditMode::Normal && rebase::is_rebase_todo(&self.doc().path) {
                true => TODO_LETTER_KEYS
                    .iter()
                    .filter_map(|binding| table::menu_typed(TODO_LETTER_KEYS, binding.action))
                    .map(|(keys, label)| EditorMenuItem::typed(keys, label))
                    .collect(),
                false => Vec::new(),
            };
        let comment = own(Act::Comment, MENU_COMMENT).filter(|_| writable);
        let save = own(Act::Save, MENU_SAVE).filter(|_| writable);
        let flat = |items: Vec<Option<EditorMenuItem>>| items.into_iter().flatten().collect();
        match mode {
            EditMode::Visual => menu_groups([
                flat(vec![vim(MENU_COPY), vim(MENU_CUT).filter(|_| writable)]),
                flat(vec![comment]),
                letters,
            ]),
            EditMode::Normal => menu_groups([
                conflicts,
                flat(vec![vim(MENU_PASTE).filter(|_| writable)]),
                flat(vec![
                    vim(MENU_UNDO).filter(|_| writable),
                    vim(MENU_REDO).filter(|_| writable),
                ]),
                flat(vec![comment, vim(MENU_SEARCH)]),
                letters,
                flat(vec![save, vim(MENU_ESCALATE)]),
                finish,
            ]),
            EditMode::Insert | EditMode::Replace => {
                menu_groups([flat(vec![comment, save]), finish])
            }
        }
    }

    /// Every chord the editor answers for the file it is showing, and the
    /// motions its Vim keys share with every editor.
    pub(super) fn key_help(&self) -> Vec<KeyHelp> {
        let mut rows: Vec<KeyHelp> = self.file_keys().into_iter().flat_map(table::help).collect();
        if rebase::is_rebase_todo(&self.doc().path) {
            rows.extend(table::help(TODO_LETTER_KEYS));
        }
        rows.extend(table::help(EDITOR_KEYS));
        rows.extend(VimNav::help());
        rows
    }

    /// The chords of the kind of file being edited: a commit message's or a
    /// rebase plan's, and a merge conflict's while the file holds one.
    fn file_keys(&self) -> Vec<&'static [Binding<Act>]> {
        let path = &self.doc().path;
        let mut keys = Vec::new();
        if commit::is_message_file(path) {
            keys.push(MESSAGE_KEYS);
        } else if rebase::is_rebase_todo(path) {
            keys.push(TODO_KEYS);
        }
        // A buffer that takes no edits is still read with a server's help.
        if self.modal.is_read_only() {
            if self.is_served() {
                keys.push(LSP_KEYS);
            }
            keys.push(STRUCTURE_KEYS);
            return keys;
        }
        if !self.doc().conflicts().is_empty() {
            keys.push(CONFLICT_KEYS);
        }
        match self.prose() {
            Some(Prose::Markdown) => keys.push(MARKDOWN_KEYS),
            Some(Prose::Latex) => keys.push(LATEX_KEYS),
            None => {}
        }
        // Last, so a conflict's own `C-c` letters keep their meaning.
        if self.is_served() {
            keys.push(LSP_KEYS);
        }
        keys.push(STRUCTURE_KEYS);
        keys
    }

    /// A chord of the kind of file being edited. `None` for every other key,
    /// which is left to the rest; a `C-c` not followed by a key that
    /// completes it is forgotten, and the key with it.
    fn file_chord(&mut self, key: &Key) -> Option<EditorOutcome> {
        let tables = self.file_keys();
        if tables.is_empty() {
            return None;
        }
        let typed: Vec<Key> = self.chord_start.take().into_iter().chain([*key]).collect();
        let found = tables
            .iter()
            .map(|keys| table::lookup(keys, &typed))
            .reduce(|found, next| match (found, next) {
                (TableMatch::Exact(act), _) | (_, TableMatch::Exact(act)) => TableMatch::Exact(act),
                (TableMatch::Prefix, _) | (_, TableMatch::Prefix) => TableMatch::Prefix,
                _ => TableMatch::Miss,
            })
            .unwrap_or(TableMatch::Miss);
        match found {
            TableMatch::Exact(act) => Some(self.on_act(act)),
            TableMatch::Prefix => {
                self.chord_start = Some(*key);
                Some(EditorOutcome::Consumed)
            }
            TableMatch::Miss if typed.len() > 1 => Some(EditorOutcome::Consumed),
            TableMatch::Miss => self.todo_letter(key),
        }
    }

    /// Vim's keys for a language server, in Normal mode on a file one
    /// serves: `gd` to the definition, `gr` to the references, `K` for what
    /// a name is, and `]d` and `[d` through the problems. `None` for every
    /// other key, which the modal keys go on to read.
    fn vim_lsp(&mut self, key: &Key) -> Option<EditorOutcome> {
        if self.modal.mode() != EditMode::Normal {
            return None;
        }
        let chord = key.chord();
        // `zj` and `zk` step between the blocks the indentation makes, as
        // they step between folds in Vim.
        let block = match (self.modal.pending(), chord) {
            (Some(Pending::Scroll), (Mods::NONE, KeyCode::Char('j'))) => Some(true),
            (Some(Pending::Scroll), (Mods::NONE, KeyCode::Char('k'))) => Some(false),
            _ => None,
        };
        if let Some(forward) = block {
            let at = self.at.min(self.docs.len() - 1);
            self.modal.reset(&self.docs[at].buffer);
            return Some(self.step_block(forward));
        }
        if !self.is_served() {
            return None;
        }
        let name = match (self.modal.pending(), chord) {
            (Some(Pending::Goto), (Mods::NONE, KeyCode::Char('d'))) => LSP_DEFINITION,
            (Some(Pending::Goto), (Mods::NONE, KeyCode::Char('r'))) => LSP_REFERENCES,
            (Some(Pending::Bracket(BracketWait { forward })), (Mods::NONE, KeyCode::Char('d'))) => match forward {
                true => PROBLEM_NEXT_IN_FILE,
                false => PROBLEM_PREV_IN_FILE,
            },
            (None, (Mods::NONE, KeyCode::Char('K'))) if self.modal.is_idle() => LSP_HOVER,
            _ => return None,
        };
        // The `g` or the bracket waiting on this key is done with.
        let at = self.at.min(self.docs.len() - 1);
        self.modal.reset(&self.docs[at].buffer);
        Some(match name {
            PROBLEM_NEXT_IN_FILE | PROBLEM_PREV_IN_FILE => self.run_problem_command(name),
            _ => self.run_lsp_command(name),
        })
    }

    /// A rebase plan's bare action key, while Normal mode has nothing
    /// part-typed for it to finish, or over a Visual selection. `None`
    /// anywhere else.
    fn todo_letter(&mut self, key: &Key) -> Option<EditorOutcome> {
        let ready = self.modal.is_idle() || self.modal.mode() == EditMode::Visual;
        if !rebase::is_rebase_todo(&self.doc().path) || !ready {
            return None;
        }
        let act = table::find(TODO_LETTER_KEYS, key)?;
        Some(self.on_act(act))
    }

    /// Do what a chord of the editor's own asks.
    fn on_act(&mut self, act: Act) -> EditorOutcome {
        let row = self.buffer().row();
        match act {
            Act::Cancel if rebase::is_rebase_todo(&self.doc().path) => {
                // An empty plan is how git is told to abort: it stops at
                // "nothing to do" and leaves the branch as it was.
                let rows = self.buffer().lines().len();
                self.buffer_mut().replace_rows(0, rows, vec![String::new()]);
                self.write_file();
                EditorOutcome::Close
            }
            Act::Cancel => self.cancel_message().unwrap_or(EditorOutcome::Consumed),
            Act::Spelling => self.toggle_spelling(),
            Act::QueryReplace => self.start_query_replace(),
            Act::Comment => self.toggle_comment(),
            Act::ConflictMenu => self.conflict_menu(),
            Act::Save => self.carry_out(EditOutcome::Save),
            Act::Lsp(name) => self.run_lsp_command(name),
            Act::Run(name) => self.run_command(name).unwrap_or(EditorOutcome::Consumed),
            #[cfg(feature = "preview")]
            Act::Window(name) => EditorOutcome::RunActionHere(name.to_string()),
            Act::Finish if rebase::is_rebase_todo(&self.doc().path) => self.finish_todo(),
            Act::Finish => {
                self.write_file();
                EditorOutcome::Close
            }
            Act::MoveLine(true) => {
                self.move_todo_line(row, row + 1);
                EditorOutcome::Consumed
            }
            Act::MoveLine(false) => {
                if row > 0 {
                    self.move_todo_line(row, row - 1);
                }
                EditorOutcome::Consumed
            }
            // A tab holds one file, so stepping through files is stepping
            // through the pane's tabs, the window's to do.
            Act::RecentBuffer(forward) => EditorOutcome::RunActionHere(
                match forward {
                    true => RECENT_TAB_FORWARD,
                    false => RECENT_TAB_BACK,
                }
                .to_string(),
            ),
            Act::Recall(back) => self.recall_message(back),
            Act::Resolve(resolution) => self.resolve_conflict(resolution),
            Act::Font(command) => self.toggle_font(command),
            Act::Heading(forward, same_level) => self.step_heading(forward, same_level),
            Act::HeadingUp => self.heading_up(),
            Act::Mark(mark) => self.toggle_mark(mark),
            Act::NewEnvironment => self.ask_new_environment(),
            Act::NewItem => self.new_item(),
            Act::RenameEnvironment => self.ask_rename_environment(),
            Act::Renumber => self.renumber_list(),
            Act::Shift(deeper, subtree) => self.shift_heading(deeper, subtree),
            Act::Task => self.toggle_task(),
            Act::ToggleDisplay => self.toggle_display(),
            Act::WordCount => self.word_count(),
            Act::SetAction(action) => self.set_todo_action(action),
            Act::StepConflict(forward) => self.step_conflict(forward),
        }
    }

    /// Run the writing command the palette calls `name`, in a file of the
    /// prose it is for. `None` for a name that is no writing command.
    pub(super) fn run_writing_command(&mut self, name: &str) -> Option<EditorOutcome> {
        let (_, prose, act) = WRITING.iter().find(|(known, _, _)| *known == name)?;
        let refusal = match (self.prose(), self.modal.is_read_only()) {
            (_, true) => Some("this file is read-only"),
            (Some(found), false) if found == *prose => None,
            _ => Some(match prose {
                Prose::Latex => "not a LaTeX file",
                Prose::Markdown => "not a Markdown file",
            }),
        };
        if let Some(refusal) = refusal {
            self.modal.report(Notice::error(refusal));
            return Some(EditorOutcome::Consumed);
        }
        Some(self.on_act(*act))
    }

    /// Move to the next merge conflict, or the one before, wrapping round
    /// the file, and say which of how many it is.
    pub(super) fn step_conflict(&mut self, forward: bool) -> EditorOutcome {
        let conflicts = self.doc().conflicts();
        let Some(found) = conflict::next(&conflicts, self.buffer().row(), forward) else {
            self.modal.report(Notice::error("no merge conflicts here"));
            return EditorOutcome::Consumed;
        };
        self.leave_to_normal();
        self.buffer_mut().move_to(found.start, 0);
        let index = conflicts.iter().position(|c| *c == found).unwrap_or(0);
        self.modal.report(Notice::info(format!(
            "conflict {} of {}",
            index + 1,
            conflicts.len()
        )));
        EditorOutcome::Consumed
    }

    /// The ways of resolving the conflict under the cursor, as a list to
    /// choose from, then the ways of resolving every conflict at once.
    pub(super) fn conflict_menu(&mut self) -> EditorOutcome {
        let conflicts = self.doc().conflicts();
        let here = conflict::at(&conflicts, self.buffer().row());
        let mut items: Vec<String> = RESOLUTIONS
            .iter()
            .filter(|resolution| {
                here.is_some_and(|c| **resolution != Resolution::Base || c.base.is_some())
            })
            .map(|resolution| resolution.label().to_string())
            .collect();
        items.extend([ALL_OURS.to_string(), ALL_THEIRS.to_string()]);
        EditorOutcome::Pick(PickRequest {
            icons: Vec::new(),
            items,
            label: format!("Merge conflicts: {} in this file", conflicts.len()),
            placement: PickPlacement::Centered,
            tag: ASK_CONFLICT,
        })
    }

    /// Carry out the choice the conflict menu came back with.
    pub(super) fn conflict_chosen(&mut self, choice: &str) -> EditorOutcome {
        match choice {
            ALL_OURS => self.resolve_every_conflict(Resolution::Ours),
            ALL_THEIRS => self.resolve_every_conflict(Resolution::Theirs),
            label => match RESOLUTIONS.iter().find(|r| r.label() == label) {
                Some(resolution) => self.resolve_conflict(*resolution),
                None => EditorOutcome::Consumed,
            },
        }
    }

    /// Keep `resolution` of the conflict under the cursor in place of the
    /// whole block, and land on what was kept.
    fn resolve_conflict(&mut self, resolution: Resolution) -> EditorOutcome {
        let conflicts = self.doc().conflicts();
        let Some(here) = conflict::at(&conflicts, self.buffer().row()) else {
            self.modal
                .report(Notice::error("the cursor is not in a merge conflict"));
            return EditorOutcome::Consumed;
        };
        let Some(kept) = here.kept(self.buffer().lines(), resolution) else {
            self.modal.report(Notice::error(
                "this conflict carries no ancestor (merge with conflictStyle diff3)",
            ));
            return EditorOutcome::Consumed;
        };
        self.buffer_mut().replace_rows(here.start, here.len(), kept);
        self.leave_to_normal();
        self.buffer_mut().move_to(here.start, 0);
        self.report_conflicts_left(conflicts.len() - 1);
        EditorOutcome::Consumed
    }

    /// Keep `resolution` of every conflict in the file, as one change one
    /// undo takes back.
    fn resolve_every_conflict(&mut self, resolution: Resolution) -> EditorOutcome {
        let lines = self.buffer().lines().to_vec();
        let conflicts = conflict::find(&lines);
        let mut kept = Vec::with_capacity(lines.len());
        let mut from = 0;
        for found in &conflicts {
            kept.extend_from_slice(&lines[from..found.start]);
            kept.extend(found.kept(&lines, resolution).unwrap_or_default());
            from = found.end + 1;
        }
        kept.extend_from_slice(&lines[from..]);
        let row = self.buffer().row().min(kept.len().saturating_sub(1));
        self.buffer_mut().replace_rows(0, lines.len(), kept);
        self.leave_to_normal();
        self.buffer_mut().move_to(row, 0);
        self.report_conflicts_left(0);
        EditorOutcome::Consumed
    }

    /// Say how many conflicts are left, and what to do once none are.
    fn report_conflicts_left(&mut self, left: usize) {
        let notice = match left {
            0 => Notice::info("every conflict is resolved: save, then stage the file"),
            1 => Notice::info("1 conflict left"),
            n => Notice::info(format!("{n} conflicts left")),
        };
        self.modal.report(notice);
    }

    /// Give `action` to the plan line under the cursor, or to every commit
    /// line a Visual selection covers, and step on past them. The lines that
    /// are not commits (a comment, a blank) are left as they are; one undo
    /// takes the whole change back.
    fn set_todo_action(&mut self, action: &str) -> EditorOutcome {
        let (first, last) = match self.selection() {
            Some(selection) => (selection.start.0, selection.end.0),
            None => (self.buffer().row(), self.buffer().row()),
        };
        let lines = self.buffer().lines()[first..=last].to_vec();
        let changed: Vec<Option<String>> = lines
            .iter()
            .map(|line| rebase::with_action(line, action))
            .collect();
        if changed.iter().all(Option::is_none) {
            self.modal.report(Notice::error("not a commit line"));
            return EditorOutcome::Consumed;
        }
        let rows = lines
            .into_iter()
            .zip(changed)
            .map(|(line, changed)| changed.unwrap_or(line))
            .collect();
        self.buffer_mut()
            .replace_rows(first, last - first + 1, rows);
        self.leave_to_normal();
        self.buffer_mut().move_to_line(last + 1);
        EditorOutcome::Consumed
    }

    /// Swap plan lines `from` and `to`, the cursor going with the moved one.
    fn move_todo_line(&mut self, from: usize, to: usize) {
        let lines = self.buffer().lines();
        if to >= lines.len() {
            return;
        }
        let first = from.min(to);
        let swapped = vec![lines[first + 1].clone(), lines[first].clone()];
        self.buffer_mut().replace_rows(first, 2, swapped);
        self.buffer_mut().move_to_line(to);
    }

    /// Write the plan and close, so git carries it out, unless git would
    /// refuse it, which is said instead.
    fn finish_todo(&mut self) -> EditorOutcome {
        if let Some(problem) = rebase::problem(self.buffer().lines()) {
            self.modal.report(Notice::error(problem));
            return EditorOutcome::Consumed;
        }
        self.write_file();
        EditorOutcome::Close
    }

    /// Comment out the cursor's line, or every line a selection covers, or
    /// comment them back in, with the file's own comment marker (`C-/`).
    /// The cursor stays on the character it was on.
    fn toggle_comment(&mut self) -> EditorOutcome {
        let marker = self
            .doc()
            .grammar
            .and_then(|grammar| comment::marker_of(grammar.name()));
        let Some(marker) = marker else {
            self.modal
                .report(Notice::error("no comment marker known for this file"));
            return EditorOutcome::Consumed;
        };
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let (first, last) = match self.selection() {
            Some(selection) => (selection.start.0, selection.end.0),
            None => (row, row),
        };
        let before = self.buffer().lines()[first..=last].to_vec();
        let after = comment::toggle(&before, marker);
        if after == before {
            return EditorOutcome::Consumed;
        }
        let shift = after[row - first].chars().count() as isize
            - before[row - first].chars().count() as isize;
        let col = (col as isize + shift).max(0) as usize;
        self.buffer_mut()
            .replace_rows(first, last - first + 1, after);
        match self.modal.is_typing() {
            true => self.buffer_mut().move_to_typing(row, col),
            false => self.buffer_mut().move_to(row, col),
        }
        self.modal.clear_selection();
        EditorOutcome::Consumed
    }

    /// Do what a modal key asked of the editor.
    fn carry_out(&mut self, outcome: EditOutcome) -> EditorOutcome {
        match outcome {
            // A file is never a field that is only typed into, and `Enter`
            // breaks its lines, so these two do not reach an editor.
            EditOutcome::Cancel | EditOutcome::Submit => EditorOutcome::Consumed,
            EditOutcome::Consumed => EditorOutcome::Consumed,
            EditOutcome::Escalate => self.escalate(),
            EditOutcome::Ignored => EditorOutcome::Ignored,
            EditOutcome::ListBuffers => EditorOutcome::RunActionHere(SWITCH_TO_BUFFER.to_string()),
            EditOutcome::Paste => EditorOutcome::Paste,
            EditOutcome::Cut(text) => EditorOutcome::Cut(text),
            EditOutcome::Quit { save: true } if rebase::is_rebase_todo(&self.doc().path) => {
                self.finish_todo()
            }
            // A save that has to ask first (as root, for a password) keeps
            // the buffer open, since closing would drop the edits it holds.
            EditOutcome::Quit { save: true } => match self.write_file() {
                EditorOutcome::Consumed => EditorOutcome::Close,
                asked => asked,
            },
            EditOutcome::Quit { save: false } => match self.cancel_message() {
                Some(outcome) => outcome,
                None => {
                    self.reject_waiter_if_closing();
                    EditorOutcome::Close
                }
            },
            EditOutcome::Save => self.save(),
            EditOutcome::Search { forward } => EditorOutcome::Prompt(PromptRequest {
                initial: String::new(),
                label: if forward {
                    "/".to_string()
                } else {
                    "?".to_string()
                },
                mode: PromptMode::Search { reverse: !forward },
                tag: if forward { ASK_SEARCH } else { ASK_SEARCH_BACK },
            }),
            EditOutcome::StepBuffer { forward } => EditorOutcome::RunActionHere(
                match forward {
                    true => NEXT_TAB,
                    false => PREV_TAB,
                }
                .to_string(),
            ),
            EditOutcome::Yank(text) => EditorOutcome::Yank(text),
            EditOutcome::Macro(step) => EditorOutcome::Macro(step),
            EditOutcome::Place(place) => {
                self.place_cursor_row(place);
                EditorOutcome::Consumed
            }
        }
    }
}

// ========================================================================
// TextEditor: buffers
// ========================================================================

impl TextEditor {
    /// Look for `pattern` from the cursor, and stay where it was found.
    pub(super) fn start_search(&mut self, pattern: String, forward: bool) -> EditorOutcome {
        self.with_modal(|modal, view| modal.start_search(view, pattern, forward));
        EditorOutcome::Consumed
    }

    /// Put text the host read out of the clipboard in at the cursor.
    pub(super) fn paste_text(&mut self, text: String) -> EditorOutcome {
        self.with_modal(|modal, view| modal.paste_text(view, text));
        EditorOutcome::Consumed
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_every_writing_command_the_palette_lists_is_one_the_editor_runs() {
        let known: Vec<&str> = WRITING.iter().map(|(name, _, _)| *name).collect();
        assert_eq!(known, crate::model::editor::WRITING_COMMANDS);
    }

    #[test]
    fn test_every_key_of_the_editor_is_spelled_right() {
        for keys in [
            EDITOR_KEYS,
            MESSAGE_KEYS,
            TODO_KEYS,
            CONFLICT_KEYS,
            MARKDOWN_KEYS,
            LATEX_KEYS,
            LSP_KEYS,
            STRUCTURE_KEYS,
        ] {
            assert_eq!(table::invalid_specs(keys), Vec::<&str>::new());
        }
    }
}
