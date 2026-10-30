//! Editor tabs: what kind each is, and opening one beside the focused tab.
//! The rest of an editor tab's life lives in the submodules:
//!
//! - [`closing`]: asking before closing what would be lost, and closing.
//! - [`compile`]: builds run in a Compile tab, and the LaTeX ones a save starts.
//! - [`buffers`]: the editors open apart from the tabs showing them, the
//!   buried ones, and reopening a closed tab.
//! - [`prompt`]: questions an editor asks, and the answers typed or picked.
//! - [`keys`]: keys, clicks, and scrolls offered to the editor, and its answers.
//! - [`cursor`]: a Vim-style text cursor over an editor's rows.
//! - [`open_with`]: the focused tab's file, opened in another editor.
//! - [`remote`]: the file browser as root, in a container, or over SSH.

pub(in crate::window) mod buffers;
pub(crate) mod closing;
pub(in crate::window) mod column;
pub(in crate::window) mod compile;
pub(in crate::window) mod copilot;
pub(in crate::window) mod lsp;
pub(crate) mod cursor;
pub(crate) mod keys;
pub(crate) mod open_with;
pub(crate) mod prompt;
pub(crate) mod remote;
pub(in crate::window) mod tab_keys;

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use crate::control::EditWaiter;
use crate::editors::dir::DirEditor;
use crate::editors::git::GitEditor;
use crate::editors::grep::GrepEditor;
use crate::editors::hex::HexEditor;
use crate::editors::image::ImageEditor;
use crate::editors::keys::KeysEditor;
use crate::editors::launcher::LauncherEditor;
use crate::editors::pdf::PdfEditor;
#[cfg(feature = "preview")]
use crate::editors::preview::PreviewEditor;
use crate::editors::docker::DockerEditor;
use crate::editors::proc::ProcEditor;
use crate::editors::sys::SysEditor;
use crate::editors::text::TextEditor;
use crate::model::editor::{CompareRequest, Editor, GitViewRequest, Notice, OpenTarget, PdfMark};
use crate::model::guard::Guarded;
use crate::model::layout::{Direction, DockState, Layout, Rect, TabId};
use crate::model::picker::PickerMode;
use crate::config::Config;
use crate::project;
use crate::session::EditorSession;

pub(in crate::window) use self::closing::{Closing, PendingClose};
pub(in crate::window) use self::prompt::{ActivePick, ActivePrompt};

use super::commands::builtin_commands;
use super::{App, DOCK_SHARE, SPLIT_RATIO};

// ========================================================================
// Constants
// ========================================================================

/// The scratch file's name, beside the settings.
const SCRATCH_FILE: &str = "scratch.md";

// ========================================================================
// Data Structures
// ========================================================================

/// Which kind of editor a tab holds: a peer of the terminal, opened by its own
/// chord.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) enum EditorKind {
    /// A build, its output, and the places it points at.
    Compile,
    Dir,
    /// The containers, images, volumes, and networks docker knows.
    Docker,
    /// Texts compared and merged region by region.
    Diff,
    /// An extension's view, by its kind's full id.
    Extension(&'static str),
    Git,
    Grep,
    /// A file's bytes.
    Hex,
    Image,
    Keys,
    /// What a split opens: the place held for a terminal, a file, a
    /// listing, or the git view.
    Launcher,
    Notifications,
    Pdf,
    /// A Markdown document as it reads rendered.
    Preview,
    Proc,
    Sys,
    /// The text editor, which edits a file's text.
    Text,
    /// The comment keywords (`TODO`, `FIXME`) across a project: a Grep
    /// editor for whole words, run as it opens.
    Todo,
}

/// What the window knows of one built-in kind of editor besides its
/// variant: what it is called, and how it opens.
struct KindInfo {
    /// Whether a buried buffer of the kind comes back with the session.
    kept_buried: bool,
    name: &'static str,
    /// The kind's editor for a file, for "Open With"; `None` for a kind
    /// that does not show one file.
    opens: Option<OpenFile>,
    /// The kind's editor opened again from a saved session; `None` for one
    /// a restart does not bring back. A build is run again by asking for it.
    reopen: Option<Reopen>,
    title: &'static str,
}

/// An editor made for a file.
type OpenFile = fn(PathBuf) -> Box<dyn Editor>;

/// An editor made again from where a session saved it, at a line where it
/// saved one, or `None` where that is gone.
type Reopen = fn(PathBuf, Option<usize>, &Config) -> Option<Box<dyn Editor>>;

// ========================================================================
// EditorKind
// ========================================================================

impl EditorKind {
    /// Every kind, for reading a stored name back.
    const ALL: [EditorKind; 17] = [
        EditorKind::Compile,
        EditorKind::Dir,
        EditorKind::Docker,
        EditorKind::Diff,
        EditorKind::Text,
        EditorKind::Git,
        EditorKind::Grep,
        EditorKind::Hex,
        EditorKind::Image,
        EditorKind::Keys,
        EditorKind::Launcher,
        EditorKind::Notifications,
        EditorKind::Pdf,
        EditorKind::Preview,
        EditorKind::Proc,
        EditorKind::Sys,
        EditorKind::Todo,
    ];

