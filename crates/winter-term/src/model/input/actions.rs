//! The commands a binding can name: window commands, line edits, and the
//! commands aimed at one numbered pane or tab.

use crate::model::layout::{Direction, FocusDir};

use super::{Action, BlockNav};

// ========================================================================
// Data Structures
// ========================================================================

/// A configurable Insert-mode line edit at the shell prompt. Each is realized by
/// sending the equivalent readline keystrokes (the app layer owns that mapping)
/// and folding the same change into the prompt undo shadow.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EditAction {
    /// Kill from the cursor to the end of the line.
    DeleteToLineEnd,
    /// Kill from the cursor to the start of the line.
    DeleteToLineStart,
    /// Kill the word before the cursor.
    DeleteWordBackward,
    /// Kill the word after the cursor.
    DeleteWordForward,
}
impl EditAction {
    /// Parse a config action name (shared with the binding docs).
    pub(super) fn from_name(name: &str) -> Option<EditAction> {
        Some(match name {
            "delete_to_line_end" => EditAction::DeleteToLineEnd,
            "delete_to_line_start" => EditAction::DeleteToLineStart,
            "delete_word_backward" => EditAction::DeleteWordBackward,
            "delete_word_forward" => EditAction::DeleteWordForward,
            _ => return None,
        })
    }
}
/// A configurable binding in the `editing` block: either a line edit, or a
/// prompt-history command (undo/redo). Line edits apply only in Insert mode;
/// undo/redo apply in Insert, Normal, and the palette.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EditBinding {
    /// Apply a line edit to the shell's current prompt line.
    Edit(EditAction),
    /// Redo the last undone prompt-line edit.
    Redo,
    /// Undo the last prompt-line edit.
    Undo,
}
impl EditBinding {
    /// Parse a config action name. Recognizes the line-edit names plus
    /// `prompt_undo`/`undo` and `prompt_redo`/`redo`.
    pub(super) fn from_name(name: &str) -> Option<EditBinding> {
        Some(match name {
            "prompt_redo" | "redo" => EditBinding::Redo,
            "prompt_undo" | "undo" => EditBinding::Undo,
            other => EditBinding::Edit(EditAction::from_name(other)?),
        })
    }

    /// The dispatched [`Action`] this binding produces.
    pub(super) fn to_action(self) -> Action {
        match self {
            EditBinding::Edit(edit) => Action::Edit(edit),
            EditBinding::Redo => Action::PromptRedo,
            EditBinding::Undo => Action::PromptUndo,
        }
    }

    /// Whether this binding is a prompt-history command, which (unlike line
    /// edits) is also active in Normal mode and the palette.
    pub(super) fn is_history(self) -> bool {
        matches!(self, EditBinding::Redo | EditBinding::Undo)
    }
}
/// A window-management command: the configurable subset of Normal-mode bindings.
/// Each maps to a layout-affecting [`Action`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WindowAction {
    /// Close the focused pane.
    Close,
    /// Close every pane except the focused one.
    CloseOthers,
    /// Move focus to the pane below.
    FocusDown,
    /// Move focus to the pane on the left.
    FocusLeft,
    /// Move focus to the pane on the right.
    FocusRight,
    /// Move focus to the pane above.
    FocusUp,
    /// Move the current tab to the pane below.
    MoveTabDown,
    /// Move the current tab to the pane on the left.
    MoveTabLeft,
    /// Move the current tab to the pane on the right.
    MoveTabRight,
    /// Move the current tab to the pane above.
    MoveTabUp,
    /// Split the focused pane into upper and lower halves.
    SplitHorizontal,
    /// Split the focused pane into left and right halves.
    SplitVertical,
    /// Toggle the focused pane filling the whole tab.
    Zoom,
    /// Scroll the focused pane up one screen.
    ScrollPageUp,
    /// Scroll the focused pane down one screen.
    ScrollPageDown,
    /// Scroll the focused pane up one line.
    ScrollLineUp,
    /// Scroll the focused pane down one line.
    ScrollLineDown,
    /// Jump to the oldest retained scrollback.
    ScrollToTop,
    /// Jump back to the live output.
    ScrollToBottom,
    /// Activate the tab to the left.
    PrevTab,
    /// Activate the tab to the right.
    NextTab,
    /// Copy the current selection to the clipboard.
    Copy,
    /// Copy the current working directory or file reference to the clipboard.
    CopyCwd,
    /// Paste the clipboard into the PTY.
    Paste,
    /// Open a new tab.
    NewTab,
    /// Open a new window.
    NewWindow,
    /// Close the active tab.
    CloseTab,
    /// Close every tab in the focused pane except the active one.
    CloseOtherTabs,
    /// Open the settings page.
    OpenSettings,
    /// Increase the font size one step.
    FontIncrease,
    /// Decrease the font size one step.
    FontDecrease,
    /// Return the font size to the configured value.
    FontReset,
    /// Show or hide the command palette.
    TogglePalette,
    /// Show or hide the palette over past queries.
    ToggleHistoryPalette,
    /// Show or hide the pane switcher.
    TogglePaneSwitcher,
    /// Show or hide buffer swoop.
    ToggleSwoop,
    /// Jump to target in current buffer view (fuzzy completion dialog).
    Jump,
    /// Move to the next command block.
    NextBlock,
    /// Move to the previous command block.
    PrevBlock,
}
impl WindowAction {
    /// Whether this action scrolls Winter's own scrollback. These bindings are
    /// bypassed when a full-screen (alt-screen) app is running so the key
    /// reaches the app instead, scrolling a TUI app's non-existent scrollback
    /// is never useful, and intercepting the key would swallow app shortcuts
    /// like Claude Code's `M-S-e/H/L`.
    pub(super) fn is_scroll(self) -> bool {
        matches!(
            self,
            WindowAction::ScrollPageUp
                | WindowAction::ScrollPageDown
                | WindowAction::ScrollLineUp
                | WindowAction::ScrollLineDown
                | WindowAction::ScrollToTop
                | WindowAction::ScrollToBottom
        )
    }

