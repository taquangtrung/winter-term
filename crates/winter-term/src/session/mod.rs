//! Session restore: save and reload pane layouts across restarts.
//!
//! On clean exit, Winter writes a session file to
//! `$XDG_STATE_HOME/winter-term/session.json` (`%LOCALAPPDATA%\winter-term\session.json`
//! on Windows). On the next launch Winter automatically restores the split
//! layout and reopens each pane at its last working directory (PTY children
//! are spawned fresh, not reattached).

use std::collections::HashMap;
use std::path::PathBuf;

use crate::model::layout::{Direction, Layout, LayoutTree, PaneTree, TabId};
use crate::terminal::session::Terminal;

// ========================================================================
// Data Structures
// ========================================================================

/// Per-pane reconstruction metadata recovered from a session snapshot:
/// `TabId -> (command, cwd)`.
pub type TerminalMetaMap = HashMap<TabId, (Option<String>, Option<String>)>;

/// A restart snapshot: the window's split layout, the terminals inside it,
/// and the tool tabs that can be opened again from where they were (a
/// listing, the git view, a search, a file in the editor).
///
/// A tool that cannot say where it was is not saved, and a pane that held
/// nothing else is dropped from the snapshot. A file written before tabs
/// moved into panes also carries a `tabs` list; it is ignored, and the layout
/// of the tab that was active is the one restored.
///
/// The file keeps the key names it was first written with, so a snapshot
/// reads the same across a reload in either direction between versions.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Session {
    /// The tab focused when the session was saved.
    pub focused: usize,
    /// The split tree, its leaves the panes.
    pub layout: SessionTree,
    /// Every tool tab in the snapshot. Absent from files written before tools
    /// were saved, which restore with terminals only.
    #[serde(default, rename = "extensions", alias = "tools")]
    pub tools: Vec<ToolSession>,
    /// Every terminal in the snapshot.
    #[serde(rename = "panes")]
    pub terminals: Vec<TerminalSession>,
}

/// One tool tab's snapshot: which kind of tool, and where it was looking.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ToolSession {
    /// Identifier tying this tab to a leaf of the layout tree.
    pub id: usize,
    /// Which kind of tool: `dir`, `git`, `grep`, or `editor`.
    pub kind: String,
    /// The line an editor had its cursor on, counting from one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// The directory the tool was rooted in, or the file the editor had open.
    pub path: String,
}

/// A terminal whose program is still running and is carried into the next
/// incarnation of this process by `exec` (see [`Session::save_handover`]).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TerminalHandover {
    /// Columns of the terminal's grid.
    pub cols: usize,
    /// The master end of the terminal's pty, still open across the `exec`.
    pub fd: i32,
    /// The program on the other end, still this process's child.
    pub pid: u32,
    /// Rows of the terminal's grid.
    pub rows: usize,
    /// What the screen showed, as terminal output that rebuilds it.
    pub screen: String,
}

/// One terminal's snapshot: the command it ran and the directory it ran in.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TerminalSession {
    /// The command the terminal ran; `None` means the default shell.
    pub command: Option<String>,
    /// The terminal's working directory when the snapshot was taken.
    pub cwd: Option<String>,
    /// Present only in a snapshot written for a reload that keeps programs
    /// running; a snapshot from a quit has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handover: Option<TerminalHandover>,
    /// Identifier tying this terminal to a leaf of the layout tree.
    pub id: usize,
}

/// Serializable mirror of [`LayoutTree`].
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "t")]
pub enum SessionTree {
    /// A pane: a leaf holding a strip of tabs.
    #[serde(rename = "Group")]
    Pane {
        /// The tab the pane was showing.
        active: usize,
        /// Every tab of the pane, in strip order.
        ids: Vec<usize>,
    },
    /// A leaf holding one tab, as files from before panes held strips wrote it.
    #[serde(rename = "Pane")]
    SingleTab {
        /// The tab this leaf refers to.
        id: usize,
    },
    /// A split of two child trees.
    Split {
        /// Split direction, stored as text so the file stays readable.
        dir: String,
        /// Fraction of the space given to the first child.
        ratio: f32,
        /// The child above or to the left.
        first: Box<SessionTree>,
        /// The child below or to the right.
        second: Box<SessionTree>,
    },
}

// ========================================================================
// Implementation
// ========================================================================

impl Session {
    /// Write a snapshot of the current layout to the state directory.
    pub fn save(layout: &Layout, terminals: &HashMap<TabId, &Terminal>, tools: Vec<ToolSession>) {
        Self::write(&Self::capture(layout, terminals, tools));
    }

    fn write(session: &Self) {
        if let Ok(json) = serde_json::to_string_pretty(session) {
            let path = session_path();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::write(&path, json).ok();
        }
    }

