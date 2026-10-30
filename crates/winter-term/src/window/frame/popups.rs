//! The popups composed over the panes each frame: the command palette and the
//! pickers built on it, and the which-key hint for a pending prefix.

use std::collections::HashMap;
use std::sync::Arc;

use crate::model::paint::IconKind;
use crate::model::picker::{Picker, PickerMode};
use winter_render::{PickerItem, PickerView};

use crate::window::input::sequence::WHICH_KEY_DELAY;

// ========================================================================
// Constants
// ========================================================================

/// What the buffer list is headed by: rows naming editors and directories do
/// not say on their own what picking one would do.
const BUFFER_LIST_TITLE: &str = "Buffers: terminals, then buried, then shown";

// ========================================================================
// Functions
// ========================================================================

/// The renderer's view of the command palette: its filtered items, the query
/// behind them, and the empty-state message matching what it searches over.
///
/// `icons` holds what each icon kind resolved to on earlier frames, so a long
/// list is not walked through the icon tables again every frame.
pub(in crate::window) fn palette_view(
    palette: &Picker,
    match_underline: bool,
    pick: Option<&str>,
    caret_visible: bool,
    icons: &mut HashMap<IconKind, Option<Arc<[u8]>>>,
) -> PickerView {
    let empty_message = match palette.mode {
        PickerMode::Buffers => "No matching buffers",
        PickerMode::Files => "Nothing here by that name",
        PickerMode::History => "No matching history",
        PickerMode::EditorPick => "Nothing matches",
        PickerMode::OpenWith => "No editor by that name",
        PickerMode::Tasks => "No task by that name",
        PickerMode::Panes => "No matching panes",
        PickerMode::TabKeys => "No key by that name",
        PickerMode::Recent => "Nothing open or recent by that name",
        PickerMode::RecentDirs => "No recent directories",
        PickerMode::ClipRing => "Nothing copied yet",
        PickerMode::ProjectFiles => "No file in the project by that name",
        PickerMode::Workspaces | PickerMode::WorkspaceRemove => "No workspace by that name",
        PickerMode::LiveSearch => "Type a pattern (2+ characters); ` @ <file>` filters by path",
        PickerMode::Swoop => "No matching lines",
        PickerMode::Jump => "No matching targets",
        PickerMode::Commands
        | PickerMode::CompileCommand
        | PickerMode::MuxAttachRemote
        | PickerMode::MuxKill
        | PickerMode::MuxNew
        | PickerMode::MuxSessions
        | PickerMode::WorkspaceName => "No matching commands",
    };
    PickerView {
        anchor: None,
        caret: palette.caret(),
        caret_visible,
        empty_message: empty_message.to_string(),
        // A browser is headed by where it is, a list given a title by that,
        // and every other list by what it is a list of.
        title: match (&palette.title, &palette.dir) {
            (Some(title), _) => title.clone(),
            (None, Some(dir)) => dir.display().to_string(),
            (None, None) => mode_title(&palette.mode, pick),
        },
        items: palette
            .filtered
            .iter()
            .map(|&i| PickerItem {
                action: palette.entries[i].action.clone(),
                icon: palette.entries[i]
                    .icon
                    .as_ref()
                    .and_then(|kind| palette_icon(icons, kind)),
                label: palette.entries[i].label.clone(),
                match_positions: palette.entries[i].match_positions.clone(),
                shortcut: palette.entries[i].shortcut.clone(),
            })
            .collect(),
        match_underline,
        query: palette.query.clone(),
        selected: palette.selected,
    }
}

/// The SVG for `kind`, from `icons` when an earlier frame resolved it.
pub(in crate::window) fn palette_icon(
    icons: &mut HashMap<IconKind, Option<Arc<[u8]>>>,
    kind: &IconKind,
) -> Option<Arc<[u8]>> {
    if let Some(hit) = icons.get(kind) {
        return hit.clone();
    }
    let svg = crate::icons::shared_svg(kind);
    icons.insert(kind.clone(), svg.clone());
    svg
}

