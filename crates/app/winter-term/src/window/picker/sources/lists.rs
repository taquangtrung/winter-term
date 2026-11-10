//! The pickers over what the window holds, each a listing built afresh when
//! it opens and when it is resumed: the recent list (`C-S-z`), the buffers
//! (`C-x C-b`), the panes, the directories the shells were in, the clipboard
//! ring, and the shell's history.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use winter_model::editor::OpenTarget;
use winter_model::layout::TabId;
use winter_model::paint::IconKind;
use winter_model::path::abbreviate_home;
use winter_picker::{ActionId, FilePreview, Preview, PreviewKind, Row, RowKey};

use super::listing::{Entry, Listing, ListingAction};
use crate::window::editor::buffers::{buffer_label, tab_icon};
use crate::window::picker::WindowOutcome;
use crate::window::recent::merged;
use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// What a buried buffer's row says at its right.
pub(in crate::window) const BURIED_HINT: &str = "buried";

/// The buffers' second action, a button too.
pub(in crate::window) const KILL_BUFFER: ListingAction = ListingAction {
    button: Some(IconKind::Symbol { name: "close" }),
    id: ActionId("kill_buffer"),
    title: "Kill Buffer",
};

/// What each listing is headed by.
const RECENT_TITLE: &str = "Recent: open tabs, then files, then directories";
const BUFFERS_TITLE: &str = "Buffers";
const PANES_TITLE: &str = "Panes: a tab's number goes straight to it";
const RECENT_DIRS_TITLE: &str = "Recent directories: Enter changes to one";
const CLIP_RING_TITLE: &str = "Clipboard ring: Enter pastes one";
const HISTORY_TITLE: &str = "Shell history: newest first, Enter inserts it";

/// How much of a copied text's first line its row shows.
const CLIP_PREVIEW_CHARS: usize = 120;

/// How many commands of the shell's history are listed.
const HISTORY_MAX: usize = 500;

/// The history files looked for in the home directory, after `$HISTFILE`.
const HISTORY_FILES: [&str; 2] = [".zsh_history", ".bash_history"];

/// zsh's meta byte: in its history file a byte that is not plain text is
/// written as this, then the byte with [`ZSH_META_FLIP`] flipped.
const ZSH_META: u8 = 0x83;
const ZSH_META_FLIP: u8 = 0x20;

// ========================================================================
// App: the listings
// ========================================================================

impl App {
    /// The recent list: the tabs open in every pane, then the buried
    /// buffers, then the files opened lately, then the directories, newest
    /// first within each. A local file or directory that is gone is left
    /// out, and a file a tab or a buried buffer holds is offered only as
    /// that tab or buffer.
    pub(in crate::window) fn recent_listing(&self) -> Listing {
        let mut list = Listing::new("recent", RECENT_TITLE)
            .placeholder("Tab, file, or directory")
            .preview(PreviewKind::Panel);
        for entry in self.open_tab_entries() {
            list.push(entry);
        }
        for entry in self.buried_entries() {
            list.push(entry);
        }
        let saved = crate::config::load_state();
        for path in merged([self.recent.files.clone(), saved.recent_files]) {
            // A remote file is offered without a round trip to check on it.
            if winter_base::remote::parse(&path).is_none() && !path.is_file() {
                continue;
            }
            if self.any_tab_holding(None, &path).is_some() {
                continue;
            }
            let row = Row::new(RowKey(0), abbreviate_home(&path)).with_icon(file_icon(&path));
            let preview = Preview::File(FilePreview {
                column: None,
                line: 1,
                path: path.clone(),
            });
            list.push(
                Entry::new(row, WindowOutcome::OpenFile(OpenTarget::file(path)))
                    .with_preview(preview),
            );
        }
        for path in merged([
            self.recent.dirs.clone(),
            saved.recent_dirs,
            self.shell_dirs(),
        ]) {
            if !path.is_dir() {
                continue;
            }
            let row = Row::new(RowKey(0), abbreviate_home(&path)).with_icon(dir_icon(&path));
            list.push(Entry::new(row, WindowOutcome::OpenDir(path)));
        }
        list
    }

