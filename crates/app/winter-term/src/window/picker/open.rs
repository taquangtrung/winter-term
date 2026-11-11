//! The ways into the pickers the window offers, each opened by its chord or
//! command and closed by it again, and saving a search's rows to a buffer.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use misty::picker::{Row, RowKey, SavedLines};
use winter_base::project;

use super::outcome::{MuxNew, MuxRemote, MuxRequest, SaveHits, WorkspaceRequest};
use super::sources::{
    BrowserSource, Entry, FileSource, Listing, LocateSource, ProjectSearch, TodoSearch, Typed,
};
use super::WindowOutcome;
use crate::editors::grep::GrepEditor;
use crate::editors::list::ListEditor;
use crate::window::editor::EditorKind;
use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// How long the mux daemon is waited on for its sessions: this runs as the
/// picker opens, on the window's thread.
const MUX_QUERY_WAIT: Duration = Duration::from_millis(200);

/// What the session pickers list when the daemon holds none, or cannot be
/// reached; a row starting `(` is read, not chosen.
const NO_DAEMON: &str = "(no daemon running; start with 'winter mux serve')";
const NO_QUERY: &str = "(could not query sessions)";
const NO_SESSIONS: &str = "(no running sessions)";
const DEFAULT_SESSION: &str = "default (80x24)";

// ========================================================================
// App: opening
// ========================================================================

impl App {
    /// The file picker over the focused tab's project (`C-M-z`).
    pub(in crate::window) fn open_project_files(&mut self) {
        let root = project::root(&self.focused_start_dir());
        self.toggle_picker("project_files", |_| {
            Some(Box::new(FileSource::project(root)))
        });
    }

    /// The file picker over what is under the focused tab's directory.
    pub(in crate::window) fn open_find_file(&mut self) {
        let root = self.focused_start_dir();
        self.toggle_picker("find_file", |_| Some(Box::new(FileSource::under(root))));
    }

    /// Project search (`C-M-g`).
    pub(in crate::window) fn open_project_search(&mut self) {
        let root = project::root(&self.focused_start_dir());
        self.toggle_picker("project_search", |_| {
            Some(Box::new(ProjectSearch::new(root)))
        });
    }

    /// The Todo picker: the comment keywords across the focused tab's project.
    pub(in crate::window) fn open_todo_list(&mut self) {
        let root = project::root(&self.focused_start_dir());
        self.toggle_picker("todo", |_| Some(Box::new(TodoSearch::new(root))));
    }

    /// Locate: the whole disk, by name.
    pub(in crate::window) fn open_locate(&mut self) {
        self.toggle_picker("locate", |_| Some(Box::new(LocateSource::new())));
    }

    /// The recent list (`C-S-z`).
    pub(in crate::window) fn open_recent_list(&mut self) {
        self.toggle_picker("recent", |app| Some(Box::new(app.recent_listing())));
    }

    /// The buffer list (`C-x C-b`).
    pub(in crate::window) fn open_buffer_list(&mut self) {
        self.toggle_picker("buffers", |app| Some(Box::new(app.buffer_listing())));
    }

    /// The file browser, at `dir`.
    pub(in crate::window) fn open_browser_at(&mut self, dir: PathBuf) {
        self.open_picker(Box::new(BrowserSource::new(dir)));
    }

    /// The running mux sessions, to attach one.
    pub(in crate::window) fn open_mux_sessions(&mut self) {
        let listing = self.mux_listing(
            "mux_sessions",
            "Mux sessions: Enter attaches",
            DEFAULT_SESSION,
            MuxRequest::Attach,
        );
        self.open_picker(Box::new(listing));
    }

    /// The running mux sessions, to kill one.
    pub(in crate::window) fn open_mux_kill(&mut self) {
        let listing = self.mux_listing(
            "mux_kill",
            "Mux: kill a background session",
            NO_SESSIONS,
            MuxRequest::Kill,
        );
        self.open_picker(Box::new(listing));
    }

