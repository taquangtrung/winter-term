//! Tabs within a pane: opening, showing, cycling, reordering, and closing them,
//! and the recency walk across the focused pane's strip.

use std::collections::HashMap;

use crate::model::layout::{FocusDir, Layout, Rect, TabId};
use crate::terminal::session::Terminal;

use super::strip::StripHit;
use super::tool::{Closing, ToolKind};
use super::App;
use super::{DEFAULT_COLS, DEFAULT_ROWS};
use winter_render::TabbarHit;

// ========================================================================
// Data Structures
// ========================================================================

/// The window's layout of panes and the state of the strips drawn over
/// them: hover, an in-progress drag, the rename prompt, and a recency walk.
pub(crate) struct TabsState {
    /// The tab being dragged along its strip and the pointer (x, y) position when
    /// the drag began. `None` when no drag is in progress. Cleared on mouse
    /// release.
    pub(crate) drag_start: Option<(TabId, f32, f32)>,
    /// Title-bar element currently under the cursor; drives hover highlights.
    pub(crate) hover: TabbarHit,
    pub(crate) hover_pos: Option<(f32, f32)>,
    /// The split tree of panes.
    pub(crate) layout: Layout,
    /// What each tab is called, by the user and by its program.
    pub(crate) labels: HashMap<TabId, TabLabel>,
    /// The focused pane's recency order, frozen while the recency commands
    /// step through it, so repeated steps walk usage history instead of
    /// reshuffling it. `None` once a deliberate switch ends the walk.
    pub(crate) recent_walk: Option<RecentWalk>,
    /// Pane-strip element currently under the cursor; drives hover highlights.
    pub(crate) strip_hover: StripHit,
}

/// What a tab is called: the name the user gave it, which wins, and the
/// title its program set (OSC 0/2).
#[derive(Clone, Debug, Default)]
pub(crate) struct TabLabel {
    pub(crate) name: Option<String>,
    pub(crate) title: Option<String>,
}

/// A recency walk in progress: the order it walks and where it has got to.
pub(crate) struct RecentWalk {
    pub(crate) at: usize,
    pub(crate) order: Vec<TabId>,
}

impl Default for TabsState {
    /// A fresh window: one pane holding one tab.
    fn default() -> Self {
        Self {
            drag_start: None,
            hover: TabbarHit::None,
            hover_pos: None,
            layout: Layout::new(),
            labels: HashMap::new(),
            recent_walk: None,
            strip_hover: StripHit::None,
        }
    }
}

// ========================================================================
// TabsState Implementations
// ========================================================================

impl TabsState {
    /// The name the user gave tab `tab`, if any.
    pub(crate) fn name(&self, tab: TabId) -> Option<&str> {
        self.labels.get(&tab)?.name.as_deref()
    }

    /// Name tab `tab`, or drop its name with `None` or an empty one.
    pub(crate) fn set_name(&mut self, tab: TabId, name: Option<String>) {
        self.labels.entry(tab).or_default().name = name.filter(|name| !name.is_empty());
    }

    /// The title tab `tab`'s program set, if it set one.
    pub(crate) fn program_title(&self, tab: TabId) -> Option<&str> {
        self.labels.get(&tab)?.title.as_deref()
    }

    /// Record the title tab `tab`'s program set.
    pub(crate) fn set_program_title(&mut self, tab: TabId, title: String) {
        self.labels.entry(tab).or_default().title = Some(title);
    }
}

// ========================================================================
// App: tabs
// ========================================================================

impl App {
    // --------------------------------------------------------------------
    // Layout access
    // --------------------------------------------------------------------

    /// The window's layout.
    pub(crate) fn layout(&self) -> &Layout {
        &self.tabs.layout
    }

    /// The window's layout, mutably.
    pub(crate) fn layout_mut(&mut self) -> &mut Layout {
        &mut self.tabs.layout
    }

    // --------------------------------------------------------------------
    // Opening
    // --------------------------------------------------------------------

