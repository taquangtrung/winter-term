//! The right-click context menu: what it offers where it was opened, and
//! running the entry picked. Misty's [`Menu`] draws it at the pointer.
//!
//! The menu is built in groups, top to bottom, the most particular first:
//!
//! 1. What was clicked offers: an editor's own entries for the row under the
//!    pointer (see [`EditorMenuItem`]), or a terminal's copy and paste and
//!    the link under the pointer.
//! 2. What the tab shows: the file's path to copy, its folder to open, and
//!    the programs to open it with; a terminal's working directory and
//!    scrollback.
//! 3. The pane: split it, or close the tab.
//!
//! Groups 2 and 3 are the window's, the same for every tab, so an editor
//! lists only what it alone can do and every menu ends alike.

use std::path::{Path, PathBuf};

use misty::{Menu, MenuEntry, MenuItem, PopupSide, Rect};

use winter_model::editor::{EditorMenuItem, MenuRun};
use winter_model::input::Key;
use winter_model::layout::TabId;
use winter_model::paint::Point;

use super::keys_label;
use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// The pane's entries, every menu's last group: the command each runs, and
/// what it is called.
const PANE_ENTRIES: [(&str, &str); 3] = [
    ("split_vertical", "Split Right"),
    ("split_horizontal", "Split Down"),
    ("close_tab", "Close Tab"),
];

// ========================================================================
// Data Structures
// ========================================================================

/// An action dispatched from a right-click context menu item.
#[derive(Clone)]
pub(in crate::window) enum ContextAction {
    /// The window's command `id`, run on `tab`.
    Command(CommandAction),
    /// The terminal's selection to the clipboard.
    Copy,
    /// `text` to the clipboard, the entry called `label`: a path, a link.
    CopyText(CopyAction),
    /// An entry an editor offered, standing for a key that editor binds.
    EditorKey(EditorKeyAction),
    OpenLink(String),
    /// The folder at the path open in Dir.
    OpenFolder(PathBuf),
    Paste,
    /// A line between two groups of entries, which does nothing.
    Separator,
}

/// A window command a menu entry runs on the tab it was opened over.
#[derive(Clone)]
pub(in crate::window) struct CommandAction {
    pub(in crate::window) id: &'static str,
    pub(in crate::window) label: String,
    pub(in crate::window) tab: TabId,
}

/// Text a menu entry copies, and what the entry is called.
#[derive(Clone)]
pub(in crate::window) struct CopyAction {
    pub(in crate::window) label: &'static str,
    pub(in crate::window) text: String,
}

/// A menu entry an editor offered: what it is called, and the keys of its
/// own the editor is typed when it is chosen.
#[derive(Clone)]
pub(in crate::window) struct EditorKeyAction {
    pub(in crate::window) keys: Vec<Key>,
    pub(in crate::window) label: String,
    /// The pane whose editor offered it, since the menu outlives the click and
    /// the focus could move under it.
    pub(in crate::window) tab: TabId,
}

// ========================================================================
// ContextAction
// ========================================================================

impl ContextAction {
    /// What the menu shows for it.
    pub(in crate::window) fn label(&self) -> String {
        match self {
            Self::Command(action) => action.label.clone(),
            Self::Copy => "Copy".into(),
            Self::CopyText(action) => action.label.into(),
            Self::EditorKey(action) => action.label.clone(),
            Self::OpenLink(_) => "Open Link".into(),
            Self::OpenFolder(_) => "Open Folder in Dir".into(),
            Self::Paste => "Paste".into(),
            Self::Separator => super::SEPARATOR.label.into(),
        }
    }

    /// The keys shown beside it: an editor's entry names the keys it types,
    /// so the menu teaches them.
    pub(in crate::window) fn shortcut(&self) -> String {
        match self {
            Self::EditorKey(action) => keys_label(&action.keys),
            _ => String::new(),
        }
    }

    /// The window's command `id` on `tab`, called `label`.
    fn command(id: &'static str, label: &str, tab: TabId) -> Self {
        Self::Command(CommandAction {
            id,
            label: label.to_string(),
            tab,
        })
    }

    /// An entry copying `text`, called `label`.
    fn copy(label: &'static str, text: String) -> Self {
        Self::CopyText(CopyAction { label, text })
    }

