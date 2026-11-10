//! Holds the workspace to its layers (see `docs/crate-structure-design.md`):
//! each crate's layer is the folder under `crates/` it sits in, and it may
//! depend only on crates in the layers below its own.
//!
//! - `app` may depend on anything.
//! - A tool (an editor, or the terminal runtime) may depend on `libs`,
//!   `base`, and `sdk` crates, and on no other tool: what two share sits in
//!   `libs` or `base`.
//! - A `libs` crate (languages, git, sessions, the picker, the extension
//!   runtime) may depend on `libs`, `base`, and `sdk` crates, so the core in
//!   `base` never carries any of their machinery.
//! - A `base` crate, the core `winter-model` and what it stands on, may
//!   depend on `base` and `sdk` crates.
//! - An `sdk` crate may depend only on `sdk` crates, so the kit an extension
//!   author builds with carries nothing of Winter's own.
//! - An extension, in `ext`, may depend only on `sdk` crates: the kit an
//!   author outside Winter has, and nothing more.
//!
//! Crates outside `crates/` (the clients, an extension in another
//! repository) are not held to a layer.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

// ========================================================================
// Constants
// ========================================================================

/// The folder under the workspace that holds the layers.
const CRATES_DIR: &str = "crates";

// ========================================================================
// Data Structures
// ========================================================================

/// A layer, by the folder its crates sit in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Layer {
    App,
    Base,
    Extensions,
    Libs,
    Sdk,
    Tools,
}

/// One crate of the workspace held to a layer, and the crates it names.
struct Member {
    dependencies: Vec<String>,
    layer: Layer,
    name: String,
}

// ========================================================================
// Layer
// ========================================================================

impl Layer {
    /// The layer of the folder named `folder`, if it is one.
    fn of_folder(folder: &str) -> Option<Self> {
        match folder {
            "app" => Some(Self::App),
            "base" => Some(Self::Base),
            "ext" => Some(Self::Extensions),
            "libs" => Some(Self::Libs),
            "sdk" => Some(Self::Sdk),
            "tools" => Some(Self::Tools),
            _ => None,
        }
    }

    /// Whether a crate in this layer may depend on one in `other`.
    fn may_depend_on(self, other: Self) -> bool {
        match self {
            Self::App => true,
            Self::Tools => matches!(other, Self::Libs | Self::Base | Self::Sdk),
            Self::Libs => matches!(other, Self::Libs | Self::Base | Self::Sdk),
            Self::Base => matches!(other, Self::Base | Self::Sdk),
            Self::Extensions | Self::Sdk => other == Self::Sdk,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Every crate under `crates/`, read from `cargo metadata` without
/// resolving what lies outside the workspace.
fn members() -> Vec<Member> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(manifest_dir)
        .output()
        .expect("cargo runs");
    assert!(
        output.status.success(),
        "cargo metadata reads the workspace"
    );
    let metadata: Value = serde_json::from_slice(&output.stdout).expect("metadata as JSON");
    let root = Path::new(
        metadata["workspace_root"]
            .as_str()
            .expect("a workspace root"),
    );
    let packages = metadata["packages"].as_array().expect("packages");
    packages
        .iter()
        .filter_map(|package| {
            let manifest = Path::new(package["manifest_path"].as_str()?);
            let relative = manifest.strip_prefix(root.join(CRATES_DIR)).ok()?;
            let folder = relative.components().next()?.as_os_str().to_str()?;
            let dependencies = package["dependencies"]
                .as_array()?
                .iter()
                .filter(|dependency| dependency["path"].is_string())
                .filter_map(|dependency| Some(dependency["name"].as_str()?.to_string()))
                .collect();
            Some(Member {
                dependencies,
                layer: Layer::of_folder(folder)?,
                name: package["name"].as_str()?.to_string(),
            })
        })
        .collect()
}

/// Every dependency between workspace crates that crosses a layer the wrong
/// way, as `crate -> dependency`.
fn upward(members: &[Member]) -> Vec<String> {
    let layer_of = |name: &str| members.iter().find(|m| m.name == name).map(|m| m.layer);
    let mut wrong: Vec<String> = members
        .iter()
        .flat_map(|member| {
            member
                .dependencies
                .iter()
                .filter_map(move |dependency| Some((member, dependency, layer_of(dependency)?)))
        })
        .filter(|(member, _, layer)| !member.layer.may_depend_on(*layer))
        .map(|(member, dependency, _)| format!("{} -> {dependency}", member.name))
        .collect();
    wrong.sort();
    wrong.dedup();
    wrong
}

// ========================================================================
// Tests
// ========================================================================

#[test]
fn test_every_crate_sits_in_a_layer_folder() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let strays: Vec<String> = std::fs::read_dir(root.join(CRATES_DIR))
        .expect("the crates folder")
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| Layer::of_folder(name).is_none())
        .collect();
    assert!(strays.is_empty(), "outside a layer folder: {strays:?}");
}

#[test]
fn test_no_crate_depends_on_a_layer_above_its_own() {
    let members = members();
    assert!(
        members.iter().any(|m| m.layer == Layer::Tools),
        "the tools are read"
    );
    let wrong = upward(&members);
    assert!(
        wrong.is_empty(),
        "dependencies against the layers: {wrong:?}"
    );
}

#[test]
fn test_an_upward_dependency_is_caught() {
    let member = |name: &str, layer, dependencies: &[&str]| Member {
        dependencies: dependencies.iter().map(|d| d.to_string()).collect(),
        layer,
        name: name.to_string(),
    };
    let members = [
        member("winter-term", Layer::App, &[]),
        member(
            "winter-git",
            Layer::Tools,
            &["winter-pdf", "winter-lsp", "winter-vcs"],
        ),
        member("winter-pdf", Layer::Tools, &[]),
        member(
            "winter-model",
            Layer::Base,
            &["winter-term", "winter-ext-api", "winter-lsp", "winter-vcs"],
        ),
        member("winter-ext-api", Layer::Sdk, &["winter-model"]),
        member(
            "winter-copilot",
            Layer::Extensions,
            &["winter-ext-api", "winter-lsp"],
        ),
        member("winter-lsp", Layer::Libs, &[]),
        member("winter-vcs", Layer::Libs, &["winter-lsp"]),
    ];
    assert_eq!(
        upward(&members),
        [
            "winter-copilot -> winter-lsp",
            "winter-ext-api -> winter-model",
            "winter-git -> winter-pdf",
            "winter-model -> winter-lsp",
            "winter-model -> winter-term",
            "winter-model -> winter-vcs",
        ],
        "an extension or the sdk on Winter's own, a tool on another, base on a library or \
         the app"
    );
}
