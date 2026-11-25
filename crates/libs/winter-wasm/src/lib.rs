//! Runs Winter extensions built as WebAssembly components, each in a
//! sandbox of its own, as extensions compiled in run: a loaded one is an
//! [`Extension`] like any other, so Winter tells the two apart nowhere else.
//!
//! The sandbox is the component's: it reaches only the directories it is
//! granted, its memory is capped, and a call into it that runs too long is
//! stopped, as is the extension, rather than holding Winter up.

mod convert;
mod plugin;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use wasmparser::{Parser, Payload};
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::error::Context;
use wasmtime::{Cache, CacheConfig, Config, Engine, Store, StoreLimitsBuilder, bail};
use wasmtime_wasi::{FsPerms, WasiCtxBuilder};
use winter_ext_api::{Extension, Host, PACKAGE_SECTION, Registry, View};

use self::bindings::winter::extension::types;
use self::plugin::{Grants, Instance, Plugin, State, WasmInline, WasmView};

pub use wasmtime::{Error, Result};

/// Winter's side of the contract with a component, generated from it.
mod bindings {
    wasmtime::component::bindgen!({
        world: "extension",
        path: "../../sdk/winter-ext-api/wit",
    });
}

// ========================================================================
// Constants
// ========================================================================

/// What a component file is named with, after its extension's id.
pub const FILE_EXTENSION: &str = "wasm";

/// The environment variable telling an extension where its own directory,
/// which it may always read and write, is.
pub const DATA_DIR_VARIABLE: &str = "WINTER_DATA_DIR";

/// The interface a component built against this Winter's API imports, and
/// the version this Winter speaks. A component built against an earlier
/// version of the same release line links, the WIT only growing within
/// one; one built against a later version or another line would fail to,
/// so it is named and refused before.
const WIT_INTERFACE: &str = "winter:extension/host";
const WIT_VERSION: &str = "0.1.0";

/// The variable an extension finds the user's home directory in.
pub const HOME_VARIABLE: &str = "HOME";

/// How often the clock calls are timed against moves.
const TICK: Duration = Duration::from_millis(10);

/// How long an extension may take to activate.
const ACTIVATE_BUDGET: Duration = Duration::from_secs(10);

/// The most memory one extension may hold.
const MEMORY_LIMIT: usize = 256 << 20;

/// What the clock's thread is called.
const TICKER_NAME: &str = "winter-wasm-clock";

// ========================================================================
// Data Structures
// ========================================================================

/// What runs extensions built as components: compiles them, caching what
/// it compiled, and times every call into them.
pub struct Runtime {
    engine: Engine,
    linker: Linker<State>,
}

/// What one extension may reach outside its sandbox: its own directory,
/// the directories the user granted it, the hosts it may make requests of,
/// and the programs it may have Winter run.
#[derive(Clone, Debug, Default)]
pub struct Sandbox {
    /// Where it keeps what it keeps; made if missing.
    pub data_dir: PathBuf,
    /// The user's home directory, told it as `HOME` so it can name what it
    /// is granted there.
    pub home: Option<PathBuf>,
    /// The hosts it may make requests of, by name.
    pub net: Vec<String>,
    /// Directories it may read.
    pub read: Vec<PathBuf>,
    /// The file names of the programs it may run, as its servers or not.
    pub run: Vec<String>,
    /// Directories it may read and write.
    pub write: Vec<PathBuf>,
}

/// An extension built as a component, activated, standing for it among
/// the extensions compiled in.
struct WasmExtension {
    manifest: types::Manifest,
    plugin: Arc<Plugin>,
}

// ========================================================================
// Runtime
// ========================================================================

impl Runtime {
    /// A runtime keeping what it compiles under `cache_dir`, or compiling
    /// afresh each start without one.
    pub fn new(cache_dir: Option<&Path>) -> Result<Self> {
        let mut config = Config::new();
        config.epoch_interruption(true);
        if let Some(dir) = cache_dir {
            let mut cache = CacheConfig::new();
            cache.with_directory(dir);
            config.cache(Some(Cache::new(cache)?));
        }
        let engine = Engine::new(&config)?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        bindings::Extension::add_to_linker::<_, HasSelf<State>>(&mut linker, |state| state)?;
        start_clock(&engine)?;
        Ok(Self { engine, linker })
    }