    /// The name a session file records and the icon tables are keyed by.
    pub(in crate::window) fn name(self) -> &'static str {
        match self {
            EditorKind::Extension(id) => id,
            builtin => builtin.info().map_or("", |info| info.name),
        }
    }

    /// The kind a stored name stands for, or `None` for one no longer known.
    pub(in crate::window) fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name).or_else(|| {
            crate::extensions::view(name).map(|view| EditorKind::Extension(view.id.as_str()))
        })
    }

    /// How the title bar and the window title name the kind.
    pub(in crate::window) fn title(self) -> &'static str {
        match self {
            EditorKind::Extension(id) => {
                crate::extensions::view(id).map_or(id, |view| view.title.as_str())
            }
            builtin => builtin.info().map_or("", |info| info.title),
        }
    }

    /// Everything the window knows of a built-in kind, in one place, so a
    /// kind added is described once. `None` for an extension's, which its
    /// extension describes.
    fn info(self) -> Option<KindInfo> {
        let info = match self {
            EditorKind::Compile => KindInfo::new("compile", "Compile"),
            EditorKind::Dir => KindInfo::new("dir", "Dir")
                .with_reopen(|path, _, _| path.is_dir().then(|| boxed(DirEditor::new(path)))),
            EditorKind::Docker => KindInfo::new("docker", "Docker"),
            EditorKind::Diff => KindInfo::new("diff", "Diff"),
            EditorKind::Extension(_) => return None,
            EditorKind::Git => KindInfo::new("git", "Git")
                .with_reopen(|path, _, _| path.is_dir().then(|| boxed(GitEditor::new(path)))),
            EditorKind::Grep => KindInfo::new("grep", "Grep")
                .with_reopen(|path, _, _| path.is_dir().then(|| boxed(GrepEditor::new(path)))),
            EditorKind::Hex => KindInfo::new("hex", "Hex")
                .with_file(|path| boxed(HexEditor::new(path)))
                .with_reopen(|path, _, _| path.is_file().then(|| boxed(HexEditor::new(path)))),
            EditorKind::Image => KindInfo::new("image", "Image")
                .with_file(|path| boxed(ImageEditor::new(path)))
                .with_reopen(|path, _, _| path.is_file().then(|| boxed(ImageEditor::new(path)))),
            EditorKind::Keys => KindInfo::new("keys", "Keys"),
            EditorKind::Launcher => KindInfo::new("launcher", "New Pane")
                .with_reopen(|path, _, _| path.is_dir().then(|| boxed(LauncherEditor::new(path)))),
            EditorKind::Notifications => KindInfo::new("notifications", "Notifications"),
            EditorKind::Pdf => KindInfo::new("pdf", "PDF")
                .with_file(|path| boxed(PdfEditor::new(path)))
                .with_reopen(|path, line, _| path.is_file().then(|| boxed(PdfEditor::at_page(path, line)))),
            EditorKind::Preview => {
                let info = KindInfo::new("preview", "Preview");
                #[cfg(feature = "preview")]
                let info = info.with_reopen(|path, _, _| path.is_file().then(|| boxed(PreviewEditor::new(path))));
                info
            }
            EditorKind::Proc => KindInfo::new("proc", "Processes"),
            EditorKind::Sys => KindInfo::new("sys", "System"),
            // The editor opens a file through its own path, which shows
            // what is no text as its bytes.
            EditorKind::Text => KindInfo {
                kept_buried: true,
                ..KindInfo::new("editor", "Text Editor")
            }
            .with_reopen(|path, line, _| TextEditor::restored(path, line).ok().map(boxed)),
            EditorKind::Todo => KindInfo::new("todo", "Todo").with_reopen(|path, _, config| {
                path.is_dir().then(|| boxed(GrepEditor::todo(path, &config.todo_keywords)))
            }),
        };
        Some(info)
    }
}

// ========================================================================
// KindInfo
// ========================================================================

impl KindInfo {
    /// A kind named `name`, titled `title`, that opens no file and is not
    /// opened again by a restart.
    fn new(name: &'static str, title: &'static str) -> Self {
        Self {
            kept_buried: false,
            name,
            opens: None,
            reopen: None,
            title,
        }
    }

    /// The same, opening a file with `opens`. A kind showing a file on disk
    /// is kept buried by a session.
    fn with_file(self, opens: OpenFile) -> Self {
        Self {
            kept_buried: true,
            opens: Some(opens),
            ..self
        }
    }

    /// The same, opened again from a session by `reopen`.
    fn with_reopen(self, reopen: Reopen) -> Self {
        Self {
            reopen: Some(reopen),
            ..self
        }
    }
}

/// `editor` boxed, as a tab holds it.
fn boxed(editor: impl Editor + 'static) -> Box<dyn Editor> {
    Box::new(editor)
}

/// How a dialog names what is holding unwritten edits: the one thing, or the
/// first of them and how many others there are.
fn named(unsaved: &[String]) -> String {
    match unsaved.split_first() {
        Some((first, [])) => first.clone(),
        Some((first, rest)) => format!("{first} and {} more", rest.len()),
        None => String::new(),
    }
}

// ========================================================================
// App: editors
// ========================================================================

impl App {
    /// Open the keys editor in a tab beside the focused one, or go to the one the
    /// pane already has.
    pub(in crate::window) fn open_keys_editor(&mut self) {
        self.open_single(EditorKind::Keys, |app| {
            boxed(KeysEditor::new(&app.keymap, builtin_commands(&app.keymap)))
        });
    }