    /// The window's entry for an editor's `item` on `tab`.
    fn of_editor(item: EditorMenuItem, tab: TabId) -> Self {
        match item.run {
            MenuRun::Command(id) => Self::command(id, &item.label, tab),
            MenuRun::Keys(keys) => Self::EditorKey(EditorKeyAction {
                keys,
                label: item.label,
                tab,
            }),
            MenuRun::Separator => Self::Separator,
        }
    }
}

// ========================================================================
// App: context menu
// ========================================================================

impl App {
    /// Close the right-click context menu.
    pub(in crate::window) fn close_context_menu(&mut self) {
        if self.chrome.menus.context.is_some() {
            self.chrome.menus.context = None;
            self.chrome.menus.context_actions.clear();
            self.dirty = true;
        }
    }

    /// Open a right-click context menu at `(x, y)`, in the groups the module
    /// describes. Over an editor the first group is the editor's own,
    /// because a terminal's entries mean nothing there: a paste would go to
    /// the shell running underneath the listing, which is not what was
    /// clicked on.
    pub(in crate::window) fn open_context_menu(&mut self, x: f32, y: f32) {
        let tab = self.pane_at_pixel(x, y).map(|(tab, _)| tab);
        let groups = match tab {
            Some(tab) if self.open_tabs.is_page(&tab) => vec![
                self.editor_context_actions(x, y),
                self.file_context_actions(tab),
                pane_context_actions(tab),
            ],
            Some(tab) => vec![
                self.terminal_context_actions(),
                self.terminal_tab_context_actions(tab),
                pane_context_actions(tab),
            ],
            None => vec![self.terminal_context_actions()],
        };
        let actions = in_groups(groups);
        // Nothing to offer gets no empty card.
        if actions.is_empty() {
            self.close_context_menu();
            return;
        }
        let mut menu = Menu::new(context_entries(&actions));
        menu.open(Rect::new(x, y, 0.0, 0.0), PopupSide::Below);
        self.chrome.menus.context = Some(menu);
        self.chrome.menus.context_actions = actions;
        self.close_menu();
        self.dirty = true;
    }

    /// What the editor under `(x, y)` offers over the row that was
    /// clicked.
    ///
    /// The pointer moves the editor's cursor first, so the menu acts on the row
    /// it was opened over rather than wherever the keyboard last left the
    /// cursor: right-clicking a file in Git and choosing "stage it" stages
    /// that file, not the one that happened to be under the cursor.
    fn editor_context_actions(&mut self, x: f32, y: f32) -> Vec<ContextAction> {
        let Some((tab_id, pane_rect)) = self.pane_at_pixel(x, y) else {
            return Vec::new();
        };
        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
        let at = Point {
            col,
            drag: false,
            row,
        };
        let outcome = match self.open_tabs.page_mut(&tab_id) {
            Some(slot) => slot.page.on_context_mouse(slot.view.editor_point(at)),
            None => return Vec::new(),
        };
        self.act_on_editor_outcome(tab_id, outcome);
        let Some(slot) = self.open_tabs.page(&tab_id) else {
            return Vec::new();
        };
        slot.page
            .context_items()
            .into_iter()
            .map(|item| ContextAction::of_editor(item, tab_id))
            .collect()
    }

    /// What every editor showing a file offers of it: its path, name, and
    /// place to copy, its folder to open (on this machine), and the programs
    /// to open it with.
    fn file_context_actions(&self, tab: TabId) -> Vec<ContextAction> {
        let Some(slot) = self.open_tabs.page(&tab) else {
            return Vec::new();
        };
        let Some(path) = slot
            .page
            .as_editor()
            .and_then(|editor| editor.document_path())
        else {
            return Vec::new();
        };
        let mut actions = vec![ContextAction::copy("Copy Path", path.display().to_string())];
        if let Some(name) = path.file_name() {
            actions.push(ContextAction::copy(
                "Copy File Name",
                name.to_string_lossy().into_owned(),
            ));
        }
        // The place, where it says more than the path (a line, a range).
        if let Some(reference) = slot
            .page
            .as_editor()
            .and_then(|editor| editor.file_reference())
        {
            if reference != winter_model::path::abbreviate_home(&path) {
                actions.push(ContextAction::copy("Copy Path and Line", reference));
            }
        }
        if let Some(folder) = local_folder_of(&path) {
            actions.push(ContextAction::OpenFolder(folder));
        }
        actions.push(ContextAction::command("open_with", "Open With…", tab));
        actions
    }

