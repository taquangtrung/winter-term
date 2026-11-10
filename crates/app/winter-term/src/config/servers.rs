//! The `server` entries of `settings.kdl`'s `lsp` block: read from the
//! document by hand, since the serde reader takes a lone `server "name"`
//! for a string and one with children for a list of them; merged over the
//! built-in servers by name, those named first; and written back as given.

use std::collections::BTreeMap;

use kdl::{KdlDocument, KdlNode, KdlValue};
use winter_lsp::servers::Server;

use super::read::builtin_settings;
use super::schema::{kdl_bool, kdl_string};
use super::{LspConfig, ServerSetting};

// ========================================================================
// Constants
// ========================================================================

/// The block the servers are listed in, and the node naming each.
const LSP_BLOCK: &str = "lsp";
const SERVER_NODE: &str = "server";

// ========================================================================
// Data Structures
// ========================================================================

/// A field of a server's entry a page sets as a list of names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerField {
    /// The file extensions written in its first language.
    Extensions,
    /// The languages it serves.
    Languages,
    /// The files marking a project's root.
    Roots,
}

// ========================================================================
// LspConfig
// ========================================================================

impl LspConfig {
    /// Put the server `name` before the others of its languages, as a
    /// `server "name"` entry of its own does. Says whether that changed
    /// which server any of them starts with.
    pub fn prefer_server(&mut self, name: &str) -> bool {
        let Some(at) = self.servers.iter().position(|server| server.name == name) else {
            return false;
        };
        let leads = |language: &String| {
            self.servers
                .iter()
                .find(|server| server.serves(language))
                .is_some_and(|first| first.name == name)
        };
        if self.servers[at].languages.iter().all(leads) {
            return false;
        }
        let server = self.servers.remove(at);
        self.servers.insert(0, server);
        let setting = match self.written.iter().position(|setting| setting.name == name) {
            Some(at) => self.written.remove(at),
            None => ServerSetting {
                name: name.to_string(),
                ..ServerSetting::default()
            },
        };
        self.written.insert(0, setting);
        true
    }

    /// Run server `name` as `command`, or with `None` as the built-in one
    /// does; a server of the user's own given no command is taken away.
    pub fn set_server_command(&mut self, name: &str, command: Option<Vec<String>>) {
        let builtin = builtin_settings().lsp.servers;
        let ours = !builtin.iter().any(|server| server.name == name);
        let written = self.written.iter().any(|setting| setting.name == name);
        match (command, ours) {
            (None, true) => self.written.retain(|setting| setting.name != name),
            // Clearing what was never set writes nothing, which would put
            // the server first for its languages.
            (None, false) if !written => {}
            (command, _) => self.written_mut(name).command = command,
        }
        self.servers = merge_servers(&builtin, &self.written);
    }

    /// Set a list field of server `name`'s entry, or with `None` leave it
    /// as the built-in one has it.
    pub fn set_server_field(
        &mut self,
        name: &str,
        field: ServerField,
        values: Option<Vec<String>>,
    ) {
        let setting = self.written_mut(name);
        match field {
            ServerField::Extensions => setting.extensions = values,
            ServerField::Languages => setting.languages = values,
            ServerField::Roots => setting.roots = values,
        }
        self.servers = merge_servers(&builtin_settings().lsp.servers, &self.written);
    }

    /// Set the commands that install server `name`, or with none leave them
    /// as the built-in one has them.
    pub fn set_server_install(&mut self, name: &str, commands: Vec<Vec<String>>) {
        let install = (!commands.is_empty()).then_some(commands);
        self.written_mut(name).install = install;
        self.servers = merge_servers(&builtin_settings().lsp.servers, &self.written);
    }

    /// Add a server of the user's own, `name`, serving `languages`.
    pub fn add_server(&mut self, name: &str, languages: Vec<String>) {
        self.set_server_field(name, ServerField::Languages, Some(languages));
    }

