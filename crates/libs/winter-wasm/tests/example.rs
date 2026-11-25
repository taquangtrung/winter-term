//! The example extension, built as a component, run through the runtime as
//! Winter runs it: its commands, its linter reading files through the
//! sandbox, and its view.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use winter_ext_api::testing::Recorder;
use winter_ext_api::{
    Document, Event, Extension, InlineAnswer, InlineKind, InlineOffer, InlineRequest, Line,
    Registry, SettingValue, Settings, TextPosition, TextRange,
};
use winter_wasm::{FILE_EXTENSION, Runtime, Sandbox};

// ========================================================================
// Constants
// ========================================================================

/// The example's id, which its component file is named after.
const EXAMPLE: &str = "example";

/// The target components are built for.
const TARGET: &str = "wasm32-wasip2";

/// A file holding one `TODO`.
const WITH_TODO: &str = "first line\nTODO: write the rest\n";

/// The misbehaving component's id.
const RUNAWAY: &str = "runaway";

/// The example component, built once for every test.
static BUILT: OnceLock<PathBuf> = OnceLock::new();

/// The misbehaving component, built once for every test.
static BUILT_RUNAWAY: OnceLock<PathBuf> = OnceLock::new();

// ========================================================================
// Data Structures
// ========================================================================

/// A directory of its own for one test, removed when it is done.
struct Scratch {
    root: PathBuf,
}

// ========================================================================
// Scratch
// ========================================================================

impl Scratch {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("winter-wasm-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch directory");
        Self { root }
    }

    /// A directory under it, made.
    fn dir(&self, name: &str) -> PathBuf {
        let dir = self.root.join(name);
        std::fs::create_dir_all(&dir).expect("a directory");
        dir
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The example built as a component, once.
fn built() -> &'static Path {
    BUILT.get_or_init(|| {
        let workspace = workspace();
        let package = ["-p", "winter-simple-ext", "--features", "wasm"];
        build_component(&workspace, &package, "winter_simple_ext")
    })
}

/// The misbehaving extension built as a component, once.
fn built_runaway() -> &'static Path {
    BUILT_RUNAWAY.get_or_init(|| {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join(RUNAWAY);
        build_component(&fixture, &[], RUNAWAY)
    })
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// The component `crate_name` builds to, built from the crate `package`
/// picks in `dir`.
fn build_component(dir: &Path, package: &[&str], crate_name: &str) -> PathBuf {
    // A target directory of its own, so this build does not wait on the
    // lock of the one running the tests.
    let target_dir = workspace().join("target/wasm-guest");
    let mut cargo = Command::new(env!("CARGO"));
    // The config the tests were built with, when it patches crates in
    // (`.cargo/local.toml` names itself), so the guest builds against the
    // same misty and Cherry rather than looking for them on crates.io.
    let config = std::env::var("WINTER_CARGO_CONFIG")
        .ok()
        .or(option_env!("WINTER_CARGO_CONFIG").map(str::to_owned));
    if let Some(config) = config {
        cargo.arg("--config").arg(config);
    }
    let status = cargo
        .current_dir(dir)
        .arg("rustc")
        .args(package)
        .args([
            "--release",
            "--target",
            TARGET,
            "--crate-type",
            "cdylib",
            "--target-dir",
        ])
        .arg(&target_dir)
        .status()
        .expect("cargo runs");
    assert!(status.success(), "{crate_name} builds for {TARGET}");
    target_dir
        .join(TARGET)
        .join("release")
        .join(format!("{crate_name}.{FILE_EXTENSION}"))
}

/// The example, installed in `scratch` under `file_name`, loaded with
/// `read` granted.
fn load(
    scratch: &Scratch,
    file_name: &str,
    read: Vec<PathBuf>,
) -> winter_wasm::Result<Box<dyn Extension>> {
    load_component(built(), scratch, file_name, read)
}

/// The component built at `built`, installed in `scratch` under
/// `file_name`, loaded with `read` granted.
fn load_component(
    built: &Path,
    scratch: &Scratch,
    file_name: &str,
    read: Vec<PathBuf>,
) -> winter_wasm::Result<Box<dyn Extension>> {
    let installed = scratch.dir("extensions").join(file_name);
    std::fs::copy(built, &installed).expect("installed");
    let sandbox = Sandbox {
        data_dir: scratch.root.join("data"),
        home: None,
        net: Vec::new(),
        read,
        run: Vec::new(),
        write: Vec::new(),
    };
    let id = installed.file_stem().expect("named").to_string_lossy();
    let bytes = std::fs::read(&installed).expect("readable");
    Runtime::new(None)?.load(&id, &bytes, &sandbox)
}

fn example_file() -> String {
    format!("{EXAMPLE}.{FILE_EXTENSION}")
}

/// Every line's text, its runs joined.
fn texts(lines: &[Line]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.iter().map(|styled| styled.text.as_str()).collect())
        .collect()
}

// ========================================================================
// Tests
// ========================================================================

