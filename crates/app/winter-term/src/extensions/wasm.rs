//! Extensions installed as files: each in a directory of its own in the
//! config directory's `extensions`, its component `<id>/<id>.wasm` and
//! nothing beside it, loaded as Winter starts and run in a sandbox of its
//! own. Those Winter ships, as Copilot, sit the same way in the `extensions`
//! directory installed with it (see [`BUNDLED_DIRS`]); one the user installs
//! takes the place of a bundled one with its id.
//!
//! Each component carries its package (see `package`). One the user
//! installed runs only as approved (see `manage`): the very component
//! approved, reaching what the user granted. One Winter ships is part of
//! Winter, and reaches what its package asks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use winter_ext_api::{Extension, Registry};
use winter_model::path::expand_home;
use winter_wasm::{FILE_EXTENSION, Runtime, Sandbox};

use super::approvals::Approvals;
use super::manage::{self, Places};
use super::package::{Grants, Package};

// ========================================================================
// Constants
// ========================================================================

/// Where the extensions shipped with Winter are, from the directory its
/// program is in: beside it (a development build, Windows), in the
/// `lib/winter` beside its `bin` (Linux), or in an app bundle's resources
/// (macOS).
const BUNDLED_DIRS: [&str; 3] = [
    "extensions",
    "../lib/winter/extensions",
    "../Resources/extensions",
];

/// The state directory's directory of their own directories, one each.
const DATA_DIR: &str = "extension-data";

/// The state directory's directory of what Winter compiled of them.
const CACHE_DIR: &str = "wasm-cache";

// ========================================================================
// Data Structures
// ========================================================================

/// Every extension, those installed as files joining those compiled in,
/// what went wrong loading them, and the commands of those that failed.
pub(super) struct Installed {
    pub(super) extensions: Vec<Box<dyn Extension>>,
    pub(super) problems: Vec<String>,
    /// The commands the packages of extensions that did not load name: a
    /// binding to one is known, its extension's failure said already.
    pub(super) unavailable: Vec<String>,
}

/// An extension whose commands take the titles its package gives them,
/// under the namespace it names.
struct TitledExtension {
    inner: Box<dyn Extension>,
    /// Kept as long as Winter runs, as the registry's ids are.
    namespace: &'static str,
    titles: HashMap<String, String>,
}

/// A component to load: its id, its bytes, read once, and what it reaches.
struct ToLoad {
    bytes: Vec<u8>,
    granted: Grants,
    id: String,
    package: Package,
}

// ========================================================================
// TitledExtension
// ========================================================================

