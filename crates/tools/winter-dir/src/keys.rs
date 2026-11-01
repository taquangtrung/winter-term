//! The listing's keys: plain, with a modifier, and the second key of a
//! two-key sequence, each written once in a table the dispatch, the help
//! list, and the right-click menu all read.

use std::path::PathBuf;

use winter_model::background::Background;
use winter_model::editor::{
    menu_groups, present, CompareRequest, DiffText, EditorMenuItem, EditorOutcome, JobRequest,
    Notice,
};
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed, TableMatch};
use winter_model::input::{transient_key, Key, TransientKey};
use winter_model::vim::nav::{VimKey, VimNav};
use winter_render::{TransientCommand, TransientView};

use super::archive::is_browseable_archive;
use super::{vcs, DirEditor, ASK_COPY, ASK_GIT_MOVE, ASK_MKDIR, ASK_MOVE, ASK_NEW_FILE};

// ========================================================================
// Constants
// ========================================================================

/// The thread files marked for the diff tool are read on.
const COMPARE_READ_THREAD: &str = "winter compare read";

/// The listing's own keys, ahead of the shared motions.
pub(super) const KEYS: KeyTable<Act> = KeyTable {
    block: "dir",
    bindings: &[
        Binding {
            action: Act::Enter,
            id: "enter",
            keys: &["RET"],
            label: "Open",
        },
        Binding {
            action: Act::OpenInEditor,
            id: "open_in_editor",
            keys: &["C-o"],
            label: "Open in $EDITOR",
        },
        Binding {
            action: Act::OpenExternal,
            id: "open_external",
            keys: &["&"],
            label: "Open With System Handler",
        },
        Binding {
            action: Act::OpenInHex,
            id: "open_in_hex",
            keys: &["H"],
            label: "Open in Hex Editor",
        },
        // Dired's archive keys.
        Binding {
            action: Act::ToggleArchive,
            id: "toggle_archive",
            keys: &["Z"],
            label: "Extract an archive here, or compress a file or directory",
        },
        Binding {
            action: Act::CompressTo,
            id: "compress_to",
            keys: &["c"],
            label: "Compress the marked entries into one archive…",
        },
        Binding {
            action: Act::ActionsMenu,
            id: "actions_menu",
            keys: &["a"],
            label: "Every action for the marked entries, or the one under the cursor",
        },
        Binding {
            action: Act::Compare,
            id: "compare",
            keys: &["="],
            label: "Compare the marked files, or two directories",
        },
        Binding {
            action: Act::Expand,
            id: "expand",
            keys: &["l", "<right>"],
            label: "Open the directory in place",
        },
        Binding {
            action: Act::FoldOrStepOut,
            id: "fold_or_step_out",
            keys: &["h", "<left>"],
            label: "Close it, else step out to the parent",
        },
        Binding {
            action: Act::Ascend,
            id: "ascend",
            keys: &["DEL"],
            label: "Leave the root for its parent",
        },
        Binding {
            action: Act::ToggleFold,
            id: "toggle_fold",
            keys: &["TAB"],
            label: "Fold the entry",
        },
        Binding {
            action: Act::ToggleFoldAll,
            id: "toggle_fold_all",
            keys: &["S-TAB", "z a", "z A"],
            label: "Fold everything",
        },
        Binding {
            action: Act::CollapseAll,
            id: "collapse_all",
            keys: &["z c"],
            label: "Close everything",
        },
        Binding {
            action: Act::ExpandSubtree,
            id: "expand_subtree",
            keys: &["z u", "z d"],
            label: "Open the whole subtree",
        },
        Binding {
            action: Act::CollapseSubtree,
            id: "collapse_subtree",
            keys: &["z f"],
            label: "Close the whole subtree",
        },
        Binding {
            action: Act::ToggleSubtree,
            id: "toggle_subtree",
            keys: &["z t"],
            label: "Toggle the whole subtree",
        },
        Binding {
            action: Act::Depth(0),
            id: "open_tree_depth_0",
            keys: &["C-c 0"],
            label: "Open the tree to depth 0",
        },
        Binding {
            action: Act::Depth(1),
            id: "open_tree_depth_1",
            keys: &["C-c 1"],
            label: "Open the tree to depth 1",
        },
        Binding {
            action: Act::Depth(2),
            id: "open_tree_depth_2",
            keys: &["C-c 2"],
            label: "Open the tree to depth 2",
        },
        Binding {
            action: Act::Depth(3),
            id: "open_tree_depth_3",
            keys: &["C-c 3"],
            label: "Open the tree to depth 3",
        },
        Binding {
            action: Act::Depth(4),
            id: "open_tree_depth_4",
            keys: &["C-c 4"],
            label: "Open the tree to depth 4",
        },
        Binding {
            action: Act::Depth(5),
            id: "open_tree_depth_5",
            keys: &["C-c 5"],
            label: "Open the tree to depth 5",
        },
        Binding {
            action: Act::Depth(6),
            id: "open_tree_depth_6",
            keys: &["C-c 6"],
            label: "Open the tree to depth 6",
        },
        Binding {
            action: Act::Depth(7),
            id: "open_tree_depth_7",
            keys: &["C-c 7"],
            label: "Open the tree to depth 7",
        },
        Binding {
            action: Act::Depth(8),
            id: "open_tree_depth_8",
            keys: &["C-c 8"],
            label: "Open the tree to depth 8",
        },
        Binding {
            action: Act::Depth(9),
            id: "open_tree_depth_9",
            keys: &["C-c 9"],
            label: "Open the tree to depth 9",
        },
        Binding {
            action: Act::Search,
            id: "search",
            keys: &["/"],
            label: "Search the listing",
        },
        Binding {
            action: Act::NextMatch,
            id: "next_match",
            keys: &["n"],
            label: "Next match",
        },
        Binding {
            action: Act::PreviousMatch,
            id: "previous_match",
            keys: &["N"],
            label: "Previous match",
        },
        Binding {
            action: Act::NextSibling,
            id: "next_sibling",
            keys: &["M-n"],
            label: "Next entry at the same level",
        },
        Binding {
            action: Act::PreviousSibling,
            id: "previous_sibling",
            keys: &["M-p"],
            label: "Previous entry at the same level",
        },
        Binding {
            action: Act::Parent,
            id: "parent",
            keys: &["M-u"],
            label: "Move to the parent",
        },
        Binding {
            action: Act::Child,
            id: "child",
            keys: &["M-d"],
            label: "Move into an open directory",
        },
        Binding {
            action: Act::Back,
            id: "back",
            keys: &["M-S-b"],
            label: "Back through directories visited",
        },
        Binding {
            action: Act::Forward,
            id: "forward",
            keys: &["M-S-f"],
            label: "Forward through directories visited",
        },
        Binding {
            action: Act::Hidden,
            id: "hidden",
            keys: &["."],
            label: "Show or hide dotfiles",
        },
        Binding {
            action: Act::Details,
            id: "details",
            keys: &[","],
            label: "Show or hide the size, age, and permission columns",
        },
        Binding {
            action: Act::Sizes,
            id: "sizes",
            keys: &["M-S-s"],
            label: "Show directory sizes, or stop the walks",
        },
        // Escape stops the size walks when any are running, since that is the
        // slow thing a listing does; with none running it closes.
        Binding {
            action: Act::StopSizes,
            id: "stop_sizes",
            keys: &["ESC"],
            label: "Stop the size walks",
        },
        Binding {
            action: Act::Sort,
            id: "sort",
            keys: &["s"],
            label: "Cycle the sort: name, time, size",
        },
        // Reload is `C-r` in every editor, so `G` is left to mean the bottom.
        Binding {
            action: Act::Reload,
            id: "reload",
            keys: &["C-r"],
            label: "Reload",
        },
        Binding {
            action: Act::Mark,
            id: "mark",
            keys: &["m"],
            label: "Mark",
        },
        // Dired's "toggle marks" key, leaving `M` to the shared layer's
        // screen-middle beside `H` and `L`.
        Binding {
            action: Act::MarkAll,
            id: "mark_all",
            keys: &["t"],
            label: "Mark everything listed",
        },
        Binding {
            action: Act::Unmark,
            id: "unmark",
            keys: &["u"],
            label: "Unmark",
        },
        Binding {
            action: Act::UnmarkAll,
            id: "unmark_all",
            keys: &["U"],
            label: "Unmark everything",
        },
        Binding {
            action: Act::NewFile,
            id: "new_file",
            keys: &["_"],
            label: "New File…",
        },
        Binding {
            action: Act::NewDir,
            id: "new_dir",
            keys: &["+"],
            label: "New Directory…",
        },
        Binding {
            action: Act::Rename,
            id: "rename",
            keys: &["R"],
            label: "Rename…",
        },
        Binding {
            action: Act::Copy,
            id: "copy",
            keys: &["C"],
            label: "Copy…",
        },
        Binding {
            action: Act::Move,
            id: "move",
            keys: &["M-m"],
            label: "Move…",
        },
        Binding {
            action: Act::Trash,
            id: "trash",
            keys: &["x"],
            label: "Move to Trash…",
        },
        Binding {
            action: Act::Delete,
            id: "delete",
            keys: &["X"],
            label: "Delete for Good…",
        },
        Binding {
            action: Act::Mode,
            id: "mode",
            keys: &["*"],
            label: "Set permission bits, as octal",
        },
        Binding {
            action: Act::ToggleExecutable,
            id: "toggle_executable",
            keys: &["C-c x"],
            label: "Toggle executable permission (+x/-x)",
        },
        Binding {
            action: Act::Edit,
            id: "edit",
            keys: &["C-x C-q"],
            label: "Toggle edit mode (edit names as text)",
        },
    ],
    configurable: true,
    title: "Dir",
};

