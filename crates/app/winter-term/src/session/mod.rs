//! Session restore: save and reload pane layouts across restarts.
//!
//! On clean exit, Winter writes a session file to
//! `$XDG_STATE_HOME/winter-term/session.json` (`%LOCALAPPDATA%\winter-term\session.json`
//! on Windows). On the next launch Winter automatically restores the split
//! layout and reopens each pane at its last working directory (PTY children
//! are spawned fresh, not reattached).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use winter_model::layout::{Direction, Layout, LayoutTree, PaneTree, TabId};
use winter_terminal::session::Terminal;

// ========================================================================
// Constants
// ========================================================================

/// The snapshot's file name in the state directory.
const SESSION_FILE_NAME: &str = "session.json";

/// The version of the snapshot format this build writes, and the newest it
/// reads. A file from a newer build is not read as this one's: a reload
/// back to an older binary would otherwise take fields it does not know for
/// fields it does.
const SESSION_VERSION: u32 = 1;

/// The name a reload's handover is written under, with the process id it is
/// for in place of `{}`: one per window, so two windows reloading, or one
/// quitting while another reloads, never read each other's programs.
const HANDOVER_FILE_NAME: &str = "session-handover-{}.json";

// ========================================================================
// Data Structures
// ========================================================================

/// Per-pane reconstruction metadata recovered from a session snapshot:
/// `TabId -> (command, cwd)`.
pub type TerminalMetaMap = HashMap<TabId, (Option<String>, Option<String>)>;

/// A restart snapshot: the window's split layout, the terminals inside it,
/// and the editor tabs that can be opened again from where they were (a
/// listing, the git view, a search, a file in the editor).
///
/// An editor that cannot say where it was is not saved, and a pane that held
/// nothing else is dropped from the snapshot. A file written before tabs
/// moved into panes also carries a `tabs` list; it is ignored, and the layout
/// of the tab that was active is the one restored.
///
/// The file keeps the key names it was first written with, so a snapshot
/// reads the same across a reload in either direction between versions.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Session {
    /// The format version the file was written in; `0` for files written
    /// before it was recorded.
    #[serde(default)]
    pub version: u32,
    /// Every editor tab in the snapshot. Absent from files written before editors
    /// were saved, which restore with terminals only.
    #[serde(default, rename = "extensions")]
    pub editors: Vec<EditorSession>,
    /// The tab focused when the session was saved.
    pub focused: usize,
    /// The split tree, its leaves the panes.
    pub layout: SessionTree,
    /// The panels around the main area and the views each shows. Absent
    /// from files written before panels showed views.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docks: Option<DocksSession>,
    /// The panels of a file written when they held tabs: read so their tabs
    /// come back in the main area (see [`Session::take_docks`]), never
    /// written.
    #[serde(default, skip_serializing)]
    pub panels: Vec<PanelSession>,
    /// Every terminal in the snapshot.
    #[serde(rename = "panes")]
    pub terminals: Vec<TerminalSession>,
    /// The workspace the window was in, so a restart comes back into it.
    /// Absent from files written before workspaces, and for a window in none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
}

/// The three panels' snapshots.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DocksSession {
    /// The bottom panel.
    pub bottom: DockSession,
    /// The left sidebar.
    pub left: DockSession,
    /// The right sidebar.
    pub right: DockSession,
}

/// One panel's snapshot: its views, the one shown, whether it was shown,
/// and its size.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DockSession {
    /// The view shown, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    /// Whether it was shown.
    pub shown: bool,
    /// Its width (a sidebar) or height, in logical pixels.
    pub size: f32,
    /// Its views by name (`files`, `output`), in the switcher's order.
    pub views: Vec<String>,
}

/// One panel's snapshot in a file written when panels held tabs: which,
/// and its tabs. Read, never written.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PanelSession {
    /// The tab the panel was showing, and every tab of it in strip order;
    /// `None` for an empty panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<SessionTree>,
    /// Whether it was shown.
    pub shown: bool,
    /// `left`, `right`, or `bottom`.
    pub side: String,
}

