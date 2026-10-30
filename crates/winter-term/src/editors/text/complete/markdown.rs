//! Markdown: where a link being written could point, inside its `](`: the
//! files beside the document and below it, or after `#` the document's own
//! headings, by the anchor GitHub gives each.

use super::{Candidate, Context, Kind, Reply};

/// What opens a link's target.
const TARGET_OPEN: &str = "](";

/// The most entries of a directory offered.
const MAX_ENTRIES: usize = 500;

/// Where the link target being typed could point.
pub(crate) fn complete(context: &Context) -> Option<Reply> {
    let before: String = context.before().iter().collect();
    let open = before.rfind(TARGET_OPEN)?;
    let target = &before[open + TARGET_OPEN.len()..];
    if target.contains(')') || target.contains(' ') {
        return None;
    }
    if let Some(partial) = target.strip_prefix('#') {
        let candidates = super::super::markdown::headings(context.lines)
            .into_iter()
            .map(|heading| {
                let title = context.lines[heading.row].trim_start_matches('#').trim().to_string();
                Candidate {
                    detail: Some(title),
                    insert: anchor(context.lines[heading.row].trim_start_matches('#').trim()),
                    kind: Kind::Heading,
                    label: anchor(context.lines[heading.row].trim_start_matches('#').trim()),
                    source: "markdown",
                }
            })
            .collect();
        return Some(Reply {
            candidates,
            start: context.col - partial.chars().count(),
        });
    }
    if crate::remote::parse(context.path).is_some() {
        return None;
    }
    let (dir_part, partial) = target.rsplit_once('/').unwrap_or(("", target));
    let base = context.path.parent()?;
    let dir = match dir_part {
        "" => base.to_path_buf(),
        part => base.join(part),
    };
    let mut entries: Vec<(bool, String)> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (!name.starts_with('.')).then(|| (entry.path().is_dir(), name))
        })
        .take(MAX_ENTRIES)
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let candidates = entries
        .into_iter()
        .map(|(is_dir, name)| Candidate {
            detail: None,
            insert: if is_dir { format!("{name}/") } else { name.clone() },
            kind: if is_dir { Kind::Directory } else { Kind::File },
            label: name,
            source: "markdown",
        })
        .collect();
    Some(Reply {
        candidates,
        start: context.col - partial.chars().count(),
    })
}

/// The anchor GitHub gives a heading: lower case, spaces as `-`, and only
/// letters, digits, `-`, and `_` kept.
pub(crate) fn anchor(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::tests::context;
    use super::*;

    #[test]
    fn test_a_link_to_a_heading_is_offered_by_its_anchor() {
        let lines = vec!["# Getting Started!".to_string(), "see [it](#get".to_string()];
        let at = super::super::Context { row: 1, ..context("see [it](#get", &lines, "markdown") };
        let reply = complete(&at).expect("anchors");
        assert_eq!(reply.start, 10);
        assert_eq!(reply.candidates[0].insert, "getting-started");
        assert!(complete(&context("no link here", &lines, "markdown")).is_none());
        assert!(complete(&context("[done](a.md) and", &lines, "markdown")).is_none());
    }
}
