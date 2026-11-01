//! An extension's package: the `package.kdl` its component carries,
//! embedded as it was built (see `winter_wasm::package_of`), saying what it
//! is, what it asks to reach beyond its own directory, and what its
//! commands are called.
//!
//! ```kdl
//! package id="magic-winter" name="Magic Winter" version="0.1.0" {
//!     description "Personal productivity suite"
//!     permissions {
//!         read "~/Notes" "~/Documents"
//!         write "~/Notes"
//!         run "some-language-server"
//!     }
//!     commands {
//!         "magic.gtd" title="Magic: Open GTD Tasks"
//!     }
//! }
//! ```
//!
//! What it asks is only asked: an extension installed as a file reaches
//! what the user approved (see `approvals`), never what it says of itself.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use crate::config::schema::present_list;

// ========================================================================
// Constants
// ========================================================================

/// What may stand in an extension's id besides letters and digits: its id
/// names its directory, so nothing that leads out of one.
const ID_PUNCTUATION: [char; 3] = ['-', '_', '.'];

// ========================================================================
// Data Structures
// ========================================================================

/// What a package says of its extension.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Package {
    /// What it asks to reach.
    pub(crate) asks: Grants,
    pub(crate) description: Option<String>,
    pub(crate) id: String,
    pub(crate) name: Option<String>,
    /// What its commands are called, by their ids.
    pub(crate) titles: HashMap<String, String>,
    pub(crate) version: Option<String>,
}

/// What an extension may reach beyond its own directory: directories to
/// read, directories to read and write, and the programs it may have Winter
/// run as its servers, each as written, a directory's `~` not yet expanded.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Grants {
    pub(crate) read: Vec<String>,
    pub(crate) run: Vec<String>,
    pub(crate) write: Vec<String>,
}

/// One kind of grant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GrantKind {
    Read,
    Run,
    Write,
}

/// A `package.kdl`, as written.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct KdlPackageFile {
    package: KdlPackage,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct KdlPackage {
    #[serde(default)]
    commands: Option<HashMap<String, KdlCommand>>,
    #[serde(default)]
    description: Option<String>,
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    permissions: Option<KdlGrants>,
    #[serde(default)]
    version: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct KdlCommand {
    title: String,
}

#[derive(Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct KdlGrants {
    #[serde(default, deserialize_with = "present_list")]
    read: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present_list")]
    run: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present_list")]
    write: Option<Vec<String>>,
}

// ========================================================================
// Package
// ========================================================================

impl Package {
    /// The package `text` writes, or why it does not read.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let file: KdlPackageFile = kdl::de::from_str(text)
            .map_err(|error| format!("its package does not read: {error}"))?;
        let package = file.package;
        if !is_valid_id(&package.id) {
            return Err(format!("its id {:?} cannot name a directory", package.id));
        }
        let asks = package.permissions.unwrap_or_default();
        let titles = package.commands.unwrap_or_default();
        Ok(Self {
            asks: Grants {
                read: asks.read.unwrap_or_default(),
                run: asks.run.unwrap_or_default(),
                write: asks.write.unwrap_or_default(),
            },
            description: package.description,
            id: package.id,
            name: package.name,
            titles: titles
                .into_iter()
                .map(|(id, command)| (id, command.title))
                .collect(),
            version: package.version,
        })
    }

    /// How it is named to the user: its name, else its id, and its version.
    pub(crate) fn heading(&self) -> String {
        let name = self.name.as_deref().unwrap_or(&self.id);
        match &self.version {
            Some(version) => format!("{name} {version} ({})", self.id),
            None => format!("{name} ({})", self.id),
        }
    }
}

// ========================================================================
// Grants
// ========================================================================

impl Grants {
    /// The grants of `kind`.
    pub(crate) fn of(&self, kind: GrantKind) -> &[String] {
        match kind {
            GrantKind::Read => &self.read,
            GrantKind::Run => &self.run,
            GrantKind::Write => &self.write,
        }
    }