/// The keys while the names are editable text, ahead of the editor's.
pub(super) const EDIT_KEYS: KeyTable<Act> = KeyTable {
    block: "dir.edit",
    bindings: &[
        Binding {
            action: Act::Edit,
            id: "edit",
            keys: &["C-x C-q", "C-S-c"],
            label: "Save renames and exit edit mode",
        },
        Binding {
            action: Act::ThrowAway,
            id: "throw_away",
            keys: &["C-S-k"],
            label: "Throw the renames away",
        },
        Binding {
            action: Act::Discard,
            id: "discard",
            keys: &["q"],
            label: "Throw the renames away, with nothing part-typed",
        },
    ],
    configurable: true,
    title: "Dir: Edit Mode",
};

/// Every key table of this surface, for the Keys editor to list.
/// The actions menu's keys: one letter for each thing that can be done to
/// the marked entries, or the one under the cursor. The menu offers those
/// that apply (opening only one entry, extracting only an archive), each
/// running what the listing's own key runs.
pub(super) const ACTION_KEYS: KeyTable<Act> = KeyTable {
    block: "dir.actions",
    bindings: &[
        Binding {
            action: Act::Enter,
            id: "open",
            keys: &["o"],
            label: "Open (or enter the directory)",
        },
        Binding {
            action: Act::OpenExternal,
            id: "open_external",
            keys: &["e"],
            label: "Open in its own application",
        },
        Binding {
            action: Act::OpenInHex,
            id: "open_in_hex",
            keys: &["h"],
            label: "Open in the hex editor",
        },
        Binding {
            action: Act::Rename,
            id: "rename",
            keys: &["r"],
            label: "Rename\u{2026}",
        },
        Binding {
            action: Act::Copy,
            id: "copy",
            keys: &["c"],
            label: "Copy\u{2026}",
        },
        Binding {
            action: Act::Move,
            id: "move",
            keys: &["m"],
            label: "Move\u{2026}",
        },
        Binding {
            action: Act::Trash,
            id: "trash",
            keys: &["d"],
            label: "Move to the trash\u{2026}",
        },
        Binding {
            action: Act::Delete,
            id: "delete",
            keys: &["D"],
            label: "Delete for good\u{2026}",
        },
        Binding {
            action: Act::Mode,
            id: "permissions",
            keys: &["p"],
            label: "Change permissions (chmod)\u{2026}",
        },
        Binding {
            action: Act::ToggleArchive,
            id: "extract",
            keys: &["u"],
            label: "Extract the archive here",
        },
        Binding {
            action: Act::CompressTo,
            id: "compress",
            keys: &["z"],
            label: "Compress into an archive\u{2026}",
        },
        Binding {
            action: Act::Compare,
            id: "compare",
            keys: &["="],
            label: "Compare the marked files",
        },
        Binding {
            action: Act::GitMove,
            id: "git_move",
            keys: &["M"],
            label: "Git: move, the index following (git mv)\u{2026}",
        },
        Binding {
            action: Act::GitForceAdd,
            id: "git_force_add",
            keys: &["a"],
            label: "Git: add though ignored (git add -f)",
        },
    ],
    configurable: true,
    title: "Dir: Actions",
};

