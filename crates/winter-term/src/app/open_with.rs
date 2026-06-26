//! "Open With": the focused tab's file, opened again in another tool that
//! can show it, beside the tab it was showing in: an SVG read as markup in
//! the editor, then drawn as a picture by the image viewer.

use std::path::{Path, PathBuf};

use crate::model::layout::TabId;
use crate::model::paint::IconKind;
use crate::model::palette::{Palette, PaletteEntry};
use crate::model::tool::OpenTarget;
use crate::remote;
use crate::tools::image::ImageTool;
use crate::tools::pdf::PdfTool;

use super::tool::ToolKind;
use super::App;

// ========================================================================
// Constants
// ========================================================================

/// Said when the focused tab shows no file to open elsewhere.
const NO_FILE: &str = "open with: this tab shows no file";

// ========================================================================
// App: open with
// ========================================================================

impl App {
    /// List the tools able to show the file in tab `focused`, other than the
    /// one showing it, to pick where to open it.
    pub(crate) fn open_with_list(&mut self, focused: TabId) {
        let Some((path, showing)) = self.shown_file(focused) else {
            self.set_error(NO_FILE);
            return;
        };
        let name = file_name(&path);
        let entries: Vec<PaletteEntry> = file_tools(&path)
            .into_iter()
            .filter(|kind| *kind != showing)
            .map(|kind| {
                PaletteEntry::new(kind.name(), kind.title(), String::new())
                    .with_icon(IconKind::Tool { kind: kind.name() })
            })
            .collect();
        if entries.is_empty() {
            self.set_error(format!("open with: no other tool shows {name}"));
            return;
        }
        self.open_palette(Palette::open_with(entries, format!("Open {name} with")));
    }

    /// Open the file in tab `focused` in the tool `kind` names, in a tab
    /// beside it, or go to the tab of that kind in the same pane already
    /// showing it, rather than opening it a second time.
    pub(crate) fn open_focused_with(&mut self, kind: &str, focused: TabId) {
        let (Some((path, _)), Some(kind)) = (self.shown_file(focused), ToolKind::from_name(kind))
        else {
            return;
        };
        if let Some((open, held)) = self.pane_tab_holding(focused, kind, &path) {
            // An editor holding the file in a buffer behind another brings
            // that buffer forward; it is not read again.
            if let Some(slot) = self.open_tabs.tool_mut(&open) {
                slot.tool.open_file(OpenTarget::file(held));
            }
            self.switch_to_tab(open);
            return;
        }
        // Where the file was read up to means nothing to another tool: a
        // PDF's page is not the editor's line.
        let target = OpenTarget::file(path);
        match kind {
            ToolKind::Image => {
                self.show_tool(ToolKind::Image, Box::new(ImageTool::new(target.path)));
            }
            ToolKind::Pdf => {
                self.show_tool(ToolKind::Pdf, Box::new(PdfTool::new(target.path)));
            }

            ToolKind::Editor => self.open_editor_tool(target),
            ToolKind::Dir
            | ToolKind::Git
            | ToolKind::Grep
            | ToolKind::Keys
            | ToolKind::Notifications
            | ToolKind::Proc
            | ToolKind::Sys => {}
        }
    }

    /// The tab of `kind` in the pane holding `focused` that has the file at
    /// `path` open, other than `focused` itself, and the path it holds the
    /// file under.
    fn pane_tab_holding(
        &self,
        focused: TabId,
        kind: ToolKind,
        path: &Path,
    ) -> Option<(TabId, PathBuf)> {
        // Either spelling, the one asked about or the one through every
        // symlink, since a tool holds a file under the path it was given.
        let real = path.canonicalize().ok();
        let spellings: Vec<&Path> = [Some(path), real.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        self.layout()
            .tabs_in_pane(focused)
            .into_iter()
            .filter(|tab| *tab != focused)
            .find_map(|tab| {
                let slot = self
                    .open_tabs
                    .tool(&tab)
                    .filter(|slot| slot.kind() == kind)?;
                let held = spellings
                    .iter()
                    .find(|spelling| slot.tool.holds_file(spelling))?;
                Some((tab, held.to_path_buf()))
            })
    }

    /// The file tab `id` shows, and the kind of tool showing it. `None` for
    /// a terminal, or a tool that shows a directory or nothing on disk.
    fn shown_file(&self, id: TabId) -> Option<(PathBuf, ToolKind)> {
        let slot = self.open_tabs.tool(&id)?;
        let path = slot.tool.snapshot()?.path;
        let local = remote::parse(&path).is_none();
        (!local || path.is_file()).then_some((path, slot.kind()))
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// The tools that can show the file at `path`, in the order they are
/// offered. The viewers read the local disk alone, and the editor refuses
/// a file it would only show as binary.
fn file_tools(path: &Path) -> Vec<ToolKind> {
    let local = remote::parse(path).is_none();
    let pdf = local && PdfTool::handles(path);
    let image = local && ImageTool::handles(path);
    let raster = image && !ImageTool::is_markup(path);
    let text = !pdf && !raster;
    [
        (ToolKind::Editor, text),
        (ToolKind::Image, image),
        (ToolKind::Pdf, pdf),
    ]
    .into_iter()
    .filter_map(|(kind, shows)| shows.then_some(kind))
    .collect()
}

/// The last component of `path`, for the picker's title.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::input::Keymap;

    #[test]
    fn test_each_file_is_offered_to_the_tools_that_can_show_it() {
        let tools = |path: &str| file_tools(Path::new(path));
        assert_eq!(
            tools("/art/logo.svg"),
            vec![ToolKind::Editor, ToolKind::Image],
            "markup and a picture both"
        );
        assert_eq!(
            tools("/art/photo.png"),
            vec![ToolKind::Image],
            "the editor would refuse it as binary"
        );
        assert_eq!(tools("/docs/paper.pdf"), vec![ToolKind::Pdf]);
        assert_eq!(tools("/src/main.rs"), vec![ToolKind::Editor]);
    }

    #[test]
    fn test_a_remote_file_is_offered_to_the_editor_alone() {
        // The viewers read the local disk; handing one a remote path opens
        // a blank surface.
        assert_eq!(
            file_tools(Path::new("ssh://host/art/logo.svg")),
            vec![ToolKind::Editor]
        );
    }

    #[test]
    fn test_the_default_keys_bind_open_with() {
        assert_eq!(Keymap::default().chord_hint("open_with"), "C-M-S-e");
    }

    #[test]
    fn test_open_with_from_a_terminal_says_there_is_no_file() {
        let mut app = App::new();
        let focused = app.layout().focused();
        app.open_with_list(focused);
        assert!(app.palette().is_none(), "nothing to pick from");
    }

    #[test]
    fn test_a_tool_already_holding_the_file_in_the_pane_is_gone_to() {
        let dir = std::env::temp_dir().join(format!("winter-open-with-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let svg = dir.join("logo.svg");
        std::fs::write(&svg, "<svg xmlns='http://www.w3.org/2000/svg'/>").expect("an svg");

        let mut app = App::new();
        let viewer = app.show_tool(ToolKind::Image, Box::new(ImageTool::new(svg.clone())));
        app.open_editor_tool(OpenTarget::file(svg.clone()));
        let editor = app.layout().focused();
        assert_ne!(editor, viewer, "the editor opened a tab of its own");
        let tabs = app.layout().tabs_in_pane(editor).len();

        app.open_focused_with(ToolKind::Image.name(), editor);
        assert_eq!(app.layout().focused(), viewer, "back to the viewer");
        assert_eq!(
            app.layout().tabs_in_pane(viewer).len(),
            tabs,
            "no second viewer"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
