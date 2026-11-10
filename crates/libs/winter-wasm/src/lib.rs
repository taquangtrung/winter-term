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
use wasmtime::{bail, Cache, CacheConfig, Config, Engine, Store, StoreLimitsBuilder};
use wasmtime_wasi::{FsPerms, WasiCtxBuilder};
use winter_ext_api::{Extension, Registry, API_VERSION, PACKAGE_SECTION};

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
/// the directories the user granted it, and the programs it may have
/// Winter run as its servers.
#[derive(Clone, Debug, Default)]
pub struct Sandbox {
    /// Where it keeps what it keeps; made if missing.
    pub data_dir: PathBuf,
    /// Directories it may read.
    pub read: Vec<PathBuf>,
    /// The file names of the programs it may run as its servers.
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
        let mut store = Store::new(&self.engine, State::new(sandbox_wasi(sandbox)?));
        store.data_mut().limits = StoreLimitsBuilder::new().memory_size(MEMORY_LIMIT).build();
        store.limiter(|state| &mut state.limits);
        store.set_epoch_deadline(ticks(ACTIVATE_BUDGET));
        let bindings = bindings::Extension::instantiate(&mut store, &component, &self.linker)?;
        let manifest = bindings
            .winter_extension_guest()
            .call_activate(&mut store)
            .context("activating it")?;
        if manifest.api_version != API_VERSION {
            bail!(
                "it is written against version {} of the extension API, not {API_VERSION}",
                manifest.api_version
            );
        }
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
        let grants = Grants {
            programs,
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
            registry.command(&command.name, &command.title, move |host| {
                plugin.run_command(host, &name)
            });
        }
        for name in &self.manifest.completions {
            let plugin = Arc::clone(&self.plugin);
            let local = name.clone();
            registry.completion(name, move |request| plugin.complete(&local, request));
        }
        for name in &self.manifest.segments {
            registry.status_segment(name);
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
            registry.view(&view.name, &view.title, keys, move || {
                Box::new(WasmView::open(Arc::clone(&plugin), name.clone()))
            });
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
        if let Payload::CustomSection(section) = payload? {
            if section.name() == PACKAGE_SECTION {
                found.push(section.data().to_vec());
            }
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

/// The WASI an extension sandboxed as `sandbox` says is given: its
/// directories, each at the path it has outside, so paths it is handed
/// mean the same inside; and somewhere to say what went wrong.
fn sandbox_wasi(sandbox: &Sandbox) -> Result<wasmtime_wasi::WasiCtx> {
    std::fs::create_dir_all(&sandbox.data_dir)
        .with_context(|| format!("making {}", sandbox.data_dir.display()))?;
    let mut wasi = WasiCtxBuilder::new();
    wasi.inherit_stderr()
        .env(DATA_DIR_VARIABLE, sandbox.data_dir.to_string_lossy());
    let granted = std::iter::once((&sandbox.data_dir, FsPerms::ReadWrite))
        .chain(sandbox.write.iter().map(|dir| (dir, FsPerms::ReadWrite)))
        .chain(sandbox.read.iter().map(|dir| (dir, FsPerms::ReadOnly)));
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