#[test]
fn test_a_component_carries_its_crate_s_package_read_without_running_it() {
    let bytes = std::fs::read(built()).expect("built");
    let package = workspace().join("crates/ext/winter-simple-ext/package.kdl");
    assert_eq!(
        winter_wasm::package_of(&bytes).expect("one package"),
        std::fs::read_to_string(package).expect("the crate's package")
    );
    let bare = std::fs::read(built_runaway()).expect("built");
    assert!(
        winter_wasm::package_of(&bare).is_ok(),
        "every export carries one"
    );
}

#[test]
fn test_a_granted_directory_not_there_does_not_stop_it_loading() {
    let scratch = Scratch::new("missing");
    let missing = scratch.root.join("never-made");
    assert!(load(&scratch, &example_file(), vec![missing]).is_ok());
}

#[test]
fn test_a_command_reads_the_file_in_front_through_the_host() {
    let scratch = Scratch::new("command");
    let extension = load(&scratch, &example_file(), Vec::new()).expect("loads");
    let registry = Registry::activate_all([extension.as_ref()]);
    let command = registry
        .find("example.word_count")
        .expect("registered under its id");
    assert_eq!(command.title, "Example: Count Words in File");
    let document = Document::new("/notes/a.md".into(), "one two three".into());
    let mut host = Recorder::default().with_document(document);
    (command.run)(&mut host);
    assert_eq!(host.said, ["3 words in a.md"]);
}

#[test]
fn test_a_component_s_settings_are_declared_and_heard_and_its_asks_reach_the_host() {
    let scratch = Scratch::new("settings");
    let extension = load(&scratch, &example_file(), Vec::new()).expect("loads");
    let registry = Registry::activate_all([extension.as_ref()]);
    let defaults = registry.default_settings(EXAMPLE);
    assert_eq!(defaults.switch("copy-count"), Some(false), "declared off");
    let document = Document::new("/notes/a.md".into(), "one two".into());
    let mut host = Recorder::default().with_document(document);
    let count = registry.find("example.word_count").expect("a command");
    (count.run)(&mut host);
    assert!(host.copied.is_empty(), "nothing copied while it is off");
    let on = Settings::new([("copy-count".to_string(), SettingValue::Switch(true))].into());
    for subscription in registry.subscriptions() {
        (subscription.handle)(&mut host, &Event::Configured(on.clone()));
    }
    (count.run)(&mut host);
    assert_eq!(host.copied, ["2"], "heard switched on");
    let guide = registry.find("example.open_guide").expect("a command");
    (guide.run)(&mut host);
    assert_eq!(host.urls.len(), 1);
    assert!(host.urls[0].starts_with("https://"));
}

#[test]
fn test_a_component_offers_ghost_text_and_answers_under_the_ticket_asked() {
    let scratch = Scratch::new("inline");
    let extension = load(&scratch, &example_file(), Vec::new()).expect("loads");
    let registry = Registry::activate_all([extension.as_ref()]);
    let inline = registry
        .find_inline("example.todo")
        .expect("an inline source");
    assert_eq!(inline.title, "Example");
    let document = Document::new("/notes/a.md".into(), "// TO\n".into());
    let mut host = Recorder::default().with_document(document);
    for subscription in registry.subscriptions() {
        (subscription.handle)(&mut host, &Event::Configured(Settings::default()));
    }
    assert_eq!(
        host.offered,
        [("todo".to_string(), InlineOffer::new(true, false))]
    );
    let at = TextPosition::new(0, 5);
    let request = InlineRequest::new(9, InlineKind::Ghost, "/notes/a.md".into(), at);
    inline.provider.ask(&mut host, &request);
    let [(ticket, InlineAnswer::Ghost(ghost))] = host.answered.as_slice() else {
        panic!("one ghost answer: {:?}", host.answered);
    };
    assert_eq!(*ticket, 9);
    assert_eq!(ghost[0].text, "DO");
    assert_eq!(
        ghost[0].range,
        TextRange::new(at, at),
        "put in at the cursor"
    );
}

#[test]
fn test_a_component_s_picker_offers_its_rows_and_hears_the_action_chosen() {
    let scratch = Scratch::new("picker");
    let extension = load(&scratch, &example_file(), Vec::new()).expect("loads");
    let registry = Registry::activate_all([extension.as_ref()]);
    let command = registry.find("example.pick_todo").expect("registered");
    let document = Document::new("/notes/a.md".into(), WITH_TODO.into());
    let mut host = Recorder::default()
        .choosing("go")
        .with_document(document.clone());
    (command.run)(&mut host);
    let [spec] = host.pickers.as_slice() else {
        panic!("one picker opened");
    };
    assert_eq!(spec.rows[0].label, "TODO: write the rest");
    let actions: Vec<&str> = spec.rows[0].actions.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(actions, ["go", "flag"]);
    assert_eq!(host.opened, [(PathBuf::from("/notes/a.md"), Some(2))]);
    let mut host = Recorder::default().choosing("flag").with_document(document);
    (command.run)(&mut host);
    assert!(host.opened.is_empty(), "the action chosen, not the first");
    assert_eq!(host.published.len(), 1);
}

