//! One source per picker, and their actions' key tables.
//!
//! - `browser`: the file browser, a directory at a time.
//! - `files`: the file pickers, a project's files or a directory's.
//! - `listing`: a fixed list of rows the window built, each doing what its
//!   outcomes say; most pickers are one.
//! - `lists`: the listings over what the window holds: recent, buffers,
//!   panes, directories, the clipboard ring, the shell's history.
//! - `locate`: the whole disk, searched by name.
//! - `scout`: the live pickers, Scout and the outline.
//! - `search`: project search and the Todo picker.
//! - `stream`: rows streamed from a program's output.
//! - `typed`: a picker whose query is the answer.
//!
//! A source's actions have a key table of their own, under the picker's
//! block (`picker.buffers`), with no keys until the user binds some: a key
//! bound there runs its action on the selected row, with no menu.

mod browser;
mod extension;
mod files;
pub(in crate::window) mod listing;
mod lists;
mod locate;
mod scout;
mod search;
mod stream;
pub(in crate::window) mod typed;

#[cfg(feature = "wasm-extensions")]
pub(in crate::window) use browser::Chooser;
pub(in crate::window) use browser::{create_file, BrowserSource};
pub(in crate::window) use extension::{ExtensionChoice, ExtensionSource, PickerLink};
pub(in crate::window) use files::FileSource;
pub(in crate::window) use listing::{Entry, Listing};
pub(in crate::window) use locate::LocateSource;
pub(in crate::window) use scout::{color_scout_rows, ScoutLines, SCOUT_PICKER};
pub(in crate::window) use search::{ProjectSearch, TodoSearch};
pub(in crate::window) use typed::Typed;

use winter_model::input::table::{find, Binding, KeyTable, Listed};
use winter_model::input::Key;
use winter_picker::{ActionId, Source};

use crate::window::picker::WindowOutcome;
use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// The buffers' actions.
const BUFFER_KEYS: KeyTable<ActionId> = KeyTable {
    block: "picker.buffers",
    bindings: &[Binding {
        action: lists::KILL_BUFFER.id,
        id: "kill_buffer",
        keys: &[],
        label: "Kill the Selected Buffer",
    }],
    configurable: true,
    title: "Picker: Buffers",
};

/// The file pickers' actions.
const FILE_KEYS: KeyTable<ActionId> = KeyTable {
    block: "picker.files",
    bindings: &[
        Binding {
            action: files::OPEN,
            id: "open",
            keys: &[],
            label: "Open the Selected File",
        },
        Binding {
            action: files::OPEN_BESIDE,
            id: "open_beside",
            keys: &[],
            label: "Open the Selected File Beside",
        },
    ],
    configurable: true,
    title: "Picker: Files",
};

/// The searches' actions.
const SEARCH_KEYS: KeyTable<ActionId> = KeyTable {
    block: "picker.search",
    bindings: &[
        Binding {
            action: files::OPEN,
            id: "open",
            keys: &[],
            label: "Open at the Selected Line",
        },
        Binding {
            action: files::OPEN_BESIDE,
            id: "open_beside",
            keys: &[],
            label: "Open at the Selected Line Beside",
        },
    ],
    configurable: true,
    title: "Picker: Search",
};

/// The sources' action tables, for the one list the Keys editor shows and
/// `keybindings.kdl` rebinds by.
pub(crate) const KEY_TABLES: &[&dyn Listed] =
    &[&browser::KEYS, &BUFFER_KEYS, &FILE_KEYS, &SEARCH_KEYS];

// ========================================================================
// Functions
// ========================================================================

/// The action a key is bound to in the table of the picker named `id`.
pub(in crate::window) fn action_for(id: &str, key: &Key) -> Option<ActionId> {
    let table = match id {
        "buffers" => &BUFFER_KEYS,
        "find_file" | "project_files" => &FILE_KEYS,
        "project_search" | "todo" => &SEARCH_KEYS,
        _ => return None,
    };
    find(table, key)
}

/// A fresh source for the picker named `id`, when it lists what the window
/// holds and so cannot read it again itself on resume; `None` for one that
/// can.
pub(in crate::window) fn rebuild(
    app: &mut App,
    id: &str,
) -> Option<Box<dyn Source<WindowOutcome>>> {
    let focused = app.layout().focused();
    let listing = match id {
        "buffers" => app.buffer_listing(),
        "clip_ring" => {
            let copied = app.clip_ring_entries();
            app.clip_ring_listing(&copied)
        }
        "commands" => crate::window::commands::command_listing(&app.keymap),
        "panes" => app.pane_listing(),
        "recent" => app.recent_listing(),
        "recent_dirs" => app.recent_dir_listing(focused),
        "shell_history" => app.shell_history_listing(),
        _ => return None,
    };
    Some(Box::new(listing))
}