/// Every key table of this surface, for the Keys editor to list.
pub const KEY_TABLES: &[&dyn Listed] = &[&KEYS, &EDIT_KEYS, &ACTION_KEYS];

/// What the menu opened below the listing offers: the things that make a new
/// entry.
const EMPTY_MENU: [Act; 3] = [Act::NewFile, Act::NewDir, Act::Reload];

// ========================================================================
// Data Structures
// ========================================================================

/// What a key of the listing's own does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Act {
    /// Open the actions menu for the marked entries, or the one under the
    /// cursor.
    ActionsMenu,
    Ascend,
    Back,
    Child,
    CollapseAll,
    CollapseSubtree,
    /// Show the differences between the two marked entries.
    Compare,
    /// Put the targets into one archive, named when asked.
    CompressTo,
    /// Throw the renames away, whatever is part-typed.
    ThrowAway,
    /// Extract each archive target, or compress each other one.
    ToggleArchive,
    Copy,
    Delete,
    /// Open the tree to this depth.
    Depth(usize),
    Details,
    /// Throw the renames away, with nothing part-typed: `q`, which may
    /// otherwise be a letter of a name.
    Discard,
    Edit,
    Enter,
    Expand,
    ExpandSubtree,
    FoldOrStepOut,
    Forward,
    /// `git add -f` the targets: add them though ignored.
    GitForceAdd,
    /// `git mv` the targets, the index following them.
    GitMove,
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
    OpenInHex,
    Parent,
    PreviousMatch,
    PreviousSibling,
    Reload,
    Rename,
    Search,
    Sizes,
    Sort,
    StopSizes,
    ToggleExecutable,
    ToggleFold,
    ToggleFoldAll,
    ToggleSubtree,
    Trash,
    Unmark,
    UnmarkAll,
}