    /// Write a snapshot for a reload that keeps the programs running: as
    /// [`Session::save`], plus for every local terminal what the next process
    /// image needs to take its program over (see [`Terminal::hand_over`]) and what
    /// its screen shows. The caller then replaces the process image.
    #[cfg(unix)]
    pub fn save_handover(
        layout: &Layout,
        terminals: &HashMap<TabId, &Terminal>,
        tools: Vec<ToolSession>,
    ) {
        let mut session = Self::capture(layout, terminals, tools);
        for entry in &mut session.terminals {
            let Some(tab) = terminals.get(&TabId(entry.id as u64)) else {
                continue;
            };
            let Some((fd, pid)) = tab.hand_over() else {
                continue;
            };
            entry.handover = Some(TerminalHandover {
                cols: tab.grid().cols(),
                fd,
                pid,
                rows: tab.grid().rows(),
                screen: String::from_utf8_lossy(&tab.dump_screen()).into_owned(),
            });
        }
        Self::write(&session);
    }

    /// Take out of this snapshot the programs handed over to this process,
    /// leaving it a plain one. Empty unless this process was started in place
    /// of the one that wrote them (see [`handover_expected`]).
    pub fn take_handovers(&mut self) -> HashMap<TabId, TerminalHandover> {
        let valid = handover_expected();
        self.terminals
            .iter_mut()
            .filter_map(|entry| {
                let handover = entry.handover.take()?;
                valid.then_some((TabId(entry.id as u64), handover))
            })
            .collect()
    }

