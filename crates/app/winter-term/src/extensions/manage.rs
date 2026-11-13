//! Installing extensions as files, and what they may reach: what the
//! command line (`winter extension …`) and the palette both do, so the two
//! never differ.
//!
//! Installing reads the package a component carries without running it,
//! shows what it asks, and only once that is approved copies the component,
//! and nothing else, to `extensions/<id>/<id>.wasm`, keeping the approval
//! with its hash (see `approvals`). Loading runs a component only when its
//! hash is the one approved, reaching only what is granted.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::approvals::{Approval, Approvals};
use super::package::{GrantKind, Grants, Package};

// ========================================================================
// Constants
// ========================================================================

/// The config directory's directory of installed extensions.
pub(crate) const DIR_NAME: &str = "extensions";

/// The state file the approvals are kept in.
const APPROVALS_FILE: &str = "extension-approvals.kdl";

/// What a component file is named with, after its extension's id.
const COMPONENT_EXTENSION: &str = winter_wasm::FILE_EXTENSION;

/// What a component is written as before it takes its name, so a component
/// half written is never one Winter finds.
const PARTIAL_EXTENSION: &str = "partial";

/// The command that reviews a component again, named in what is said of one
/// not approved.
const INSTALL_COMMAND: &str = "winter extension install";

// ========================================================================
// Data Structures
// ========================================================================

/// Where extensions are installed, and their approvals kept.
#[derive(Clone, Debug)]
pub(crate) struct Places {
    pub(crate) approvals: PathBuf,
    pub(crate) extensions: PathBuf,
}

/// A component read to install, and what installing it would grant anew.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Review {
    bytes: Vec<u8>,
    hash: String,
    /// What it asks that was not approved before: to grant on install.
    pub(crate) new: Grants,
    pub(crate) package: Package,
    /// The approval of the one installed before it, when there is one.
    pub(crate) previous: Option<Approval>,
}

/// A component approved to run, read once, and what it is granted.
#[derive(Debug)]
pub(crate) struct Approved {
    pub(crate) bytes: Vec<u8>,
    pub(crate) granted: Grants,
    pub(crate) package: Package,
}

/// An extension installed as a file, and how it stands.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Installed {
    pub(crate) approval: Option<Approval>,
    pub(crate) id: String,
    pub(crate) standing: Standing,
}

/// Whether an installed component may run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Standing {
    /// It is the one approved.
    Approved,
    /// It is not the one approved: replaced, or rebuilt, since.
    Changed,
    /// It was never approved: put there by hand.
    NotApproved,
}

// ========================================================================
// Places
// ========================================================================

impl Places {
    /// The user's: extensions in the config directory, approvals in the
    /// state directory; `None` without a state directory.
    pub(crate) fn user() -> Option<Self> {
        Some(Self {
            approvals: crate::config::state_file(APPROVALS_FILE)?,
            extensions: crate::config::config_dir().join(DIR_NAME),
        })
    }

    /// Where the component of `id` is installed.
    pub(crate) fn component(&self, id: &str) -> PathBuf {
        self.extensions
            .join(id)
            .join(format!("{id}.{COMPONENT_EXTENSION}"))
    }