    /// Open a directory listing in a tab beside the focused one, rooted where
    /// that tab is looking, or go to the pane's listing already rooted there.
    /// A listing rooted anywhere else is another place, so it is left alone.
    pub(in crate::window) fn open_dir_editor(&mut self) {
        let focused = self.layout().focused();
        // From inside a listing the chord stays put, as every editor's does,
        // rather than opening the folder under its cursor.
        if self
            .open_tabs
            .editor(&focused)
            .is_some_and(|slot| slot.kind == EditorKind::Dir)
        {
            return;
        }
        let dir = self.focused_start_dir();
        let rooted_here = |editor: &dyn Editor| editor.root_dir().as_ref() == Some(&dir);
        if self
            .show_open_editor_matching(EditorKind::Dir, &rooted_here)
            .is_some()
        {
            return;
        }
        self.record_recent_dir(&dir);
        self.show_editor(EditorKind::Dir, Box::new(DirEditor::new(dir)));
    }

    /// List `dir` in a new tab beside the focused one, as a directory opened
    /// from the terminal's text is, or go to the listing of it the pane has or
    /// a buried one.
    pub(in crate::window) fn open_dir_editor_at(&mut self, dir: PathBuf) {
        self.record_recent_dir(&dir);
        let rooted_there = |editor: &dyn Editor| editor.root_dir().as_deref() == Some(dir.as_path());
        if self
            .show_open_editor_matching(EditorKind::Dir, &rooted_there)
            .is_some()
        {
            return;
        }
        self.show_editor(EditorKind::Dir, Box::new(DirEditor::new(dir)));
    }

    /// Open the working tree's state in a tab beside the focused one, for the
    /// repository containing where that tab is looking. If a view for this
    /// repository is already open, it is reused.
    pub(in crate::window) fn open_git_editor(&mut self) {
        let start = self.focused_start_dir();
        let matches_repo = |editor: &dyn Editor| {
            editor.root_dir().is_some_and(|root| {
                start == root || start.starts_with(&root) || root.starts_with(&start)
            })
        };
        // A visible git status view for this repository in any existing pane takes
        // focus and refreshes, rather than opening a second view.
        let visible_status = self.layout().shown_tabs().into_iter().find(|&tab| {
            self.open_tabs.editor(&tab).is_some_and(|slot| {
                slot.kind == EditorKind::Git
                    && matches_repo(slot.editor.as_ref())
                    && slot.editor.with_kind(GitEditor::is_git_status).unwrap_or(false)
            })
        });
        if let Some(tab) = visible_status {
            self.show_tab(tab);
            self.resume_editor(tab);
            return;
        }
        // A visible git editor for this repository (e.g. showing a log or blame)
        // takes focus and returns to its status view.
        let visible_git = self.layout().shown_tabs().into_iter().find(|&tab| {
            self.open_tabs.editor(&tab).is_some_and(|slot| {
                slot.kind == EditorKind::Git
                    && matches_repo(slot.editor.as_ref())
                    && slot.editor.with_kind(GitEditor::is_git_editor).unwrap_or(false)
            })
        });
        if let Some(tab) = visible_git {
            self.show_tab(tab);
            self.show_status_editor(tab);
            return;
        }
        // A view already open is read again: asking for the status is asking
        // for what it is now, which the view's watcher may not have seen (a
        // change under a directory it does not watch, or one made while it
        // was showing output).
        if let Some(tab) = self.show_pane_editor_matching(EditorKind::Git, &matches_repo) {
            self.resume_editor(tab);
            return;
        }
        // Where the view looks is decided by the pane it was asked from, before
        // focus moves to the one it opens in.
        self.prefer_other_pane();
        if let Some(tab) = self.show_pane_editor_matching(EditorKind::Git, &matches_repo) {
            self.resume_editor(tab);
            return;
        }
        if let Some(tab) = self.buried_matching(EditorKind::Git, &matches_repo) {
            self.show_buried_beside(tab);
            return;
        }
        let editor = GitEditor::new(start);
        // The view knows nothing until git answers, so its first request goes
        // out with it rather than waiting for a keystroke.
        let first = editor.initial_request();
        let tab_id = self.show_editor_beside(EditorKind::Git, Box::new(editor));
        self.host.jobs.spawn(tab_id, first);
    }

    /// Show the differences between the two files `request` names, in a diff
    /// beside the work, read as a commit is.
    pub(in crate::window) fn open_comparison(&mut self, request: CompareRequest) {
        self.prefer_other_pane();
        let editor = GitEditor::comparing(request);
        let first = editor.initial_request();
        let tab_id = self.show_editor_beside(EditorKind::Git, Box::new(editor));
        self.host.jobs.spawn(tab_id, first);
    }

    /// Read a git view (a commit, a log, a blame) in a tab of its own beside
    /// the one it was asked from, in the same pane, so closing it goes back
    /// there. A view already open in a tab, shown or buried, is gone to
    /// instead.
    pub(in crate::window) fn open_git_view(&mut self, view: GitViewRequest) {
        let request = view.request.clone();
        let open = self
            .open_tabs
            .editors()
            .find(|(_, slot)| {
                slot.editor.with_kind(|git: &GitEditor| git.shows_view(&request)).unwrap_or(false)
            })
            .map(|(tab, _)| *tab);
        if let Some(tab) = open {
            self.show_buffer(tab);
            return;
        }
        let editor = GitEditor::viewing(view);
        let tab_id = self.show_editor(EditorKind::Git, Box::new(editor));
        self.host.jobs.spawn(tab_id, request);
    }