// ========================================================================
// DirEditor: keys
// ========================================================================

impl DirEditor {
    /// One key over the listing: an edit session, a sequence part-way
    /// through, and the shared motions all get it before the listing's own.
    pub(super) fn dispatch_key(&mut self, key: &Key) -> EditorOutcome {
        self.list.view_mut().release();
        if self.edit.is_some() {
            return self.on_edit_key(key);
        }
        // An unrecognized follow key abandons the sequence rather than
        // holding it for the next keystroke.
        if let Some(first) = self.pending.take() {
            if let TableMatch::Exact(act) = table::lookup(&KEYS, &[first, *key]) {
                self.on_act(act);
            }
            return EditorOutcome::Consumed;
        }
        if self.nav.in_sequence() {
            return match self.nav.row_key(key) {
                VimKey::Motion(motion) => {
                    self.apply_motion(motion);
                    EditorOutcome::Consumed
                }
                _ => EditorOutcome::Consumed,
            };
        }
        match table::lookup(&KEYS, std::slice::from_ref(key)) {
            TableMatch::Exact(act) => {
                if let Some(outcome) = self.on_act(act) {
                    return outcome;
                }
            }
            TableMatch::Prefix => {
                self.pending = Some(*key);
                return EditorOutcome::Consumed;
            }
            TableMatch::Miss => {}
        }
        // Everything unclaimed falls through to the shared Vim motion layer,
        // whose motions this listing interprets over its rows; the window
        // keeps the rest.
        match self.nav.row_key(key) {
            VimKey::Motion(motion) => {
                self.apply_motion(motion);
                EditorOutcome::Consumed
            }
            VimKey::Pending => EditorOutcome::Consumed,
            VimKey::Unhandled => EditorOutcome::Ignored,
        }
    }

    /// Every key the listing answers in the state it is in.
    pub(super) fn key_help(&self) -> Vec<KeyHelp> {
        if self.edit.is_some() {
            return table::help(&EDIT_KEYS);
        }
        let mut rows = table::help(&KEYS);
        rows.extend(VimNav::help());
        rows
    }