    fn approvals(&self) -> Result<Approvals, String> {
        Approvals::load(&self.approvals)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Read the component `file` to install: its package, and what it asks
/// beyond what its extension was approved before.
pub(crate) fn review(file: &Path, places: &Places) -> Result<Review, String> {
    let bytes = std::fs::read(file)
        .map_err(|error| format!("{} cannot be read: {error}", file.display()))?;
    let package = package_in(&bytes)?;
    let previous = places.approvals()?.get(&package.id).cloned();
    let approved = previous
        .as_ref()
        .map(|approval| approval.asked.with(&approval.granted));
    let new = package.asks.beyond(&approved.unwrap_or_default());
    Ok(Review {
        hash: hash_of(&bytes),
        bytes,
        new,
        package,
        previous,
    })
}

/// Install the component reviewed, granting what it asks anew; where it
/// was installed. What the user changed of an earlier approval is kept.
pub(crate) fn install(review: Review, places: &Places) -> Result<PathBuf, String> {
    let id = &review.package.id;
    let path = places.component(id);
    let dir = path.parent().unwrap_or(&places.extensions);
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("{} cannot be made: {error}", dir.display()))?;
    let partial = path.with_extension(PARTIAL_EXTENSION);
    std::fs::write(&partial, &review.bytes)
        .and_then(|()| std::fs::rename(&partial, &path))
        .map_err(|error| format!("{} cannot be written: {error}", path.display()))?;
    let granted = match &review.previous {
        Some(previous) => previous.granted.with(&review.new),
        None => review.package.asks.clone(),
    };
    let mut approvals = places.approvals()?;
    approvals.set(Approval {
        asked: review.package.asks.clone(),
        granted,
        hash: review.hash,
        id: id.clone(),
    });
    approvals.save(&places.approvals)?;
    Ok(path)
}

/// The installed component of `id`, when it is the one approved: read once,
/// so what runs is what was checked.
pub(crate) fn approved(
    places: &Places,
    approvals: &Approvals,
    id: &str,
) -> Result<Approved, String> {
    let path = places.component(id);
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
    let again = format!("review it with `{INSTALL_COMMAND} {}`", path.display());
    let approval = approvals
        .get(id)
        .ok_or_else(|| format!("{id} was never approved to run: {again}"))?;
    if approval.hash != hash_of(&bytes) {
        return Err(format!("{id} has changed since it was approved: {again}"));
    }
    let package = package_in(&bytes)?;
    if package.id != id {
        return Err(format!("{} holds {}, not {id}", path.display(), package.id));
    }
    Ok(Approved {
        bytes,
        granted: approval.granted.clone(),
        package,
    })
}

/// Every extension installed as a file, by id, and how each stands.
pub(crate) fn installed(places: &Places) -> Result<Vec<Installed>, String> {
    let approvals = places.approvals()?;
    let Ok(entries) = std::fs::read_dir(&places.extensions) else {
        return Ok(Vec::new());
    };
    let mut ids: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|id| places.component(id).is_file())
        .collect();
    ids.sort();
    Ok(ids
        .into_iter()
        .map(|id| {
            let approval = approvals.get(&id).cloned();
            let standing = match &approval {
                None => Standing::NotApproved,
                Some(approval) => match std::fs::read(places.component(&id)) {
                    Ok(bytes) if hash_of(&bytes) == approval.hash => Standing::Approved,
                    Ok(_) | Err(_) => Standing::Changed,
                },
            };
            Installed {
                approval,
                id,
                standing,
            }
        })
        .collect())
}

/// Grant the extension `id` `value` of `kind`; whether it was not already.
pub(crate) fn grant(
    places: &Places,
    id: &str,
    kind: GrantKind,
    value: &str,
) -> Result<bool, String> {
    check_grant(kind, value)?;
    change_grants(places, id, |granted| granted.add(kind, value))
}

/// Take back `value` of `kind` from the extension `id`; whether it had it.
pub(crate) fn revoke(
    places: &Places,
    id: &str,
    kind: GrantKind,
    value: &str,
) -> Result<bool, String> {
    change_grants(places, id, |granted| granted.remove(kind, value))
}

/// Uninstall the extension `id`: its component, and its approval.
pub(crate) fn remove(places: &Places, id: &str) -> Result<(), String> {
    let mut approvals = places.approvals()?;
    let approved = approvals.remove(id);
    let dir = places.extensions.join(id);
    let installed = places.component(id).is_file();
    if !approved && !installed {
        return Err(format!("{id} is not installed"));
    }
    if installed {
        std::fs::remove_dir_all(&dir)
            .map_err(|error| format!("{} cannot be removed: {error}", dir.display()))?;
    }
    approvals.save(&places.approvals)
}

/// The package the component `bytes` carries.
fn package_in(bytes: &[u8]) -> Result<Package, String> {
    let text = winter_wasm::package_of(bytes).map_err(|error| format!("{error:#}"))?;
    Package::parse(&text)
}

/// Change what the extension `id` is granted with `change`, keeping what it
/// says.
fn change_grants(
    places: &Places,
    id: &str,
    change: impl FnOnce(&mut Grants) -> bool,
) -> Result<bool, String> {
    let mut approvals = places.approvals()?;
    let mut approval = approvals
        .get(id)
        .cloned()
        .ok_or_else(|| format!("{id} is not installed"))?;
    let changed = change(&mut approval.granted);
    if changed {
        approvals.set(approval);
        approvals.save(&places.approvals)?;
    }
    Ok(changed)
}

/// Refuse a grant that would not say plainly what it reaches: a directory
/// not from the root or home, a program named by more than its file name.
fn check_grant(kind: GrantKind, value: &str) -> Result<(), String> {
    let plain = match kind {
        GrantKind::Read | GrantKind::Write => {
            value == "~" || value.starts_with("~/") || Path::new(value).is_absolute()
        }
        GrantKind::Run => !value.is_empty() && !value.contains(std::path::is_separator),
    };
    match plain {
        true => Ok(()),
        false => Err(match kind {
            GrantKind::Read | GrantKind::Write => {
                format!("{value:?} is not a directory from / or ~")
            }
            GrantKind::Run => format!("{value:?} is not a program's file name"),
        }),
    }
}

