//! Which language server serves a language, and which project a file
//! belongs to: the commands Winter knows to try for each language, the
//! first of them installed, and the files that mark a project's root.

use std::path::{Path, PathBuf};

// ========================================================================
// Constants
// ========================================================================

/// The servers Winter knows, each with the languages it serves (as
/// settings name them), the commands to try in order, the files marking a
/// project's root, and whether the outermost of those is the root (a Cargo
/// workspace over its members) rather than the nearest.
const KNOWN: &[Known] = &[
    Known {
        commands: &[&["rust-analyzer"]],
        languages: &["rust"],
        markers: &["Cargo.toml"],
        outermost: true,
    },
    Known {
        commands: &[
            &["pyright-langserver", "--stdio"],
            &["basedpyright-langserver", "--stdio"],
            &["pylsp"],
            &["ruff", "server"],
        ],
        languages: &["python"],
        markers: &["pyproject.toml", "setup.py", "setup.cfg", "requirements.txt"],
        outermost: false,
    },
    Known {
        commands: &[&["gopls"]],
        languages: &["go"],
        markers: &["go.work", "go.mod"],
        outermost: true,
    },
    Known {
        commands: &[&["clangd"]],
        languages: &["c", "cpp"],
        markers: &["compile_commands.json", ".clangd", "CMakeLists.txt", "Makefile"],
        outermost: false,
    },
    Known {
        commands: &[&["typescript-language-server", "--stdio"]],
        languages: &["javascript", "typescript"],
        markers: &["tsconfig.json", "jsconfig.json", "package.json"],
        outermost: false,
    },
    Known {
        commands: &[&["texlab"]],
        languages: &["latex"],
        markers: &[".latexmkrc", "latexmkrc", "Tectonic.toml"],
        outermost: false,
    },
    Known {
        commands: &[&["marksman", "server"]],
        languages: &["markdown"],
        markers: &[".marksman.toml"],
        outermost: false,
    },
    Known {
        commands: &[&["bash-language-server", "start"]],
        languages: &["shell"],
        markers: &[],
        outermost: false,
    },
    Known {
        commands: &[&["ocamllsp"]],
        languages: &["ocaml"],
        markers: &["dune-project", "dune-workspace"],
        outermost: true,
    },
    Known {
        commands: &[&["taplo", "lsp", "stdio"]],
        languages: &["toml"],
        markers: &[],
        outermost: false,
    },
    Known {
        commands: &[&["yaml-language-server", "--stdio"]],
        languages: &["yaml"],
        markers: &[],
        outermost: false,
    },
    Known {
        commands: &[&["vscode-json-language-server", "--stdio"]],
        languages: &["json"],
        markers: &["package.json"],
        outermost: false,
    },
    Known {
        commands: &[&["lua-language-server"]],
        languages: &["lua"],
        markers: &[".luarc.json"],
        outermost: false,
    },
    Known {
        commands: &[&["haskell-language-server-wrapper", "--lsp"]],
        languages: &["haskell"],
        markers: &["stack.yaml", "cabal.project", "package.yaml"],
        outermost: false,
    },
    Known {
        commands: &[&["zls"]],
        languages: &["zig"],
        markers: &["build.zig"],
        outermost: false,
    },
    Known {
        commands: &[&["nil"], &["nixd"]],
        languages: &["nix"],
        markers: &["flake.nix"],
        outermost: false,
    },
];

/// What marks a repository, the root of a project no marker of its own
/// language names, and the edge an outermost marker is not looked past.
const REPOSITORY: &str = ".git";

// ========================================================================
// Data Structures
// ========================================================================

/// A language server Winter knows how to start.
struct Known {
    commands: &'static [&'static [&'static str]],
    languages: &'static [&'static str],
    markers: &'static [&'static str],
    outermost: bool,
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
// Functions
// ========================================================================

/// The server to start for `path`, written in `language`: the command the
/// setting gives, else the first known one installed. An error says why
/// there is none.
pub fn plan(language: &str, path: &Path, setting: Option<&[String]>) -> Result<Plan, String> {
    let known = KNOWN.iter().find(|known| known.languages.contains(&language));
    let command = match setting {
        Some(command) if !command.is_empty() => command.to_vec(),
        _ => {
            let known = known.ok_or_else(|| {
                format!("no language server is known for {language}: name one in settings.kdl's lsp servers")
            })?;
            let found = known.commands.iter().find(|command| installed(command[0]));
            let found = found.ok_or_else(|| {
                let names: Vec<&str> = known.commands.iter().map(|command| command[0]).collect();
                format!("no language server for {language} is installed (tried {})", names.join(", "))
            })?;
            found.iter().map(|part| part.to_string()).collect()
        }
    };
    let languages = known.map_or_else(
        || vec![language.to_string()],
        |known| known.languages.iter().map(|language| language.to_string()).collect(),
    );
    let (markers, outermost) = known.map_or((&[][..], false), |known| (known.markers, known.outermost));
    Ok(Plan {
        command,
        everywhere: false,
        init_options: None,
        languages,
        root: root_of(path, markers, outermost),
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

/// Whether `program` is on the search path.
fn installed(program: &str) -> bool {
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
        assert_eq!(root_of(&file, &["Cargo.toml"], false), repo.join("crates/one"));
        assert_eq!(root_of(&file, &["Cargo.toml"], true), repo, "not past the repository");
        assert_eq!(root_of(&file, &["go.mod"], false), repo, "the repository without a marker");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_a_setting_s_command_wins_and_an_unknown_language_needs_one() {
        let setting = vec!["my-server".to_string(), "--stdio".to_string()];
        let planned = plan("rust", Path::new("/nowhere/a.rs"), Some(&setting)).expect("a plan");
        assert_eq!(planned.command, setting);
        assert!(planned.languages.contains(&"rust".to_string()));
        assert!(plan("klingon", Path::new("/nowhere/a.k"), None).is_err());
        let typed = plan("klingon", Path::new("/nowhere/a.k"), Some(&setting)).expect("named");
        assert_eq!(typed.languages, ["klingon"]);
    }
}
