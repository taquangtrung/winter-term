//! The extension WIT as each release left it, against the WIT now: what
//! holds every change to adding, so a component built against a release
//! links with every later Winter of its release line. A release adds its WIT here,
//! and none is ever edited.

use wit_parser::Resolve;

// ========================================================================
// Constants
// ========================================================================

/// The WIT each release left, by its version.
const RELEASED: [(&str, &str); 1] = [("0.1.0", include_str!("released/winter-0.1.0.wit"))];

/// The WIT now.
const CURRENT: &str = include_str!("../../../sdk/winter-ext-api/wit/winter.wit");

/// The world a component is built for.
const WORLD: &str = "extension";

// ========================================================================
// Functions
// ========================================================================

/// Whether a component built against the `released` WIT of `version`
/// links where the `current` one is spoken, or why not.
fn grows_from(version: &str, released: &str, current: &str) -> Result<(), String> {
    let mut resolve = Resolve::default();
    // Named apart from the WIT now, which may still be this release.
    let released = released.replace(&format!("@{version};"), &format!("@{version}-released;"));
    let parsed = |resolve: &mut Resolve, path: &str, text: &str| {
        let package = resolve.push_str(path, text).map_err(|e| format!("{e:#}"))?;
        resolve
            .select_world(&[package], Some(WORLD))
            .map_err(|e| format!("{e:#}"))
    };
    let prev = parsed(&mut resolve, "released.wit", &released)?;
    let new = parsed(&mut resolve, "winter.wit", current)?;
    wit_component::semver_check(resolve, prev, new).map_err(|e| format!("{e:#}"))
}

// ========================================================================
// Tests
// ========================================================================

#[test]
fn test_the_wit_only_grows_from_each_release() {
    for (version, released) in RELEASED {
        if let Err(why) = grows_from(version, released, CURRENT) {
            panic!("a component built against {version} would no longer link: {why}");
        }
    }
}

#[test]
fn test_a_field_added_to_a_released_record_is_caught() {
    let (version, released) = RELEASED[0];
    let grown = CURRENT.replacen(
        "record status {",
        "record status {\n        extra: bool,",
        1,
    );
    assert_ne!(grown, CURRENT, "the record is there to grow");
    assert!(grows_from(version, released, &grown).is_err());
}