#[test]
fn test_the_sandbox_lets_slow_work_read_only_what_was_granted() {
    let scratch = Scratch::new("sandbox");
    let granted = scratch.dir("granted");
    let hidden = scratch.dir("hidden");
    std::fs::write(granted.join("a.md"), WITH_TODO).expect("written");
    std::fs::write(hidden.join("b.md"), WITH_TODO).expect("written");
    let extension = load(&scratch, &example_file(), vec![granted.clone()]).expect("loads");
    let registry = Registry::activate_all([extension.as_ref()]);
    let [subscription] = registry.subscriptions() else {
        panic!("the linter listens");
    };
    let mut host = Recorder::default();
    (subscription.handle)(&mut host, &Event::Saved(granted.join("a.md")));
    (subscription.handle)(&mut host, &Event::Saved(hidden.join("b.md")));
    let [(path, found)] = host.published.as_slice() else {
        panic!("only the granted file is linted: {:?}", host.published);
    };
    assert_eq!(path, &granted.join("a.md"));
    assert_eq!(found.len(), 1, "its one TODO");
    assert_eq!((found[0].line, found[0].start), (1, 0));
}

#[test]
fn test_a_view_draws_what_the_extensions_work_found() {
    let scratch = Scratch::new("view");
    let granted = scratch.dir("granted");
    std::fs::write(granted.join("a.md"), WITH_TODO).expect("written");
    let extension = load(&scratch, &example_file(), vec![granted.clone()]).expect("loads");
    let registry = Registry::activate_all([extension.as_ref()]);
    let kind = registry.find_view("example.todo_board").expect("a view");
    let mut board = (kind.open)();
    let before = texts(&board.render(10, 80));
    assert_eq!(before[2], "none yet: save a file holding a TODO");
    let mut host = Recorder::default();
    (registry.subscriptions()[0].handle)(&mut host, &Event::Saved(granted.join("a.md")));
    let after = texts(&board.render(10, 80));
    assert_eq!(after[2], granted.join("a.md").display().to_string());
    assert_eq!(after[3].trim(), "2  TODO: write the rest");
    assert_eq!(board.cursor_line(), Some(3), "the first TODO chosen");
}

#[test]
fn test_a_view_closed_is_dropped_and_its_extension_runs_on() {
    let scratch = Scratch::new("closed");
    let extension = load(&scratch, &example_file(), Vec::new()).expect("loads");
    let registry = Registry::activate_all([extension.as_ref()]);
    let kind = registry.find_view("example.todo_board").expect("a view");
    for _ in 0..2 {
        let mut board = (kind.open)();
        assert!(!texts(&board.render(10, 80)).is_empty(), "drawn while open");
    }
    let document = Document::new("/notes/a.md".into(), "one".into());
    let mut host = Recorder::default().with_document(document);
    (registry.find("example.word_count").expect("a command").run)(&mut host);
    assert_eq!(
        host.said,
        ["1 words in a.md"],
        "not stopped by the views closed"
    );
}

#[test]
fn test_a_component_not_named_for_its_id_is_refused() {
    let scratch = Scratch::new("misnamed");
    let refused = load(&scratch, &format!("other.{FILE_EXTENSION}"), Vec::new());
    let error = refused.err().expect("refused").to_string();
    assert!(error.contains("example"), "says whose it is: {error}");
}

#[test]
fn test_a_call_that_never_returns_stops_its_extension_not_winter() {
    let scratch = Scratch::new("runaway");
    let file_name = format!("{RUNAWAY}.{FILE_EXTENSION}");
    let extension =
        load_component(built_runaway(), &scratch, &file_name, Vec::new()).expect("loads");
    let registry = Registry::activate_all([extension.as_ref()]);
    let run = |id: &str, host: &mut Recorder| (registry.find(id).expect("registered").run)(host);
    let mut host = Recorder::default();
    run("runaway.ping", &mut host);
    assert_eq!(host.said, ["pong"], "it runs");
    let started = std::time::Instant::now();
    run("runaway.spin", &mut host);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "stopped within its budget"
    );
    let [_, stopped] = host.said.as_slice() else {
        panic!("one notice of its stopping: {:?}", host.said);
    };
    assert!(
        stopped.starts_with("the runaway extension failed and was stopped"),
        "{stopped}"
    );
    run("runaway.ping", &mut host);
    assert_eq!(host.said.len(), 2, "a stopped extension is called no more");
}

#[test]
fn test_a_panic_stops_its_extension_with_a_notice() {
    let scratch = Scratch::new("panic");
    let file_name = format!("{RUNAWAY}.{FILE_EXTENSION}");
    let extension =
        load_component(built_runaway(), &scratch, &file_name, Vec::new()).expect("loads");
    let registry = Registry::activate_all([extension.as_ref()]);
    let mut host = Recorder::default();
    (registry.find("runaway.panic").expect("registered").run)(&mut host);
    let [stopped] = host.said.as_slice() else {
        panic!("one notice of its stopping: {:?}", host.said);
    };
    assert!(
        stopped.starts_with("the runaway extension failed and was stopped"),
        "{stopped}"
    );
}