    /// What a terminal offers wherever it is clicked: its selection to copy,
    /// a paste, and the link under the pointer to open or copy.
    fn terminal_context_actions(&self) -> Vec<ContextAction> {
        let mut actions = Vec::new();
        if self.term.selection.span.is_some() {
            actions.push(ContextAction::Copy);
        }
        actions.push(ContextAction::Paste);
        if let Some(url) = &self.input.pointer.hovered_url {
            actions.push(ContextAction::OpenLink(url.clone()));
            actions.push(ContextAction::copy("Copy Link", url.clone()));
        }
        actions
    }

    /// What a terminal's tab offers of itself: where its shell works, and
    /// everything it has printed.
    fn terminal_tab_context_actions(&self, tab: TabId) -> Vec<ContextAction> {
        let mut actions = Vec::new();
        if let Some(cwd) = self
            .open_tabs
            .terminal(&tab)
            .and_then(|terminal| terminal.cwd())
        {
            actions.push(ContextAction::copy("Copy Working Directory", cwd.clone()));
            // A shell on another machine reports a folder that is not here.
            let folder = PathBuf::from(cwd);
            if folder.is_dir() {
                actions.push(ContextAction::OpenFolder(folder));
            }
        }
        actions.push(ContextAction::command(
            "copy_scrollback",
            "Copy Scrollback",
            tab,
        ));
        actions
    }

    /// Carry out a context-menu entry. Both the tabbar's click handling and
    /// the window's own reach this, so an entry means one thing wherever the
    /// click that chose it was resolved.
    pub(in crate::window) fn run_context_action(&mut self, action: ContextAction) {
        match action {
            ContextAction::Command(action) => {
                // A pane's command acts on the focused pane: the one clicked.
                if self.layout().contains(action.tab) {
                    self.set_focus(action.tab);
                }
                self.run_command(action.id, action.tab);
            }
            ContextAction::Copy => self.copy_selection(),
            ContextAction::CopyText(action) => self.copy_text(&action.text),
            ContextAction::EditorKey(action) => {
                for key in &action.keys {
                    self.offer_key_to_editor(action.tab, key);
                }
            }
            ContextAction::OpenFolder(folder) => self.open_dir_editor_at(folder),
            ContextAction::OpenLink(url) => self.open_link(&url),
            ContextAction::Paste => self.paste_from_clipboard(),
            ContextAction::Separator => {}
        }
    }

