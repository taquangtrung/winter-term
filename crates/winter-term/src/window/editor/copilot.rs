//! GitHub Copilot in the window: the `copilot-language-server` started the
//! first time a file is open in the editor (while `copilot { enable }` is
//! on), run through the same hub as the language servers but reading every
//! file, and the sign-in it keeps on this machine checked once it is up.
//! `Copilot: Sign In` runs GitHub's device flow (the code on the clipboard,
//! the page opened in the browser), and `Copilot: Install Language Server`
//! fetches the server where none is found.
//!
//! The suggestions themselves are the text editor's (see
//! `editors/text/copilot.rs`): the window only starts the server, signs in,
//! and opens the pages the server asks for.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::editors::text::TextEditor;
use winter_lsp::servers::Plan;
use winter_lsp::{Event, Incoming, Port, ServerId};
use crate::model::editor::Notice;
use crate::window::App;
use crate::window::EditorHost;

// ========================================================================
// Constants
// ========================================================================

/// What the server's program is called.
const SERVER_BINARY: &str = "copilot-language-server";

/// The argument that has it speak over its standard input and output.
const SERVER_ARGS: &[&str] = &["--stdio"];

/// The version `Copilot: Install Language Server` fetches, the one
/// magic-vscode runs.
const SERVER_VERSION: &str = "1.520.0";

/// Where magic-vscode (in each build of VS Code) keeps the server it
/// fetched, under the home directory: one directory per version.
const MAGIC_VSCODE_SERVERS: &[&str] = &[
    ".config/Code/User/globalStorage/mstudio.magic-vscode/server",
    ".config/Code - Insiders/User/globalStorage/mstudio.magic-vscode/server",
    ".config/VSCodium/User/globalStorage/mstudio.magic-vscode/server",
];

/// Who the editor is, as `initialize` tells the server.
const EDITOR_NAME: &str = "winter-term";
const PLUGIN_NAME: &str = "winter-copilot";

// ========================================================================
// Data Structures
// ========================================================================

/// Copilot as the window runs it.
#[derive(Debug, Default)]
pub(in crate::window) struct CopilotState {
    /// What each request the window itself sent is for, by its id.
    asked: HashMap<u64, Asked>,
    /// The running server, once started.
    port: Option<Port>,
    /// Whether the server says it is signed in, once it has said.
    signed_in: Option<bool>,
    /// Whether starting was tried this session, so a server that is not
    /// there is not looked for on every frame.
    tried: bool,
    /// Who it is signed in as.
    user: Option<String>,
}

/// A request the window sent the server.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Asked {
    /// Whether it is signed in; `quiet` says nothing where it is.
    Status { quiet: bool },
    /// The device flow's code and the command that finishes it.
    SignIn,
    /// The command that waits for the sign-in on GitHub to finish.
    FinishSignIn,
    SignOut,
}

// ========================================================================
// CopilotState
// ========================================================================

impl CopilotState {
    /// The running server's id, if it is running.
    pub(in crate::window) fn server(&self) -> Option<ServerId> {
        self.port.as_ref().map(Port::id)
    }
}

// ========================================================================
// App: Copilot
// ========================================================================

impl App {
    /// Start Copilot once a file is open in the editor, while it is enabled,
    /// trying only once a session.
    pub(in crate::window) fn start_copilot_if_wanted(&mut self) {
        if !self.config.copilot.enable || self.host.lsp.copilot.tried {
            return;
        }
        let editing = self.open_tabs.editors().any(|(_, slot)| {
            slot.editor.with_kind(|text: &TextEditor| text.lsp_subject().is_some()).unwrap_or(false)
        });
        if editing {
            self.start_copilot(true);
        }
    }