    /// Read the stored snapshot, or `None` when there is none to read.
    pub fn load() -> Option<Self> {
        let path = session_path();
        let text = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Delete the stored snapshot.
    pub fn remove() {
        let path = session_path();
        if path.exists() {
            std::fs::remove_file(&path).ok();
        }
    }

    fn capture(
        layout: &Layout,
        terminals: &HashMap<TabId, &Terminal>,
        tools: Vec<ToolSession>,
    ) -> Self {
        let kept = |id: TabId| {
            terminals.contains_key(&id) || tools.iter().any(|tool| tool.id == id.0 as usize)
        };
        let tree = layout.export_tree().retain(&kept);
        let terminal_sessions: Vec<TerminalSession> = terminals
            .iter()
            .map(|(id, terminal)| TerminalSession {
                id: id.0 as usize,
                command: Some(terminal.shell_command().to_string()),
                cwd: terminal.cwd(),
                handover: None,
            })
            .collect();
        Session {
            focused: layout.focused().0 as usize,
            layout: layout_tree_to_session(&tree),
            tools,
            terminals: terminal_sessions,
        }
    }

    /// Take the tool tabs out of this snapshot, to be opened again.
    pub fn take_tools(&mut self) -> Vec<ToolSession> {
        std::mem::take(&mut self.tools)
    }

    /// Split this snapshot into the layout to rebuild, the tab to focus, and a
    /// map of `TabId -> (command, cwd)` for the terminals to respawn.
    pub fn into_parts(self) -> (LayoutTree, TabId, TerminalMetaMap) {
        let terminal_map: TerminalMetaMap = self
            .terminals
            .into_iter()
            .map(|t| (TabId(t.id as u64), (t.command, t.cwd)))
            .collect();
        let focused = TabId(self.focused as u64);
        (session_to_layout_tree(&self.layout), focused, terminal_map)
    }
}

// ========================================================================
// Handover
// ========================================================================

/// The environment variable a process sets, to its own id, just before
/// replacing itself with the next binary.
const HANDOVER_ENV: &str = "WINTER_HANDOVER";

/// Say, to the process image that is about to replace this one, that the
/// descriptors in the snapshot are open in it. The process id survives `exec`,
/// so the image can tell its own handover from a stale snapshot, whose
/// descriptors mean nothing.
pub fn announce_handover() {
    std::env::set_var(HANDOVER_ENV, std::process::id().to_string());
}

/// Take back an announcement whose `exec` did not happen.
pub fn withdraw_handover() {
    std::env::remove_var(HANDOVER_ENV);
}

/// Whether this process was started in place of one that announced a handover
/// to it. Taken once: asking again says no.
pub fn handover_expected() -> bool {
    let expected = std::env::var(HANDOVER_ENV).ok();
    std::env::remove_var(HANDOVER_ENV);
    expected.is_some_and(|id| id == std::process::id().to_string())
}

/// Whether a handover has been announced to this process, without taking it.
pub fn handover_announced() -> bool {
    std::env::var(HANDOVER_ENV).is_ok_and(|id| id == std::process::id().to_string())
}

// ========================================================================
// Helpers
// ========================================================================

fn layout_tree_to_session(tree: &LayoutTree) -> SessionTree {
    match tree {
        LayoutTree::Pane(pane) => SessionTree::Pane {
            active: pane.active.0 as usize,
            ids: pane.members.iter().map(|id| id.0 as usize).collect(),
        },
        LayoutTree::Split {
            direction,
            ratio,
            first,
            second,
        } => SessionTree::Split {
            dir: match direction {
                Direction::Horizontal => "h".to_string(),
                Direction::Vertical => "v".to_string(),
            },
            ratio: *ratio,
            first: Box::new(layout_tree_to_session(first)),
            second: Box::new(layout_tree_to_session(second)),
        },
    }
}

fn session_to_layout_tree(tree: &SessionTree) -> LayoutTree {
    match tree {
        SessionTree::Pane { active, ids } => LayoutTree::Pane(PaneTree {
            active: TabId(*active as u64),
            members: ids.iter().map(|id| TabId(*id as u64)).collect(),
        }),
        SessionTree::SingleTab { id } => LayoutTree::single(TabId(*id as u64)),
        SessionTree::Split {
            dir,
            ratio,
            first,
            second,
        } => LayoutTree::Split {
            direction: if dir == "h" {
                Direction::Horizontal
            } else {
                Direction::Vertical
            },
            ratio: *ratio,
            first: Box::new(session_to_layout_tree(first)),
            second: Box::new(session_to_layout_tree(second)),
        },
    }
}

fn session_path() -> PathBuf {
    #[cfg(windows)]
    {
        match std::env::var("LOCALAPPDATA") {
            Ok(local_appdata) => PathBuf::from(local_appdata).join("winter-term/session.json"),
            Err(_) => PathBuf::from("session.json"),
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
            PathBuf::from(xdg).join("winter-term/session.json")
        } else if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home).join(".local/state/winter-term/session.json")
        } else {
            PathBuf::from("session.json")
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_file_from_before_tools_were_saved_still_loads() {
        let old = r#"{"focused":0,"layout":{"t":"Pane","id":0},"panes":[{"command":null,"cwd":null,"id":0}]}"#;
        let session: Session = serde_json::from_str(old).expect("loads");
        assert!(session.tools.is_empty());
    }

    #[test]
    fn test_an_old_tools_key_loads_into_tools() {
        let old = r#"{"focused":0,"layout":{"t":"Pane","id":0},"panes":[],"tools":[{"id":1,"kind":"git","path":"/repo"}]}"#;
        let mut session: Session = serde_json::from_str(old).expect("loads");
        assert_eq!(session.tools.len(), 1);
        assert_eq!(session.tools[0].kind, "git");
        let taken = session.take_tools();
        assert_eq!(taken.len(), 1);
    }

    #[test]
    fn test_session_round_trip() {
        let session = Session {
            focused: 1,
            terminals: vec![
                TerminalSession {
                    id: 0,
                    command: Some("/bin/bash".into()),
                    cwd: Some("/home/user".into()),
                    handover: None,
                },
                TerminalSession {
                    id: 1,
                    command: None,
                    cwd: None,
                    handover: None,
                },
            ],
            layout: SessionTree::Split {
                dir: "v".into(),
                ratio: 0.5,
                first: Box::new(SessionTree::SingleTab { id: 0 }),
                second: Box::new(SessionTree::Pane {
                    active: 1,
                    ids: vec![1, 2],
                }),
            },
            tools: vec![ToolSession {
                id: 2,
                kind: "editor".into(),
                line: Some(40),
                path: "/home/user/notes.md".into(),
            }],
        };
        let json = serde_json::to_string_pretty(&session).unwrap();
        let restored: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored.tools, session.tools,
            "the page tab comes back where it was"
        );
        assert_eq!(restored.focused, 1);
        assert_eq!(restored.terminals.len(), 2);
        assert_eq!(restored.terminals[0].command.as_deref(), Some("/bin/bash"));
        assert!(matches!(restored.layout, SessionTree::Split { .. }));
    }

    #[test]
    fn test_session_path_is_deterministic() {
        let p1 = session_path();
        let p2 = session_path();
        assert_eq!(p1, p2);
    }

    #[test]
    fn test_load_missing_returns_none() {
        let path = session_path();
        if path.exists() {
            std::fs::remove_file(&path).ok();
        }
        assert!(Session::load().is_none());
    }

