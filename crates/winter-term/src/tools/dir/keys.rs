//! The listing's keys: plain, with a modifier, and the second key of a
//! two-key sequence, each written once in a table the dispatch, the help
//! list, and the right-click menu all read.

use crate::model::input::table::{self, Binding, KeyHelp, TableMatch};
use crate::model::input::Key;
use crate::model::tool::{menu_groups, ToolMenuItem, ToolOutcome};
use crate::model::vim::nav::{VimKey, VimNav};

use super::{DirTool, ASK_COPY, ASK_MKDIR, ASK_MOVE, ASK_NEW_FILE};

// ========================================================================
// Constants
// ========================================================================

/// The listing's own keys, ahead of the shared motions.
pub(super) const KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Enter,
        keys: &["RET"],
        label: "Open",
    },
    Binding {
        action: Act::OpenInEditor,
        keys: &["C-o"],
        label: "Open in $EDITOR",
    },
    Binding {
        action: Act::OpenExternal,
        keys: &["&"],
        label: "Open With System Handler",
    },
    Binding {
        action: Act::Expand,
        keys: &["l", "<right>"],
        label: "Open the directory in place",
    },
    Binding {
        action: Act::FoldOrStepOut,
        keys: &["h", "<left>"],
        label: "Close it, else step out to the parent",
    },
    Binding {
        action: Act::Ascend,
        keys: &["DEL"],
        label: "Leave the root for its parent",
    },
    Binding {
        action: Act::ToggleFold,
        keys: &["TAB"],
        label: "Fold the entry",
    },
    Binding {
        action: Act::ToggleFoldAll,
        keys: &["S-TAB", "z a", "z A"],
        label: "Fold everything",
    },
    Binding {
        action: Act::CollapseAll,
        keys: &["z c"],
        label: "Close everything",
    },
    Binding {
        action: Act::ExpandSubtree,
        keys: &["z u", "z d"],
        label: "Open the whole subtree",
    },
    Binding {
        action: Act::CollapseSubtree,
        keys: &["z f"],
        label: "Close the whole subtree",
    },
    Binding {
        action: Act::ToggleSubtree,
        keys: &["z t"],
        label: "Toggle the whole subtree",
    },
    Binding {
        action: Act::Depth(0),
        keys: &["C-c 0"],
        label: "Open the tree to depth 0",
    },
    Binding {
        action: Act::Depth(1),
        keys: &["C-c 1"],
        label: "Open the tree to depth 1",
    },
    Binding {
        action: Act::Depth(2),
        keys: &["C-c 2"],
        label: "Open the tree to depth 2",
    },
    Binding {
        action: Act::Depth(3),
        keys: &["C-c 3"],
        label: "Open the tree to depth 3",
    },
    Binding {
        action: Act::Depth(4),
        keys: &["C-c 4"],
        label: "Open the tree to depth 4",
    },
    Binding {
        action: Act::Depth(5),
        keys: &["C-c 5"],
        label: "Open the tree to depth 5",
    },
    Binding {
        action: Act::Depth(6),
        keys: &["C-c 6"],
        label: "Open the tree to depth 6",
    },
    Binding {
        action: Act::Depth(7),
        keys: &["C-c 7"],
        label: "Open the tree to depth 7",
    },
    Binding {
        action: Act::Depth(8),
        keys: &["C-c 8"],
        label: "Open the tree to depth 8",
    },
    Binding {
        action: Act::Depth(9),
        keys: &["C-c 9"],
        label: "Open the tree to depth 9",
    },
    Binding {
        action: Act::Search,
        keys: &["/"],
        label: "Search the listing",
    },
    Binding {
        action: Act::NextMatch,
        keys: &["n"],
        label: "Next match",
    },
    Binding {
        action: Act::PreviousMatch,
        keys: &["N"],
        label: "Previous match",
    },
    Binding {
        action: Act::NextSibling,
        keys: &["M-n"],
        label: "Next entry at the same level",
    },
    Binding {
        action: Act::PreviousSibling,
        keys: &["M-p"],
        label: "Previous entry at the same level",
    },
    Binding {
        action: Act::Parent,
        keys: &["M-u"],
        label: "Move to the parent",
    },
    Binding {
        action: Act::Child,
        keys: &["M-d"],
        label: "Move into an open directory",
    },
    Binding {
        action: Act::Back,
        keys: &["M-S-b"],
        label: "Back through directories visited",
    },
    Binding {
        action: Act::Forward,
        keys: &["M-S-f"],
        label: "Forward through directories visited",
    },
    Binding {
        action: Act::Hidden,
        keys: &["."],
        label: "Show or hide dotfiles",
    },
    Binding {
        action: Act::Details,
        keys: &[","],
        label: "Show or hide the size, age, and permission columns",
    },
    Binding {
        action: Act::Sizes,
        keys: &["M-S-s"],
        label: "Show directory sizes, or stop the walks",
    },
    // Escape stops the size walks when any are running, since that is the
    // slow thing a listing does; with none running it closes.
    Binding {
        action: Act::StopSizes,
        keys: &["ESC"],
        label: "Stop the size walks",
    },
    Binding {
        action: Act::Sort,
        keys: &["s"],
        label: "Cycle the sort: name, time, size",
    },
    // Reload is `C-r` in every tool, so `G` is left to mean the bottom.
    Binding {
        action: Act::Reload,
        keys: &["C-r"],
        label: "Reload",
    },
    Binding {
        action: Act::Mark,
        keys: &["m"],
        label: "Mark",
    },
    // Dired's "toggle marks" key, leaving `M` to the shared layer's
    // screen-middle beside `H` and `L`.
    Binding {
        action: Act::MarkAll,
        keys: &["t"],
        label: "Mark everything listed",
    },
    Binding {
        action: Act::Unmark,
        keys: &["u"],
        label: "Unmark",
    },
    Binding {
        action: Act::UnmarkAll,
        keys: &["U"],
        label: "Unmark everything",
    },
    Binding {
        action: Act::NewFile,
        keys: &["_"],
        label: "New File...",
    },
    Binding {
        action: Act::NewDir,
        keys: &["+"],
        label: "New Directory...",
    },
    Binding {
        action: Act::Rename,
        keys: &["R"],
        label: "Rename...",
    },
    Binding {
        action: Act::Copy,
        keys: &["C"],
        label: "Copy...",
    },
    Binding {
        action: Act::Move,
        keys: &["M-m"],
        label: "Move...",
    },
    Binding {
        action: Act::Trash,
        keys: &["x"],
        label: "Move to Trash...",
    },
    Binding {
        action: Act::Delete,
        keys: &["X"],
        label: "Delete for Good...",
    },
    Binding {
        action: Act::Mode,
        keys: &["*"],
        label: "Set permission bits, as octal",
    },
    Binding {
        action: Act::Edit,
        keys: &["C-x C-q"],
        label: "Edit the names as text",
    },
];