    /// Focus the open git editor for `repo` if one exists, reading it again,
    /// otherwise open a new git editor for `repo` and focus it.
    pub(in crate::window) fn focus_or_open_git_editor(&mut self, repo: PathBuf) {
        let matches_repo = |editor: &dyn Editor| {
            editor.root_dir().is_some_and(|root| {
                repo == root || repo.starts_with(&root) || root.starts_with(&repo)
            })
        };
        let open_tab = self.layout().members().into_iter().find(|&member| {
            self.open_tabs
                .editor(&member)
                .is_some_and(|slot| slot.kind == EditorKind::Git && matches_repo(slot.editor.as_ref()))
        });
        if let Some(tab) = open_tab {
            self.show_tab(tab);
            self.resume_editor(tab);
        } else if let Some(tab) = self.buried_matching(EditorKind::Git, &matches_repo) {
            self.show_buried_beside(tab);
        } else {
            let editor = GitEditor::new(repo);
            let first = editor.initial_request();
            let tab_id = self.show_editor_beside(EditorKind::Git, Box::new(editor));
            self.host.jobs.spawn(tab_id, first);
        }
    }

    /// Move focus to another pane when the window has more than one, so what
    /// opens next opens there: editors that are read beside the work, not over it.
    /// The pane beside this one in the same row is preferred, then the next in
    /// tree order. With a single pane [`Self::show_editor_beside`] splits one off
    /// instead.
    pub(in crate::window) fn prefer_other_pane(&mut self) {
        if self.layout().shown_tabs().len() < 2 {
            return;
        }
        let viewport = self.viewport_rect();
        let viewport = Rect::new(viewport.x, viewport.y, viewport.width, viewport.height);
        if !self.layout_mut().focus_side_by_side(viewport) {
            self.layout_mut().focus_next();
        }
    }

    /// Show `editor` beside the work: as a tab of the pane focus was moved to by
    /// [`Self::prefer_other_pane`], or, where there is only one pane, in a new
    /// one split off to its right.
    pub(in crate::window) fn show_editor_beside(&mut self, kind: EditorKind, editor: Box<dyn Editor>) -> TabId {
        match self.layout().shown_tabs().len() > 1 {
            true => self.show_editor(kind, editor),
            false => self.show_editor_split(kind, editor),
        }
    }

    /// Show the buried buffer of tab `tab` beside the work, as
    /// [`Self::show_editor_beside`] shows a new editor.
    pub(in crate::window) fn show_buried_beside(&mut self, tab: TabId) {
        self.open_tabs.unbury(tab);
        match self.layout().shown_tabs().len() > 1 {
            true => self.layout_mut().add_tab(tab),
            false => self.split_for(tab),
        }
        self.resume_editor(tab);
        self.after_tab_change();
    }

    /// Split the focused pane vertically and show `editor` in the new half, which
    /// takes focus.
    pub(in crate::window) fn show_editor_split(&mut self, kind: EditorKind, editor: Box<dyn Editor>) -> TabId {
        let tab_id = self.alloc_tab_id();
        self.split_for(tab_id);
        self.open_tabs
            .insert_editor(tab_id, kind, editor);
        self.after_tab_change();
        tab_id
    }

    /// Split the focused pane vertically for tab `tab`, which takes the new
    /// half and the focus.
    fn split_for(&mut self, tab: TabId) {
        self.layout_mut()
            .split(Direction::Vertical, SPLIT_RATIO, tab);
        self.layout_mut().balance();
    }

    /// Show `editor` in the dock along the bottom of the window (VS Code style panel),
    /// opening the dock where there is none, or adding it as a tab in the dock's pane.
    pub(in crate::window) fn show_editor_in_dock(
        &mut self,
        kind: EditorKind,
        editor: Box<dyn Editor>,
    ) -> TabId {
        let tab_id = self.alloc_tab_id();
        self.open_tabs
            .insert_editor(tab_id, kind, editor);
        self.place_in_dock(tab_id);
        tab_id
    }

    /// Show the buried buffer of tab `tab` in the dock, as
    /// [`Self::show_editor_in_dock`] shows a new editor.
    pub(in crate::window) fn show_buried_in_dock(&mut self, tab: TabId) {
        self.open_tabs.unbury(tab);
        self.place_in_dock(tab);
        self.resume_editor(tab);
    }

    /// Put tab `tab` in the dock, opening the dock where there is none.
    fn place_in_dock(&mut self, tab_id: TabId) {
        match self.layout().dock_state() {
            DockState::Absent => {
                self.layout_mut().open_dock(tab_id, DOCK_SHARE);
            }
            DockState::Shown | DockState::Hidden => {
                if let Some(dock_tab) = self.layout().dock_tab() {
                    self.layout_mut().add_tab_beside(dock_tab, tab_id, true);
                } else {
                    self.layout_mut().open_dock(tab_id, DOCK_SHARE);
                }
                self.layout_mut().show_dock();
            }
        }
        self.after_tab_change();
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
    }

    /// Show the `kind` tab matching `predicate` in the focused pane when it
    /// already has one, else bring back the buried buffer that matches, the
    /// most recently buried first, into the focused pane. One that is already
    /// showing stays showing: the editor's chord never steps away from it.
    /// Returns the tab, where there was one.
    fn show_open_editor_matching(
        &mut self,
        kind: EditorKind,
        predicate: &dyn Fn(&dyn Editor) -> bool,
    ) -> Option<TabId> {
        if let Some(tab) = self.show_pane_editor_matching(kind, predicate) {
            return Some(tab);
        }
        let tab = self.buried_matching(kind, predicate)?;
        self.show_buffer(tab);
        Some(tab)
    }

