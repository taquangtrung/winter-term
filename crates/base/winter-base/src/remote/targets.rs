//! The targets there are to go to: the containers Docker has running, and
//! the hosts named in the SSH configuration. Only names a URI can carry
//! safely are offered (see [`super::uri::is_name`]).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::paths::home_dir;
use crate::process;

use super::uri::is_name;

// ========================================================================
// Constants
// ========================================================================

/// What `docker ps` is asked to print: one container name a line.
const DOCKER_PS: [&str; 3] = ["ps", "--format", "{{.Names}}"];

/// The SSH configuration, under the home directory.
const SSH_CONFIG: &str = ".ssh/config";

/// Where a relative `Include` is read from, under the home directory.
const SSH_DIR: &str = ".ssh";

/// How deep `Include`s are followed, so one that includes itself ends.
const MAX_INCLUDE_DEPTH: usize = 16;

/// What makes a `Host` entry a pattern rather than a host to go to.
const HOST_PATTERN: [char; 3] = ['*', '?', '!'];

// ========================================================================
// Functions
// ========================================================================

/// The names of the containers Docker has running, sorted. Empty where
/// Docker is not installed or will not answer.
pub(super) fn containers() -> Vec<String> {
    let Ok(output) = process::command("docker")
        .args(DOCKER_PS)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    let mut names: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|name| is_name(name))
        .map(str::to_string)
        .collect();
    names.sort();
    names
}

/// The hosts the SSH configuration names, sorted, patterns left out.
pub(super) fn ssh_hosts() -> Vec<String> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    let mut hosts = Vec::new();
    read_config(&home.join(SSH_CONFIG), &home, 0, &mut hosts);
    hosts.retain(|host| is_name(host));
    hosts.sort();
    hosts.dedup();
    hosts
}

/// Add the hosts `path` names to `hosts`, following its `Include`s.
fn read_config(path: &Path, home: &Path, depth: usize, hosts: &mut Vec<String>) {
    if depth > MAX_INCLUDE_DEPTH {
        return;
    }
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    for line in text.lines() {
        let line = line.trim();
        let Some((keyword, values)) = line.split_once(|c: char| c.is_whitespace() || c == '=')
        else {
            continue;
        };
        let values = values.trim_start_matches(|c: char| c.is_whitespace() || c == '=');
        match keyword.to_ascii_lowercase().as_str() {
            "host" => hosts.extend(
                values
                    .split_whitespace()
                    .filter(|host| !host.contains(HOST_PATTERN))
                    .map(str::to_string),
            ),
            "include" => {
                for pattern in values.split_whitespace() {
                    for included in included_files(pattern, home) {
                        read_config(&included, home, depth + 1, hosts);
                    }
                }
            }
            _ => {}
        }
    }
}

/// The files an `Include` names: `~` is home, a relative path is under
/// `~/.ssh`, and `*` and `?` in the last component match within its
/// directory.
fn included_files(pattern: &str, home: &Path) -> Vec<PathBuf> {
    let path = match pattern.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if Path::new(pattern).is_absolute() => PathBuf::from(pattern),
        None => home.join(SSH_DIR).join(pattern),
    };
    let Some(name) = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
    else {
        return Vec::new();
    };
    if !name.contains(['*', '?']) {
        return vec![path];
    }
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut matched: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| wildcard_matches(&name, &entry.file_name().to_string_lossy()))
        .map(|entry| entry.path())
        .collect();
    matched.sort();
    matched
}

/// Whether `text` matches `pattern`, where `*` is any run of characters
/// and `?` any one.
fn wildcard_matches(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    // Where the last `*` was, and how much of the text it had taken.
    let (mut p, mut t, mut star) = (0, 0, None);
    while t < text.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p, t));
                p += 1;
            }
            Some(&c) if c == '?' || c == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match star {
                Some((star_p, star_t)) => {
                    p = star_p + 1;
                    t = star_t + 1;
                    star = Some((star_p, star_t + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wildcards_match_runs_and_single_characters() {
        assert!(wildcard_matches("*.conf", "work.conf"));
        assert!(wildcard_matches("host-?", "host-1"));
        assert!(wildcard_matches("a*b*c", "aXXbYYc"));
        assert!(!wildcard_matches("*.conf", "work.cfg"));
        assert!(!wildcard_matches("host-?", "host-12"));
    }

    #[test]
    fn test_hosts_are_read_through_includes_without_their_patterns() {
        let home = std::env::temp_dir().join(format!("winter-remote-ssh-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(home.join(".ssh/config.d")).expect("dirs");
        fs::write(
            home.join(SSH_CONFIG),
            "Host alpha *.internal !beta\n  User me\nInclude config.d/*.conf\nHost=gamma\n",
        )
        .expect("config");
        fs::write(home.join(".ssh/config.d/work.conf"), "Host delta\n").expect("include");
        fs::write(home.join(".ssh/config.d/skip.txt"), "Host never\n").expect("unmatched");

        let mut hosts = Vec::new();
        read_config(&home.join(SSH_CONFIG), &home, 0, &mut hosts);
        hosts.sort();
        assert_eq!(hosts, ["alpha", "delta", "gamma"]);
        let _ = fs::remove_dir_all(&home);
    }
}