    /// Load and activate the extension in the component `bytes`, whose id
    /// must be `id`, sandboxed as `sandbox` says. The bytes are those read
    /// once, so what runs is what was checked, not a file since replaced.
    pub fn load(&self, id: &str, bytes: &[u8], sandbox: &Sandbox) -> Result<Box<dyn Extension>> {
        let component = Component::new(&self.engine, bytes)?;
        let imported = component
            .component_type()
            .imports(&self.engine)
            .find_map(|(name, _)| built_against(name).map(str::to_string));
        if let Some(version) = imported.filter(|version| !speaks(version)) {
            bail!(
                "it was built for version {version} of the extension interface, and this \
                 Winter speaks {WIT_VERSION}: update Winter, or rebuild it against this \
                 Winter's winter-ext-guest"
            );
        }
        let mut store = Store::new(&self.engine, State::new(sandbox_wasi(sandbox)?));
        store.data_mut().limits = StoreLimitsBuilder::new().memory_size(MEMORY_LIMIT).build();
        store.limiter(|state| &mut state.limits);
        store.set_epoch_deadline(ticks(ACTIVATE_BUDGET));
        let bindings = bindings::Extension::instantiate(&mut store, &component, &self.linker)?;
        let manifest = bindings
            .winter_extension_guest()
            .call_activate(&mut store)
            .context("activating it")?;
        if manifest.id != id {
            bail!("its id is {}, but it was installed as {id}", manifest.id);
        }
        let id: &'static str = Box::leak(manifest.id.clone().into_boxed_str());
        let programs = manifest
            .servers
            .iter()
            .map(|server| (server.name.clone(), server.program.clone()))
            .collect();
        let writable = std::iter::once(&sandbox.data_dir)
            .chain(&sandbox.write)
            .cloned()
            .collect();
        let readable = std::iter::once(&sandbox.data_dir)
            .chain(&sandbox.write)
            .chain(&sandbox.read)
            .cloned()
            .collect();
        let grants = Grants {
            net: sandbox.net.clone(),
            programs,
            readable,
            run: sandbox.run.clone(),
            writable,
        };
        let plugin = Plugin::new(id, Instance::new(store, bindings), grants);
        Ok(Box::new(WasmExtension { manifest, plugin }))
    }
}

// ========================================================================
// WasmExtension
// ========================================================================