    /// Whether this action must intercept the key before overlays (the command
    /// palette, tab rename, settings page) get a chance to consume it: e.g.
    /// `C-,` must open Settings even while the palette is open. Window
    /// layout actions (split/close/focus/scroll/zoom/tab-cycle) are
    /// deliberately excluded: they only apply once no overlay owns the key.
    pub(super) fn is_global(self) -> bool {
        matches!(
            self,
            WindowAction::Copy
                | WindowAction::CopyCwd
                | WindowAction::Paste
                | WindowAction::NewTab
                | WindowAction::NewWindow
                | WindowAction::CloseTab
                | WindowAction::CloseOtherTabs
                | WindowAction::OpenSettings
                | WindowAction::FontIncrease
                | WindowAction::FontDecrease
                | WindowAction::FontReset
                | WindowAction::TogglePalette
                | WindowAction::ToggleHistoryPalette
                | WindowAction::TogglePaneSwitcher
                | WindowAction::ToggleSwoop
                | WindowAction::Jump
                | WindowAction::NextBlock
                | WindowAction::PrevBlock
        )
    }
}
/// Which digit-indexed pane/tab operation an [`IndexAction`] performs.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum IndexKind {
    ClosePane,
    FocusPane,
    GotoTab,
}
/// A chord bound to one *specific* pane or tab index (e.g. config action
/// `focus_pane_3` always focuses pane 3, whichever key triggers it).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct IndexAction {
    pub(super) kind: IndexKind,
    pub(super) n: usize,
}
impl IndexAction {
    pub(super) fn to_action(self) -> Action {
        match self.kind {
            IndexKind::ClosePane => Action::ClosePaneByIndex(self.n),
            IndexKind::FocusPane => Action::FocusPaneByIndex(self.n),
            IndexKind::GotoTab => Action::GotoTab(self.n),
        }
    }

    /// Parse a config action name like `close_pane_3`, `focus_pane_3`, or
    /// `goto_tab_3` (`n` must be `1..=9`).
    pub(super) fn from_name(name: &str) -> Option<Self> {
        let (kind, rest) = if let Some(rest) = name.strip_prefix("close_pane_") {
            (IndexKind::ClosePane, rest)
        } else if let Some(rest) = name.strip_prefix("focus_pane_") {
            (IndexKind::FocusPane, rest)
        } else if let Some(rest) = name.strip_prefix("goto_tab_") {
            (IndexKind::GotoTab, rest)
        } else {
            return None;
        };
        let n: usize = rest.parse().ok()?;
        (1..=9).contains(&n).then_some(IndexAction { kind, n })
    }
}

// ========================================================================
// WindowAction
// ========================================================================

