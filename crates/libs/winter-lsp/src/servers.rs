//! Which language server serves a language, and which project a file
//! belongs to: the servers settings list, the first of a language's
//! installed, the command that installs one, and the files that mark a
//! project's root.

use std::path::{Path, PathBuf};

// ========================================================================
// Constants
// ========================================================================

/// What marks a repository, the root of a project no marker of its own
/// language names, and the edge an outermost marker is not looked past.
const REPOSITORY: &str = ".git";

/// The palette command that installs a server, as an error names it.
pub const INSTALL_COMMAND_TITLE: &str = "LSP: Install a Language Server…";

// ========================================================================
// Data Structures
// ========================================================================

/// A language server as settings list it: how to start it, what it serves,
/// how its project is found, and how it is installed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Server {
    /// The program and its arguments.
    pub command: Vec<String>,
    /// The file extensions written in its first language, for a language
    /// the editor does not know by its files.
    pub extensions: Vec<String>,
    /// The commands that install it, each tried where its program is
    /// installed, the first such one used.
    pub install: Vec<Vec<String>>,
    /// The languages it serves, as settings name them.
    pub languages: Vec<String>,
    /// What settings call it.
    pub name: String,
    /// Whether the outermost directory holding one of [`Self::roots`] is
    /// the project (a Cargo workspace over its members), not the nearest.
    pub outermost: bool,
    /// The files marking a project's root.
    pub roots: Vec<String>,
}

/// The server to start for a file: the command, the project it serves,
/// and every language it serves there.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    /// The program and its arguments.
    pub command: Vec<String>,
    /// Whether the server reads every file on this machine, whatever its
    /// language or project, as Copilot does, rather than a project's.
    pub everywhere: bool,
    /// What `initialize` is told of the client beyond its capabilities
    /// (`initializationOptions`): who the editor is, for Copilot.
    pub init_options: Option<serde_json::Value>,
    /// Every language it serves there, as settings name them.
    pub languages: Vec<String>,
    /// The project it serves.
    pub root: PathBuf,
}

// ========================================================================
// Server Implementations
// ========================================================================

impl Server {
    /// Whether its program is installed: on the search path, or a file
    /// where the command names one by its path.
    pub fn is_installed(&self) -> bool {
        self.command
            .first()
            .is_some_and(|program| installed(program))
    }

    /// The command that installs it, the first whose program is installed.
    pub fn install_command(&self) -> Option<&[String]> {
        self.install
            .iter()
            .find(|command| command.first().is_some_and(|program| installed(program)))
            .map(Vec::as_slice)
    }

