//! Paths: the files and directories a path being typed could go on to,
//! read from disk, relative to the document's own directory.

use std::path::{Path, PathBuf};

use super::{Candidate, Context, Kind, Reply};

/// The most entries of a directory offered.
const MAX_ENTRIES: usize = 500;

/// The entries the path ending at the cursor could go on to, once it has a
/// `/` in it or starts as a path does (`~`, `.`).
pub(crate) fn complete(context: &Context) -> Option<Reply> {
    if crate::remote::parse(context.path).is_some() {
        return None;
    }
    let before = context.before();
    let token_start = before
        .iter()
        .rposition(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '(' | '[' | '{' | '<' | '=' | ',' | '`'))
        .map_or(0, |at| at + 1);
    let token: String = before[token_start..].iter().collect();
    if !token.contains('/') {
        return None;
    }
    let (dir_part, partial) = token.rsplit_once('/').unwrap_or(("", token.as_str()));
    let dir = resolve(context.path, &format!("{dir_part}/"))?;
    let show_hidden = partial.starts_with('.');
    let mut entries: Vec<(bool, String)> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (show_hidden || !name.starts_with('.')).then(|| (entry.path().is_dir(), name))
        })
        .take(MAX_ENTRIES)
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let candidates = entries
        .into_iter()
        .map(|(is_dir, name)| Candidate {
            detail: None,
            insert: match is_dir {
                true => format!("{name}/"),
                false => name.clone(),
            },
            kind: match is_dir {
                true => Kind::Directory,
                false => Kind::File,
            },
            label: name,
            source: "path",
        })
        .collect();
    Some(Reply {
        candidates,
        start: context.col - partial.chars().count(),
    })
}

/// The directory `typed` (ending in `/`) names, read from the document's
/// directory where it is relative.
fn resolve(document: &Path, typed: &str) -> Option<PathBuf> {
    let path = match typed.strip_prefix("~/") {
        Some(rest) => crate::paths::home_dir()?.join(rest),
        None if typed.starts_with('/') => PathBuf::from(typed),
        None => document.parent()?.join(typed),
    };
    path.is_dir().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::super::tests::context;
    use super::*;

    #[test]
    fn test_a_path_lists_its_directory_directories_first() {
        let dir = std::env::temp_dir().join(format!("winter-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("figs")).expect("a directory");
        std::fs::write(dir.join("fig1.pdf"), "").expect("a file");
        std::fs::write(dir.join(".hidden"), "").expect("a dotfile");
        let lines = vec![String::new()];
        let document = dir.join("paper.tex");
        let line = "\\includegraphics{./fi";
        let at = super::super::Context { path: &document, ..context(line, &lines, "latex") };
        let reply = complete(&at).expect("a path");
        assert_eq!(reply.start, line.chars().count() - 2);
        let inserts: Vec<&str> = reply.candidates.iter().map(|c| c.insert.as_str()).collect();
        assert_eq!(inserts, ["figs/", "fig1.pdf"], "no dotfile unless asked");
        assert!(complete(&super::super::Context { path: &document, ..context("no path", &lines, "latex") }).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