    /// Put `text` on the clipboard, and say so.
    fn copy_text(&mut self, text: &str) {
        let copied = self
            .input
            .clipboard()
            .and_then(|clipboard| clipboard.set_text(text).ok())
            .is_some();
        match copied {
            true => self.set_unlogged_notice(format!("copied {text}")),
            false => self.set_error("clipboard unavailable"),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Misty's entries for `actions`, each item numbered by its place there and
/// showing the keys an editor's entry types.
fn context_entries(actions: &[ContextAction]) -> Vec<MenuEntry<usize>> {
    actions
        .iter()
        .enumerate()
        .map(|(index, action)| {
            if matches!(action, ContextAction::Separator) {
                return MenuEntry::Separator;
            }
            let item = MenuItem::new(index, action.label());
            let shortcut = action.shortcut();
            match shortcut.is_empty() {
                true => item.into(),
                false => item.with_accelerator(shortcut).into(),
            }
        })
        .collect()
}

/// What every tab offers of its pane, the menu's last group.
fn pane_context_actions(tab: TabId) -> Vec<ContextAction> {
    PANE_ENTRIES
        .iter()
        .map(|(id, label)| ContextAction::command(id, label, tab))
        .collect()
}

/// `groups` of entries set apart by separators, an empty group dropped and
/// a group's own leading or trailing separator with it.
fn in_groups(groups: Vec<Vec<ContextAction>>) -> Vec<ContextAction> {
    let mut actions = Vec::new();
    for group in groups {
        let entries = trimmed(group);
        if entries.is_empty() {
            continue;
        }
        if !actions.is_empty() {
            actions.push(ContextAction::Separator);
        }
        actions.extend(entries);
    }
    actions
}

/// `group` without separators at its ends.
fn trimmed(group: Vec<ContextAction>) -> Vec<ContextAction> {
    let is_entry = |action: &ContextAction| !matches!(action, ContextAction::Separator);
    let start = group.iter().position(is_entry);
    let end = group.iter().rposition(is_entry);
    match (start, end) {
        (Some(start), Some(end)) => group[start..=end].to_vec(),
        _ => Vec::new(),
    }
}

/// The folder holding `path`, where it is on this machine: one on another,
/// reached through a remote path, is no folder Dir can open here.
fn local_folder_of(path: &Path) -> Option<PathBuf> {
    if winter_base::remote::parse(path).is_some() {
        return None;
    }
    path.parent()
        .filter(|folder| !folder.as_os_str().is_empty())
        .map(Path::to_path_buf)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use crate::window::editor::EditorKind;
    use crate::window::test_support::app_pane_and_id;
    use winter_text::TextEditor;

    /// The open menu's shape: labels, `-` for a separator.
    fn shape(app: &App) -> Vec<String> {
        app.chrome
            .menus
            .context_actions
            .iter()
            .map(|action| match action {
                ContextAction::Separator => "-".to_string(),
                action => action.label(),
            })
            .collect()
    }

    /// The middle of tab `tab`'s pane, in pixels.
    fn middle_of(app: &App, tab: TabId) -> (f32, f32) {
        let (_, rect) = app
            .pane_rects()
            .into_iter()
            .find(|(id, _)| *id == tab)
            .expect("shown");
        let pane = App::layout_rect_to_pane(rect);
        (pane.x + pane.width / 2.0, pane.y + pane.height / 2.0)
    }

    #[test]
    fn test_an_editor_s_menu_is_its_own_then_its_file_s_then_the_pane_s() {
        let path = std::env::temp_dir().join(format!("winter-context-{}.txt", std::process::id()));
        std::fs::write(&path, "text\n").expect("temp file");
        let mut app = App::new();
        let editor = TextEditor::new(path.clone(), None).expect("opens");
        let tab = app.show_editor(EditorKind::Text, Box::new(editor));
        let (x, y) = middle_of(&app, tab);
        app.open_context_menu(x, y);
        let shape = shape(&app);
        assert!(
            matches!(
                app.chrome.menus.context_actions.first(),
                Some(ContextAction::EditorKey(_))
            ),
            "the editor's own entries come first: {shape:?}"
        );
        let file = shape
            .iter()
            .position(|label| label == "Copy Path")
            .expect("the file's group");
        assert_eq!(shape[file - 1], "-", "set apart: {shape:?}");
        for label in ["Copy File Name", "Open Folder in Dir", "Open With…"] {
            assert!(
                shape[file..].contains(&label.to_string()),
                "{label}: {shape:?}"
            );
        }
        assert_eq!(
            shape[shape.len() - 4..],
            ["-", "Split Right", "Split Down", "Close Tab"]
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_a_terminal_s_menu_pastes_copies_its_scrollback_and_ends_with_the_pane() {
        let (mut app, tab) = app_pane_and_id();
        let (x, y) = middle_of(&app, tab);
        app.open_context_menu(x, y);
        let shape = shape(&app);
        assert_eq!(
            shape.first().map(String::as_str),
            Some("Paste"),
            "{shape:?}"
        );
        assert!(shape.contains(&"Copy Scrollback".to_string()), "{shape:?}");
        assert_eq!(
            shape[shape.len() - 3..],
            ["Split Right", "Split Down", "Close Tab"]
        );
    }

    #[test]
    fn test_groups_drop_their_own_end_separators_and_the_empty_ones() {
        let groups = vec![
            vec![
                ContextAction::Separator,
                ContextAction::Paste,
                ContextAction::Separator,
            ],
            Vec::new(),
            vec![ContextAction::Separator],
            vec![ContextAction::Copy],
        ];
        let labels: Vec<String> = in_groups(groups)
            .iter()
            .map(|action| match action {
                ContextAction::Separator => "-".to_string(),
                action => action.label(),
            })
            .collect();
        assert_eq!(labels, ["Paste", "-", "Copy"]);
    }
}