    /// The buried buffer of `kind` matching `predicate`, the most recently
    /// buried first, by the tab it was last shown in.
    pub(in crate::window) fn buried_matching(
        &self,
        kind: EditorKind,
        predicate: &dyn Fn(&dyn Editor) -> bool,
    ) -> Option<TabId> {
        self.open_tabs.buried().iter().rev().copied().find(|tab| {
            self.open_tabs
                .editor(tab)
                .is_some_and(|slot| slot.kind == kind && predicate(slot.editor.as_ref()))
        })
    }

    /// Show the `kind` tab matching `predicate` in the focused pane when it
    /// already has one, as [`Self::show_open_editor_matching`] does, but
    /// leaving the buried buffers buried.
    fn show_pane_editor_matching(
        &mut self,
        kind: EditorKind,
        predicate: &dyn Fn(&dyn Editor) -> bool,
    ) -> Option<TabId> {
        let focused = self.layout().focused();
        let open = self
            .layout()
            .recent_in_pane(focused)
            .into_iter()
            .find(|tab| {
                self.open_tabs
                    .editor(tab)
                    .is_some_and(|slot| slot.kind == kind && predicate(slot.editor.as_ref()))
            });
        if let Some(tab) = open.filter(|tab| *tab != focused) {
            self.show_tab(tab);
        }
        open
    }

    /// Have the editor in `tab` read again what it shows, as it does when a tab
    /// that covered it closes.
    fn resume_editor(&mut self, tab: TabId) {
        let Some(slot) = self.open_tabs.editor_mut(&tab) else {
            return;
        };
        let outcome = slot.editor.on_resume();
        self.act_on_editor_outcome(tab, outcome);
    }

    /// Return the editor in `tab` to its status view and read it again.
    fn show_status_editor(&mut self, tab: TabId) {
        let outcome = self
            .open_tabs
            .editor_mut(&tab)
            .and_then(|slot| slot.editor.with_kind_mut(GitEditor::show_status));
        if let Some(outcome) = outcome {
            self.act_on_editor_outcome(tab, outcome);
        }
    }

    /// Search the focused tab's project for text, in a tab of its own.
    pub(in crate::window) fn open_grep_editor(&mut self) {
        self.open_single(EditorKind::Grep, |app| {
            boxed(GrepEditor::new(project::root(&app.focused_start_dir())))
        });
    }

    /// List the comment keywords across the focused tab's project, in a tab of
    /// their own. The list searches for them on its first tick.
    pub(in crate::window) fn open_todo_editor(&mut self) {
        self.open_single(EditorKind::Todo, |app| {
            let root = project::root(&app.focused_start_dir());
            boxed(GrepEditor::todo(root, &app.config.todo_keywords))
        });
    }

    /// Show what docker knows in a tab of its own, starting from the
    /// containers. The view lists them on its first tick.
    pub(in crate::window) fn open_docker_editor(&mut self) {
        self.open_single(EditorKind::Docker, |_| boxed(DockerEditor::new()));
    }

    /// Show the Markdown file in front rendered, in the pane beside it,
    /// following its text as it is written. The focus stays on the file.
    #[cfg(feature = "preview")]
    pub(in crate::window) fn open_markdown_preview(&mut self) {
        let focused = self.layout().focused();
        let path = self
            .open_tabs
            .editor(&focused)
            .and_then(|slot| slot.editor.snapshot())
            .map(|target| target.path)
            .filter(|path| PreviewEditor::handles(path) && crate::remote::parse(path).is_none());
        let Some(path) = path else {
            self.set_error("Markdown: Preview needs a Markdown file on this machine in front");
            return;
        };
        let open = self
            .open_tabs
            .editors()
            .find(|(_, slot)| slot.kind() == EditorKind::Preview && slot.editor.holds_file(&path))
            .map(|(tab, _)| *tab);
        if let Some(tab) = open {
            self.show_tab(tab);
            self.set_focus(focused);
            return;
        }
        self.prefer_other_pane();
        self.show_editor_beside(EditorKind::Preview, Box::new(PreviewEditor::new(path)));
        self.set_focus(focused);
    }

    /// Tell, as error notifications, of each editor a panic stopped since
    /// last asked, wherever its tab is. Says whether any was told.
    pub(in crate::window) fn announce_stopped_editors(&mut self) -> bool {
        // Through the trait object itself: the box, asked as it is, is just
        // a box.
        let failures: Vec<Notice> = self
            .open_tabs
            .buffers_mut()
            .filter_map(|buffer| (*buffer.editor).as_any_mut().downcast_mut::<Guarded>()?.take_failure())
            .collect();
        let told = !failures.is_empty();
        for failure in failures {
            self.notify(failure);
        }
        told
    }

    /// Hand each view following a file's text (a Markdown preview) that
    /// text from the editor holding it, where it has changed or its cursor
    /// has moved. Says whether any view took something.
    #[cfg(feature = "preview")]
    pub(in crate::window) fn feed_followers(&mut self) -> bool {
        let followers: Vec<(TabId, PathBuf, Option<usize>)> = self
            .open_tabs
            .editors()
            .filter_map(|(tab, slot)| {
                let (path, known) = slot.editor.with_kind(PreviewEditor::follows_text).flatten()?;
                Some((*tab, path, known))
            })
            .collect();
        let mut any = false;
        for (tab, path, known) in followers {
            let text = self
                .open_tabs
                .editors()
                .filter(|(other, _)| **other != tab)
                .find_map(|(_, slot)| {
                    slot.editor.with_kind(|text: &TextEditor| text.shared_text(&path, known)).flatten()
                });
            let Some(text) = text else {
                continue;
            };
            let taken = self.open_tabs.editor_mut(&tab).and_then(|slot| {
                slot.editor.with_kind_mut(|preview: &mut PreviewEditor| preview.take_shared_text(text))
            });
            any |= taken.is_some();
        }
        any
    }