    /// Start the server, saying why it cannot be where it is not found
    /// (always where `asked`, else once).
    fn start_copilot(&mut self, quiet: bool) {
        self.host.lsp.copilot.tried = true;
        let Some(server) = find_server(&self.config.copilot.server_path) else {
            let said = "Copilot: no copilot-language-server found: run Copilot: Install Language Server, or name one in settings.kdl's copilot { server-path }";
            match quiet {
                true => self.notify(Notice::info(said).unlogged()),
                false => self.set_error(said),
            }
            return;
        };
        let version = env!("CARGO_PKG_VERSION");
        let plan = Plan {
            command: std::iter::once(server.display().to_string())
                .chain(SERVER_ARGS.iter().map(|arg| arg.to_string()))
                .collect(),
            everywhere: true,
            init_options: Some(json!({
                "editorInfo": {"name": EDITOR_NAME, "version": version},
                "editorPluginInfo": {"name": PLUGIN_NAME, "version": version},
            })),
            languages: Vec::new(),
            root: home(),
        };
        match self.host.lsp.hub.start(&plan) {
            Ok(port) => self.host.lsp.copilot.port = Some(port),
            Err(why) => self.set_error(format!("Copilot: {why}")),
        }
    }

    /// Run GitHub's device flow: start the server where it is not running,
    /// and ask it for the code.
    pub(in crate::window) fn copilot_sign_in(&mut self) {
        if self.host.lsp.copilot.port.is_none() {
            self.start_copilot(false);
        }
        if !self.host.copilot_ask("signIn", json!({}), Asked::SignIn) {
            self.set_notice("Copilot: still starting; sign in again in a moment");
        }
    }

    /// Sign out of Copilot.
    pub(in crate::window) fn copilot_sign_out(&mut self) {
        if !self.host.copilot_ask("signOut", json!({}), Asked::SignOut) {
            self.set_notice("Copilot: not running");
        }
    }

    /// Say whether Copilot is running and who it is signed in as.
    pub(in crate::window) fn copilot_status(&mut self) {
        let copilot = &self.host.lsp.copilot;
        let said = match (&copilot.port, copilot.signed_in, &copilot.user) {
            (None, _, _) if !self.config.copilot.enable => "Copilot: off (copilot { enable #false })".to_string(),
            (None, _, _) => "Copilot: not running".to_string(),
            (Some(_), Some(true), Some(user)) => format!("Copilot: signed in as {user}"),
            (Some(_), Some(true), None) => "Copilot: signed in".to_string(),
            (Some(_), Some(false), _) => "Copilot: not signed in: run Copilot: Sign In".to_string(),
            (Some(_), None, _) => "Copilot: starting".to_string(),
        };
        self.set_notice(said);
    }

    /// Fetch the server into Winter's own directory, in a Compile tab: the
    /// package magic-vscode runs, from npm.
    pub(in crate::window) fn copilot_install(&mut self) {
        let Some(platform) = npm_platform() else {
            self.set_error("Copilot: no copilot-language-server package for this machine");
            return;
        };
        let dir = install_dir();
        let package = format!("copilot-language-server-{platform}");
        let url = format!("https://registry.npmjs.org/@github/{package}/-/{package}-{SERVER_VERSION}.tgz");
        let command = format!(
            "mkdir -p '{dir}' && curl -fSL '{url}' | tar -xz -C '{dir}' --strip-components=1 package/{SERVER_BINARY} && echo 'Installed: Copilot: Sign In starts it'",
            dir = dir.display()
        );
        self.host.lsp.copilot.tried = false;
        self.run_shell_task(&command);
    }


    /// Take what Copilot's server said that is the window's: the answers to
    /// its own requests, the server being ready, a page to open, and its
    /// state. Says whether it was.
    pub(in crate::window) fn copilot_heard(&mut self, incoming: &Incoming) -> bool {
        if Some(incoming.server) != self.host.lsp.copilot.server() {
            return false;
        }
        match &incoming.event {
            Event::Ready => {
                self.host.copilot_ask("checkStatus", json!({"localChecksOnly": false}), Asked::Status { quiet: true });
                true
            }
            Event::OpenUrl(url) => {
                if let Err(e) = ::open::that_detached(url) {
                    self.set_error(format!("Copilot: could not open {url}: {e}"));
                }
                true
            }
            Event::Status { error, message } => {
                if *error {
                    self.host.lsp.copilot.signed_in = Some(false);
                    if let Some(message) = message {
                        self.notify(Notice::warning(format!("Copilot: {message}")).unlogged());
                    }
                }
                true
            }
            Event::Answer { id, result } => {
                let Some(asked) = self.host.lsp.copilot.asked.remove(id) else {
                    return false;
                };
                match result {
                    Ok(value) => self.copilot_answered(asked, value),
                    Err(error) => self.set_error(format!("Copilot: {error}")),
                }
                true
            }
            Event::Exited(why) => {
                self.host.lsp.copilot.port = None;
                self.host.lsp.copilot.signed_in = None;
                self.set_error(format!("Copilot stopped: {why}"));
                true
            }
            _ => false,
        }
    }