    /// A mux session to start, typed as `name [command]`.
    pub(in crate::window) fn open_mux_new(&mut self) {
        let typed = Typed::new("mux_new", "Mux: new session", |query| {
            let (name, command) = parse_mux_spawn_query(query)?;
            Some(WindowOutcome::Mux(MuxRequest::New(MuxNew {
                command,
                name,
            })))
        })
        .placeholder("Session name, then a command");
        self.open_picker(Box::new(typed));
    }

    /// A session on another host to attach, typed as `host [session]`.
    pub(in crate::window) fn open_mux_attach_remote(&mut self) {
        let typed = Typed::new("mux_attach", "Mux: attach a remote session", |query| {
            let (host, session) = parse_mux_attach_query(query)?;
            let session = session.unwrap_or_else(|| "default".to_string());
            Some(WindowOutcome::Mux(MuxRequest::AttachRemote(MuxRemote {
                host,
                session,
            })))
        })
        .placeholder("Host, then a session");
        self.open_picker(Box::new(typed));
    }

    /// A workspace's name, typed: given to the workspace being marked or
    /// renamed.
    pub(in crate::window) fn open_workspace_name(&mut self, initial: &str, title: String) {
        let typed = Typed::new("workspace_name", title, |query| {
            Some(WindowOutcome::Workspace(WorkspaceRequest::Name(
                query.to_string(),
            )))
        })
        .placeholder("Workspace name");
        self.open_typed(typed, initial);
    }

    /// A build's command line, typed, starting from `initial`: run as the
    /// project's build.
    pub(in crate::window) fn open_compile_command(&mut self, initial: &str, title: String) {
        let typed = Typed::new("compile_command", title, |query| {
            Some(WindowOutcome::RunTyped(query.to_string()))
        })
        .placeholder("Command line");
        self.open_typed(typed, initial);
    }

    /// Open the picker `typed`, its query starting as `initial`.
    fn open_typed(&mut self, typed: Typed, initial: &str) {
        self.open_picker(Box::new(typed));
        if let Some(picker) = self.chrome.picker_mut() {
            picker.paste(initial, std::time::Instant::now());
        }
    }

    /// Keep the lines a search found in a Grep buffer of its own, shown
    /// beside the tab the picker was opened from.
    pub(in crate::window) fn save_hits(&mut self, save: SaveHits) {
        let (kind, editor) = match save.todo {
            Some(keywords) => (
                EditorKind::Todo,
                GrepEditor::todo_from_hits(save.root, &keywords, save.hits, save.filtered),
            ),
            None => (
                EditorKind::Grep,
                GrepEditor::from_hits(save.root, save.query, save.hits, save.capped),
            ),
        };
        self.show_editor(kind, Box::new(editor));
    }

    /// Keep a picker's rows in a buffer of their own, named after the picker
    /// and what they were filtered by.
    pub(in crate::window) fn save_lines(&mut self, saved: SavedLines) {
        let editor = ListEditor::new(&saved.name, &saved.query, saved.lines);
        self.show_editor(EditorKind::List, Box::new(editor));
    }

