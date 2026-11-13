//! The servers extensions run (see [`ServerSpec`]): started through the
//! same hub as the language servers, each reading every local file open,
//! and what each says passed on to its extension as `winter_ext_api::Event::Server`.
//!
//! An extension names its servers, never a process: it asks for one by
//! name, its requests carry tags it chooses, and the answers come back under
//! them. Where a server's program is looked for, and where it is fetched to,
//! is the window's to say.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use winter_ext_api::{
    Install, Platform, ServerAnswer, ServerEvent, ServerSaid, ServerSpec, ServerStatus,
    ID_SEPARATOR,
};
use winter_lsp::servers::Plan;
use winter_lsp::{Event, Incoming, Port, ServerId};

use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// The state directory's directory the extensions' servers are fetched to,
/// one for each extension, one in that for each server. It is none of an
/// extension's own, which a sandboxed one may write to: a program it could
/// put there is never one Winter runs.
const SERVERS_DIR: &str = "extension-servers";

/// What an install's address must start with.
const HTTPS: &str = "https://";

/// What the command fetching an install quotes its words in, which no word
/// of an extension's may hold: it would end the quoting, and run as a
/// command of its own.
const QUOTE: char = '\'';

// ========================================================================
// Data Structures
// ========================================================================

/// The extensions' servers running, and what they have said that their
/// extensions are yet to hear.
#[derive(Default)]
pub(in crate::window) struct ExtensionServers {
    /// The tag each request was sent with, by its server and id.
    asked: HashMap<(ServerId, u64), u64>,
    /// What the servers said, by the extension each is for.
    heard: Vec<(&'static str, winter_ext_api::Event)>,
    /// Each running server, by its extension and its name.
    running: HashMap<(&'static str, String), Port>,
}

// ========================================================================
// App: extensions' servers
// ========================================================================

impl App {
    /// Start `extension`'s server `name`: `program` where given, else its
    /// own as found. How it went is heard by the extension.
    pub(in crate::window) fn start_extension_server(
        &mut self,
        extension: &'static str,
        name: &str,
        program: Option<PathBuf>,
    ) {
        let servers = &mut self.host.lsp.extension_servers;
        let key = (extension, name.to_string());
        if servers.running.contains_key(&key) {
            return;
        }
        let Some(spec) = server_spec(extension, name) else {
            return servers.hear(extension, name, ServerSaid::Exited(undeclared(name)));
        };
        let Some(found) = find_program(extension, spec, program.as_deref()) else {
            let why = match program {
                Some(named) => format!("{} is not a file", named.display()),
                None => format!("no {} found on the PATH or installed", spec.program),
            };
            return servers.hear(extension, name, ServerSaid::Exited(why));
        };
        let init_options = spec
            .init_options
            .as_deref()
            .and_then(|options| serde_json::from_str(options).ok());
        let plan = Plan {
            command: std::iter::once(found.display().to_string())
                .chain(spec.args.iter().cloned())
                .collect(),
            everywhere: true,
            init_options,
            languages: Vec::new(),
            root: winter_base::paths::home_dir().unwrap_or_else(|| PathBuf::from("/")),
        };
        match self.host.lsp.hub.start(&plan) {
            Ok(port) => {
                self.host.lsp.extension_servers.running.insert(key, port);
            }
            Err(why) => {
                self.host
                    .lsp
                    .extension_servers
                    .hear(extension, name, ServerSaid::Exited(why))
            }
        }
    }

    /// Fetch `extension`'s server `name` for this machine, in a Compile tab
    /// so its progress shows.
    pub(in crate::window) fn install_extension_server(
        &mut self,
        extension: &'static str,
        name: &str,
    ) {
        let Some(spec) = server_spec(extension, name) else {
            return self.set_error(format!("{extension}: {}", undeclared(name)));
        };
        let install = Platform::this_machine().and_then(|platform| {
            spec.install
                .iter()
                .find(|install| install.platform == platform)
        });
        let (Some(install), Some(dir)) = (install, install_dir(extension, name)) else {
            return self.set_error(format!(
                "{extension}: {} cannot be fetched for this machine",
                spec.program
            ));
        };
        if let Err(why) = check_install(install, &dir) {
            return self.set_error(format!("{extension}: {} not fetched: {why}", spec.program));
        }
        self.run_shell_task(&install_command(install, &dir));
    }

    /// Send `extension`'s server `name` the request `method`, its answer to
    /// be heard under `tag`.
    pub(in crate::window) fn extension_server_request(
        &mut self,
        extension: &'static str,
        name: &str,
        tag: u64,
        (method, params): (&str, &str),
    ) {
        let servers = &mut self.host.lsp.extension_servers;
        let refused = |why: String| ServerSaid::Answer(ServerAnswer::new(tag, Err(why)));
        let Some(port) = servers.running.get(&(extension, name.to_string())).cloned() else {
            return servers.hear(extension, name, refused(format!("{name} is not running")));
        };
        let Ok(params) = serde_json::from_str::<Value>(params) else {
            return servers.hear(
                extension,
                name,
                refused(format!("{method}: params are not JSON")),
            );
        };
        match port.request(method, params) {
            Some(id) => {
                servers.asked.insert((port.id(), id), tag);
            }
            None => servers.hear(extension, name, refused(format!("{name} is not ready"))),
        }
    }

    /// Send `extension`'s server `name` the notification `method`.
    pub(in crate::window) fn extension_server_notify(
        &mut self,
        extension: &'static str,
        name: &str,
        (method, params): (&str, &str),
    ) {
        let servers = &self.host.lsp.extension_servers;
        let port = servers.running.get(&(extension, name.to_string()));
        if let (Some(port), Ok(params)) = (port, serde_json::from_str::<Value>(params)) {
            port.notify(method, params);
        }
    }

    /// Take what an extension's server said, to be heard by its extension.
    /// Says whether it was one of theirs and theirs alone: what it says of
    /// a document goes on to the editors.
    pub(in crate::window) fn extension_server_heard(&mut self, incoming: &Incoming) -> bool {
        let servers = &mut self.host.lsp.extension_servers;
        let Some(((extension, name), _)) = servers
            .running
            .iter()
            .find(|(_, port)| port.id() == incoming.server)
        else {
            return false;
        };
        let (extension, name) = (*extension, name.clone());
        let said = match &incoming.event {
            Event::Answer { id, result } => {
                let Some(tag) = servers.asked.remove(&(incoming.server, *id)) else {
                    return false;
                };
                let result = result
                    .as_ref()
                    .map(|value| value.to_string())
                    .map_err(String::clone);
                ServerSaid::Answer(ServerAnswer::new(tag, result))
            }
            Event::Exited(why) => {
                servers.running.remove(&(extension, name.clone()));
                ServerSaid::Exited(why.clone())
            }
            Event::Message(message) => ServerSaid::Message(message.clone()),
            Event::OpenUrl(url) => ServerSaid::OpenUrl(url.clone()),
            Event::Ready => ServerSaid::Ready,
            Event::Status { error, message } => {
                ServerSaid::Status(ServerStatus::new(*error, message.clone()))
            }
            Event::ApplyEdit(_) | Event::Diagnostics(_) | Event::Progress(_) => return false,
        };
        servers.hear(extension, &name, said);
        true
    }

    /// Have each extension hear what its servers said since last asked.
    pub(in crate::window) fn tell_extension_servers(&mut self) {
        let heard = std::mem::take(&mut self.host.lsp.extension_servers.heard);
        for (extension, event) in heard {
            self.tell_extension(extension, &event);
        }
    }
}

// ========================================================================
// ExtensionServers
// ========================================================================

impl ExtensionServers {
    /// The ids of `extension`'s servers running.
    pub(in crate::window) fn running_ids(&self, extension: &str) -> Vec<ServerId> {
        self.running
            .iter()
            .filter(|((owner, _), _)| *owner == extension)
            .map(|(_, port)| port.id())
            .collect()
    }

    /// Keep `said`, by `extension`'s server `name`, for it to hear.
    fn hear(&mut self, extension: &'static str, name: &str, said: ServerSaid) {
        let event = winter_ext_api::Event::Server(ServerEvent::new(name, said));
        self.heard.push((extension, event));
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What `extension` declared of its server `name`.
fn server_spec(extension: &str, name: &str) -> Option<&'static ServerSpec> {
    let id = format!("{extension}{ID_SEPARATOR}{name}");
    crate::extensions::registry()
        .find_server(&id)
        .map(|server| &server.spec)
}

/// What is said of a server an extension never declared.
fn undeclared(name: &str) -> String {
    format!("no server named {name} is declared")
}

/// The program to run as `extension`'s server `spec`: `named` where given
/// and a file, else its own on the `PATH`, else where it was fetched to.
fn find_program(extension: &str, spec: &ServerSpec, named: Option<&Path>) -> Option<PathBuf> {
    if let Some(named) = named {
        return named.is_file().then(|| named.to_path_buf());
    }
    // A program is named without the `.exe` Windows gives it.
    let file_name = format!("{}{}", spec.program, std::env::consts::EXE_SUFFIX);
    let on_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(&file_name))
            .find(|path| path.is_file())
    });
    let fetched = install_dir(extension, &spec.name)
        .map(|dir| dir.join(&file_name))
        .filter(|path| path.is_file());
    on_path.or(fetched)
}