    /// The same, with no view that follows a file's text built in.
    #[cfg(not(feature = "preview"))]
    pub(in crate::window) fn feed_followers(&mut self) -> bool {
        false
    }

    /// The scratch file, a page kept beside the settings for notes that
    /// belong to no project, made the first time it is asked for.
    pub(in crate::window) fn open_scratch(&mut self) {
        let path = crate::config::config_dir().join(SCRATCH_FILE);
        if !path.exists() {
            if let Err(e) = std::fs::create_dir_all(crate::config::config_dir()).and_then(|()| std::fs::write(&path, "")) {
                self.set_error(format!("scratch: {e}"));
                return;
            }
        }
        self.open_path_editor(OpenTarget::file(path));
    }

    /// Two directory listings side by side, as a two-pane file manager:
    /// each copies and moves to the other's directory by default, and `=`
    /// with nothing marked compares the two.
    pub(in crate::window) fn open_commander(&mut self) {
        let start = self.focused_start_dir();
        self.show_editor(EditorKind::Dir, Box::new(DirEditor::new(start.clone())));
        self.show_editor_split(EditorKind::Dir, Box::new(DirEditor::new(start)));
    }

    /// Show the machine's processes in a tab of their own. The monitor asks
    /// for its own first snapshot on the next tick.
    pub(in crate::window) fn open_proc_editor(&mut self) {
        self.open_single(EditorKind::Proc, |_| boxed(ProcEditor::new()));
    }

    /// Show the machine's CPU, memory, disks, and GPUs in a tab of their own.
    pub(in crate::window) fn open_sys_editor(&mut self) {
        self.open_single(EditorKind::Sys, |_| boxed(SysEditor::new()));
    }

    /// Refresh each editor whose tick came due by `now`, then note when each
    /// next wants one, and say when the earliest does. An editor that asks for a
    /// later tick than it did keeps the earlier one, and may be ticked before
    /// it has anything to do, unless it called the earlier one off.
    pub(in crate::window) fn tick_editors(&mut self, now: Instant) -> Option<Instant> {
        // One tick per buffer, however many tabs show it.
        let mut buffers = HashSet::new();
        let due: Vec<TabId> = self
            .open_tabs
            .editors()
            .filter(|(_, slot)| slot.tick_at.is_some_and(|at| at <= now))
            .filter(|(_, slot)| buffers.insert(slot.view.buffer))
            .map(|(tab, _)| *tab)
            .collect();
        for tab_id in due {
            let Some(slot) = self.open_tabs.editor_mut(&tab_id) else {
                continue;
            };
            *slot.tick_at = None;
            let outcome = slot.editor.on_tick(now);
            self.act_on_editor_outcome(tab_id, outcome);
        }
        let buried: HashSet<TabId> = self.open_tabs.buried().iter().copied().collect();
        self.open_tabs
            .buffers_mut()
            .filter_map(|slot| {
                // Taken whatever happens next, so a deadline called off never
                // lingers to be matched against a later one.
                let cancelled = slot.editor.take_cancelled_tick();
                // A buried buffer does nothing on a timer: it is ticked only
                // to see work already under way through, and catches up on
                // the rest once a pane shows it again.
                let is_buried = slot.loaded.is_some_and(|tab| buried.contains(&tab));
                if is_buried && !slot.editor.has_work_in_flight() {
                    slot.tick_at = None;
                    return None;
                }
                let kept = slot.tick_at.filter(|kept| Some(*kept) != cancelled);
                let wanted = slot.editor.next_tick();
                slot.tick_at = match (kept, wanted) {
                    (Some(kept), Some(asked)) => Some(kept.min(asked)),
                    (_, asked) => asked,
                };
                slot.tick_at
            })
            .min()
    }

    /// Open `target`'s file in a tab beside the focused one, in whichever
    /// editor can show it.
    ///
    /// A PDF or a picture goes to its viewer, an SVG included: drawn, as
    /// a picture is, while "Open With" opens its markup in the editor.
    /// Everything else is the editor's.
    pub(in crate::window) fn open_path_editor(&mut self, target: OpenTarget) {
        self.record_recent_file(&target.path);
        if self.reveal_held_file(&target, None) {
            return;
        }
        // The viewers read from the local disk alone.
        let local = crate::remote::parse(&target.path).is_none();
        if local && PdfEditor::handles(&target.path) {
            self.show_editor(EditorKind::Pdf, Box::new(PdfEditor::new(target.path)));
            return;
        }
        if local && ImageEditor::handles(&target.path) {
            self.show_editor(EditorKind::Image, Box::new(ImageEditor::new(target.path)));
            return;
        }
        self.open_text_editor(target);
    }