    /// Act on the answer to the window's request `asked`.
    fn copilot_answered(&mut self, asked: Asked, value: &Value) {
        match asked {
            Asked::Status { quiet } => {
                let status = value.get("status").and_then(Value::as_str).unwrap_or_default();
                let signed_in = matches!(status, "OK" | "AlreadySignedIn" | "MaybeOk");
                self.host.lsp.copilot.signed_in = Some(signed_in);
                self.host.lsp.copilot.user = value.get("user").and_then(Value::as_str).map(str::to_string);
                match (signed_in, quiet) {
                    (true, true) => {}
                    (true, false) => self.copilot_status(),
                    (false, _) => self.notify(Notice::warning("Copilot: not signed in: run Copilot: Sign In")),
                }
            }
            Asked::SignIn => {
                let code = value.get("userCode").and_then(Value::as_str);
                let command = value.get("command").cloned();
                match (code, command) {
                    (Some(code), Some(command)) => {
                        let copied = self.input.clipboard().is_some_and(|clipboard| clipboard.set_text(code).is_ok());
                        let where_ = if copied { " (copied)" } else { "" };
                        self.notify(Notice::info(format!(
                            "Copilot: enter the code {code}{where_} on the GitHub page opening now"
                        )));
                        let params = json!({
                            "command": command.get("command").cloned().unwrap_or(Value::Null),
                            "arguments": command.get("arguments").cloned().unwrap_or(json!([])),
                        });
                        self.host.copilot_ask("workspace/executeCommand", params, Asked::FinishSignIn);
                    }
                    // Signed in already: there is no code to enter.
                    _ => {
                        self.host.copilot_ask("checkStatus", json!({"localChecksOnly": false}), Asked::Status { quiet: false });
                    }
                }
            }
            Asked::FinishSignIn => {
                self.host.copilot_ask("checkStatus", json!({"localChecksOnly": false}), Asked::Status { quiet: false });
            }
            Asked::SignOut => {
                self.host.lsp.copilot.signed_in = Some(false);
                self.host.lsp.copilot.user = None;
                self.set_notice("Copilot: signed out");
            }
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The server to run: the one `setting` names, else the first found on the
/// search path, in Winter's own directory, or in magic-vscode's, the newest
/// version there first.
fn find_server(setting: &str) -> Option<PathBuf> {
    if !setting.is_empty() {
        let named = PathBuf::from(setting);
        return named.is_file().then_some(named);
    }
    let on_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).map(|dir| dir.join(SERVER_BINARY)).find(|path| path.is_file())
    });
    let installed = Some(install_dir().join(SERVER_BINARY)).filter(|path| path.is_file());
    on_path.or(installed).or_else(|| {
        let home = home();
        MAGIC_VSCODE_SERVERS.iter().find_map(|dir| newest_version(&home.join(dir)))
    })
}

/// The server in the newest version directory under `dir`.
fn newest_version(dir: &Path) -> Option<PathBuf> {
    let mut versions: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|entry| entry.path()).collect();
    versions.sort_by_key(|path| version_key(path));
    versions.into_iter().rev().map(|version| version.join(SERVER_BINARY)).find(|path| path.is_file())
}

/// A version directory's name as numbers, for sorting `1.10.0` after
/// `1.9.0`.
fn version_key(path: &Path) -> Vec<u64> {
    path.file_name()
        .map(|name| name.to_string_lossy().split('.').filter_map(|part| part.parse().ok()).collect())
        .unwrap_or_default()
}

/// Where `Copilot: Install Language Server` puts the server.
fn install_dir() -> PathBuf {
    crate::config::config_dir().join("copilot").join(SERVER_VERSION)
}

/// The home directory, the root the server is started in.
fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// What npm calls this machine's platform package: `linux-x64`.
fn npm_platform() -> Option<String> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        "windows" => "win32",
        _ => return None,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => return None,
    };
    Some(format!("{os}-{arch}"))
}

// ========================================================================
// EditorHost
// ========================================================================