/// The keys while the names are editable text, ahead of the editor's.
pub(super) const EDIT_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Edit,
        keys: &["C-x C-q"],
        label: "Throw the renames away",
    },
    Binding {
        action: Act::Discard,
        keys: &["q"],
        label: "Throw the renames away, with nothing part-typed",
    },
];

/// What the menu opened below the listing offers: the things that make a new
/// entry.
const EMPTY_MENU: [Act; 3] = [Act::NewFile, Act::NewDir, Act::Reload];

// ========================================================================
// Data Structures
// ========================================================================

/// What a key of the listing's own does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Act {
    Ascend,
    Back,
    Child,
    CollapseAll,
    CollapseSubtree,
    Copy,
    Delete,
    /// Open the tree to this depth.
    Depth(usize),
    Details,
    Discard,
    Edit,
    Enter,
    Expand,
    ExpandSubtree,
    FoldOrStepOut,
    Forward,
    Hidden,
    Mark,
    MarkAll,
    Mode,
    Move,
    NewDir,
    NewFile,
    NextMatch,
    NextSibling,
    OpenExternal,
    OpenInEditor,
    Parent,
    PreviousMatch,
    PreviousSibling,
    Reload,
    Rename,
    Search,
    Sizes,
    Sort,
    StopSizes,
    ToggleFold,
    ToggleFoldAll,
    ToggleSubtree,
    Trash,
    Unmark,
    UnmarkAll,
}

// ========================================================================
// DirTool: keys
// ========================================================================

impl DirTool {
    /// One key over the listing: an edit session, a sequence part-way
    /// through, and the shared motions all get it before the listing's own.
    pub(super) fn dispatch_key(&mut self, key: &Key) -> ToolOutcome {
        self.list.view_mut().release();
        if self.edit.is_some() {
            return self.on_edit_key(key);
        }
        // An unrecognized follow key abandons the sequence rather than
        // holding it for the next keystroke.
        if let Some(first) = self.pending.take() {
            if let TableMatch::Exact(act) = table::lookup(KEYS, &[first, *key]) {
                self.on_act(act);
            }
            return ToolOutcome::Consumed;
        }
        if self.nav.in_sequence() {
            return match self.nav.key(key) {
                VimKey::Motion(motion) => {
                    self.apply_motion(motion);
                    ToolOutcome::Consumed
                }
                _ => ToolOutcome::Consumed,
            };
        }
        match table::lookup(KEYS, std::slice::from_ref(key)) {
            TableMatch::Exact(act) => {
                if let Some(outcome) = self.on_act(act) {
                    return outcome;
                }
            }
            TableMatch::Prefix => {
                self.pending = Some(*key);
                return ToolOutcome::Consumed;
            }
            TableMatch::Miss => {}
        }
        // Everything unclaimed falls through to the shared Vim motion layer,
        // whose motions this listing interprets over its rows; the window
        // keeps the rest.
        match self.nav.key(key) {
            VimKey::Motion(motion) => {
                self.apply_motion(motion);
                ToolOutcome::Consumed
            }
            VimKey::Pending => ToolOutcome::Consumed,
            VimKey::Unhandled => ToolOutcome::Ignored,
        }
    }