    /// Open a new terminal tab beside the focused one and show it.
    pub(crate) fn new_tab(&mut self) {
        let id = self.alloc_tab_id();
        // Open the new tab where the focused tab was being used rather than
        // at the process default (usually `$HOME`): beside a tool that is the
        // directory the tool is looking at, not a shell's own.
        let cwd = self.focused_start_cwd();
        let (cols, rows) = self.focused_grid_size();
        let shell = self.config.active_shell().map(String::from);
        let scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_render::MAX_SCROLLBACK);
        let Some(terminal) = self.spawn_terminal_or_notify(
            cols.max(1),
            rows.max(1),
            shell.as_deref(),
            scrollback,
            cwd.as_deref(),
        ) else {
            return;
        };
        self.push_new_tab(id, terminal);
    }

    /// Open a new window running the binary in a new process.
    pub(crate) fn new_window(&mut self) {
        let Some(exe) = crate::terminal::session::runnable_current_exe() else {
            self.set_error("new_window: could not determine binary path".to_string());
            return;
        };
        let mut cmd = std::process::Command::new(exe);
        if let Some(cwd) = self.focused_start_cwd() {
            cmd.env("WINTER_CWD", &cwd);
            cmd.current_dir(&cwd);
        }
        // The new window keeps no terminal of this one's: it reads no input
        // from it and writes nowhere it is watching.
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        match cmd.spawn() {
            // Waited for on a thread of its own, so a window closed before
            // this one is reaped then, not left a zombie until this one exits.
            Ok(mut child) => {
                let reaper = std::thread::Builder::new()
                    .name("winter window reap".into())
                    .spawn(move || {
                        let _ = child.wait();
                    });
                if let Err(e) = reaper {
                    eprintln!("winter: could not watch the new window: {e}");
                }
            }
            Err(e) => self.set_error(format!("failed to spawn new window: {e}")),
        }
    }

    /// Open a new tab whose terminal is attached to a running mux session;
    /// the session's buffered output replays into it.
    pub(crate) fn new_mux_tab(&mut self, session: &str) {
        self.new_mux_tab_at(&crate::mux::server::default_socket_path(), session);
    }

    /// Open a new tab attached to a session on the mux server at `path`.
    pub(crate) fn new_mux_tab_at(&mut self, path: &str, session: &str) {
        let id = self.alloc_tab_id();
        let (cols, rows) = self.focused_grid_size();
        let scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_render::MAX_SCROLLBACK);
        match Terminal::new_mux_at(path, cols.max(1), rows.max(1), session, scrollback) {
            Ok(terminal) => {
                self.set_notice(format!("attached to mux session '{session}'"));
                self.push_new_tab(id, terminal);
            }
            Err(e) => self.set_error(format!(
                "could not attach to '{session}' ({e}); start the server with 'winter mux serve'"
            )),
        }
    }

    /// Open a new tab attached to a session on a mux server reached over ssh
    /// at `host`.
    pub(crate) fn new_mux_tab_remote_at(&mut self, host: &str, session: &str) {
        let id = self.alloc_tab_id();
        let (cols, rows) = self.focused_grid_size();
        let scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_render::MAX_SCROLLBACK);
        match Terminal::new_mux_remote(host, cols.max(1), rows.max(1), session, scrollback) {
            Ok(terminal) => {
                self.set_notice(format!("attached to '{host}:{session}'"));
                self.push_new_tab(id, terminal);
            }
            Err(e) => self.set_error(format!("could not reach '{host}' over ssh ({e})")),
        }
    }

    /// Install an already-spawned terminal as a new tab beside the focused
    /// one and show it. Shared by every way a terminal tab is opened so all of
    /// them get the same bookkeeping: mode default, tile repositioning,
    /// resize, and title update.
    pub(crate) fn push_new_tab(&mut self, id: TabId, terminal: Terminal) {
        self.open_tabs.insert_terminal(id, terminal);
        self.layout_mut().add_tab(id);
        self.after_tab_change();
    }

    /// The grid a new tab in the focused pane starts at, before
    /// [`Self::resize_all_panes`] fixes the exact size once it is placed.
    fn focused_grid_size(&self) -> (usize, usize) {
        let focused = self.layout().focused();
        if let Some((_, rect)) = self.pane_rects().into_iter().find(|(id, _)| *id == focused) {
            if let Some(renderer) = &self.renderer {
                return renderer.grid_size_for(Self::layout_rect_to_pane(rect));
            }
        }
        self.renderer
            .as_ref()
            .map(|r| r.grid_size())
            .unwrap_or((DEFAULT_COLS as usize, DEFAULT_ROWS as usize))
    }

    // --------------------------------------------------------------------
    // Switching
    // --------------------------------------------------------------------

    /// Show `pane` as a deliberate selection, wherever it is, ending any
    /// recency walk.
    pub(crate) fn show_tab(&mut self, tab: TabId) {
        if !self.set_focus(tab) {
            return;
        }
        self.tabs.recent_walk = None;
        self.after_tab_change();
    }

    /// Show the most recently shown terminal of the focused pane, for the way
    /// back from a tool tab to the shell it was opened from. A terminal that
    /// is already showing stays showing; a pane with none gets a new one,
    /// started in the directory its tool is looking at.
    pub(crate) fn reveal_recent_terminal(&mut self) {
        let focused = self.layout().focused();
        let terminal = self
            .layout()
            .recent_in_pane(focused)
            .into_iter()
            .find(|tab| self.open_tabs.is_terminal(tab) && !self.open_tabs.is_tool(tab));
        match terminal {
            Some(tab) => self.show_tab(tab),
            None => self.new_tab(),
        }
    }

    /// Show the tab at strip position `index` (0-based) of the focused pane.
    pub(crate) fn switch_tab(&mut self, index: usize) {
        let members = self.layout().tabs_in_pane(self.layout().focused());
        if let Some(&pane) = members.get(index) {
            self.show_tab(pane);
        }
    }

    /// Show the next (`forward`) or previous tab of the focused pane by
    /// strip position, wrapping around.
    pub(crate) fn cycle_tab(&mut self, forward: bool) {
        if self.layout_mut().cycle_tab(forward) {
            self.tabs.recent_walk = None;
            self.after_tab_change();
        }
    }

    /// Step through the focused pane's tabs in most-recently-shown order:
    /// `forward` steps toward more recently shown, otherwise toward less,
    /// wrapping around. The order is held still across consecutive calls (a
    /// "walk") so the user can step back and forth through usage history; the
    /// next deliberate switch ends the walk.
    pub(crate) fn recent_tab(&mut self, forward: bool) {
        let focused = self.layout().focused();
        let walk = match self.tabs.recent_walk.take() {
            Some(walk) if walk.order.contains(&focused) => walk,
            _ => RecentWalk {
                at: 0,
                order: self.layout().recent_in_pane(focused),
            },
        };
        let count = walk.order.len();
        if count <= 1 {
            return;
        }
        let at = match forward {
            true => (walk.at + count - 1) % count,
            false => (walk.at + 1) % count,
        };
        let tab = walk.order[at];
        self.set_focus(tab);
        self.tabs.recent_walk = Some(RecentWalk {
            at,
            order: walk.order,
        });
        self.after_tab_change();
    }

    /// Move the focused tab one place along its strip.
    pub(crate) fn move_tab(&mut self, forward: bool) {
        if self.layout_mut().move_tab(forward) {
            self.dirty = true;
        }
    }

    /// Move `pane` to where `target` sits on its strip or across panes.
    pub(crate) fn move_tab_to(&mut self, tab: TabId, target: TabId) {
        if self.layout_mut().move_tab_to(tab, target) {
            self.after_tab_change();
        }
    }

    /// Move `tab` into the pane holding `target_pane_tab`, at the end of that pane.
    pub(crate) fn move_tab_to_pane(&mut self, tab: TabId, target_pane_tab: TabId) {
        if self.layout_mut().move_tab_to_pane(tab, target_pane_tab) {
            self.after_tab_change();
        }
    }

    /// Split `target_pane_tab`'s pane to make `tab` a new pane in `direction`.
    pub(crate) fn split_tab_to_new_pane(
        &mut self,
        tab: TabId,
        target_pane_tab: TabId,
        direction: FocusDir,
    ) {
        if self
            .layout_mut()
            .split_tab_to_new_pane(tab, target_pane_tab, direction)
        {
            self.after_tab_change();
        }
    }

    /// Move the focused tab into the neighbouring pane in the given direction.
    /// If there is no pane in that direction, make that tab become the new pane.
    pub(crate) fn move_tab_in_direction(&mut self, direction: FocusDir) {
        if self.layout().is_zoomed() {
            return;
        }
        let viewport = self.viewport_rect();
        let layout_vp = Rect::new(viewport.x, viewport.y, viewport.width, viewport.height);
        let focused = self.layout().focused();
        if let Some(target) = self.layout().pane_in_direction(direction, layout_vp) {
            self.move_tab_to_pane(focused, target);
        } else if self.layout_mut().move_tab_to_new_pane(focused, direction) {
            self.after_tab_change();
        }
    }

    // --------------------------------------------------------------------
    // Closing
    // --------------------------------------------------------------------

    /// Close one tab. Closing the last tab of the window asks for the app to
    /// exit instead.
    ///
    /// Unwritten edits in a tool tab are asked about first, and the tab
    /// closes on the answer rather than here.
    pub(crate) fn close_tab(&mut self, tab: TabId) {
        if self.ask_before_closing_tab(tab) {
            return;
        }
        self.close_tab_now(tab, Closing::Keep);
    }

    /// Close every other tab in the pane holding `keep`, leaving that tab.
    pub(crate) fn close_other_tabs(&mut self, keep: TabId) {
        if self.ask_before_closing_other_tabs(keep) {
            return;
        }
        self.close_other_tabs_now(keep);
    }

    /// The same, once the question has been answered or there was none.
    pub(crate) fn close_other_tabs_now(&mut self, keep: TabId) {
        let others: Vec<TabId> = self
            .layout()
            .tabs_in_pane(keep)
            .into_iter()
            .filter(|tab| *tab != keep)
            .collect();
        for tab in others {
            self.close_tab_now(tab, Closing::Keep);
        }
    }

    /// Close one tab without asking, keeping what it held for reopening or
    /// not. A pane left with no tabs merges back into its sibling.
    pub(crate) fn close_tab_now(&mut self, tab: TabId, closing: Closing) {
        if !self.layout().contains(tab) {
            return;
        }
        if self.layout().members().len() <= 1 {
            self.exit_requested = true;
            return;
        }
        let snapshot = self.layout().export_tree();
        let pane_gone = self.layout().tabs_in_pane(tab).len() <= 1;
        let is_commit_editor = self.commit_companions.contains_key(&tab)
            || self.open_tabs.tool(&tab).is_some_and(|slot| {
                slot.kind() == ToolKind::Editor && slot.tool.is_commit_editor()
            });
        let commit_repo = if is_commit_editor {
            self.open_tabs
                .tool(&tab)
                .and_then(|slot| slot.tool.cwd())
                .map(|p| {
                    if p.file_name().is_some_and(|n| n == ".git") {
                        p.parent().unwrap_or(&p).to_path_buf()
                    } else {
                        p
                    }
                })
        } else {
            None
        };
        if let Some(slot) = self.open_tabs.remove_tool(&tab) {
            if closing == Closing::Keep {
                self.stash_closed_tool(tab, slot);
            }
        } else if closing == Closing::Keep {
            self.stash_closed_pane(&[tab], snapshot, Vec::new(), pane_gone);
        }
        self.drop_tab_state(tab);
        self.layout_mut().close(tab);
        if pane_gone {
            // Rebalance the remaining panes so they stay evenly spaced,
            // without reshaping the tree.
            self.layout_mut().balance();
        }
        self.after_tab_change();
        if let Some(companion) = self.commit_companions.remove(&tab) {
            self.close_tab_now(companion, Closing::Forget);
        }
        if let Some(repo) = commit_repo {
            self.focus_or_open_git_tool(repo);
        }
    }

    /// Forget everything kept about one tab: its terminal or its tool, and
    /// every per-tab record the rest of the app holds under its id.
    pub(crate) fn drop_tab_state(&mut self, tab: TabId) {
        // Work the tab asked for would come back to a tab that is gone, and
        // with its id given back out on a restore, to the wrong one.
        self.jobs.cancel_for(tab);
        self.commit_companions
            .retain(|_, companion| *companion != tab);
        self.open_tabs.remove_tool(&tab);
        self.drop_overlay_of(tab);
        if self
            .search_debounce
            .as_ref()
            .is_some_and(|(_, t)| *t == tab)
        {
            self.search_debounce = None;
        }
        if self.tool_cursor.as_ref().is_some_and(|c| c.tab == tab) {
            self.stop_tool_cursor();
        }
        self.open_tabs.remove(&tab);
        self.vim.tabs.remove(&tab);
        self.tabs.labels.remove(&tab);
        self.retain_image_blocks(|img| img.tab_id != tab);
        if self.selection.span.as_ref().is_some_and(|s| s.tab == tab) {
            self.selection.span = None;
        }
    }

    // --------------------------------------------------------------------
    // Mux
    // --------------------------------------------------------------------

    /// Spawn a named session on the mux server at `path`, running
    /// `command` (or the default shell) in `cwd`, then open a tab attached
    /// to it. The spawn is confirmed with a bounded wait (the server answers
    /// on its poll cycle) before attaching, so failures surface as notices
    /// instead of a tab pointing at a session that never existed.
    pub(crate) fn spawn_mux_tab_at(
        &mut self,
        path: &str,
        name: &str,
        command: Option<&str>,
        cwd: Option<&str>,
        focused: TabId,
    ) {
        let mut client = match crate::mux::client::MuxClient::connect(path) {
            Ok(client) => client,
            Err(_) => {
                self.set_error("could not connect to mux daemon");
                return;
            }
        };
        // Size the session to the focused pane so the first frame fits.
        let (cols, rows) = self
            .open_tabs
            .terminal(&focused)
            .map(|terminal| {
                (
                    terminal.grid().cols().max(1) as u16,
                    terminal.grid().rows().max(1) as u16,
                )
            })
            .unwrap_or((80, 24));
        match client.spawn_confirmed(
            name,
            cols,
            rows,
            cwd,
            command,
            std::time::Duration::from_millis(200),
        ) {
            Ok((cols, rows)) => {
                self.new_mux_tab_at(path, name);
                let note = match command {
                    Some(command) => format!("started '{name}' ({cols}x{rows}): {command}"),
                    None => format!("started '{name}' ({cols}x{rows})"),
                };
                self.set_notice(note);
            }
            Err(message) => self.set_error(format!("mux: {message}")),
        }
    }

    // --------------------------------------------------------------------
    // Relayout
    // --------------------------------------------------------------------

    /// Re-lay-out panes after a change to the reserved top-tabbar or status-bar
    /// rows (menu style, status-bar visibility), and request a redraw.
    pub(crate) fn relayout_tabbar(&mut self) {
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
    }

    /// Bring everything that depends on which tabs are shown up to date after
    /// one was opened, shown, or closed.
    pub(crate) fn after_tab_change(&mut self) {
        self.selection.span = self
            .selection
            .span
            .take()
            .filter(|s| self.layout().shown_tabs().contains(&s.tab));
        self.close_menu();
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
        self.update_window_title();
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::*;
    use std::time::{Duration, Instant};

    #[test]
    fn test_revealing_a_terminal_in_a_pane_without_one_opens_one() {
        // A pane holding only tools used to answer with a notice, which left
        // the key doing nothing where a shell was most wanted.
        let mut app = App::new();
        let tool = app.show_tool(
            crate::app::tool::ToolKind::Dir,
            Box::new(crate::tools::dir::DirTool::new(std::env::temp_dir())),
        );
        assert!(!app.open_tabs.is_terminal(&app.layout().focused()));

        app.reveal_recent_terminal();
        let shown = app.layout().focused();
        assert_ne!(shown, tool);
        assert!(
            app.open_tabs.is_terminal(&shown),
            "a new terminal is showing"
        );
        assert!(
            app.layout().tabs_in_pane(shown).contains(&tool),
            "in the same pane as the tool"
        );
    }

    #[test]
    fn test_closing_a_tab_forgets_everything_kept_under_its_id() {
        // A restore gives the id back out, and whatever was left under it
        // would belong to the tab that comes back.
        let mut app = App::new();
        let tab = app.layout().focused();
        app.vim.tab_mut(tab).marks.insert('a', (1, 0));
        app.vim.tab_mut(tab).jump_list.push((2, 0));
        app.tabs.set_name(tab, Some("work".to_string()));
        app.tabs.set_program_title(tab, "vim".to_string());

        app.drop_tab_state(tab);

        assert!(app.vim.tab(tab).is_none());
        assert_eq!(app.tabs.name(tab), None);
        assert_eq!(app.tabs.program_title(tab), None);
    }

    #[test]
    fn test_an_empty_name_puts_the_tab_back_to_its_own_title() {
        let mut state = TabsState::default();
        let tab = TabId(3);
        state.set_name(tab, Some("work".to_string()));
        state.set_name(tab, Some(String::new()));
        assert_eq!(state.name(tab), None);
    }

    #[test]
    fn test_new_mux_tab_attaches_and_replays() {
        use crate::mux::protocol::{self, ServerMessage};
        use std::io::{Read, Write};
        #[cfg(unix)]
        use std::os::unix::net::UnixListener;
        #[cfg(windows)]
        use uds_windows::UnixListener;

        fn read_frame(conn: &mut impl Read) -> Vec<u8> {
            let mut len_bytes = [0u8; 4];
            conn.read_exact(&mut len_bytes).unwrap();
            let mut framed = len_bytes.to_vec();
            let mut body = vec![0u8; u32::from_be_bytes(len_bytes) as usize];
            conn.read_exact(&mut body).unwrap();
            framed.extend(body);
            framed
        }

        let path =
            std::env::temp_dir().join(format!("winter-app-mux-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let socket_path = path.to_string_lossy().to_string();

        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let _attach = read_frame(&mut conn); // pane's Attach
            for frame in [
                protocol::encode(&ServerMessage::Attached {
                    session: "work".into(),
                    cols: 80,
                    rows: 24,
                }),
                protocol::encode(&ServerMessage::Output {
                    session: "work".into(),
                    bytes: b"mux tab live\n".to_vec(),
                }),
            ] {
                conn.write_all(&frame).unwrap();
            }
            // Consume the attach-time Resize, then close.
            let _ = read_frame(&mut conn);
        });

        let mut app = App::new();
        app.new_mux_tab_at(&socket_path, "work");
        // The bootstrap tab plus the new mux tab, side by side in one pane.
        assert_eq!(app.layout().members().len(), 2);
        let focused = app.layout().focused();
        assert_eq!(
            app.open_tabs.terminal(&focused).unwrap().mux_session(),
            Some("work")
        );

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut text = String::new();
        while Instant::now() < deadline {
            if let Some(terminal) = app.open_tabs.terminal_mut(&focused) {
                terminal.drain_output();
            }
            text = app.open_tabs.terminal(&focused).unwrap().grid().to_text();
            if text.contains("mux tab live") {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            text.contains("mux tab live"),
            "attached pane must render session output, got: {text}"
        );
        assert_eq!(app.tab_title(focused), "mux: work");

        // Detaching closes the pane's tab view; the session itself lives
        // on the server, out of the app's hands.
        app.run_command("mux_detach_session", focused);
        assert_eq!(
            app.layout().members().len(),
            1,
            "the mux tab must be closed"
        );
        assert!(!app.open_tabs.is_terminal(&focused));
        assert!(app.notice_text().is_some());

        let _ = server.join();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_closing_the_shown_tab_shows_the_one_shown_before_it() {
        let mut app = app_with_tabs(3);
        app.switch_tab(2);
        app.close_tab(TabId(2));
        assert_eq!(app.layout().members(), vec![TabId(0), TabId(1)]);
        assert_eq!(app.layout().focused(), TabId(0));
    }

    #[test]
    fn test_switch_tab_shows_the_tab_at_that_strip_position() {
        let mut app = app_with_tabs(3);
        app.switch_tab(2);
        assert_eq!(app.layout().focused(), TabId(2));
        assert_eq!(
            app.layout().recent_in_pane(TabId(2)),
            vec![TabId(2), TabId(0), TabId(1)]
        );
        assert!(app.tabs.recent_walk.is_none());
    }

    #[test]
    fn test_recent_tab_walks_backward_and_forward_without_reshuffling() {
        let mut app = app_with_tabs(3);
        app.switch_tab(2);
        app.switch_tab(1); // Recency now [1, 2, 0], showing 1.

        // Backward steps toward less recently shown, holding the order still.
        app.recent_tab(false);
        assert_eq!(app.layout().focused(), TabId(2));
        app.recent_tab(false);
        assert_eq!(app.layout().focused(), TabId(0));

        // Forward steps back toward more recently shown.
        app.recent_tab(true);
        assert_eq!(app.layout().focused(), TabId(2));
        let walk = app.tabs.recent_walk.as_ref().unwrap();
        assert_eq!(walk.order, vec![TabId(1), TabId(2), TabId(0)]);
    }

    #[test]
    fn test_deliberate_switch_ends_the_recency_walk() {
        let mut app = app_with_tabs(3);
        app.switch_tab(1);
        app.recent_tab(false);
        assert!(app.tabs.recent_walk.is_some());
        app.switch_tab(2);
        assert!(app.tabs.recent_walk.is_none());
        assert_eq!(app.layout().recent_in_pane(TabId(2))[0], TabId(2));
    }

    #[test]
    fn test_recent_tab_is_noop_with_one_tab() {
        let mut app = App::new();
        let only = app.layout().focused();
        app.recent_tab(false);
        app.recent_tab(true);
        assert_eq!(app.layout().focused(), only);
        assert!(app.tabs.recent_walk.is_none());
    }

    #[test]
    fn test_switch_to_pane_shows_a_hidden_tab() {
        let mut app = App::new();
        let first = app.layout().focused();
        app.new_tab();
        let second = app.layout().focused();
        assert_ne!(first, second);
        assert_eq!(
            app.layout().shown_tabs(),
            vec![second],
            "one pane, showing the new tab"
        );

        app.switch_to_tab(first);
        assert_eq!(app.layout().focused(), first);
        assert_eq!(app.layout().shown_tabs(), vec![first]);
    }
}