impl EditorHost {
    /// Send the server `method`, for the window. Says whether it went.
    fn copilot_ask(&mut self, method: &str, params: Value, asked: Asked) -> bool {
        let Some(port) = self.lsp.copilot.port.clone() else {
            return false;
        };
        match port.request(method, params) {
            Some(id) => {
                self.lsp.copilot.asked.insert(id, asked);
                true
            }
            None => false,
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_newest_version_s_server_is_found_by_number_not_by_letter() {
        let dir = std::env::temp_dir().join(format!("winter-copilot-{}", std::process::id()));
        for version in ["1.9.0", "1.10.0", "1.2.0"] {
            std::fs::create_dir_all(dir.join(version)).expect("dir");
        }
        for version in ["1.9.0", "1.2.0"] {
            std::fs::write(dir.join(version).join(SERVER_BINARY), "").expect("server");
        }
        assert_eq!(newest_version(&dir), Some(dir.join("1.9.0").join(SERVER_BINARY)), "1.10.0 has none");
        std::fs::write(dir.join("1.10.0").join(SERVER_BINARY), "").expect("server");
        assert_eq!(newest_version(&dir), Some(dir.join("1.10.0").join(SERVER_BINARY)));
        assert_eq!(find_server(&dir.join("1.2.0").join(SERVER_BINARY).display().to_string()), Some(dir.join("1.2.0").join(SERVER_BINARY)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Start the real server, hand a text editor it, type, and wait for the
    /// ghost text. Ignored: it needs copilot-language-server, a Copilot
    /// sign-in, and the network; run it with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn test_copilot_offers_ghost_text_to_a_text_editor_that_types() {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        use crate::editors::text::TextEditor;
        use winter_lsp::Hub;
        use crate::model::editor::Editor;
        use crate::model::input::{Key, KeyCode};

        let server = find_server("").expect("copilot-language-server found");
        let dir = std::env::temp_dir().join(format!("winter-copilot-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("fib.py");
        std::fs::write(&file, "def fibonacci(n):\n    \"\"\"Return the nth Fibonacci number.\"\"\"\n").expect("source");
        let (to, from) = mpsc::channel();
        let mut hub = Hub::new(to, from);
        let plan = Plan {
            command: vec![server.display().to_string(), SERVER_ARGS[0].to_string()],
            everywhere: true,
            init_options: Some(json!({
                "editorInfo": {"name": EDITOR_NAME, "version": "0"},
                "editorPluginInfo": {"name": PLUGIN_NAME, "version": "0"},
            })),
            languages: Vec::new(),
            root: home(),
        };
        hub.start(&plan).expect("started");
        let mut editor = TextEditor::new(file.clone(), None).expect("opens");
        editor.set_copilot_config(&crate::config::CopilotConfig::default());
        let deadline = Instant::now() + Duration::from_secs(60);
        while hub.generation() == 0 && Instant::now() < deadline {
            hub.drain();
            std::thread::sleep(Duration::from_millis(50));
        }
        editor.connect_lsp(hub.generation(), &hub.ports());
        let press = |editor: &mut TextEditor, code: KeyCode| {
            editor.on_key(&Key {
                alt: false,
                code,
                ctrl: false,
                shift: false,
                sup: false,
            });
        };
        // `j` to the docstring, `o` to open the body's first line.
        for c in ['j', 'o', 'i', 'f'] {
            press(&mut editor, KeyCode::Char(c));
        }
        let mut shown = None;
        while shown.is_none() && Instant::now() < deadline {
            if let Some(due) = editor.next_tick() {
                if due <= Instant::now() {
                    editor.on_tick(Instant::now());
                }
            }
            for incoming in hub.drain() {
                editor.on_lsp(&incoming);
            }
            editor.connect_lsp(hub.generation(), &hub.ports());
            let painted = editor.content(6, 80, false);
            let row = crate::model::paint::row_text(&painted.rows[2]);
            if editor.status().segments.iter().any(|segment| segment.starts_with("Copilot")) {
                shown = Some(row);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let row = shown.expect("ghost text shown");
        assert!(row.trim_start().len() > "    if".len(), "the row runs past what was typed: {row}");
        press(&mut editor, KeyCode::Tab);
        assert!(editor.status().segments.iter().all(|segment| !segment.starts_with("Copilot")), "taken");
        hub.stop_all();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