    /// Every key the listing answers in the state it is in.
    pub(super) fn key_help(&self) -> Vec<KeyHelp> {
        if self.edit.is_some() {
            return table::help(EDIT_KEYS);
        }
        let mut rows = table::help(KEYS);
        rows.extend(VimNav::help());
        rows
    }

    /// The right-click menu: what can be done with the entry under the
    /// pointer, by the same keys and names the help list shows.
    pub(super) fn menu_items(&self) -> Vec<ToolMenuItem> {
        let entry =
            |act| table::menu_entry(KEYS, act).map(|(key, label)| ToolMenuItem::new(key, label));
        let Some(row) = self.selected() else {
            // Below the listing there is no entry to act on, so the only
            // things left are the ones that make a new one.
            return EMPTY_MENU.into_iter().filter_map(entry).collect();
        };
        let mut open = vec![Act::Enter];
        if !row.entry.is_dir() {
            open.extend([Act::OpenInEditor, Act::OpenExternal]);
        }
        let mark = match self.marks.contains(&row.entry.path) {
            true => Act::Unmark,
            false => Act::Mark,
        };
        let group = |acts: Vec<Act>| acts.into_iter().filter_map(entry).collect();
        menu_groups([
            group(open),
            group(vec![Act::Rename, Act::Copy, Act::Move]),
            group(vec![mark]),
            group(vec![Act::Trash, Act::Delete]),
            group(vec![Act::NewFile, Act::NewDir]),
        ])
    }

    /// Do what a key of the listing's own asks. `None` for a key with nothing
    /// to do in the state the listing is in, which then reaches the motion
    /// layer.
    fn on_act(&mut self, act: Act) -> Option<ToolOutcome> {
        let cursor = self.list.cursor();
        match act {
            Act::Enter => return Some(self.enter()),
            Act::OpenInEditor => return Some(self.open_external()),
            Act::OpenExternal => {
                return Some(
                    self.selected()
                        .map(|row| ToolOutcome::OpenExternal(row.entry.path.clone()))
                        .unwrap_or(ToolOutcome::Consumed),
                )
            }
            Act::Expand => self.expand_selected(true),
            Act::FoldOrStepOut => self.fold_or_step_out(),
            Act::Ascend => self.ascend(),
            Act::ToggleFold => self.toggle_fold(),
            Act::ToggleFoldAll => self.toggle_fold_all(),
            Act::CollapseAll => {
                self.folds.collapse_all();
                self.reload();
            }
            Act::ExpandSubtree => self.expand_subtree(),
            Act::CollapseSubtree => self.collapse_subtree(),
            Act::ToggleSubtree => self.toggle_subtree(),
            Act::Depth(depth) => self.expand_to_depth(depth),
            Act::Search => return Some(self.ask_search()),
            Act::NextMatch => return Some(self.search_step(true)),
            Act::PreviousMatch => return Some(self.search_step(false)),
            Act::NextSibling => self.move_to(self.sibling_after(cursor)),
            Act::PreviousSibling => self.move_to(self.sibling_before(cursor)),
            Act::Parent => self.move_to(self.parent_row(cursor)),
            Act::Child => self.move_to(self.first_child(cursor)),
            Act::Back => self.go_back(),
            Act::Forward => self.go_forward(),
            Act::Hidden => {
                self.show_hidden = !self.show_hidden;
                self.reload();
            }
            Act::Details => self.show_details = !self.show_details,
            Act::Sizes => return Some(self.toggle_sizes()),
            Act::StopSizes if self.show_sizes => return Some(self.toggle_sizes()),
            Act::StopSizes => return None,
            Act::Sort => {
                self.sort = self.sort.next();
                self.reload();
            }
            Act::Reload => self.reload(),
            Act::Mark => self.toggle_mark(),
            Act::MarkAll => self.mark_all(),
            Act::Unmark => self.unmark_selected(),
            Act::UnmarkAll => self.marks.clear(),
            Act::NewFile => {
                return Some(self.ask(ASK_NEW_FILE, "New file: ".to_string(), String::new()))
            }
            Act::NewDir => {
                return Some(self.ask(ASK_MKDIR, "New directory: ".to_string(), String::new()))
            }
            Act::Rename => return Some(self.ask_rename()),
            Act::Copy => return Some(self.ask_destination(ASK_COPY, "Copy")),
            Act::Move => return Some(self.ask_destination(ASK_MOVE, "Move")),
            Act::Trash => return Some(self.ask_delete(false)),
            Act::Delete => return Some(self.ask_delete(true)),
            Act::Mode => return Some(self.ask_mode()),
            Act::Edit => self.enter_edit(),
            // Only an edit session answers it, before this is reached.
            Act::Discard => return None,
        }
        Some(ToolOutcome::Consumed)
    }
}