    /// Whether it serves `language`.
    pub fn serves(&self, language: &str) -> bool {
        self.languages.iter().any(|served| served == language)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The server to start for `path`, written in `language`: the first of
/// `servers` serving it that is installed. An error says why there is
/// none.
pub fn plan(language: &str, path: &Path, servers: &[Server]) -> Result<Plan, String> {
    let serving: Vec<&Server> = servers
        .iter()
        .filter(|server| server.serves(language))
        .collect();
    if serving.is_empty() {
        return Err(format!(
            "no language server is known for {language}: add one to settings.kdl's lsp block"
        ));
    }
    let server = serving.iter().find(|server| server.is_installed()).ok_or_else(|| {
        let names: Vec<&str> = serving.iter().map(|server| server.name.as_str()).collect();
        format!(
            "no language server for {language} is installed (tried {}); {INSTALL_COMMAND_TITLE} installs one",
            names.join(", ")
        )
    })?;
    let roots: Vec<&str> = server.roots.iter().map(String::as_str).collect();
    Ok(Plan {
        command: server.command.clone(),
        everywhere: false,
        init_options: None,
        languages: server.languages.clone(),
        root: root_of(path, &roots, server.outermost),
    })
}

/// The id the wire calls `language` by, where it differs from the name
/// settings give it.
pub fn language_id(language: &str) -> &str {
    match language {
        "shell" => "shellscript",
        other => other,
    }
}

/// The project `path` belongs to: the nearest directory over it holding
/// one of `markers`, or the outermost one short of the repository's root
/// when `outermost` is set; else the repository; else the file's own
/// directory.
pub fn root_of(path: &Path, markers: &[&str], outermost: bool) -> PathBuf {
    let start = path.parent().unwrap_or(path);
    let mut found: Option<&Path> = None;
    let mut repository: Option<&Path> = None;
    for dir in start.ancestors() {
        if markers.iter().any(|marker| dir.join(marker).exists()) {
            found = Some(dir);
            if !outermost {
                break;
            }
        }
        if dir.join(REPOSITORY).exists() {
            repository = Some(dir);
            break;
        }
    }
    found.or(repository).unwrap_or(start).to_path_buf()
}

/// Whether `program` is on the search path, or is a file where it is
/// named by its path.
fn installed(program: &str) -> bool {
    if Path::new(program).components().count() > 1 {
        return Path::new(program).is_file();
    }
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| dir.join(program).is_file())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_a_root_is_the_nearest_marker_or_the_outermost_inside_the_repository() {
        let dir = std::env::temp_dir().join(format!("winter-lsp-root-{}", std::process::id()));
        let repo = dir.join("repo");
        let member = repo.join("crates/one/src");
        fs::create_dir_all(&member).expect("dirs");
        fs::create_dir_all(repo.join(".git")).expect("git");
        fs::write(dir.join("Cargo.toml"), "").expect("outside");
        fs::write(repo.join("Cargo.toml"), "").expect("workspace");
        fs::write(repo.join("crates/one/Cargo.toml"), "").expect("member");
        let file = member.join("lib.rs");
        assert_eq!(
            root_of(&file, &["Cargo.toml"], false),
            repo.join("crates/one")
        );
        assert_eq!(
            root_of(&file, &["Cargo.toml"], true),
            repo,
            "not past the repository"
        );
        assert_eq!(
            root_of(&file, &["go.mod"], false),
            repo,
            "the repository without a marker"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A server for `languages` that runs `program`.
    fn server(name: &str, program: &str, languages: &[&str]) -> Server {
        Server {
            command: vec![program.to_string()],
            languages: languages
                .iter()
                .map(|language| language.to_string())
                .collect(),
            name: name.to_string(),
            ..Server::default()
        }
    }

    #[test]
    fn test_the_first_installed_server_of_a_language_is_planned() {
        let servers = [
            server("missing", "winter-no-such-server", &["rust"]),
            server("other", "sh", &["python"]),
            server("present", "sh", &["rust", "ron"]),
        ];
        let planned = plan("rust", Path::new("/nowhere/a.rs"), &servers).expect("a plan");
        assert_eq!(planned.command, ["sh"]);
        assert_eq!(planned.languages, ["rust", "ron"]);
    }

    #[test]
    fn test_no_server_installed_names_those_tried_and_an_unknown_language_says_so() {
        let servers = [server("missing", "winter-no-such-server", &["rust"])];
        let why = plan("rust", Path::new("/nowhere/a.rs"), &servers).expect_err("none installed");
        assert!(why.contains("tried missing"), "{why}");
        assert!(why.contains(INSTALL_COMMAND_TITLE), "{why}");
        let why = plan("klingon", Path::new("/nowhere/a.k"), &servers).expect_err("unknown");
        assert!(
            why.contains("no language server is known for klingon"),
            "{why}"
        );
    }

    #[test]
    fn test_the_install_command_is_the_first_whose_program_is_installed() {
        let mut ra = server("rust-analyzer", "winter-no-such-server", &["rust"]);
        ra.install = vec![
            vec!["winter-no-such-installer".to_string(), "x".to_string()],
            vec!["sh".to_string(), "-c".to_string(), "true".to_string()],
        ];
        assert_eq!(
            ra.install_command().map(|command| command[0].as_str()),
            Some("sh")
        );
        assert!(!ra.is_installed());
        ra.install.truncate(1);
        assert_eq!(ra.install_command(), None);
    }
}
