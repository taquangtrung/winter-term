//! What the user approved each extension installed as a file to reach:
//! kept in Winter's state directory, never beside the extension, each
//! approval for one component, known by its hash. A component not approved,
//! or changed since it was, is not run.
//!
//! ```kdl
//! approval "example" hash="9f2c…" {
//!     asked { read "~/Notes"; write "~/Notes"; }
//!     granted { read "~/Notes" "~/Papers"; write "~/Notes"; }
//! }
//! ```
//!
//! `asked` is what the component asked when it was approved, so a new one
//! asking the same again is not asked about; `granted` is what it reaches,
//! the user's to narrow or widen.

use std::path::Path;

use kdl::{KdlDocument, KdlEntry, KdlNode};

use super::package::{GrantKind, Grants};

// ========================================================================
// Constants
// ========================================================================

/// The node of one approval.
const APPROVAL_NODE: &str = "approval";

/// What an approval names its component's hash by.
const HASH_KEY: &str = "hash";

/// The approval's lists of what was asked and what is granted.
const ASKED_NODE: &str = "asked";
const GRANTED_NODE: &str = "granted";

// ========================================================================
// Data Structures
// ========================================================================

/// Every approval, by extension id.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Approvals {
    list: Vec<Approval>,
}

/// What the user approved one extension to reach.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Approval {
    /// What its component asked when it was approved.
    pub(crate) asked: Grants,
    /// What it reaches.
    pub(crate) granted: Grants,
    /// The hash of the component approved.
    pub(crate) hash: String,
    pub(crate) id: String,
}

// ========================================================================
// Approvals
// ========================================================================

impl Approvals {
    /// The approvals kept at `path`: none where there is no file yet.
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                Self::parse(&text).map_err(|why| format!("{} does not read: {why}", path.display()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!("{} cannot be read: {error}", path.display())),
        }
    }

    /// Keep these at `path`.
    pub(crate) fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|error| format!("{} cannot be made: {error}", dir.display()))?;
        }
        std::fs::write(path, self.to_kdl())
            .map_err(|error| format!("{} cannot be written: {error}", path.display()))
    }

    pub(crate) fn get(&self, id: &str) -> Option<&Approval> {
        self.list.iter().find(|approval| approval.id == id)
    }

    /// Keep `approval`, in place of the one for its extension before.
    pub(crate) fn set(&mut self, approval: Approval) {
        self.remove(&approval.id);
        self.list.push(approval);
        self.list.sort_by(|a, b| a.id.cmp(&b.id));
    }

    /// Forget the approval for `id`; whether there was one.
    pub(crate) fn remove(&mut self, id: &str) -> bool {
        let before = self.list.len();
        self.list.retain(|approval| approval.id != id);
        self.list.len() != before
    }

    fn parse(text: &str) -> Result<Self, String> {
        let document: KdlDocument = text.parse().map_err(|e: kdl::KdlError| e.to_string())?;
        let mut approvals = Self::default();
        for node in document.nodes() {
            if node.name().value() != APPROVAL_NODE {
                return Err(format!("{} is not an approval", node.name().value()));
            }
            let id = node
                .get(0)
                .and_then(|value| value.as_string())
                .ok_or("an approval names no extension")?;
            let hash = node
                .get(HASH_KEY)
                .and_then(|value| value.as_string())
                .ok_or_else(|| format!("the approval of {id} has no hash"))?;
            approvals.set(Approval {
                asked: grants_in(node, ASKED_NODE)?,
                granted: grants_in(node, GRANTED_NODE)?,
                hash: hash.to_string(),
                id: id.to_string(),
            });
        }
        Ok(approvals)
    }

    fn to_kdl(&self) -> String {
        let mut document = KdlDocument::new();
        for approval in &self.list {
            let mut node = KdlNode::new(APPROVAL_NODE);
            node.push(KdlEntry::new(approval.id.clone()));
            node.push((HASH_KEY, approval.hash.clone()));
            let children = node.ensure_children();
            children
                .nodes_mut()
                .push(grants_node(ASKED_NODE, &approval.asked));
            children
                .nodes_mut()
                .push(grants_node(GRANTED_NODE, &approval.granted));
            document.nodes_mut().push(node);
        }
        document.autoformat();
        document.to_string()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The grants listed in `node`'s child `name`.
fn grants_in(node: &KdlNode, name: &str) -> Result<Grants, String> {
    let mut grants = Grants::default();
    let Some(list) = node.children().and_then(|children| children.get(name)) else {
        return Ok(grants);
    };
    for line in list.children().map(KdlDocument::nodes).unwrap_or_default() {
        let kind: GrantKind = line.name().value().parse()?;
        for entry in line.entries() {
            let value = entry.value().as_string().ok_or("a grant is not a string")?;
            grants.add(kind, value);
        }
    }
    Ok(grants)
}

/// A node `name` listing `grants`, a line for each kind granted.
fn grants_node(name: &str, grants: &Grants) -> KdlNode {
    let mut node = KdlNode::new(name);
    let children = node.ensure_children();
    for kind in GrantKind::ALL {
        let values = grants.of(kind);
        if values.is_empty() {
            continue;
        }
        let mut line = KdlNode::new(kind.name());
        for value in values {
            line.push(KdlEntry::new(value.clone()));
        }
        children.nodes_mut().push(line);
    }
    node
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn approval(id: &str, granted: Grants) -> Approval {
        Approval {
            asked: Grants::default(),
            granted,
            hash: format!("{id}-hash"),
            id: id.to_string(),
        }
    }

    #[test]
    fn test_approvals_written_read_back_the_same() {
        let mut approvals = Approvals::default();
        let mut granted = Grants::default();
        granted.add(GrantKind::Read, "~/Notes");
        granted.add(GrantKind::Read, "/odd \"quoted\" \\ path\nwith a newline");
        granted.add(GrantKind::Run, "server");
        approvals.set(approval("b", granted));
        approvals.set(approval("a", Grants::default()));
        let text = approvals.to_kdl();
        assert_eq!(Approvals::parse(&text), Ok(approvals.clone()), "{text}");
        let ids: Vec<&str> = approvals.list.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn test_a_path_cannot_write_a_grant_of_its_own() {
        let mut granted = Grants::default();
        granted.add(GrantKind::Read, "x\" }\ngranted { write \"/\"");
        let mut approvals = Approvals::default();
        approvals.set(approval("a", granted));
        let read = Approvals::parse(&approvals.to_kdl()).expect("reads");
        assert!(read.get("a").expect("kept").granted.write.is_empty());
    }

    #[test]
    fn test_setting_an_approval_replaces_the_one_before() {
        let mut approvals = Approvals::default();
        approvals.set(approval("a", Grants::default()));
        let mut granted = Grants::default();
        granted.add(GrantKind::Write, "~/Notes");
        approvals.set(approval("a", granted.clone()));
        assert_eq!(approvals.list.len(), 1);
        assert_eq!(approvals.get("a").expect("kept").granted, granted);
        assert!(approvals.remove("a"));
        assert!(!approvals.remove("a"));
    }
}
