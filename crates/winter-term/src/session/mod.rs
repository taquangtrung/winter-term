//! Session restore: save and reload pane layouts across restarts.
//!
//! On clean exit, Winter writes a session file to
//! `$XDG_STATE_HOME/winter-term/session.json` (`%LOCALAPPDATA%\winter-term\session.json`
//! on Windows). On the next launch Winter automatically restores the split
//! layout and reopens each pane at its last working directory (PTY children
//! are spawned fresh, not reattached).

use std::collections::HashMap;
use std::path::PathBuf;

use crate::model::layout::{Direction, GroupTree, Layout, LayoutTree, PaneId};
use crate::terminal::pane::Pane;

// ========================================================================
// Data Structures
// ========================================================================

/// Per-pane reconstruction metadata recovered from a session snapshot:
/// `PaneId -> (command, cwd)`.
pub type PaneMetaMap = HashMap<PaneId, (Option<String>, Option<String>)>;

/// A restart snapshot: the window's split layout and the terminals inside it.
///
/// Tool tabs are not saved: a tool is reopened from where the reader is, and
/// a group that held nothing else is dropped from the snapshot. A file written
/// before tabs moved into panes also carries a `tabs` list; it is ignored,
/// and the layout of the tab that was active is the one restored.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Session {
    /// The tab focused when the session was saved.
    pub focused: usize,
    /// The split tree, its leaves the tab groups.
    pub layout: SessionTree,
    /// Every terminal in the snapshot.
    pub panes: Vec<PaneSession>,
}

/// One pane's snapshot: the command it ran and the directory it ran in.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PaneSession {
    /// The command the pane ran; `None` means the default shell.
    pub command: Option<String>,
    /// The pane's working directory when the snapshot was taken.
    pub cwd: Option<String>,
    /// Identifier tying this pane to a leaf of the layout tree.
    pub id: usize,
}

/// Serializable mirror of [`LayoutTree`].
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "t")]
pub enum SessionTree {
    /// A leaf holding a group of tabs.
    Group {
        /// The tab the group was showing.
        active: usize,
        /// Every tab of the group, in strip order.
        ids: Vec<usize>,
    },
    /// A leaf holding one pane, as files from before tab groups wrote it.
    Pane {
        /// The pane this leaf refers to.
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
    pub fn save(layout: &Layout, panes: &HashMap<PaneId, Pane>) {
        let session = Self::capture(layout, panes);
        if let Ok(json) = serde_json::to_string_pretty(&session) {
            let path = session_path();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::write(&path, json).ok();
        }
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

    fn capture(layout: &Layout, panes: &HashMap<PaneId, Pane>) -> Self {
        let tree = layout.export_tree().retain(&|id| panes.contains_key(&id));
        let pane_sessions: Vec<PaneSession> = panes
            .iter()
            .map(|(id, pane)| PaneSession {
                id: id.0 as usize,
                command: Some(pane.shell_command().to_string()),
                cwd: pane.cwd(),
            })
            .collect();
        Session {
            focused: layout.focused().0 as usize,
            layout: layout_tree_to_session(&tree),
            panes: pane_sessions,
        }
    }

    /// Split this snapshot into the layout to rebuild, the tab to focus, and a
    /// map of `PaneId -> (command, cwd)` for the terminals to respawn.
    pub fn into_parts(self) -> (LayoutTree, PaneId, PaneMetaMap) {
        let pane_map: PaneMetaMap = self
            .panes
            .into_iter()
            .map(|p| (PaneId(p.id as u64), (p.command, p.cwd)))
            .collect();
        let focused = PaneId(self.focused as u64);
        (session_to_layout_tree(&self.layout), focused, pane_map)
    }
}

// ========================================================================
// Helpers
// ========================================================================

fn layout_tree_to_session(tree: &LayoutTree) -> SessionTree {
    match tree {
        LayoutTree::Group(group) => SessionTree::Group {
            active: group.active.0 as usize,
            ids: group.members.iter().map(|id| id.0 as usize).collect(),
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
        SessionTree::Group { active, ids } => LayoutTree::Group(GroupTree {
            active: PaneId(*active as u64),
            members: ids.iter().map(|id| PaneId(*id as u64)).collect(),
        }),
        SessionTree::Pane { id } => LayoutTree::single(PaneId(*id as u64)),
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
    fn test_session_round_trip() {
        let session = Session {
            focused: 1,
            panes: vec![
                PaneSession {
                    id: 0,
                    command: Some("/bin/bash".into()),
                    cwd: Some("/home/user".into()),
                },
                PaneSession {
                    id: 1,
                    command: None,
                    cwd: None,
                },
            ],
            layout: SessionTree::Split {
                dir: "v".into(),
                ratio: 0.5,
                first: Box::new(SessionTree::Pane { id: 0 }),
                second: Box::new(SessionTree::Group {
                    active: 1,
                    ids: vec![1],
                }),
            },
        };
        let json = serde_json::to_string_pretty(&session).unwrap();
        let restored: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.focused, 1);
        assert_eq!(restored.panes.len(), 2);
        assert_eq!(restored.panes[0].command.as_deref(), Some("/bin/bash"));
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
    fn test_a_file_from_before_tab_groups_restores_its_active_layout() {
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
        let (tree, focused, pane_map) = session.into_parts();
        let layout = Layout::from_tree(tree, focused).unwrap();
        assert_eq!(layout.members(), vec![PaneId(2)]);
        assert_eq!(layout.focused(), PaneId(2));
        assert_eq!(pane_map[&PaneId(2)].0.as_deref(), Some("/bin/zsh"));
    }

    #[test]
    fn test_layout_tree_round_trip() {
        let tree = SessionTree::Split {
            dir: "h".into(),
            ratio: 0.3,
            first: Box::new(SessionTree::Group {
                active: 11,
                ids: vec![10, 11],
            }),
            second: Box::new(SessionTree::Group {
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
}