impl Extension for WasmExtension {
    fn id(&self) -> &'static str {
        self.plugin.id
    }

    fn activate(&self, registry: &mut Registry) {
        for command in &self.manifest.commands {
            let plugin = Arc::clone(&self.plugin);
            let name = command.name.clone();
            let run = move |host: &mut dyn Host| plugin.run_command(host, &name);
            match command.icon.clone() {
                Some(icon) => registry.command_with_icon(
                    &command.name,
                    &command.title,
                    convert::to_icon(icon),
                    run,
                ),
                None => registry.command(&command.name, &command.title, run),
            }
        }
        for name in &self.manifest.completions {
            let plugin = Arc::clone(&self.plugin);
            let local = name.clone();
            registry.completion(name, move |request| plugin.complete(&local, request));
        }
        for segment in &self.manifest.segments {
            match segment.icon.clone() {
                Some(icon) => {
                    registry.status_segment_with_icon(&segment.name, convert::to_icon(icon))
                }
                None => registry.status_segment(&segment.name),
            }
        }
        for inline in &self.manifest.inlines {
            let provider = WasmInline::new(Arc::clone(&self.plugin), inline.name.clone());
            registry.inline(&inline.name, &inline.title, provider);
        }
        for server in &self.manifest.servers {
            registry.server(convert::to_server_spec(server.clone()));
        }
        for setting in &self.manifest.settings {
            let default = convert::to_setting_value(setting.default.clone());
            registry.setting(&setting.name, default, &setting.doc);
        }
        for view in &self.manifest.views {
            let plugin = Arc::clone(&self.plugin);
            let name = view.name.clone();
            let keys = view
                .keys
                .iter()
                .cloned()
                .map(convert::to_view_key)
                .collect();
            let open = move || -> Box<dyn View> {
                Box::new(WasmView::open(Arc::clone(&plugin), name.clone()))
            };
            match view.icon.clone() {
                Some(icon) => registry.view_with_icon(
                    &view.name,
                    &view.title,
                    keys,
                    convert::to_icon(icon),
                    open,
                ),
                None => registry.view(&view.name, &view.title, keys, open),
            }
        }
        if self.manifest.listens {
            let plugin = Arc::clone(&self.plugin);
            registry.on_event(move |host, event| plugin.handle_event(host, event));
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The `package.kdl` the component `bytes` carries, read without running
/// any of it: there must be exactly one.
pub fn package_of(bytes: &[u8]) -> Result<String> {
    let mut found = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        if let Payload::CustomSection(section) = payload?
            && section.name() == PACKAGE_SECTION
        {
            found.push(section.data().to_vec());
        }
    }
    let [package] = <[Vec<u8>; 1]>::try_from(found).map_err(|found| {
        wasmtime::format_err!(
            "it carries {} packages where one belongs: build it with winter_ext_guest::export!",
            found.len()
        )
    })?;
    String::from_utf8(package).context("its package is not UTF-8 text")
}

/// The version of the extension interface an import named `name` is of, if
/// it is that interface: `0.2.0` of `winter:extension/host@0.2.0`.
fn built_against(name: &str) -> Option<&str> {
    name.strip_prefix(WIT_INTERFACE)?.strip_prefix('@')
}

/// Whether this Winter runs a component built against `version` of the
/// interface.
fn speaks(version: &str) -> bool {
    links(version, WIT_VERSION)
}

/// Whether a component built against `built` links where `spoken` is: the
/// same release line, and no later. A line is named by the major, or,
/// before 1.0, by the minor too, as semver and wasmtime's linker read it.
fn links(built: &str, spoken: &str) -> bool {
    match (semver_of(built), semver_of(spoken)) {
        (Some(built), Some(spoken)) => {
            let same_line = built.0 == spoken.0 && (spoken.0 > 0 || built.1 == spoken.1);
            same_line && built <= spoken
        }
        _ => false,
    }
}

/// The major, minor, and patch numbers `version` writes, as `1.2.3`.
fn semver_of(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.').map(str::parse);
    let semver = (
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    );
    parts.next().is_none().then_some(semver)
}

/// The WASI an extension sandboxed as `sandbox` says is given: its
/// directories, each at the path it has outside, so paths it is handed
/// mean the same inside; and somewhere to say what went wrong. A granted
/// directory not there is left out, with nothing in it to reach: a
/// bundled extension asks for one the user may never have made.
fn sandbox_wasi(sandbox: &Sandbox) -> Result<wasmtime_wasi::WasiCtx> {
    std::fs::create_dir_all(&sandbox.data_dir)
        .with_context(|| format!("making {}", sandbox.data_dir.display()))?;
    let mut wasi = WasiCtxBuilder::new();
    wasi.inherit_stderr()
        .env(DATA_DIR_VARIABLE, sandbox.data_dir.to_string_lossy());
    if let Some(home) = &sandbox.home {
        wasi.env(HOME_VARIABLE, home.to_string_lossy());
    }
    let granted = std::iter::once((&sandbox.data_dir, FsPerms::ReadWrite))
        .chain(sandbox.write.iter().map(|dir| (dir, FsPerms::ReadWrite)))
        .chain(sandbox.read.iter().map(|dir| (dir, FsPerms::ReadOnly)))
        .filter(|(dir, _)| dir.is_dir());
    for (dir, perms) in granted {
        wasi.preopened_dir(dir, dir.to_string_lossy(), perms)
            .with_context(|| format!("granting {}", dir.display()))?;
    }
    Ok(wasi.build())
}

/// How many clock ticks `budget` lasts.
fn ticks(budget: Duration) -> u64 {
    (budget.as_millis() / TICK.as_millis()) as u64
}

/// Move `engine`'s clock on every [`TICK`] for as long as it is in use, so
/// a call outrunning its budget is stopped.
fn start_clock(engine: &Engine) -> Result<()> {
    let engine = engine.weak();
    thread::Builder::new()
        .name(TICKER_NAME.to_string())
        .spawn(move || {
            while let Some(engine) = engine.upgrade() {
                engine.increment_epoch();
                drop(engine);
                thread::sleep(TICK);
            }
        })?;
    Ok(())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_version_refused_by_name_is_the_one_the_wit_declares() {
        // Bumped in the WIT alone, every current component was refused as old.
        let wit = include_str!("../../../sdk/winter-ext-api/wit/winter.wit");
        let declared = format!("package winter:extension@{WIT_VERSION};");
        assert!(wit.contains(&declared), "{declared} in winter.wit");
    }

    #[test]
    fn test_a_component_runs_only_on_a_winter_of_its_release_line_as_new_or_newer() {
        assert!(speaks(WIT_VERSION));
        assert!(links("0.1.0", "0.1.4"), "an earlier patch of 0.1");
        assert!(!links("0.1.1", "0.1.0"), "a later patch");
        assert!(!links("0.1.0", "0.2.0"), "another line, before 1.0");
        assert!(links("1.0.3", "1.2.0"), "an earlier minor of 1");
        assert!(!links("1.3.0", "1.2.0"), "a later minor");
        assert!(!links("1.0.0", "2.0.0"), "another major");
        assert!(!links("0.1", "0.1.0"), "not a version");
    }

    #[test]
    fn test_an_import_says_which_version_of_the_interface_it_was_built_for() {
        assert_eq!(built_against("winter:extension/host@0.2.0"), Some("0.2.0"));
        assert_eq!(built_against("wasi:cli/stderr@0.2.0"), None);
        assert_eq!(built_against("winter:extension/types@0.2.0"), None);
    }
}