impl Extension for TitledExtension {
    fn id(&self) -> &'static str {
        self.inner.id()
    }

    fn namespace(&self) -> &'static str {
        self.namespace
    }

    fn activate(&self, registry: &mut Registry) {
        self.inner.activate(registry);
        let ext_id = self.id();
        for (id, title) in &self.titles {
            registry.set_command_title(id, title);
        }
        for command in registry.commands() {
            if command.extension != ext_id {
                continue;
            }
            let id = &command.id;
            if !self.titles.contains_key(id) {
                eprintln!(
                    "winter: extension '{ext_id}' registered command '{id}', but its package \
                     gives it no title"
                );
            } else if command.title.trim().is_empty() {
                eprintln!("winter: extension '{ext_id}' command '{id}' has an empty title");
            }
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `compiled`, joined by every extension installed as a file but those
/// whose id another has, and what went wrong loading them.
pub(super) fn with_installed(compiled: Vec<Box<dyn Extension>>) -> Installed {
    let mut extensions = compiled;
    let mut problems = Vec::new();
    let mut unavailable = Vec::new();
    let (installed, mut found) = user_installed();
    problems.append(&mut found);
    let bundled: Vec<ToLoad> = bundled_dirs()
        .iter()
        .flat_map(|dir| installed_components(dir))
        .filter(|path| {
            !installed
                .iter()
                .any(|own| Some(own.id.as_str()) == id_of(path))
        })
        .filter_map(|path| match bundled(&path) {
            Ok(to_load) => Some(to_load),
            Err(why) => {
                problems.push(format!(
                    "the extension in {} did not load: {why}",
                    path.display()
                ));
                None
            }
        })
        .collect();
    let components: Vec<ToLoad> = installed.into_iter().chain(bundled).collect();
    if components.is_empty() {
        return Installed {
            extensions,
            problems,
            unavailable,
        };
    }
    let runtime = match Runtime::new(crate::config::state_file(CACHE_DIR).as_deref()) {
        Ok(runtime) => runtime,
        Err(error) => {
            problems.push(format!(
                "extensions installed as files cannot run: {error:#}"
            ));
            unavailable.extend(
                components
                    .iter()
                    .flat_map(|component| component.package.titles.keys().cloned()),
            );
            return Installed {
                extensions,
                problems,
                unavailable,
            };
        }
    };
    for component in components {
        let id = component.id.clone();
        let commands: Vec<String> = component.package.titles.keys().cloned().collect();
        match load(&runtime, component) {
            Ok(extension) if extensions.iter().any(|other| other.id() == extension.id()) => {
                problems.push(format!(
                    "the extension {id} was not loaded: one named {id} already is"
                ))
            }
            Ok(extension) => match namespace_owner(&extensions, extension.namespace()) {
                Some(owner) => {
                    let namespace = extension.namespace();
                    problems.push(format!(
                        "the extension {id} was not loaded: its namespace {namespace} is \
                         {owner}'s"
                    ));
                    unavailable.extend(commands);
                }
                None => extensions.push(extension),
            },
            Err(why) => {
                problems.push(format!("the extension {id} did not load: {why}"));
                unavailable.extend(commands);
            }
        }
    }
    Installed {
        extensions,
        problems,
        unavailable,
    }
}

/// The extensions the user installed that are approved to run, and why
/// each other did not; none for tests, which run with what is compiled in
/// alone, whatever the user has installed.
fn user_installed() -> (Vec<ToLoad>, Vec<String>) {
    let Some(places) = Places::user().filter(|_| !cfg!(test)) else {
        return (Vec::new(), Vec::new());
    };
    let approvals = match Approvals::load(&places.approvals) {
        Ok(approvals) => approvals,
        Err(why) => return (Vec::new(), vec![format!("no extension was loaded: {why}")]),
    };
    let mut problems = Vec::new();
    let to_load = installed_components(&places.extensions)
        .iter()
        .filter_map(|path| {
            let id = id_of(path)?;
            match manage::approved(&places, &approvals, id) {
                Ok(approved) => Some(ToLoad {
                    bytes: approved.bytes,
                    granted: approved.granted,
                    id: id.to_string(),
                    package: approved.package,
                }),
                Err(why) => {
                    problems.push(why);
                    None
                }
            }
        })
        .collect();
    (to_load, problems)
}

/// The extension Winter ships at `path`, reaching what its package asks.
fn bundled(path: &Path) -> Result<ToLoad, String> {
    let id = id_of(path).ok_or("it names no extension")?;
    let bytes = std::fs::read(path).map_err(|error| format!("it cannot be read: {error}"))?;
    let text = winter_wasm::package_of(&bytes).map_err(|error| format!("{error:#}"))?;
    let package = Package::parse(&text)?;
    Ok(ToLoad {
        bytes,
        granted: package.asks.clone(),
        id: id.to_string(),
        package,
    })
}

/// Where the extensions shipped with Winter are; nowhere for tests.
fn bundled_dirs() -> Vec<PathBuf> {
    let program = std::env::current_exe().ok();
    let Some(beside) = program
        .as_deref()
        .and_then(Path::parent)
        .filter(|_| !cfg!(test))
    else {
        return Vec::new();
    };
    BUNDLED_DIRS
        .iter()
        .map(|dir| beside.join(dir))
        .filter(|dir| dir.is_dir())
        .collect()
}

/// The components in `dir`, each `<id>/<id>.wasm`, in the order of their ids.
fn installed_components(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut components: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter_map(|extension_dir| {
            let mut file_name = extension_dir.file_name()?.to_owned();
            file_name.push(format!(".{FILE_EXTENSION}"));
            let component = extension_dir.join(file_name);
            component.is_file().then_some(component)
        })
        .collect();
    components.sort();
    components
}

/// The id the component at `path` is installed under.
fn id_of(path: &Path) -> Option<&str> {
    path.file_stem()?.to_str()
}

/// Run `component`, sandboxed to what it is granted.
fn load(runtime: &Runtime, component: ToLoad) -> Result<Box<dyn Extension>, String> {
    let data_dir = crate::config::state_file(DATA_DIR)
        .ok_or("there is no state directory to keep its files in")?
        .join(&component.id);
    let granted = component.granted;
    let sandbox = Sandbox {
        data_dir,
        home: winter_base::paths::home_dir(),
        net: granted.net,
        read: expand_all(&granted.read),
        run: granted.run,
        write: expand_all(&granted.write),
    };
    let extension = runtime
        .load(&component.id, &component.bytes, &sandbox)
        .map_err(|error| format!("{error:#}"))?;
    Ok(Box::new(TitledExtension {
        inner: extension,
        namespace: Box::leak(component.package.namespace.into_boxed_str()),
        titles: component.package.titles,
    }))
}

/// The extension among `loaded` whose namespace is `namespace`, by id.
pub(super) fn namespace_owner(
    loaded: &[Box<dyn Extension>],
    namespace: &str,
) -> Option<&'static str> {
    loaded
        .iter()
        .find(|other| other.namespace() == namespace)
        .map(|other| other.id())
}

/// The directories `listed` names, a leading `~` the home directory.
fn expand_all(listed: &[String]) -> Vec<PathBuf> {
    listed.iter().map(|dir| expand_home(dir)).collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_namespace_another_extension_has_is_found_with_its_owner() {
        let loaded: Vec<Box<dyn Extension>> = vec![Box::new(crate::extensions::tests::Probe)];
        assert_eq!(
            namespace_owner(&loaded, "probe"),
            Some("probe"),
            "its id by default"
        );
        assert_eq!(namespace_owner(&loaded, "notes"), None);
        assert_eq!(
            crate::extensions::compiled_namespace_owner("probe"),
            Some("probe")
        );
    }

    #[test]
    fn test_only_components_named_for_their_directory_are_installed() {
        let dir = std::env::temp_dir().join(format!("winter-extensions-{}", std::process::id()));
        for (extension, file) in [
            ("b", "b.wasm"),
            ("a", "a.wasm"),
            ("a", "package.kdl"),
            ("my.ext", "my.ext.wasm"),
            ("misnamed", "other.wasm"),
        ] {
            std::fs::create_dir_all(dir.join(extension)).expect("a directory");
            std::fs::write(dir.join(extension).join(file), "").expect("written");
        }
        std::fs::write(dir.join("loose.wasm"), "").expect("written");
        let found = installed_components(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            found,
            [
                dir.join("a").join("a.wasm"),
                dir.join("b").join("b.wasm"),
                dir.join("my.ext").join("my.ext.wasm"),
            ]
        );
    }
}