/// The SHA-256 of `bytes`, in hex.
fn hash_of(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// A WebAssembly module of nothing but the package section, for tests:
/// enough to read, as installing does, though not to run.
#[cfg(test)]
pub(crate) fn module_with(package: &str) -> Vec<u8> {
    let name = winter_ext_api::PACKAGE_SECTION.as_bytes();
    let mut body = leb(name.len());
    body.extend_from_slice(name);
    body.extend_from_slice(package.as_bytes());
    let mut bytes = b"\0asm\x01\0\0\0\0".to_vec();
    bytes.extend(leb(body.len()));
    bytes.extend(body);
    bytes
}

/// `n` as an unsigned LEB128, as a module writes lengths.
#[cfg(test)]
fn leb(mut n: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        match n {
            0 => return [out, vec![byte]].concat(),
            _ => out.push(byte | 0x80),
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of its own for one test, removed when it is done.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("winter-manage-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("scratch");
            Self(root)
        }

        fn places(&self) -> Places {
            Places {
                approvals: self.0.join("state").join(APPROVALS_FILE),
                extensions: self.0.join(DIR_NAME),
            }
        }

        /// A component carrying `package`, built as `name`.
        fn built(&self, name: &str, package: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, module_with(package)).expect("built");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn package(asks: &str) -> String {
        format!("package id=\"notes\" version=\"1\" {{ permissions {{ {asks} }} }}")
    }

    #[test]
    fn test_only_the_component_approved_runs() {
        let scratch = Scratch::new("approved");
        let places = scratch.places();
        let built = scratch.built("notes.wasm", &package("read \"~/Notes\""));
        let review = review(&built, &places).expect("reads");
        assert_eq!(review.new.read, ["~/Notes"], "all of it is new");
        let path = install(review, &places).expect("installs");
        assert_eq!(path, places.component("notes"));
        let approvals = places.approvals().expect("kept");
        let running = approved(&places, &approvals, "notes").expect("runs");
        assert_eq!(running.granted.read, ["~/Notes"]);

        std::fs::write(&path, module_with(&package("read \"~\""))).expect("swapped");
        let refused = approved(&places, &approvals, "notes").expect_err("refused");
        assert!(
            refused.contains("changed since it was approved"),
            "{refused}"
        );
        let standing = installed(&places).expect("listed")[0].standing;
        assert_eq!(standing, Standing::Changed);
    }

    #[test]
    fn test_a_component_put_there_by_hand_never_runs() {
        let scratch = Scratch::new("by-hand");
        let places = scratch.places();
        std::fs::create_dir_all(places.extensions.join("notes")).expect("dir");
        std::fs::write(places.component("notes"), module_with(&package(""))).expect("put");
        let approvals = places.approvals().expect("none yet");
        let refused = approved(&places, &approvals, "notes").expect_err("refused");
        assert!(refused.contains("never approved"), "{refused}");
        assert_eq!(
            installed(&places).expect("listed")[0].standing,
            Standing::NotApproved
        );
    }

    #[test]
    fn test_reinstalling_asks_only_what_is_new_and_keeps_the_user_s_changes() {
        let scratch = Scratch::new("reinstall");
        let places = scratch.places();
        let first = scratch.built("v1.wasm", &package("read \"~/Notes\" \"~/Downloads\""));
        install(review(&first, &places).expect("reads"), &places).expect("installs");
        revoke(&places, "notes", GrantKind::Read, "~/Downloads").expect("revoked");
        grant(&places, "notes", GrantKind::Read, "~/Papers").expect("granted");

        let asks = "read \"~/Notes\" \"~/Downloads\"; write \"~/Notes\"";
        let second = scratch.built("v2.wasm", &package(asks));
        let review = review(&second, &places).expect("reads");
        assert_eq!(review.new.lines(), ["write  ~/Notes"], "only what is new");
        install(review, &places).expect("installs");
        let approvals = places.approvals().expect("kept");
        let granted = &approvals.get("notes").expect("approved").granted;
        assert_eq!(
            granted.read,
            ["~/Notes", "~/Papers"],
            "the revoked stays so"
        );
        assert_eq!(granted.write, ["~/Notes"]);
    }

    #[test]
    fn test_removing_takes_the_component_and_its_approval() {
        let scratch = Scratch::new("remove");
        let places = scratch.places();
        let built = scratch.built("notes.wasm", &package(""));
        install(review(&built, &places).expect("reads"), &places).expect("installs");
        remove(&places, "notes").expect("removed");
        assert!(!places.extensions.join("notes").exists());
        assert_eq!(places.approvals().expect("kept").get("notes"), None);
        assert!(remove(&places, "notes").is_err(), "nothing left to remove");
    }

    #[test]
    fn test_a_grant_must_say_plainly_what_it_reaches() {
        assert!(check_grant(GrantKind::Read, "~/Notes").is_ok());
        assert!(check_grant(GrantKind::Write, "/data").is_ok());
        assert!(check_grant(GrantKind::Read, "notes").is_err(), "relative");
        assert!(check_grant(GrantKind::Run, "server").is_ok());
        assert!(
            check_grant(GrantKind::Run, "/tmp/server").is_err(),
            "a path"
        );
    }
}