/// One editor tab's snapshot: which kind of editor, and where it was looking.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct EditorSession {
    /// Identifier tying this tab to a leaf of the layout tree.
    pub id: usize,
    /// Which kind of editor: `dir`, `git`, `editor`, or `pdf`.
    pub kind: String,
    /// The line an editor had its cursor on, or the page a PDF was on,
    /// counting from one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// The directory the editor was rooted in, or the file the editor or the
    /// PDF had open.
    pub path: String,
    /// Whether this is a buried buffer rather than a tab: one showing a file,
    /// which comes back buried, in no pane.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub buried: bool,
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
    /// Write this snapshot to the state directory, as the one the next
    /// start restores.
    pub fn save(&self) -> std::io::Result<()> {
        Self::write(self)
    }

    /// Write `session` in one step (see [`winter_base::persist::write_atomic`]), so
    /// a write cut short leaves the previous snapshot rather than a truncated
    /// one that restores nothing.
    fn write(session: &Self) -> std::io::Result<()> {
        let path = session_path().ok_or_else(|| std::io::Error::other("no state directory"))?;
        let json = serde_json::to_string_pretty(session).map_err(std::io::Error::other)?;
        winter_base::persist::write_atomic(&path, json)
    }

    /// Write a snapshot for a reload that keeps the programs running: as
    /// [`Session::save`], plus for every local terminal what the next process
    /// image needs to take its program over (see [`Terminal::hand_over`]) and what
    /// its screen shows. The caller then replaces the process image.
    ///
    /// An error means the snapshot was not written, and the reload must not
    /// go ahead: the next image would find no programs to take over.
    #[cfg(unix)]
    pub fn save_handover(
        layout: &Layout,
        terminals: &HashMap<TabId, &Terminal>,
        editors: Vec<EditorSession>,
        workspace: Option<String>,
        docks: DocksSession,
    ) -> std::io::Result<()> {
        let mut session = Self::capture(layout, terminals, editors, workspace).with_docks(docks);
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
        let path = handover_path().ok_or_else(|| std::io::Error::other("no state directory"))?;
        let json = serde_json::to_string_pretty(&session).map_err(std::io::Error::other)?;
        winter_base::persist::write_atomic(&path, json)
    }

    /// Take out of this snapshot the programs handed over to this process,
    /// leaving it a plain one. Empty unless this process was started in place
    /// of the one that wrote them (see [`handover_expected`]).
    ///
    /// The handover file goes once read: it carries what every screen
    /// showed, which should not outlive the reload on disk.
    pub fn take_handovers(&mut self) -> HashMap<TabId, TerminalHandover> {
        let valid = handover_expected();
        let handovers = self
            .terminals
            .iter_mut()
            .filter_map(|entry| {
                let handover = entry.handover.take()?;
                valid.then_some((TabId(entry.id as u64), handover))
            })
            .collect();
        if let Some(path) = handover_path() {
            let _ = std::fs::remove_file(path);
        }
        handovers
    }

    /// Read the snapshot this start restores: the handover a reload wrote
    /// for this process, else the stored one; `None` when there is none.
    pub fn load() -> Option<Self> {
        if handover_announced() {
            let path = handover_path()?;
            let session = Self::load_from(&path);
            if session.is_none() {
                abandon_handover(&path);
            }
            return session;
        }
        Self::load_from(&session_path()?)
    }

    fn load_from(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        let session: Self = serde_json::from_str(&text).ok()?;
        (session.version <= SESSION_VERSION).then_some(session)
    }

    /// A snapshot of the window: its layout, the terminals and editor tabs in
    /// it, and the workspace it is in.
    pub fn capture(
        layout: &Layout,
        terminals: &HashMap<TabId, &Terminal>,
        editors: Vec<EditorSession>,
        workspace: Option<String>,
    ) -> Self {
        let kept = |id: TabId| {
            terminals.contains_key(&id) || editors.iter().any(|editor| editor.id == id.0 as usize)
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
            version: SESSION_VERSION,
            editors,
            focused: layout.focused().0 as usize,
            docks: None,
            layout: layout_tree_to_session(&tree),
            panels: Vec::new(),
            terminals: terminal_sessions,
            workspace,
        }
    }

    /// Give every tab in this snapshot a new id from `fresh`, so it can be
    /// opened beside tabs already holding the ids it was saved with: a
    /// workspace switched to while others are kept open.
    pub fn renumber(&mut self, mut fresh: impl FnMut() -> usize) {
        let mut ids: HashMap<usize, usize> = HashMap::new();
        let mut new_id = |old: usize| *ids.entry(old).or_insert_with(&mut fresh);
        for terminal in &mut self.terminals {
            terminal.id = new_id(terminal.id);
        }
        for editor in &mut self.editors {
            editor.id = new_id(editor.id);
        }
        renumber_tree(&mut self.layout, &mut new_id);
        for panel in &mut self.panels {
            if let Some(pane) = &mut panel.pane {
                renumber_tree(pane, &mut new_id);
            }
        }
        self.focused = new_id(self.focused);
    }

    /// This snapshot with `docks`, the panels and their views, recorded.
    pub fn with_docks(mut self, docks: DocksSession) -> Self {
        self.docks = Some(docks);
        self
    }

    /// The panels of this snapshot, and the tabs of a file written when
    /// panels held them, to come back in the main area. Such a file's
    /// sidebar tabs of a tool that is now a sidebar view (Dir, Git, the
    /// Outline, Todo) become that view, their editors dropped.
    pub fn take_docks(&mut self) -> (Option<DocksSession>, Vec<TabId>) {
        let mut docks = self.docks.take();
        let mut tabs = Vec::new();
        for panel in std::mem::take(&mut self.panels) {
            let Some(SessionTree::Pane { ids, .. }) = panel.pane else {
                continue;
            };
            let sidebar = match panel.side.as_str() {
                "left" | "right" => Some(panel.side.as_str()),
                _ => None,
            };
            for id in ids {
                let view = sidebar.and_then(|_| {
                    let editor = self.editors.iter().find(|editor| editor.id == id)?;
                    legacy_view(&editor.kind)
                });
                match (sidebar, view) {
                    (Some(side), Some(view)) => {
                        self.editors.retain(|editor| editor.id != id);
                        let held = docks.get_or_insert_with(DocksSession::default);
                        let dock = match side {
                            "left" => &mut held.left,
                            _ => &mut held.right,
                        };
                        dock.views.push(view.to_string());
                        dock.active.get_or_insert_with(|| view.to_string());
                        dock.shown |= panel.shown;
                    }
                    _ => tabs.push(TabId(id as u64)),
                }
            }
        }
        (docks, tabs)
    }

    /// Take the editor tabs out of this snapshot, to be opened again.
    pub fn take_editors(&mut self) -> Vec<EditorSession> {
        std::mem::take(&mut self.editors)
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

/// The environment variable a process hands the next binary, set to its own
/// id, when it replaces itself with it.
const HANDOVER_ENV: &str = "WINTER_HANDOVER";

/// The id a handover was announced to this process for, read once from the
/// environment: by [`take_handover_env`] at startup, else on first ask.
static ANNOUNCED: OnceLock<Option<String>> = OnceLock::new();

/// Whether this process has taken its handover (see [`handover_expected`]).
static TAKEN: AtomicBool = AtomicBool::new(false);

/// The variable to start the process image replacing this one with, saying
/// that the descriptors in the snapshot are open in it. The process id
/// survives `exec`, so the image can tell its own handover from a stale
/// snapshot, whose descriptors mean nothing. It is given to the new image
/// alone, so this process's environment is never changed while it runs.
pub fn handover_env() -> (&'static str, String) {
    (HANDOVER_ENV, std::process::id().to_string())
}

/// Read the handover announced to this process out of the environment and
/// remove it there, so the programs this process starts do not inherit it.
///
/// # Safety
///
/// Changes the environment, so it must run before any other thread starts.
#[allow(unsafe_code)]
pub unsafe fn take_handover_env() {
    let announced = std::env::var(HANDOVER_ENV).ok();
    // SAFETY: the caller runs this before any other thread starts, so no
    // thread reads the environment while it changes.
    unsafe { std::env::remove_var(HANDOVER_ENV) };
    let _ = ANNOUNCED.set(announced);
}

/// Whether this process was started in place of one that announced a handover
/// to it. Taken once: asking again says no.
pub fn handover_expected() -> bool {
    let announced = handover_announced();
    TAKEN.store(true, Ordering::Relaxed);
    announced
}

/// Whether a handover has been announced to this process, without taking it.
pub fn handover_announced() -> bool {
    let announced = ANNOUNCED.get_or_init(|| std::env::var(HANDOVER_ENV).ok());
    !TAKEN.load(Ordering::Relaxed)
        && announced
            .as_deref()
            .is_some_and(|id| id == std::process::id().to_string())
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
            first,
            ratio,
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

/// Put the id `new_id` gives for each tab in place of the old one, all
/// through `tree`.
fn renumber_tree(tree: &mut SessionTree, new_id: &mut impl FnMut(usize) -> usize) {
    match tree {
        SessionTree::Pane { active, ids } => {
            *active = new_id(*active);
            for id in ids {
                *id = new_id(*id);
            }
        }
        SessionTree::Split { first, second, .. } => {
            renumber_tree(first, new_id);
            renumber_tree(second, new_id);
        }
    }
}

fn session_to_layout_tree(tree: &SessionTree) -> LayoutTree {
    match tree {
        SessionTree::Pane { active, ids } => LayoutTree::Pane(PaneTree {
            active: TabId(*active as u64),
            members: ids.iter().map(|id| TabId(*id as u64)).collect(),
        }),
        SessionTree::Split {
            dir,
            first,
            ratio,
            second,
            ..
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

/// The sidebar view a tool tab of `kind`, kept in a sidebar by a file
/// written when sidebars held tabs, now is.
fn legacy_view(kind: &str) -> Option<&'static str> {
    match kind {
        "dir" => Some("files"),
        "git" => Some("git"),
        "outline" => Some("outline"),
        "todo" => Some("todo"),
        _ => None,
    }
}

/// Where the snapshot lives (see [`winter_base::paths::state_file`]).
fn session_path() -> Option<PathBuf> {
    crate::config::state_file(SESSION_FILE_NAME)
}

/// Where a reload's handover for this process is written.
fn handover_path() -> Option<PathBuf> {
    crate::config::state_file(&HANDOVER_FILE_NAME.replace("{}", &std::process::id().to_string()))
}

/// Let go of every program a handover file names, when the file cannot be
/// read as a snapshot (written by a build whose format differs): each one's
/// terminal is closed and the program hung up, rather than left running
/// with no one reading it. The file goes too.
fn abandon_handover(path: &Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let _ = std::fs::remove_file(path);
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let terminals = value.get("panes").and_then(serde_json::Value::as_array);
    for handover in terminals
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("handover"))
    {
        let fd = handover.get("fd").and_then(serde_json::Value::as_i64);
        let pid = handover.get("pid").and_then(serde_json::Value::as_u64);
        #[cfg(unix)]
        if let (Some(fd), Some(pid)) = (fd, pid)
            && let (Ok(fd), Ok(pid)) = (i32::try_from(fd), u32::try_from(pid))
        {
            winter_base::process::abandon_program(fd, pid);
        }
        #[cfg(not(unix))]
        let _ = (fd, pid);
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_snapshot_from_a_newer_build_is_not_read_as_this_one_s() {
        // With no version in the file, a reload into an older binary read a
        // newer one's snapshot as its own, fields and all.
        let path =
            std::env::temp_dir().join(format!("winter-session-newer-{}.json", std::process::id()));
        let newer = format!(
            r#"{{"version":{},"focused":0,"layout":{{"t":"Group","active":0,"ids":[0]}},"panes":[]}}"#,
            SESSION_VERSION + 1
        );
        std::fs::write(&path, newer).expect("file");
        assert!(Session::load_from(&path).is_none());
        let current = format!(
            r#"{{"version":{SESSION_VERSION},"focused":0,"layout":{{"t":"Group","active":0,"ids":[0]}},"panes":[]}}"#
        );
        std::fs::write(&path, current).expect("file");
        assert!(Session::load_from(&path).is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_session_round_trip() {
        let session = Session {
            version: SESSION_VERSION,
            docks: None,
            panels: Vec::new(),
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
                first: Box::new(SessionTree::Pane {
                    active: 0,
                    ids: vec![0],
                }),
                second: Box::new(SessionTree::Pane {
                    active: 1,
                    ids: vec![1, 2],
                }),
            },
            editors: vec![EditorSession {
                buried: false,
                id: 2,
                kind: "editor".into(),
                line: Some(40),
                path: "/home/user/notes.md".into(),
            }],
            workspace: Some("notes".into()),
        };
        let json = serde_json::to_string_pretty(&session).unwrap();
        let restored: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored.editors, session.editors,
            "the page tab comes back where it was"
        );
        assert_eq!(restored.focused, 1);
        assert_eq!(restored.terminals.len(), 2);
        assert_eq!(restored.terminals[0].command.as_deref(), Some("/bin/bash"));
        assert!(matches!(restored.layout, SessionTree::Split { .. }));
        assert_eq!(restored.workspace.as_deref(), Some("notes"));
    }

    #[test]
    fn test_renumbering_moves_every_tab_to_its_new_id_alike() {
        let mut session = Session {
            version: SESSION_VERSION,
            docks: None,
            panels: Vec::new(),
            editors: vec![EditorSession {
                buried: false,
                id: 2,
                kind: "dir".into(),
                line: None,
                path: "/p".into(),
            }],
            focused: 2,
            layout: SessionTree::Split {
                dir: "v".into(),
                ratio: 0.5,
                first: Box::new(SessionTree::Pane {
                    active: 1,
                    ids: vec![1],
                }),
                second: Box::new(SessionTree::Pane {
                    active: 2,
                    ids: vec![1, 2],
                }),
            },
            terminals: vec![TerminalSession {
                command: None,
                cwd: None,
                handover: None,
                id: 1,
            }],
            workspace: None,
        };
        let mut next = 100;
        session.renumber(|| {
            next += 1;
            next
        });
        assert_eq!(session.terminals[0].id, 101);
        assert_eq!(session.editors[0].id, 102);
        assert_eq!(session.focused, 102);
        let SessionTree::Split { first, second, .. } = &session.layout else {
            panic!("still a split");
        };
        assert!(matches!(&**first, SessionTree::Pane { active: 101, ids } if ids == &[101]));
        assert!(matches!(
            &**second,
            SessionTree::Pane { active: 102, ids } if ids == &[101, 102]
        ));
    }

    #[test]
    fn test_load_missing_returns_none() {
        // Regression: this deleted the real session file to test with, so
        // running the suite threw away the developer's saved layout.
        let missing = std::env::temp_dir().join(format!(
            "winter-session-missing-{}.json",
            std::process::id()
        ));
        assert!(Session::load_from(&missing).is_none());
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

    #[test]
    fn test_the_panels_views_come_back_from_a_saved_session() {
        let docks = DocksSession {
            bottom: DockSession {
                active: Some("problems".to_string()),
                shown: true,
                size: 240.0,
                views: vec!["output".to_string(), "problems".to_string()],
            },
            ..DocksSession::default()
        };
        let session = Session::capture(&Layout::new(), &HashMap::new(), Vec::new(), None)
            .with_docks(docks.clone());
        let json = serde_json::to_string(&session).expect("written");
        let mut read: Session = serde_json::from_str(&json).expect("read");
        assert_eq!(read.take_docks(), (Some(docks), Vec::new()));
    }

    #[test]
    fn test_a_file_from_when_panels_held_tabs_brings_them_to_the_main_area_and_tools_become_views()
    {
        let json = r#"{
            "version": 1,
            "focused": 1,
            "layout": {"t": "Group", "active": 1, "ids": [1]},
            "panes": [
                {"id": 1, "command": null, "cwd": null},
                {"id": 2, "command": null, "cwd": null}
            ],
            "extensions": [
                {"id": 3, "kind": "dir", "path": "/repo"},
                {"id": 4, "kind": "compile", "path": "/repo"}
            ],
            "panels": [
                {"side": "bottom", "shown": true, "size": 14,
                 "pane": {"t": "Group", "active": 2, "ids": [2, 4]}},
                {"side": "left", "shown": true, "size": 32,
                 "pane": {"t": "Group", "active": 3, "ids": [3]}}
            ],
            "placement": {"compile": "bottom"},
            "sidebar_tools": {"left": "files"}
        }"#;
        let mut session: Session = serde_json::from_str(json).expect("read");
        let (docks, tabs) = session.take_docks();
        assert_eq!(
            tabs,
            [TabId(2), TabId(4)],
            "the bottom panel's shell and build"
        );
        let docks = docks.expect("the sidebar's tool as a view");
        assert_eq!(docks.left.views, ["files"]);
        assert!(docks.left.shown);
        assert!(
            session.editors.iter().all(|editor| editor.id != 3),
            "the Dir tab it stood for is dropped"
        );
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
        use std::os::unix::process::CommandExt;
        use winter_model::layout::Layout;

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
                winter_emulator::MAX_SCROLLBACK,
            )
            .expect("spawn");
            terminal.write(b"before\n");
            assert!(wait_for(&mut terminal, "before"), "the program echoed");
            let mut terminals = HashMap::new();
            terminals.insert(id, &terminal);
            Session::save_handover(
                &Layout::with_root(id),
                &terminals,
                Vec::new(),
                None,
                DocksSession::default(),
            )
            .expect("handover saved");
            let (handover, announced) = handover_env();
            let error = std::process::Command::new(std::env::current_exe().expect("exe"))
                .args(["--exact", "session::tests::handover_probe", "--nocapture"])
                .env(handover, announced)
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
            winter_emulator::MAX_SCROLLBACK,
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
