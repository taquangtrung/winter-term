//! The example component as each release of the extension interface built
//! it, run by this Winter: what holds Winter to its promise that an
//! extension built against any 0.1.x runs on every later 0.1.x. A release adds
//! its build here, and none is ever rebuilt: a change that breaks one broke
//! every extension built against that release.

use winter_ext_api::testing::Recorder;
use winter_ext_api::{Document, Event, Extension, Registry, Settings};
use winter_wasm::{Runtime, Sandbox};

// ========================================================================
// Constants
// ========================================================================

/// The example's id, as its package names it.
const EXAMPLE: &str = "example";

/// The example component each release built, by its version.
const RELEASED: [(&str, &[u8]); 1] = [("0.1.0", include_bytes!("released/example-0.1.0.wasm"))];

// ========================================================================
// Functions
// ========================================================================

/// What `test` makes of the example `version` built, loaded with a data
/// directory of its own, removed once done.
fn with_loaded(version: &str, bytes: &[u8], test: impl FnOnce(Box<dyn Extension>)) {
    let data_dir = std::env::temp_dir().join(format!(
        "winter-wasm-released-{version}-{}",
        std::process::id()
    ));
    let sandbox = Sandbox {
        data_dir: data_dir.clone(),
        ..Sandbox::default()
    };
    let loaded = Runtime::new(None)
        .expect("a runtime")
        .load(EXAMPLE, bytes, &sandbox);
    let extension = loaded.unwrap_or_else(|error| panic!("{version} loads: {error:#}"));
    test(extension);
    let _ = std::fs::remove_dir_all(data_dir);
}

// ========================================================================
// Tests
// ========================================================================

#[test]
fn test_every_released_example_runs_its_commands_events_and_views() {
    for (version, bytes) in RELEASED {
        with_loaded(version, bytes, |extension| {
            let registry = Registry::activate_all([extension.as_ref()]);
            let document = Document::new("/notes/a.md".into(), "one two three".into());
            let mut host = Recorder::default().with_document(document);
            let count = registry.find("example.word_count");
            (count.unwrap_or_else(|| panic!("{version}: a command")).run)(&mut host);
            assert_eq!(host.said, ["3 words in a.md"], "{version}: a command runs");

            for subscription in registry.subscriptions() {
                (subscription.handle)(&mut host, &Event::Configured(Settings::default()));
            }
            assert!(!host.offered.is_empty(), "{version}: an event is heard");

            let kind = registry.find_view("example.todo_board");
            let mut board = (kind.unwrap_or_else(|| panic!("{version}: a view")).open)();
            assert!(!board.render(10, 80).is_empty(), "{version}: a view draws");
        });
    }
}