    /// Show the tab in the focused pane that already has `target`'s file
    /// open, of `kind` where one is named, at `target`'s line, rather than
    /// opening the file a second time: an editor holding it in a buffer
    /// behind another brings that buffer forward. Returns whether there was
    /// such a tab.
    pub(in crate::window) fn reveal_held_file(
        &mut self,
        target: &OpenTarget,
        kind: Option<EditorKind>,
    ) -> bool {
        let focused = self.layout().focused();
        let Some((tab, held)) = self
            .pane_tab_holding(focused, kind, &target.path)
            .or_else(|| self.any_tab_holding(kind, &target.path))
        else {
            return false;
        };
        let target = OpenTarget {
            path: held,
            ..target.clone()
        };
        if let Some(slot) = self.open_tabs.editor_mut(&tab) {
            // Through the outcome, as a file joining the editor is, so one
            // that could not be read again says so now.
            if slot.editor.open_file(target) {
                let outcome = slot.editor.on_resume();
                self.act_on_editor_outcome(tab, outcome);
            }
        }
        if tab != focused {
            self.switch_to_tab(tab);
        }
        true
    }

    /// Open `target`'s file beside the tab `from` asked for it from, so that
    /// tab stays in sight: in the pane beside it when `from` has the focus,
    /// splitting one off where there is only one, else where the focus is.
    pub(in crate::window) fn open_beside(&mut self, from: TabId, target: OpenTarget) {
        if self.layout().focused() != from {
            self.open_path_editor(target);
            return;
        }
        if self.layout().shown_tabs().len() > 1 {
            self.prefer_other_pane();
            self.open_path_editor(target);
            return;
        }
        self.record_recent_file(&target.path);
        let local = crate::remote::parse(&target.path).is_none();
        if local && PdfEditor::handles(&target.path) {
            self.show_editor_split(EditorKind::Pdf, Box::new(PdfEditor::new(target.path)));
            return;
        }
        if local && ImageEditor::handles(&target.path) {
            self.show_editor_split(EditorKind::Image, Box::new(ImageEditor::new(target.path)));
            return;
        }
        match TextEditor::at(target) {
            Ok(mut editor) => {
                let outcome = editor.on_resume();
                let tab = self.show_editor_split(EditorKind::Text, Box::new(editor));
                self.act_on_editor_outcome(tab, outcome);
            }
            Err(e) => self.set_error(format!("{e}")),
        }
    }

    /// Show `path` in the PDF viewer at `mark`, or as it is without one, as
    /// magic-vscode's SyncTeX does: a tab already showing it is brought up
    /// and moved there, else one is opened in this pane, or with `beside`
    /// in the other pane (a tab there showing it being taken first).
    pub(in crate::window) fn show_in_pdf(&mut self, path: PathBuf, mark: Option<PdfMark>, beside: bool) {
        let here = self.layout().tabs_in_pane(self.layout().focused());
        let open = self
            .open_tabs
            .editors()
            .filter(|(tab, _)| !beside || !here.contains(tab))
            .find(|(_, slot)| {
                slot.kind() == EditorKind::Pdf && slot.editor.holds_file(&path)
            })
            .map(|(tab, _)| *tab);
        let tab = match open {
            Some(tab) => {
                self.show_tab(tab);
                tab
            }
            None if beside => {
                self.prefer_other_pane();
                self.show_editor_beside(EditorKind::Pdf, Box::new(PdfEditor::new(path)))
            }
            None => self.show_editor(EditorKind::Pdf, Box::new(PdfEditor::new(path))),
        };
        if let (Some(mark), Some(slot)) = (mark, self.open_tabs.editor_mut(&tab)) {
            slot.editor.with_kind_mut(|pdf: &mut PdfEditor| pdf.show_mark(mark));
        }
        self.dirty = true;
    }

    /// Open `target`'s file as editable text in a tab beside the focused one,
    /// so closing it returns to the tab the file was opened from.
    ///
    /// A file the editor will not open (binary, not UTF-8, too large to
    /// repaint) says so and stays closed; `C-o` hands those to `$EDITOR`.
    pub(in crate::window) fn open_text_editor(&mut self, target: OpenTarget) {
        let path = target.path.clone();
        match TextEditor::at(target) {
            Ok(mut editor) => {
                let outcome = editor.on_resume();
                let tab_id = self.show_editor(EditorKind::Text, Box::new(editor));
                self.act_on_editor_outcome(tab_id, outcome);
            }
            // A file that is no text is shown as its bytes instead.
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData && path.is_file() && crate::remote::parse(&path).is_none() => {
                self.show_editor(EditorKind::Hex, Box::new(HexEditor::new(path)));
                self.set_notice(format!("{e}: shown as its bytes"));
            }
            Err(e) => self.set_error(format!("{e}")),
        }
    }

    /// Open `path` in an editor tab of its own for a program blocked on it
    /// (`git commit` waiting on its message), answering `waiter` when the tab
    /// goes. Never joins an editor already up, since closing that one is not
    /// the end of this edit.
    pub(in crate::window) fn open_waited_editor(&mut self, path: PathBuf, waiter: EditWaiter) {
        let start = self.focused_start_dir();
        self.prefer_other_pane();
        match TextEditor::new(path, None) {
            Ok(text) => {
                let with_changes = text.edits_commit();
                let message =
                    self.show_editor_beside(EditorKind::Text, Box::new(text.with_waiter(waiter)));
                if with_changes {
                    self.show_staged_beside(message, start);
                }
            }
            Err(e) => self.set_error(format!("{e}")),
        }
    }