    fn of_mut(&mut self, kind: GrantKind) -> &mut Vec<String> {
        match kind {
            GrantKind::Read => &mut self.read,
            GrantKind::Run => &mut self.run,
            GrantKind::Write => &mut self.write,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        GrantKind::ALL.iter().all(|kind| self.of(*kind).is_empty())
    }

    /// Grant `value` of `kind`; whether it was not granted already.
    pub(crate) fn add(&mut self, kind: GrantKind, value: &str) -> bool {
        let listed = self.of_mut(kind);
        let fresh = !listed.iter().any(|had| had == value);
        if fresh {
            listed.push(value.to_string());
        }
        fresh
    }

    /// Take back `value` of `kind`; whether it was granted.
    pub(crate) fn remove(&mut self, kind: GrantKind, value: &str) -> bool {
        let listed = self.of_mut(kind);
        let before = listed.len();
        listed.retain(|had| had != value);
        listed.len() != before
    }

    /// What these hold that `other` does not.
    pub(crate) fn beyond(&self, other: &Grants) -> Grants {
        let mut beyond = Grants::default();
        for kind in GrantKind::ALL {
            for value in self.of(kind) {
                if !other.of(kind).contains(value) {
                    beyond.add(kind, value);
                }
            }
        }
        beyond
    }

    /// These, with every one of `more` besides.
    pub(crate) fn with(&self, more: &Grants) -> Grants {
        let mut joined = self.clone();
        for kind in GrantKind::ALL {
            for value in more.of(kind) {
                joined.add(kind, value);
            }
        }
        joined
    }

    /// Each grant as a line: its kind, then what it reaches.
    pub(crate) fn lines(&self) -> Vec<String> {
        GrantKind::ALL
            .iter()
            .flat_map(|kind| {
                self.of(*kind)
                    .iter()
                    .map(move |value| format!("{kind}  {value}"))
            })
            .collect()
    }
}

// ========================================================================
// GrantKind
// ========================================================================

impl GrantKind {
    /// Every kind, in the order they are shown.
    pub(crate) const ALL: [GrantKind; 3] = [GrantKind::Read, GrantKind::Write, GrantKind::Run];

    /// Its name, as a package and the command line write it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            GrantKind::Read => "read",
            GrantKind::Run => "run",
            GrantKind::Write => "write",
        }
    }
}

impl fmt::Display for GrantKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:<5}", self.name())
    }
}

impl FromStr for GrantKind {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        GrantKind::ALL
            .into_iter()
            .find(|kind| kind.name() == name)
            .ok_or_else(|| format!("{name:?} is no kind of grant: read, write, or run"))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Whether `id` can name an extension's directory: letters, digits, and
/// [`ID_PUNCTUATION`], not starting with a `.`.
fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ID_PUNCTUATION.contains(&c))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_package_says_what_it_is_asks_and_calls_its_commands() {
        let text = r#"
package id="magic" name="Magic Winter" version="0.1.0" {
    description "Productivity suite"
    permissions {
        read "~/Notes" "~/Documents"
        write "~/Notes"
    }
    commands {
        "magic.gtd" title="Magic: Open GTD Tasks"
    }
}
"#;
        let package = Package::parse(text).expect("reads");
        assert_eq!(package.id, "magic");
        assert_eq!(package.heading(), "Magic Winter 0.1.0 (magic)");
        assert_eq!(package.asks.read, ["~/Notes", "~/Documents"]);
        assert_eq!(package.asks.write, ["~/Notes"]);
        assert!(package.asks.run.is_empty());
        assert_eq!(
            package.titles.get("magic.gtd").map(String::as_str),
            Some("Magic: Open GTD Tasks")
        );
    }

    #[test]
    fn test_the_shipped_packages_read() {
        // Read from the workspace: the files are the extension crates'.
        let ext = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ext");
        for (dir, id) in [
            ("winter-copilot", "copilot"),
            ("winter-simple-ext", "example"),
        ] {
            let text = std::fs::read_to_string(ext.join(dir).join("package.kdl")).expect("there");
            assert_eq!(Package::parse(&text).expect(dir).id, id);
        }
    }

    #[test]
    fn test_a_package_naming_what_it_has_no_place_for_is_refused() {
        for text in [
            "package id=\"a\" { bindings { \"C-S-t\" action=\"a.b\" } }",
            "package id=\"a\" { permissions { network true } }",
            "read \"/notes\"",
        ] {
            assert!(Package::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn test_an_id_that_could_lead_out_of_its_directory_is_refused() {
        for id in ["", "..", "../x", "a/b", ".hidden", "a b"] {
            let text = format!("package id={id:?}");
            assert!(Package::parse(&text).is_err(), "{id:?}");
        }
        assert!(Package::parse("package id=\"my.ext-2_b\"").is_ok());
    }

    #[test]
    fn test_grants_beyond_others_are_those_they_lack() {
        let asked = Grants {
            read: vec!["~/a".into(), "~/b".into()],
            run: vec!["srv".into()],
            write: Vec::new(),
        };
        let mut granted = Grants::default();
        granted.add(GrantKind::Read, "~/a");
        let beyond = asked.beyond(&granted);
        assert_eq!(beyond.read, ["~/b"]);
        assert_eq!(beyond.run, ["srv"]);
        assert_eq!(granted.with(&beyond).beyond(&asked), Grants::default());
    }
}