/// What a palette of `mode` is headed by when nothing more particular names
/// it. A popup opened by a key is read after the key is forgotten, so every one
/// says what its rows are and, where it is not plain, what picking one does.
pub(in crate::window) fn mode_title(mode: &PickerMode, pick: Option<&str>) -> String {
    let title = match mode {
        PickerMode::Commands => "Commands",
        PickerMode::Files => "Files",
        PickerMode::History => "Shell history: newest first, Enter inserts it",
        PickerMode::Jump => "Jump",
        PickerMode::MuxAttachRemote => "Mux: attach a remote session",
        PickerMode::MuxKill => "Mux: kill a background session",
        PickerMode::MuxNew => "Mux: new session",
        PickerMode::MuxSessions => "Mux sessions: Enter attaches",
        PickerMode::OpenWith => "Open with",
        PickerMode::Tasks => "Run a task",
        PickerMode::CompileCommand => "Compile: type a command line",
        PickerMode::Panes => "Panes: a tab's number goes straight to it",
        PickerMode::TabKeys => "Keys",
        PickerMode::Recent => "Recent: open tabs, then files, then directories",
        PickerMode::RecentDirs => "Recent directories: Enter changes to one",
        PickerMode::ClipRing => "Clipboard ring: Enter pastes one",
        PickerMode::ProjectFiles => "Project files",
        PickerMode::Workspaces => "Workspaces",
        PickerMode::WorkspaceRemove => "Workspaces: forget one",
        PickerMode::WorkspaceName => "Workspace: name it",
        PickerMode::LiveSearch => "Search",
        PickerMode::Swoop => "Swoop: lines of this pane",
        PickerMode::Buffers => BUFFER_LIST_TITLE,
        // An editor's question names its own list.
        PickerMode::EditorPick => pick.unwrap_or("Pick one"),
    };
    title.to_string()
}

/// The which-key popup, once a pending prefix has been held long enough to be
/// worth explaining. `None` until then, or when the prefix has no hint.
pub(in crate::window) fn which_key_view(
    pending: &crate::model::input::PendingPrefix,
    since: Option<std::time::Instant>,
) -> Option<winter_render::WhichKeyView> {
    since.filter(|s| s.elapsed() >= WHICH_KEY_DELAY)?;
    pending
        .hint()
        .map(|(title, items)| winter_render::WhichKeyView {
            items: items
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            title: key_hint_title(title),
        })
}

/// The key-hint popup's title for a pending `prefix`, worded as
/// magic-vscode's editor, terminal, and webview popups word it, so the two
/// apps read as one feature.
pub(in crate::window) fn key_hint_title(prefix: &str) -> String {
    format!("Key hints: ({prefix}) was pressed, wait for the next key stroke...")
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn test_a_pending_prefix_is_explained_only_once_it_has_sat_unresolved() {
        let pending = crate::model::input::PendingPrefix::G;
        assert!(which_key_view(&pending, Some(Instant::now())).is_none());
        assert!(which_key_view(&pending, None).is_none(), "nothing pending");
        let view = which_key_view(&pending, Some(Instant::now() - WHICH_KEY_DELAY))
            .expect("shown once the delay is up");
        assert!(view.title.contains("(g)"), "title: {}", view.title);
        assert!(!view.items.is_empty());
    }

    #[test]
    fn test_every_palette_is_headed_by_its_title_its_directory_or_its_kind() {
        // The command palette and the other lists opened by a key used to
        // draw no heading at all, so nothing on screen said what they were.
        let title = |palette: &Picker, pick: Option<&str>| {
            palette_view(palette, false, pick, true, &mut HashMap::new()).title
        };
        let keymap = crate::model::input::Keymap::default();
        assert_eq!(title(&Picker::open(crate::window::commands::builtin_commands(&keymap)), None), "Commands");
        assert_eq!(
            title(
                &Picker::open_pick(Vec::new(), Vec::new()),
                Some("Checkout")
            ),
            "Checkout",
            "an editor's question names its own list"
        );

        let browsing = Picker::open_files(std::env::temp_dir());
        assert_eq!(
            title(&browsing, None),
            std::env::temp_dir().display().to_string(),
            "a browser says where it is"
        );
        let jump = Picker::open_jump(Vec::new(), Some("Jump: notes.md".to_string()));
        assert_eq!(title(&jump, None), "Jump: notes.md", "a given title wins");
    }
}