    #[test]
    fn test_a_file_from_before_panes_held_strips_restores_its_active_layout() {
        // Written when the window had its own tabs: `layout`/`focused` held
        // the active tab, and the `tabs` list is no longer read.
        let json = r#"{
            "active_tab": 1,
            "focused": 2,
            "panes": [{"id": 2, "command": "/bin/zsh", "cwd": null}],
            "layout": {"t": "Pane", "id": 2},
            "tabs": [{"focused": 1, "layout": {"t": "Pane", "id": 1}}]
        }"#;
        let session: Session = serde_json::from_str(json).unwrap();
        let (tree, focused, terminal_map) = session.into_parts();
        let layout = Layout::from_tree(tree, focused).unwrap();
        assert_eq!(layout.members(), vec![TabId(2)]);
        assert_eq!(layout.focused(), TabId(2));
        assert_eq!(terminal_map[&TabId(2)].0.as_deref(), Some("/bin/zsh"));
    }

    #[test]
    fn test_layout_tree_round_trip() {
        let tree = SessionTree::Split {
            dir: "h".into(),
            ratio: 0.3,
            first: Box::new(SessionTree::Pane {
                active: 11,
                ids: vec![10, 11],
            }),
            second: Box::new(SessionTree::Pane {
                active: 12,
                ids: vec![12],
            }),
        };
        let layout = session_to_layout_tree(&tree);
        let back = layout_tree_to_session(&layout);
        let json1 = serde_json::to_string(&tree).unwrap();
        let json2 = serde_json::to_string(&back).unwrap();
        assert_eq!(json1, json2);
    }

    // --------------------------------------------------------------------
    // Handover across a real `exec`
    // --------------------------------------------------------------------

    /// Which half of the handover test this process is, set by the test that
    /// starts it.
    #[cfg(unix)]
    const ROLE_ENV: &str = "WINTER_TEST_HANDOVER_ROLE";

    /// Both halves of a reload that keeps a program running, run as the test
    /// binary itself: the first starts a program on a pane, writes the handover
    /// and replaces its own process image; the second must find the same
    /// program still running, on a screen that shows what the first saw.
    /// Does nothing unless started by [`test_a_running_program_survives_a_process_image_replacement`].
    #[cfg(unix)]
    #[test]
    fn handover_probe() {
        use crate::model::layout::Layout;
        use std::os::unix::process::CommandExt;

        let Ok(role) = std::env::var(ROLE_ENV) else {
            return;
        };
        let id = TabId(1);
        let wait = std::time::Duration::from_secs(5);
        let wait_for = |terminal: &mut Terminal, needle: &str| {
            let deadline = std::time::Instant::now() + wait;
            while std::time::Instant::now() < deadline {
                terminal.drain_output();
                if terminal.grid().to_text().contains(needle) {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            false
        };

        if role == "first" {
            let mut terminal = Terminal::with_command(
                30,
                6,
                portable_pty::CommandBuilder::new("cat"),
                winter_render::MAX_SCROLLBACK,
            )
            .expect("spawn");
            terminal.write(b"before\n");
            assert!(wait_for(&mut terminal, "before"), "the program echoed");
            let mut terminals = HashMap::new();
            terminals.insert(id, &terminal);
            Session::save_handover(&Layout::with_root(id), &terminals, Vec::new());
            announce_handover();
            let error = std::process::Command::new(std::env::current_exe().expect("exe"))
                .args(["--exact", "session::tests::handover_probe", "--nocapture"])
                .env(ROLE_ENV, "second")
                .exec();
            panic!("exec failed: {error}");
        }

        let mut session = Session::load().expect("the first half wrote a session");
        let mut handovers = session.take_handovers();
        let handover = handovers.remove(&id).expect("the program was handed over");
        let mut terminal = Terminal::adopt(
            handover.cols,
            handover.rows,
            handover.fd,
            handover.pid,
            winter_render::MAX_SCROLLBACK,
            "cat".into(),
        )
        .expect("adopts");
        terminal.restore_screen(handover.screen.as_bytes());
        assert!(
            terminal.grid().to_text().contains("before"),
            "the screen came back: {}",
            terminal.grid().to_text()
        );
        assert!(terminal.is_alive(), "the same program is still running");
        terminal.write(b"after\n");
        assert!(wait_for(&mut terminal, "after"), "and still answers");
        println!("HANDOVER-OK pid={}", handover.pid);
    }

    #[cfg(unix)]
    #[test]
    fn test_a_running_program_survives_a_process_image_replacement() {
        let state = std::env::temp_dir().join(format!("winter-handover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&state);
        let output = std::process::Command::new(std::env::current_exe().expect("exe"))
            .args(["--exact", "session::tests::handover_probe", "--nocapture"])
            .env(ROLE_ENV, "first")
            .env("XDG_STATE_HOME", &state)
            .output()
            .expect("runs the probe");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let _ = std::fs::remove_dir_all(&state);
        assert!(
            output.status.success() && stdout.contains("HANDOVER-OK"),
            "stdout: {stdout}\nstderr: {stderr}"
        );
    }
}