    /// The right-click menu: what can be done with the entry under the
    /// pointer, by the same keys the help list shows, under names shorter
    /// than its sentences. The view's own entries close it.
    pub(super) fn menu_items(&self) -> Vec<EditorMenuItem> {
        let entry = |act| EditorMenuItem::bound(&KEYS, act);
        let named = |act, label| EditorMenuItem::bound_as(&KEYS, act, label);
        let view = present([
            named(Act::Hidden, "Show Hidden Files"),
            named(Act::Sort, "Change Sort Order"),
            entry(Act::Reload),
        ]);
        let Some(row) = self.selected() else {
            // Below the listing there is no entry to act on, so the only
            // things left are the ones that make a new one.
            let new = present(
                EMPTY_MENU
                    .into_iter()
                    .filter(|act| *act != Act::Reload)
                    .map(entry),
            );
            return menu_groups([new, view]);
        };
        let file = !row.entry.is_dir();
        let mut open = vec![entry(Act::Enter)];
        if file {
            open.extend([
                entry(Act::OpenInEditor),
                entry(Act::OpenInHex),
                entry(Act::OpenExternal),
            ]);
        }
        let mark = match self.marks.contains(&row.entry.path) {
            true => entry(Act::Unmark),
            false => entry(Act::Mark),
        };
        // Comparing needs two to compare.
        let compare = named(Act::Compare, "Compare Marked").filter(|_| self.marks.len() >= 2);
        menu_groups([
            present([named(Act::ActionsMenu, "Actions\u{2026}")]),
            present(open),
            present([entry(Act::Rename), entry(Act::Copy), entry(Act::Move)]),
            present([
                mark,
                compare,
                named(Act::ToggleArchive, "Extract or Compress"),
            ]),
            present([entry(Act::Trash), entry(Act::Delete)]),
            present([entry(Act::NewFile), entry(Act::NewDir)]),
            view,
        ])
    }