    /// The buffer list: every terminal tab in pane order, then the buried
    /// buffers, the most recently buried first, then the buffers shown, one
    /// row each however many tabs show it. A page holds no buffer, so none
    /// is listed.
    pub(in crate::window) fn buffer_listing(&self) -> Listing {
        let mut list = Listing::new("buffers", BUFFERS_TITLE)
            .placeholder("Buffer name")
            .extra_actions(&[KILL_BUFFER]);
        let mut shown = Vec::new();
        let mut listed = HashSet::new();
        for (pane_index, tabs) in self.layout().pane_members().into_iter().enumerate() {
            let pane = format!("pane {}", pane_index + 1);
            for tab in tabs {
                match self.open_tabs.page(&tab).map(|slot| slot.buffer) {
                    // A page holds no buffer to list.
                    Some(None) => {}
                    Some(Some(buffer)) if !listed.insert(buffer) => {}
                    Some(Some(_)) => shown.extend(self.buffer_entry(tab, &pane)),
                    None => {
                        let row = self.tab_row(tab).with_hint(pane.clone());
                        list.push(Entry::new(row, WindowOutcome::FocusTab(tab)));
                    }
                }
            }
        }
        for entry in self.buried_entries().into_iter().chain(shown) {
            list.push(entry);
        }
        list
    }

    /// Every tab of every pane, named after its pane, its place on that
    /// pane's strip at its right.
    pub(in crate::window) fn pane_listing(&self) -> Listing {
        let mut list = Listing::new("panes", PANES_TITLE).placeholder("Tab name or number");
        for (pane_index, &shown) in self.layout().shown_tabs().iter().enumerate() {
            for (tab_index, tab) in self.layout().tabs_in_pane(shown).into_iter().enumerate() {
                let label = format!("{}: {}", pane_index + 1, self.tab_full_name(tab));
                let row = Row::new(RowKey(0), label)
                    .with_hint((tab_index + 1).to_string())
                    .with_icon(self.tab_icon_kind(tab));
                list.push(Entry::new(row, WindowOutcome::FocusTab(tab)));
            }
        }
        list
    }

    /// The directories the focused terminal's commands ran in, newest first,
    /// to `cd` back into.
    pub(in crate::window) fn recent_dir_listing(&self, focused: TabId) -> Listing {
        let mut list = Listing::new("recent_dirs", RECENT_DIRS_TITLE).placeholder("Directory path");
        let mut seen = HashSet::new();
        let dirs = self.open_tabs.terminal(&focused).map(|terminal| {
            terminal
                .scrollback()
                .blocks()
                .iter()
                .rev()
                .filter_map(|block| block.cwd.as_deref())
                .filter(|dir| seen.insert(dir.to_string()))
                .map(str::to_string)
                .collect::<Vec<_>>()
        });
        for dir in dirs.unwrap_or_default() {
            let row = Row::new(RowKey(0), dir.clone()).with_icon(dir_icon(Path::new(&dir)));
            list.push(Entry::new(row, WindowOutcome::ChangeDirectory(dir)));
        }
        list
    }

    /// What was copied lately, newest first, each by its first line, the
    /// number of lines at its right when it has more than one.
    pub(in crate::window) fn clip_ring_listing(&self, copied: &[String]) -> Listing {
        let mut list = Listing::new("clip_ring", CLIP_RING_TITLE)
            .placeholder("Filter copied text")
            .preview(PreviewKind::Panel);
        for (at, text) in copied.iter().enumerate() {
            let first: String = text
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or_default()
                .chars()
                .take(CLIP_PREVIEW_CHARS)
                .collect();
            let row = match text.lines().count() {
                0 | 1 => Row::new(RowKey(0), first),
                lines => Row::new(RowKey(0), first).with_hint(format!("{lines} lines")),
            };
            let entry =
                Entry::new(row, WindowOutcome::Paste(at)).with_preview(Preview::Text(text.clone()));
            list.push(entry);
        }
        list
    }