impl WindowAction {
    /// The dispatched [`Action`] this window command produces.
    pub(super) fn to_action(self) -> Action {
        match self {
            WindowAction::Close => Action::ClosePane,
            WindowAction::CloseOthers => Action::CloseOtherPanes,
            WindowAction::FocusDown => Action::FocusPane(FocusDir::Down),
            WindowAction::FocusLeft => Action::FocusPane(FocusDir::Left),
            WindowAction::FocusRight => Action::FocusPane(FocusDir::Right),
            WindowAction::FocusUp => Action::FocusPane(FocusDir::Up),
            WindowAction::MoveTabDown => Action::MoveTabToPane(FocusDir::Down),
            WindowAction::MoveTabLeft => Action::MoveTabToPane(FocusDir::Left),
            WindowAction::MoveTabRight => Action::MoveTabToPane(FocusDir::Right),
            WindowAction::MoveTabUp => Action::MoveTabToPane(FocusDir::Up),
            WindowAction::SplitHorizontal => Action::SplitPane(Direction::Horizontal),
            WindowAction::SplitVertical => Action::SplitPane(Direction::Vertical),
            WindowAction::Zoom => Action::ZoomPane,
            WindowAction::ScrollPageUp => Action::ScrollPageUp,
            WindowAction::ScrollPageDown => Action::ScrollPageDown,
            WindowAction::ScrollLineUp => Action::ScrollLineUp,
            WindowAction::ScrollLineDown => Action::ScrollLineDown,
            WindowAction::ScrollToTop => Action::ScrollToTop,
            WindowAction::ScrollToBottom => Action::ScrollToBottom,
            WindowAction::PrevTab => Action::PrevTab,
            WindowAction::NextTab => Action::NextTab,
            WindowAction::Copy => Action::Copy,
            WindowAction::CopyCwd => Action::CopyCwd,
            WindowAction::Paste => Action::Paste,
            WindowAction::NewTab => Action::NewTab,
            WindowAction::NewWindow => Action::NewWindow,
            WindowAction::CloseTab => Action::CloseTab(None),
            WindowAction::CloseOtherTabs => Action::CloseOtherTabs,
            WindowAction::OpenSettings => Action::OpenSettings,
            WindowAction::FontIncrease => Action::IncreaseFontSize,
            WindowAction::FontDecrease => Action::DecreaseFontSize,
            WindowAction::FontReset => Action::ResetFontSize,
            WindowAction::TogglePalette => Action::TogglePalette,
            WindowAction::ToggleHistoryPalette => Action::ToggleHistoryPalette,
            WindowAction::TogglePaneSwitcher => Action::TogglePaneSwitcher,
            WindowAction::ToggleSwoop => Action::ToggleSwoop,
            WindowAction::Jump => Action::Jump,
            WindowAction::NextBlock => Action::FocusBlock(BlockNav::Next),
            WindowAction::PrevBlock => Action::FocusBlock(BlockNav::Previous),
        }
    }

    /// Parse a config action name (matching the command-palette names).
    pub(super) fn from_name(name: &str) -> Option<WindowAction> {
        Some(match name {
            "close_pane" => WindowAction::Close,
            "close_other_panes" => WindowAction::CloseOthers,
            "focus_down" => WindowAction::FocusDown,
            "focus_left" => WindowAction::FocusLeft,
            "focus_right" => WindowAction::FocusRight,
            "focus_up" => WindowAction::FocusUp,
            "move_tab_down" | "move_tab_down_pane" | "move_tab_to_down_pane" => {
                WindowAction::MoveTabDown
            }
            "move_tab_left" | "move_tab_left_pane" | "move_tab_to_left_pane" => {
                WindowAction::MoveTabLeft
            }
            "move_tab_right" | "move_tab_right_pane" | "move_tab_to_right_pane" => {
                WindowAction::MoveTabRight
            }
            "move_tab_up" | "move_tab_up_pane" | "move_tab_to_up_pane" => WindowAction::MoveTabUp,
            "split_horizontal" => WindowAction::SplitHorizontal,
            "split_vertical" => WindowAction::SplitVertical,
            "toggle_pane_zoom" | "zoom_pane" => WindowAction::Zoom,
            "scroll_page_up" => WindowAction::ScrollPageUp,
            "scroll_page_down" => WindowAction::ScrollPageDown,
            "scroll_line_up" => WindowAction::ScrollLineUp,
            "scroll_line_down" => WindowAction::ScrollLineDown,
            "scroll_to_top" => WindowAction::ScrollToTop,
            "scroll_to_bottom" => WindowAction::ScrollToBottom,
            "prev_tab" => WindowAction::PrevTab,
            "next_tab" => WindowAction::NextTab,
            "copy_selection" => WindowAction::Copy,
            "copy_cwd" | "copy_reference" | "copy_file_reference" => WindowAction::CopyCwd,
            "paste_from_clipboard" => WindowAction::Paste,
            "new_tab" => WindowAction::NewTab,
            "new_window" => WindowAction::NewWindow,
            "close_tab" => WindowAction::CloseTab,
            "close_other_tabs" => WindowAction::CloseOtherTabs,
            "open_settings" => WindowAction::OpenSettings,
            "font_increase" => WindowAction::FontIncrease,
            "font_decrease" => WindowAction::FontDecrease,
            "font_reset" => WindowAction::FontReset,
            "toggle_command_palette" => WindowAction::TogglePalette,
            "toggle_history_palette" => WindowAction::ToggleHistoryPalette,
            "select_pane" => WindowAction::TogglePaneSwitcher,
            "swoop" | "toggle_swoop" => WindowAction::ToggleSwoop,
            "jump" | "toggle_jump" => WindowAction::Jump,
            "next_block" => WindowAction::NextBlock,
            "prev_block" => WindowAction::PrevBlock,
            _ => return None,
        })
    }
}
/// The dispatched action for a config action name (`copy_selection`, …) that
/// mirrors a window command, or `None` when the name names none. Shared by
/// the keymap parser and the command palette so both dispatch identically.
pub(crate) fn window_action_by_name(name: &str) -> Option<Action> {
    WindowAction::from_name(name).map(WindowAction::to_action)
}