    /// The mux sessions listed, each choosing `request` with its name; the
    /// daemon's own messages (`(no daemon running…)`) read, not chosen.
    fn mux_listing(
        &self,
        id: &'static str,
        title: &str,
        empty: &str,
        request: fn(String) -> MuxRequest,
    ) -> Listing {
        let mut listing = Listing::new(id, title)
            .placeholder("Session name")
            .resume(misty::picker::Resume::Never);
        for session in mux_session_entries(empty) {
            let row = Row::new(RowKey(0), session.clone());
            let name = session
                .split_whitespace()
                .next()
                .unwrap_or(&session)
                .to_string();
            let entry = match name.starts_with('(') {
                true => Entry::read_only(row),
                false => Entry::new(row, WindowOutcome::Mux(request(name))),
            };
            listing.push(entry);
        }
        listing
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The mux daemon's sessions, each as `name (colsxrows, up UPTIME, N
/// attached) - command`, its first word its name; `empty` when it holds
/// none, since attaching to a missing session creates it.
fn mux_session_entries(empty: &str) -> Vec<String> {
    let socket = winter_mux::server::default_socket_path();
    let Ok(mut client) = winter_mux::client::MuxClient::connect(&socket) else {
        return vec![NO_DAEMON.to_string()];
    };
    match client.query_sessions(MUX_QUERY_WAIT) {
        Ok(sessions) if sessions.is_empty() => vec![empty.to_string()],
        Ok(sessions) => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or(0);
            sessions
                .iter()
                .map(|s| {
                    let uptime = winter_mux::protocol::format_uptime(s.created, now);
                    format!(
                        "{} ({}x{}, up {uptime}, {} attached) - {}",
                        s.name, s.cols, s.rows, s.attach_count, s.command
                    )
                })
                .collect()
        }
        Err(_) => vec![NO_QUERY.to_string()],
    }
}

/// A typed new session taken apart: its name, and the command it runs (the
/// default shell when absent). `None` with no usable name.
fn parse_mux_spawn_query(query: &str) -> Option<(String, Option<String>)> {
    let mut words = query.split_whitespace();
    let name = words.next()?.to_string();
    if name.starts_with('(') || name.chars().any(char::is_control) {
        return None;
    }
    let command = words.collect::<Vec<_>>().join(" ");
    Some((name, (!command.is_empty()).then_some(command)))
}

/// A typed remote attach taken apart: the ssh host, and the session.
fn parse_mux_attach_query(query: &str) -> Option<(String, Option<String>)> {
    let mut words = query.split_whitespace();
    let host = words.next()?.to_string();
    if host.starts_with('(') || host.chars().any(char::is_control) {
        return None;
    }
    let session = words.collect::<Vec<_>>().join(" ");
    Some((host, (!session.is_empty()).then_some(session)))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use winter_model::editor::SearchHit;

    fn hits(root: &std::path::Path) -> Vec<SearchHit> {
        vec![SearchHit {
            line: 3,
            path: root.join("a.rs"),
            text: "let found = 1;".to_string(),
        }]
    }

    fn grep_tabs(app: &App, kind: EditorKind) -> usize {
        app.open_tabs
            .pages()
            .filter(|(_, slot)| slot.kind() == kind)
            .count()
    }

    #[test]
    fn test_each_save_makes_a_grep_buffer_of_its_own_named_after_the_query() {
        let mut app = App::new();
        let root = std::env::temp_dir();
        let save = SaveHits {
            capped: false,
            filtered: None,
            hits: hits(&root),
            query: "found".to_string(),
            root: root.clone(),
            todo: None,
        };
        app.save_hits(save.clone());
        app.save_hits(save);
        assert_eq!(
            grep_tabs(&app, EditorKind::Grep),
            2,
            "two saves, two buffers"
        );
        let focused = app.layout().focused();
        let title = app.open_tabs.page(&focused).expect("shown").page.title();
        assert!(title.contains("found"), "{title}");
    }

    #[test]
    fn test_two_saved_searches_are_told_apart_in_the_buffer_list_by_their_queries() {
        let mut app = App::new();
        let root = std::env::temp_dir();
        for query in ["alpha", "beta"] {
            app.save_hits(SaveHits {
                capped: false,
                filtered: None,
                hits: hits(&root),
                query: query.to_string(),
                root: root.clone(),
                todo: None,
            });
        }
        let labels: Vec<String> = app
            .open_tabs
            .pages()
            .filter(|(_, slot)| slot.kind() == EditorKind::Grep)
            .map(|(_, slot)| {
                crate::window::editor::buffers::buffer_label(
                    slot.kind(),
                    slot.page.as_editor().expect("an editor"),
                )
            })
            .collect();
        assert!(
            labels.iter().any(|label| label.contains("alpha")),
            "{labels:?}"
        );
        assert!(
            labels.iter().any(|label| label.contains("beta")),
            "{labels:?}"
        );
    }

    #[test]
    fn test_rows_saved_as_lines_open_in_a_buffer_named_by_the_picker_and_its_filter() {
        let mut app = App::new();
        app.save_lines(SavedLines {
            lines: vec!["cargo build".to_string(), "cargo test".to_string()],
            name: "Shell history".to_string(),
            query: "cargo".to_string(),
        });
        let focused = app.layout().focused();
        let slot = app.open_tabs.page(&focused).expect("shown");
        assert_eq!(slot.kind(), EditorKind::List);
        let editor = slot.page.as_editor().expect("a buffer");
        assert_eq!(
            crate::window::editor::buffers::buffer_label(slot.kind(), editor),
            "Shell history: cargo",
            "no kind put in front of the picker's name"
        );
    }

    #[test]
    fn test_a_listing_is_named_by_its_title_without_the_hint_after_it() {
        use misty::picker::Source;
        let listing = super::super::sources::listing::Listing::new(
            "recent_dirs",
            "Recent directories: Enter changes to one",
        );
        assert_eq!(listing.name(), "Recent directories");
    }

    #[test]
    fn test_a_saved_todo_list_says_what_its_rows_were_filtered_by() {
        let mut app = App::new();
        let root = std::env::temp_dir();
        app.save_hits(SaveHits {
            capped: false,
            filtered: Some("cache".to_string()),
            hits: hits(&root),
            query: "TODO".to_string(),
            root,
            todo: Some(vec!["TODO".to_string()]),
        });
        assert_eq!(grep_tabs(&app, EditorKind::Todo), 1);
        let focused = app.layout().focused();
        let slot = app.open_tabs.page_mut(&focused).expect("shown");
        let frame = slot.page.content(5, 80, false);
        let header = winter_model::paint::row_text(&frame.rows[0]);
        assert!(header.contains("filtered by cache"), "{header}");
    }

    #[test]
    fn test_the_side_panel_is_hidden_until_f6_shows_it_and_then_remembered() {
        let mut app = App::new();
        app.open_recent_list();
        assert!(
            !app.chrome.picker().expect("open").view().panel,
            "hidden at first"
        );
        app.press_in_picker("F6");
        assert!(app.chrome.picker().expect("open").view().panel);
        app.press_in_picker("ESC");
        app.open_recent_list();
        assert!(
            app.chrome.picker().expect("open").view().panel,
            "shown again next time"
        );
    }

    #[test]
    fn test_the_picker_keys_are_listed_with_every_editors() {
        for block in [
            "picker",
            "picker.buffers",
            "picker.files",
            "picker.search",
            "picker.browser",
        ] {
            assert!(
                crate::editors::key_table(block).is_some(),
                "{block} is listed"
            );
        }
    }

    #[test]
    fn test_choosing_a_row_remembers_the_query_and_the_rows_use() {
        let mut app = App::new();
        let focused = app.layout().focused();
        app.handle_action(winter_model::input::Action::TogglePalette, focused);
        app.settle_picker();
        app.press_in_picker("n e w SPC t a b RET");
        assert_eq!(app.pickers.history.of("commands"), ["new tab"]);
        assert!(
            app.pickers.recency.rank("commands", "new_tab") > 0,
            "its use is remembered"
        );
    }

    #[test]
    fn test_a_new_session_is_its_name_then_its_command() {
        assert_eq!(
            parse_mux_spawn_query("work htop -d 5"),
            Some(("work".to_string(), Some("htop -d 5".to_string())))
        );
        assert_eq!(
            parse_mux_spawn_query("work"),
            Some(("work".to_string(), None))
        );
        assert_eq!(parse_mux_spawn_query("(placeholder)"), None);
        assert_eq!(parse_mux_spawn_query(""), None);
    }

    #[test]
    fn test_a_remote_attach_is_its_host_then_its_session() {
        assert_eq!(
            parse_mux_attach_query("box main"),
            Some(("box".to_string(), Some("main".to_string())))
        );
        assert_eq!(parse_mux_attach_query("box\u{7}"), None);
    }
}