    /// The shell's history, newest first, each command once, to type into
    /// the focused terminal.
    pub(in crate::window) fn shell_history_listing(&self) -> Listing {
        let mut list = Listing::new("shell_history", HISTORY_TITLE).placeholder("Command");
        for command in parse_history_lines(&unmetafy(&read_shell_history())) {
            let row = Row::new(RowKey(0), command.clone());
            list.push(Entry::new(row, WindowOutcome::TypeIntoTerminal(command)));
        }
        list
    }

    /// Show the buffer of tab `tab` in the focused pane and focus it: the
    /// pane's own tab on it where it has one, the buried buffer brought back,
    /// else a new view of a buffer only other panes show. A pane never has
    /// two tabs on one buffer. A terminal's tab is gone to wherever it is.
    pub(in crate::window) fn show_buffer(&mut self, tab: TabId) {
        if !self.open_tabs.is_page(&tab) {
            self.switch_to_tab(tab);
            return;
        }
        if !self.open_tabs.unbury(tab) {
            self.show_view_here(tab);
            return;
        }
        self.layout_mut().add_tab(tab);
        let outcome = self
            .open_tabs
            .page_mut(&tab)
            .map(|slot| slot.page.on_resume());
        if let Some(outcome) = outcome {
            self.act_on_editor_outcome(tab, outcome);
        }
        self.after_tab_change();
    }

    /// Go to the focused pane's tab on the buffer tab `tab` shows, or open a
    /// view of it there.
    fn show_view_here(&mut self, tab: TabId) {
        let pane = self.layout().tabs_in_pane(self.layout().focused());
        let here = self
            .open_tabs
            .views_of(&tab)
            .into_iter()
            .find(|view| pane.contains(view));
        if let Some(here) = here {
            self.switch_to_tab(here);
            return;
        }
        let view = self.alloc_tab_id();
        self.open_tabs.add_view(view, &tab);
        self.layout_mut().add_tab(view);
        self.after_tab_change();
    }

    /// One row per tab open in any pane, in pane order, each saying which
    /// pane it is in.
    fn open_tab_entries(&self) -> Vec<Entry> {
        let mut entries = Vec::new();
        for (pane_index, tabs) in self.layout().pane_members().into_iter().enumerate() {
            for tab in tabs {
                let row = self
                    .tab_row(tab)
                    .with_hint(format!("pane {}", pane_index + 1));
                entries.push(Entry::new(row, WindowOutcome::FocusTab(tab)));
            }
        }
        entries
    }

    /// One row per buried buffer, the most recently buried first.
    fn buried_entries(&self) -> Vec<Entry> {
        self.open_tabs
            .buried()
            .iter()
            .rev()
            .filter_map(|tab| self.buffer_entry(*tab, BURIED_HINT))
            .collect()
    }

    /// The row of the buffer of editor tab `tab`, `hint` at its right: shown
    /// in the focused pane by its first action, killed by its second.
    fn buffer_entry(&self, tab: TabId, hint: &str) -> Option<Entry> {
        let slot = self.open_tabs.page(&tab)?;
        let label = buffer_label(slot.kind(), slot.page.as_editor()?);
        let row = Row::new(RowKey(0), label)
            .with_hint(hint)
            .with_icon(tab_icon(slot.kind(), slot.page));
        let kill = Some(WindowOutcome::KillBuffer(tab));
        Some(Entry::new(row, WindowOutcome::ShowBuffer(tab)).with_actions(vec![kill]))
    }

    /// A tab's row: its full name and its icon.
    fn tab_row(&self, tab: TabId) -> Row {
        Row::new(RowKey(0), self.tab_full_name(tab)).with_icon(self.tab_icon_kind(tab))
    }