    /// Whether `name` is a server of the user's own rather than a built-in.
    pub fn is_own_server(&self, name: &str) -> bool {
        !builtin_settings()
            .lsp
            .servers
            .iter()
            .any(|server| server.name == name)
    }

    /// The `server` entry for `name`, made at the end where there is none.
    fn written_mut(&mut self, name: &str) -> &mut ServerSetting {
        let at = match self.written.iter().position(|setting| setting.name == name) {
            Some(at) => at,
            None => {
                self.written.push(ServerSetting {
                    name: name.to_string(),
                    ..ServerSetting::default()
                });
                self.written.len() - 1
            }
        };
        &mut self.written[at]
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The `server` entries of `text`'s `lsp` block, and what to say of those
/// that cannot be read, which are left out. Text that is not KDL gives
/// none, for the rest of the reading to say what is wrong with it.
pub(super) fn read_servers(text: &str) -> (Vec<ServerSetting>, Vec<String>) {
    let Ok(document) = KdlDocument::parse(text) else {
        return (Vec::new(), Vec::new());
    };
    let mut settings = Vec::new();
    let mut slips = Vec::new();
    let nodes = document
        .nodes()
        .iter()
        .filter(|node| node.name().value() == LSP_BLOCK)
        .flat_map(|block| block.children().map(KdlDocument::nodes).unwrap_or_default())
        .filter(|node| node.name().value() == SERVER_NODE);
    for node in nodes {
        match read_server(node) {
            Ok(setting) => settings.push(setting),
            Err(slip) => slips.push(slip),
        }
    }
    (settings, slips)
}

/// Every server in `base`, with `written` taken over those it names and
/// the new ones it gives added, those it names first in its order.
pub(super) fn merge_servers(base: &[Server], written: &[ServerSetting]) -> Vec<Server> {
    let mut servers: Vec<Server> = Vec::new();
    for setting in written {
        let at = match servers
            .iter()
            .position(|server| server.name == setting.name)
        {
            Some(at) => at,
            None => {
                let server = base
                    .iter()
                    .find(|server| server.name == setting.name)
                    .cloned()
                    .unwrap_or_else(|| Server {
                        command: vec![setting.name.clone()],
                        name: setting.name.clone(),
                        ..Server::default()
                    });
                servers.push(server);
                servers.len() - 1
            }
        };
        overlay(&mut servers[at], setting);
    }
    let named = |server: &&Server| written.iter().any(|setting| setting.name == server.name);
    servers.extend(base.iter().filter(|server| !named(server)).cloned());
    servers
}

/// Which language each file extension the servers give is written in:
/// the first language of the server giving it, the first such server's.
pub(super) fn extension_languages(servers: &[Server]) -> BTreeMap<String, String> {
    let mut languages = BTreeMap::new();
    for server in servers {
        let Some(language) = server.languages.first() else {
            continue;
        };
        for extension in &server.extensions {
            languages
                .entry(extension.to_ascii_lowercase())
                .or_insert_with(|| language.clone());
        }
    }
    languages
}

/// `written` as the lines of an `lsp` block, which read back as the same.
pub(super) fn write_servers(written: &[ServerSetting]) -> String {
    let mut out = String::new();
    for setting in written {
        let mut fields = String::new();
        let mut field = |name: &str, values: &[String]| {
            let values: Vec<String> = values.iter().map(|value| kdl_string(value)).collect();
            fields.push_str(&format!("        {name} {}\n", values.join(" ")));
        };
        if let Some(command) = &setting.command {
            field("command", command);
        }
        if let Some(languages) = &setting.languages {
            field("languages", languages);
        }
        if let Some(extensions) = &setting.extensions {
            field("extensions", extensions);
        }
        if let Some(roots) = &setting.roots {
            field("roots", roots);
        }
        for command in setting.install.iter().flatten() {
            field("install", command);
        }
        if let Some(outermost) = setting.outermost {
            fields.push_str(&format!("        outermost {}\n", kdl_bool(outermost)));
        }
        let name = kdl_string(&setting.name);
        match fields.is_empty() {
            true => out.push_str(&format!("    server {name}\n")),
            false => out.push_str(&format!("    server {name} {{\n{fields}    }}\n")),
        }
    }
    out
}

/// One `server` node read, or what to say of why it cannot be.
fn read_server(node: &KdlNode) -> Result<ServerSetting, String> {
    let Some(name) = only_strings(node).and_then(|names| names.into_iter().next()) else {
        return Err("lsp's server takes its name: server \"name\"".to_string());
    };
    let mut setting = ServerSetting {
        name: name.clone(),
        ..ServerSetting::default()
    };
    for child in node.children().map(KdlDocument::nodes).unwrap_or_default() {
        let field = child.name().value();
        if field == "outermost" {
            match child.entries().first().map(|entry| entry.value()) {
                Some(KdlValue::Bool(outermost)) => setting.outermost = Some(*outermost),
                _ => return Err(format!("server {name}'s outermost takes #true or #false")),
            }
            continue;
        }
        let values = only_strings(child)
            .filter(|values| !values.is_empty())
            .ok_or_else(|| format!("server {name}'s {field} takes text in quotes"))?;
        match field {
            "command" => setting.command = Some(values),
            "extensions" => setting.extensions = Some(values),
            "install" => setting.install.get_or_insert_with(Vec::new).push(values),
            "languages" => setting.languages = Some(values),
            "roots" => setting.roots = Some(values),
            _ => return Err(format!("server {name} has no setting `{field}`")),
        }
    }
    Ok(setting)
}

/// `setting` taken over `server`, field by field.
fn overlay(server: &mut Server, setting: &ServerSetting) {
    if let Some(command) = &setting.command {
        server.command = command.clone();
    }
    if let Some(extensions) = &setting.extensions {
        server.extensions = extensions.clone();
    }
    if let Some(install) = &setting.install {
        server.install = install.clone();
    }
    if let Some(languages) = &setting.languages {
        server.languages = languages.clone();
    }
    if let Some(outermost) = setting.outermost {
        server.outermost = outermost;
    }
    if let Some(roots) = &setting.roots {
        server.roots = roots.clone();
    }
}

/// A node's arguments, where every entry is text in quotes and none is a
/// property.
fn only_strings(node: &KdlNode) -> Option<Vec<String>> {
    node.entries()
        .iter()
        .map(|entry| match (entry.name(), entry.value()) {
            (None, KdlValue::String(text)) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The built-in rust-analyzer and pyright, as a base to merge over.
    fn base() -> Vec<Server> {
        let server = |name: &str, language: &str| Server {
            command: vec![name.to_string()],
            install: vec![vec!["installer".to_string(), name.to_string()]],
            languages: vec![language.to_string()],
            name: name.to_string(),
            roots: vec!["root-file".to_string()],
            ..Server::default()
        };
        vec![
            server("pyright", "python"),
            server("rust-analyzer", "rust"),
            server("basedpyright", "python"),
        ]
    }

    #[test]
    fn test_a_named_server_comes_first_with_only_what_it_sets_changed() {
        let text = "lsp {\n    server \"basedpyright\"\n    server \"rust-analyzer\" {\n        command \"/opt/ra\"\n    }\n}\n";
        let (written, slips) = read_servers(text);
        assert!(slips.is_empty(), "{slips:?}");
        let servers = merge_servers(&base(), &written);
        let names: Vec<&str> = servers.iter().map(|server| server.name.as_str()).collect();
        assert_eq!(names, ["basedpyright", "rust-analyzer", "pyright"]);
        assert_eq!(servers[1].command, ["/opt/ra"]);
        assert_eq!(
            servers[1].roots,
            ["root-file"],
            "kept from the built-in one"
        );
        assert_eq!(servers[1].install, base()[1].install);
    }

    #[test]
    fn test_a_new_server_runs_its_name_and_its_extensions_map_to_its_first_language() {
        let text = "lsp {\n    server \"elixir-ls\" {\n        languages \"elixir\" \"eex\"\n        extensions \"ex\" \"EXS\"\n        install \"mix\" \"archive.install\"\n        install \"brew\" \"install\" \"elixir-ls\"\n    }\n}\n";
        let (written, _) = read_servers(text);
        let servers = merge_servers(&base(), &written);
        assert_eq!(servers[0].command, ["elixir-ls"]);
        assert_eq!(servers[0].install.len(), 2);
        let languages = extension_languages(&servers);
        assert_eq!(languages["ex"], "elixir");
        assert_eq!(languages["exs"], "elixir");
    }

    #[test]
    fn test_a_server_that_cannot_be_read_is_left_out_and_said() {
        let text = "lsp {\n    server\n    server \"a\" {\n        colour \"red\"\n    }\n    server \"b\" {\n        outermost \"yes\"\n    }\n    server \"c\" {\n        command 3\n    }\n    server \"d\"\n}\n";
        let (written, slips) = read_servers(text);
        assert_eq!(written.len(), 1);
        assert_eq!(slips.len(), 4, "{slips:?}");
        assert!(slips[1].contains("no setting `colour`"), "{slips:?}");
    }

    #[test]
    fn test_preferring_a_server_puts_it_first_once_and_keeps_what_was_written() {
        let text = "lsp {\n    server \"rust-analyzer\" {\n        command \"/opt/ra\"\n    }\n    server \"basedpyright\" {\n        command \"bp\"\n    }\n}\n";
        let (written, _) = read_servers(text);
        let mut lsp = LspConfig {
            servers: merge_servers(&base(), &written),
            written,
            ..LspConfig::default()
        };
        assert!(!lsp.prefer_server("rust-analyzer"), "already Rust's first");
        assert!(lsp.prefer_server("pyright"));
        assert!(!lsp.prefer_server("pyright"), "once");
        let names: Vec<&str> = lsp
            .written
            .iter()
            .map(|setting| setting.name.as_str())
            .collect();
        assert_eq!(names, ["pyright", "rust-analyzer", "basedpyright"]);
        assert!(lsp.prefer_server("basedpyright"));
        assert_eq!(
            lsp.written[0].command.as_deref(),
            Some(&["bp".to_string()][..])
        );
        assert_eq!(lsp.servers[0].name, "basedpyright");
    }

    #[test]
    fn test_a_command_is_set_cleared_and_a_server_of_the_user_s_own_added_and_taken_away() {
        let mut lsp = crate::config::Config::default().lsp;
        lsp.set_server_command("gopls", None);
        assert!(
            lsp.written.is_empty(),
            "clearing what was never set writes nothing"
        );
        lsp.set_server_command("gopls", Some(vec!["/opt/gopls".to_string()]));
        let gopls = lsp.servers.iter().find(|server| server.name == "gopls");
        assert_eq!(
            gopls.map(|server| server.command.clone()),
            Some(vec!["/opt/gopls".to_string()])
        );
        lsp.add_server("elixir-ls", vec!["elixir".to_string()]);
        assert!(lsp.is_own_server("elixir-ls"));
        assert!(lsp
            .servers
            .iter()
            .any(|server| server.name == "elixir-ls" && server.serves("elixir")));
        lsp.set_server_command("elixir-ls", None);
        assert!(
            !lsp.servers.iter().any(|server| server.name == "elixir-ls"),
            "taken away"
        );
    }

    #[test]
    fn test_written_servers_read_back_as_the_same() {
        let text = "lsp {\n    server \"basedpyright\"\n    server \"x\" {\n        command \"x-ls\" \"--stdio\"\n        languages \"x\"\n        extensions \"x\"\n        roots \"x.toml\"\n        install \"cargo\" \"install\" \"x-ls\"\n        install \"brew\" \"install\" \"x-ls\"\n        outermost #true\n    }\n}\n";
        let (written, _) = read_servers(text);
        let again = format!("lsp {{\n{}}}\n", write_servers(&written));
        assert_eq!(read_servers(&again).0, written);
    }
}