/// Where `extension`'s server `name` is fetched to.
fn install_dir(extension: &str, name: &str) -> Option<PathBuf> {
    Some(
        crate::config::state_file(SERVERS_DIR)?
            .join(extension)
            .join(name),
    )
}

/// Why `install` may not be fetched into `dir`, if it may not: an address
/// that is not `https`, a program outside the archive's own directories,
/// or a word that would end the command's quoting.
fn check_install(install: &Install, dir: &Path) -> Result<(), String> {
    let dir = dir.to_string_lossy();
    let words = [install.url.as_str(), install.member.as_str(), &dir];
    if words
        .iter()
        .any(|word| word.contains([QUOTE, '\n', '\r', '\0']))
    {
        return Err("its address or path holds a quote or a line break".to_string());
    }
    if !install.url.starts_with(HTTPS) {
        return Err(format!("{} is not an https address", install.url));
    }
    let member = Path::new(&install.member);
    let escapes = member.is_absolute()
        || member
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir));
    match escapes {
        true => Err(format!("{} lies outside the archive", install.member)),
        false => Ok(()),
    }
}

/// The command fetching `install` into `dir`: the archive downloaded and
/// the program alone taken out of it, at the top of `dir`.
fn install_command(install: &Install, dir: &Path) -> String {
    let depth = install.member.matches('/').count();
    format!(
        "mkdir -p '{dir}' && curl -fSL '{url}' | tar -xz -C '{dir}' --strip-components={depth} '{member}' && echo 'Installed in {dir}'",
        dir = dir.display(),
        url = install.url,
        member = install.member,
    )
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    use crate::extensions::tests::{heard, PILOT};

    /// What the probe heard its servers say since last asked.
    fn heard_from_servers() -> Vec<ServerSaid> {
        heard()
            .into_iter()
            .filter_map(|event| match event {
                winter_ext_api::Event::Server(event) => Some(event.said),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn test_a_server_that_cannot_start_or_is_not_running_says_so_to_its_extension() {
        let mut app = App::new();
        heard();
        app.start_extension_server("probe", PILOT, None);
        app.extension_server_request("probe", PILOT, 7, ("ask", "{}"));
        app.start_extension_server("probe", "undeclared", None);
        app.tell_extension_servers();
        assert_eq!(
            heard_from_servers(),
            [
                ServerSaid::Exited(
                    "no winter-no-such-program found on the PATH or installed".to_string()
                ),
                ServerSaid::Answer(ServerAnswer::new(
                    7,
                    Err("pilot is not running".to_string())
                )),
                ServerSaid::Exited(undeclared("undeclared")),
            ]
        );
    }

    #[test]
    fn test_a_request_s_answer_reaches_its_extension_under_its_tag() {
        let mut app = App::new();
        heard();
        let (port, sent) = Port::fake("", Path::new("/"), json!({}));
        let servers = &mut app.host.lsp.extension_servers;
        servers
            .running
            .insert(("probe", PILOT.to_string()), port.clone());
        app.extension_server_request("probe", PILOT, 42, ("checkStatus", r#"{"a":1}"#));
        let asked = sent.try_recv().expect("sent to the server");
        assert_eq!(asked["method"], "checkStatus");
        assert_eq!(asked["params"], json!({"a": 1}));
        let answer = Incoming {
            event: Event::Answer {
                id: asked["id"].as_u64().expect("an id"),
                result: Ok(json!({"status": "OK"})),
            },
            server: port.id(),
        };
        assert!(
            app.extension_server_heard(&answer),
            "the extension's, not an editor's"
        );
        let diagnostics = Incoming {
            event: Event::Diagnostics(PathBuf::from("/a.rs")),
            server: port.id(),
        };
        assert!(
            !app.extension_server_heard(&diagnostics),
            "a document's goes on"
        );
        app.tell_extension_servers();
        let result = Ok(r#"{"status":"OK"}"#.to_string());
        assert_eq!(
            heard_from_servers(),
            [ServerSaid::Answer(ServerAnswer::new(42, result))]
        );
    }

    #[test]
    fn test_a_named_program_is_run_only_where_it_is_a_file() {
        let dir = std::env::temp_dir().join(format!("winter-ext-server-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let named = dir.join("pilot-server");
        std::fs::write(&named, "").expect("program");
        let spec = ServerSpec::new("pilot", "winter-no-such-program");
        assert_eq!(
            find_program("probe", &spec, Some(&named)),
            Some(named.clone())
        );
        assert_eq!(
            find_program("probe", &spec, Some(&dir.join("missing"))),
            None,
            "a name that is no file runs nothing, not another found"
        );
        assert_eq!(find_program("probe", &spec, None), None, "found nowhere");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_an_install_that_could_run_anything_or_write_anywhere_is_refused() {
        let dir = Path::new("/data/pilot");
        let install = |url: &str, member: &str| Install::new(Platform::LinuxX64, url, member);
        let good = install("https://example.org/p.tgz", "package/pilot");
        assert_eq!(check_install(&good, dir), Ok(()));
        let quoted = install(
            "https://example.org/p.tgz'; rm -rf ~; echo '",
            "package/pilot",
        );
        assert!(
            check_install(&quoted, dir).is_err(),
            "a quote ends the quoting"
        );
        assert!(check_install(&install("http://example.org/p.tgz", "p"), dir).is_err());
        assert!(check_install(&install("https://e.org/p.tgz", "../../bin/sh"), dir).is_err());
        assert!(check_install(&install("https://e.org/p.tgz", "/etc/passwd"), dir).is_err());
    }

    #[test]
    fn test_an_install_keeps_the_program_alone_at_the_top_of_its_directory() {
        let install = Install::new(
            Platform::LinuxX64,
            "https://example.org/pilot.tgz",
            "package/bin/pilot-server",
        );
        let command = install_command(&install, Path::new("/data/pilot"));
        assert!(command.contains("--strip-components=2 'package/bin/pilot-server'"));
        assert!(command.contains("curl -fSL 'https://example.org/pilot.tgz'"));
    }
}