    /// Compare the entries the operation would act on, the first listed as
    /// the side the changes come from, as Emacs' dired does with `=`: two
    /// files (or three) in the diff tool, where each is written back, or two
    /// directories and everything under them in a diff of the trees.
    fn compare_marked(&mut self) -> EditorOutcome {
        let mut targets = self.targets();
        // Nothing marked, beside one other listing: the two directories.
        let marked = !self.marks.is_empty();
        if !marked {
            if let [other] = self.neighbors.as_slice() {
                targets = vec![self.root.clone(), other.clone()];
            }
        }
        if targets
            .iter()
            .any(|path| winter_base::remote::parse(path).is_some())
        {
            self.notice = Some(Notice::error("compare: only entries on this machine"));
            return EditorOutcome::Consumed;
        }
        let files = targets.iter().all(|path| path.is_file());
        if files && (2..=3).contains(&targets.len()) {
            // Read off the window's thread, as large as the files are; the
            // diff opens once they land (see `take_compared`).
            self.comparing = Some(Background::start(COMPARE_READ_THREAD, move |_| {
                read_sides(&targets)
            }));
            return EditorOutcome::Consumed;
        }
        let [old, new] = targets.as_slice() else {
            self.notice = Some(Notice::error(
                "mark two entries to compare them, or three files",
            ));
            return EditorOutcome::Consumed;
        };
        let name = |path: &std::path::Path| {
            path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            )
        };
        EditorOutcome::Compare(CompareRequest {
            copies: Vec::new(),
            new: new.clone(),
            old: old.clone(),
            title: format!("{} against {}", name(new), name(old)),
        })
    }

    /// The actions the menu offers the marked entries, or the one under the
    /// cursor: every one that applies to them, in the table's order.
    pub(super) fn offered_actions(&self) -> Vec<&'static Binding<Act>> {
        let targets = self.targets();
        let one = match targets.as_slice() {
            [only] => Some(only),
            _ => None,
        };
        let one_file = one.is_some_and(|path| path.is_file());
        let files = targets.iter().all(|path| path.is_file());
        ACTION_KEYS
            .iter()
            .filter(|binding| match binding.action {
                Act::Enter | Act::OpenExternal => one.is_some(),
                Act::OpenInHex => one_file,
                Act::ToggleArchive => one.is_some_and(|path| is_browseable_archive(path)),
                Act::Compare => files && (2..=3).contains(&targets.len()),
                // Only inside a repository, which git found the root of.
                Act::GitMove | Act::GitForceAdd => self.git_top.is_some() && !targets.is_empty(),
                _ => !targets.is_empty(),
            })
            .collect()
    }

    /// Open the actions menu, or say there is nothing to act on.
    fn open_actions_menu(&mut self) -> EditorOutcome {
        if self.offered_actions().is_empty() {
            self.notice = Some(Notice::info("nothing here to act on").unlogged());
            return EditorOutcome::Consumed;
        }
        self.actions_menu = true;
        self.actions_selected = 0;
        EditorOutcome::Consumed
    }

    /// The open actions menu, as a transient window: each action offered,
    /// on the key it is bound to now, the one selected marked.
    pub(super) fn actions_transient(&self) -> Option<TransientView> {
        if !self.actions_menu {
            return None;
        }
        // Each key as the table writes it (`D`, not `S-d`), the user's own
        // where they rebound it.
        let listed = ACTION_KEYS.listed();
        let commands: Vec<TransientCommand> = self
            .offered_actions()
            .iter()
            .filter_map(|binding| {
                let entry = listed.iter().find(|entry| entry.id == binding.id)?;
                Some(TransientCommand::new(
                    entry.keys.first()?.clone(),
                    binding.label,
                ))
            })
            .collect();
        let selected = self.actions_selected.min(commands.len().saturating_sub(1));
        Some(TransientView::new(ACTION_KEYS.title, commands, Vec::new()).with_selected(selected))
    }

    /// A key while the actions menu is open: a letter offered runs its
    /// action; the arrows (or `C-n` and `C-p`) move the selection and `RET`
    /// runs it, as in every transient; `Esc` closes the menu; any other key
    /// says so and leaves it open, so a mistyped letter costs nothing.
    pub(super) fn actions_key(&mut self, key: &Key) -> EditorOutcome {
        let offered = self.offered_actions();
        let chosen = match transient_key(key) {
            Some(TransientKey::Close) => {
                self.actions_menu = false;
                return EditorOutcome::Consumed;
            }
            Some(TransientKey::Next) => {
                self.actions_selected =
                    (self.actions_selected + 1).min(offered.len().saturating_sub(1));
                return EditorOutcome::Consumed;
            }
            Some(TransientKey::Previous) => {
                self.actions_selected = self.actions_selected.saturating_sub(1);
                return EditorOutcome::Consumed;
            }
            Some(TransientKey::Choose) => offered
                .get(self.actions_selected)
                .map(|binding| binding.action),
            None => table::find(&ACTION_KEYS, key)
                .filter(|act| offered.iter().any(|binding| binding.action == *act)),
        };
        let Some(act) = chosen else {
            self.notice =
                Some(Notice::info("no such action here: press one listed, or Esc").unlogged());
            return EditorOutcome::Consumed;
        };
        self.actions_menu = false;
        self.on_act(act).unwrap_or(EditorOutcome::Consumed)
    }

    /// Do what a key of the listing's own asks. `None` for a key with nothing
    /// to do in the state the listing is in, which then reaches the motion
    /// layer.
    fn on_act(&mut self, act: Act) -> Option<EditorOutcome> {
        let cursor = self.list.cursor();
        match act {
            Act::ActionsMenu => return Some(self.open_actions_menu()),
            Act::GitMove => return Some(self.ask_destination(ASK_GIT_MOVE, "Git move")),
            Act::GitForceAdd => {
                let command = vcs::force_add_request(&self.root, &self.targets());
                return Some(EditorOutcome::Job(JobRequest::Command(command)));
            }
            Act::Enter => return Some(self.enter()),
            Act::OpenInEditor => return Some(self.open_external()),
            Act::OpenExternal => {
                return Some(
                    self.selected()
                        .map(|row| EditorOutcome::OpenExternal(row.entry.path.clone()))
                        .unwrap_or(EditorOutcome::Consumed),
                )
            }
            Act::OpenInHex => {
                return Some(
                    self.selected()
                        .filter(|row| !row.entry.is_dir())
                        .map(|row| EditorOutcome::OpenHex(row.entry.path.clone()))
                        .unwrap_or(EditorOutcome::Consumed),
                );
            }
            Act::Compare => return Some(self.compare_marked()),
            Act::ToggleArchive => return Some(self.toggle_archive()),
            Act::CompressTo => return Some(self.ask_compress()),
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
            Act::Hidden => self.toggle_hidden(),
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
            Act::ToggleExecutable => return Some(self.toggle_executable()),
            Act::Edit => return Some(self.toggle_edit()),
            // Only an edit session answers these, before this is reached.
            Act::Discard | Act::ThrowAway => return None,
        }
        Some(EditorOutcome::Consumed)
    }
}

/// Each of `paths` read as a side of a diff, written back to its file in the
/// file's own shape; what is wrong with the first that cannot be read.
fn read_sides(paths: &[PathBuf]) -> Result<Vec<DiffText>, String> {
    paths
        .iter()
        .map(|path| {
            winter_model::text_file::load(path)
                .map(|loaded| DiffText {
                    label: winter_model::path::abbreviate_home(path),
                    lines: loaded.lines,
                    path: Some(path.clone()),
                    shape: Some(loaded.shape),
                })
                .map_err(|e| format!("compare: {}: {e}", path.display()))
        })
        .collect()
}