    /// Add what is staged in the repository containing `start` as a tab in
    /// `message`'s pane, so the change a commit message describes is one tab
    /// away without a pane of its own. The message stays the one showing, and
    /// the tab goes when the message does.
    fn show_staged_beside(&mut self, message: TabId, start: PathBuf) {
        let git = GitEditor::staged_diff(start);
        let first = git.initial_request();
        let diff = self.show_editor(EditorKind::Git, Box::new(git));
        self.set_focus(message);
        self.host.commit_companions.insert(message, diff);
        self.host.jobs.spawn(diff, first);
        self.after_tab_change();
    }

    /// Open the file browser where the focused terminal points, else over its
    /// working directory (see [`Self::files_palette_at_point`]): the palette,
    /// listing a directory a row at a time. The chord that opened it closes
    /// it again.
    pub(in crate::window) fn open_file_browser(&mut self) {
        let showing = self
            .chrome.picker()
            .is_some_and(|palette| palette.mode == PickerMode::Files);
        match showing {
            true => self.dismiss_overlay(),
            false => {
                let browser = self.files_palette_at_point();
                self.open_picker(browser);
            }
        }
    }

    /// The editor tabs as a session saves them: each that can say where it was.
    pub(in crate::window) fn editor_sessions(&self) -> Vec<EditorSession> {
        let mut sessions = self.editor_sessions_in(self.layout());
        sessions.extend(self.buried_sessions());
        sessions
    }

    /// The buried buffers that show a file on disk, to come back buried: an
    /// editor of text, a picture, a PDF, or bytes. A buried listing or build
    /// is not kept.
    pub(in crate::window) fn buried_sessions(&self) -> Vec<EditorSession> {
        self.open_tabs
            .buried()
            .iter()
            .filter_map(|id| {
                let slot = self.open_tabs.editor(id)?;
                if !slot.kind.info().is_some_and(|info| info.kept_buried) {
                    return None;
                }
                let at = slot.editor.snapshot()?;
                Some(EditorSession {
                    buried: true,
                    id: id.0 as usize,
                    kind: slot.kind.name().to_string(),
                    line: at.line,
                    path: at.path.to_string_lossy().to_string(),
                })
            })
            .collect()
    }

    /// The same, of the editor tabs in `layout` alone.
    pub(in crate::window) fn editor_sessions_in(&self, layout: &Layout) -> Vec<EditorSession> {
        self.open_tabs
            .editors()
            .filter(|(id, _)| layout.contains(**id))
            .filter_map(|(id, slot)| {
                let at = slot.editor.snapshot()?;
                Some(EditorSession {
                    buried: false,
                    id: id.0 as usize,
                    kind: slot.kind.name().to_string(),
                    line: at.line,
                    path: at.path.to_string_lossy().to_string(),
                })
            })
            .collect()
    }

    /// Open a saved editor tab again as tab `id`, outside the layout, which the
    /// restore rebuilds from the session. `false` for an editor that cannot be
    /// opened there any more (a file since deleted, a kind no longer known).
    pub(in crate::window) fn restore_editor(&mut self, id: TabId, saved: &EditorSession) -> bool {
        let Some(kind) = EditorKind::from_name(&saved.kind) else {
            return false;
        };
        let reopen = kind.info().and_then(|info| info.reopen);
        let Some(editor) = reopen.and_then(|reopen| reopen(PathBuf::from(&saved.path), saved.line, &self.config)) else {
            return false;
        };
        // A view that knows nothing until git answers has to ask, as it does
        // when opened; restored without the request it would wait for ever.
        let first = editor.with_kind(GitEditor::initial_request);
        self.open_tabs
            .insert_editor(id, kind, editor);
        if let Some(first) = first {
            self.host.jobs.spawn(id, first);
        }
        true
    }

    /// Open `editor` in a new tab beside the focused one and show it. Returns
    /// the tab it went into.
    pub(in crate::window) fn show_editor(
        &mut self,
        kind: EditorKind,
        editor: Box<dyn Editor>,
    ) -> TabId {
        let tab_id = self.alloc_tab_id();
        self.insert_editor_tab(tab_id, kind, editor);
        tab_id
    }

    /// Put `editor` into the focused pane's strip as a new tab, after the one
    /// it shows, without showing it or taking the focus.
    pub(in crate::window) fn add_background_editor(
        &mut self,
        kind: EditorKind,
        editor: Box<dyn Editor>,
    ) -> TabId {
        let tab_id = self.alloc_tab_id();
        self.open_tabs
            .insert_editor(tab_id, kind, editor);
        let focused = self.layout().focused();
        self.layout_mut().add_background_tab(focused, tab_id);
        self.dirty = true;
        tab_id
    }

    /// Put `editor` into the layout as tab `tab_id`, beside the focused tab.
    fn insert_editor_tab(&mut self, tab_id: TabId, kind: EditorKind, editor: Box<dyn Editor>) {
        self.open_tabs
            .insert_editor(tab_id, kind, editor);
        self.layout_mut().add_tab(tab_id);
        self.after_tab_change();
        // Extensions hear of the file at once.
        self.watch_for_extensions();
    }

    /// Show the focused pane's `kind` tab when it already has one, else open
    /// the editor `make` builds beside the focused tab: for a kind a pane
    /// needs only one of.
    pub(in crate::window) fn open_single(&mut self, kind: EditorKind, make: impl FnOnce(&Self) -> Box<dyn Editor>) {
        if self.show_open_editor_matching(kind, &|_| true).is_none() {
            let editor = make(self);
            self.show_editor(kind, editor);
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests;