    /// Where the shells have been, newest first, from each terminal's record
    /// of the directory every command ran in.
    fn shell_dirs(&self) -> Vec<PathBuf> {
        self.open_tabs
            .terminals()
            .flat_map(|(_, terminal)| {
                terminal
                    .scrollback()
                    .blocks()
                    .iter()
                    .rev()
                    .filter_map(|block| block.cwd.as_deref())
                    .map(PathBuf::from)
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

// ========================================================================
// Functions
// ========================================================================

fn file_icon(path: &Path) -> IconKind {
    IconKind::File {
        name: file_name(path),
    }
}

/// A folder's icon, by its own name, so `src` and `.git` get their artwork.
fn dir_icon(path: &Path) -> IconKind {
    IconKind::Dir {
        expanded: false,
        name: file_name(path),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The shell's history file, as bytes: `$HISTFILE`, else zsh's or bash's in
/// the home directory, the first that has anything in it.
fn read_shell_history() -> Vec<u8> {
    let home = env::var_os("HOME").map(PathBuf::from);
    env::var_os("HISTFILE")
        .map(PathBuf::from)
        .into_iter()
        .chain(
            HISTORY_FILES
                .iter()
                .filter_map(|name| home.as_ref().map(|home| home.join(name))),
        )
        .filter_map(|path| fs::read(path).ok())
        .find(|contents| !contents.is_empty())
        .unwrap_or_default()
}

/// A history file's bytes as text: zsh's metafied bytes put back, and
/// anything still not UTF-8 replaced rather than costing the whole file.
fn unmetafy(contents: &[u8]) -> String {
    let mut bytes = Vec::with_capacity(contents.len());
    let mut meta = false;
    for &byte in contents {
        match (meta, byte) {
            (false, ZSH_META) => meta = true,
            (true, byte) => {
                bytes.push(byte ^ ZSH_META_FLIP);
                meta = false;
            }
            (false, byte) => bytes.push(byte),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The commands of a history file, newest first, each once: zsh's extended
/// lines read for their command, comments left out.
fn parse_history_lines(content: &str) -> Vec<String> {
    let joined = join_continued_lines(content);
    let commands: Vec<&str> = joined
        .iter()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            // zsh's extended history: ": 1234567890:0;command"
            if let Some(rest) = line.strip_prefix(": ") {
                return rest
                    .split_once(';')
                    .map(|(_, command)| command.trim())
                    .filter(|command| !command.is_empty());
            }
            Some(line)
        })
        .collect();
    let mut seen = HashSet::new();
    let mut newest_first: Vec<String> = commands
        .iter()
        .rev()
        .filter(|&&command| seen.insert(command))
        .map(|&command| command.to_string())
        .collect();
    newest_first.truncate(HISTORY_MAX);
    newest_first
}

/// The lines of a history file with each command written over several
/// lines (each but its last ending in `\`) joined back into one.
fn join_continued_lines(content: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut continuing = false;
    for line in content.lines() {
        match (continuing, lines.last_mut()) {
            (true, Some(last)) => {
                last.push('\n');
                last.push_str(line);
            }
            (true, None) | (false, _) => lines.push(line.to_string()),
        }
        continuing = line.ends_with('\\');
        if continuing {
            if let Some(last) = lines.last_mut() {
                last.pop();
            }
        }
    }
    lines
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_history_is_newest_first_each_command_once_zsh_stamps_dropped() {
        let commands = parse_history_lines("ls\n: 1700000000:0;pwd\nls\n# a note\ngit status\n");
        assert_eq!(commands, ["git status", "ls", "pwd"]);
    }

    #[test]
    fn test_a_command_over_several_lines_is_one() {
        let commands = parse_history_lines("echo a \\\nb\nls\n");
        assert_eq!(commands, ["ls", "echo a \nb"]);
    }

    #[test]
    fn test_a_zsh_history_with_metafied_bytes_is_read_not_skipped() {
        // `é` is 0xC3 0xA9; zsh writes the 0xA9 as 0x83 0x89.
        let text = unmetafy(&[
            b'e',
            b'c',
            b'h',
            b'o',
            b' ',
            0xC3,
            ZSH_META,
            0xA9 ^ ZSH_META_FLIP,
        ]);
        assert_eq!(text, "echo é");
    }
}
