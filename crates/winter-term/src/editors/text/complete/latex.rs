//! LaTeX: what a document's commands take where one is being typed: the
//! commands themselves after `\`, environments after `\begin{` (the one
//! left open first after `\end{`), the project's labels in a reference, and
//! its bibliography's keys in a citation, as texlab and AUCTeX offer them.
//!
//! The labels, the keys, and the commands a project defines are read from
//! the `.tex` and `.bib` files under the document's directory.

use std::path::{Path, PathBuf};

use super::{Candidate, Context, Kind, Reply};

/// How deep under the document's directory its project is read, and how
/// many files of it at most.
const MAX_DEPTH: usize = 3;
const MAX_FILES: usize = 200;

/// The commands a document is written with, and what each takes after its
/// name, in snippet syntax.
const COMMANDS: &[(&str, &str)] = &[
    ("begin", "{$1}"), ("end", "{$1}"), ("section", "{$1}"), ("subsection", "{$1}"),
    ("subsubsection", "{$1}"), ("paragraph", "{$1}"), ("chapter", "{$1}"), ("part", "{$1}"),
    ("label", "{$1}"), ("ref", "{$1}"), ("eqref", "{$1}"), ("cref", "{$1}"), ("Cref", "{$1}"),
    ("autoref", "{$1}"), ("pageref", "{$1}"), ("cite", "{$1}"), ("citep", "{$1}"),
    ("citet", "{$1}"), ("textbf", "{$1}"), ("textit", "{$1}"), ("emph", "{$1}"),
    ("texttt", "{$1}"), ("textsf", "{$1}"), ("textsc", "{$1}"), ("underline", "{$1}"),
    ("footnote", "{$1}"), ("caption", "{$1}"), ("includegraphics", "[width=\\linewidth]{$1}"),
    ("input", "{$1}"), ("include", "{$1}"), ("usepackage", "{$1}"), ("documentclass", "{$1}"),
    ("title", "{$1}"), ("author", "{$1}"), ("date", "{$1}"), ("maketitle", ""),
    ("tableofcontents", ""), ("item", " "), ("centering", ""), ("noindent", ""),
    ("newcommand", "{\\\\$1}{$2}"), ("renewcommand", "{\\\\$1}{$2}"), ("frac", "{$1}{$2}"),
    ("sqrt", "{$1}"), ("sum", ""), ("int", ""), ("prod", ""), ("infty", ""), ("alpha", ""),
    ("beta", ""), ("gamma", ""), ("delta", ""), ("epsilon", ""), ("lambda", ""), ("mu", ""),
    ("sigma", ""), ("theta", ""), ("pi", ""), ("mathbb", "{$1}"), ("mathcal", "{$1}"),
    ("mathrm", "{$1}"), ("mathbf", "{$1}"), ("mathit", "{$1}"), ("left", ""), ("right", ""),
    ("cdot", ""), ("ldots", ""), ("leq", ""), ("geq", ""), ("neq", ""), ("in", ""),
    ("subseteq", ""), ("cup", ""), ("cap", ""), ("hline", ""), ("vspace", "{$1}"),
    ("hspace", "{$1}"), ("url", "{$1}"), ("href", "{$1}{$2}"),
];

/// The environments a document opens.
const ENVIRONMENTS: &[&str] = &[
    "abstract", "algorithm", "align", "align*", "center", "definition", "description",
    "document", "enumerate", "equation", "equation*", "figure", "figure*", "flushleft",
    "flushright", "itemize", "lemma", "lstlisting", "minipage", "proof", "quote", "table",
    "table*", "tabular", "theorem", "verbatim",
];

/// The commands a reference names a label with, and a citation a key with.
const REFERENCES: &[&str] = &["ref", "eqref", "cref", "Cref", "autoref", "pageref", "nameref", "vref"];

/// What is being typed after a backslash, in braces after a command, and
/// where it starts.
enum Spot {
    /// A command's name, after `\`.
    Command(usize),
    /// What goes in the braces of the command named, from the column given.
    Argument(String, usize),
}

/// What could go at the cursor in a LaTeX document.
pub(crate) fn complete(context: &Context) -> Option<Reply> {
    let candidates;
    let start;
    match spot(&context.before())? {
        Spot::Command(at) => {
            start = at;
            candidates = commands(context);
        }
        Spot::Argument(command, at) => {
            start = at;
            candidates = match command.as_str() {
                "begin" => environments(),
                "end" => {
                    let mut open = open_environment(context).into_iter().collect::<Vec<_>>();
                    open.extend(environments());
                    open
                }
                name if REFERENCES.contains(&name) => labels(context),
                name if name.contains("cite") => citations(context),
                _ => return None,
            };
        }
    }
    Some(Reply { candidates, start })
}

/// Where the cursor is in the LaTeX it ends: in a command's name, or in the
/// braces after one (after the last `,` there, for a list of keys).
fn spot(before: &[char]) -> Option<Spot> {
    let mut at = before.len();
    while at > 0 && before[at - 1].is_ascii_alphabetic() {
        at -= 1;
    }
    if at > 0 && before[at - 1] == '\\' {
        return Some(Spot::Command(at));
    }
    let open = before.iter().rposition(|c| *c == '{')?;
    if before[open..].contains(&'}') {
        return None;
    }
    let mut name_start = open;
    while name_start > 0 && before[name_start - 1].is_ascii_alphabetic() {
        name_start -= 1;
    }
    if name_start == 0 || before[name_start - 1] != '\\' {
        // An optional argument between the name and the braces: `\cite[p.~3]{`.
        return None;
    }
    let name: String = before[name_start..open].iter().collect();
    let key_start = before[open..]
        .iter()
        .rposition(|c| *c == ',')
        .map_or(open + 1, |comma| open + comma + 1);
    let key_start = (key_start..before.len())
        .find(|at| !before[*at].is_whitespace())
        .unwrap_or(before.len());
    Some(Spot::Argument(name, key_start))
}

/// The built-in commands, and those the project defines.
fn commands(context: &Context) -> Vec<Candidate> {
    let mut found: Vec<Candidate> = COMMANDS
        .iter()
        .map(|(name, args)| Candidate {
            detail: None,
            insert: format!("{name}{args}"),
            kind: Kind::Command,
            label: name.to_string(),
            source: "latex",
        })
        .collect();
    for (text, _) in project_text(context, "tex") {
        for name in after_each(&text, "\\newcommand{\\").chain(after_each(&text, "\\renewcommand{\\")) {
            found.push(Candidate {
                detail: Some("defined here".to_string()),
                insert: name.clone(),
                kind: Kind::Command,
                label: name,
                source: "latex",
            });
        }
    }
    found
}

/// Every environment offered after `\begin{`.
fn environments() -> Vec<Candidate> {
    ENVIRONMENTS
        .iter()
        .map(|name| Candidate {
            detail: None,
            insert: name.to_string(),
            kind: Kind::Environment,
            label: name.to_string(),
            source: "latex",
        })
        .collect()
}

/// The environment the lines above the cursor left open, to close.
fn open_environment(context: &Context) -> Option<Candidate> {
    let mut open: Vec<String> = Vec::new();
    for (row, line) in context.lines.iter().enumerate().take(context.row + 1) {
        let line = if row == context.row { context.before().iter().collect() } else { line.clone() };
        for name in after_each(&line, "\\begin{") {
            open.push(name);
        }
        for name in after_each(&line, "\\end{") {
            if let Some(at) = open.iter().rposition(|open| *open == name) {
                open.truncate(at);
            }
        }
    }
    let name = open.pop()?;
    Some(Candidate {
        detail: Some("left open above".to_string()),
        insert: name.clone(),
        kind: Kind::Environment,
        label: name,
        source: "latex",
    })
}

/// Every `\label` in the buffer and the project, each with the file it is in.
fn labels(context: &Context) -> Vec<Candidate> {
    let mut found: Vec<Candidate> = context
        .lines
        .iter()
        .flat_map(|line| after_each(line, "\\label{").collect::<Vec<_>>())
        .map(|label| label_candidate(label, "this file".to_string()))
        .collect();
    for (text, path) in project_text(context, "tex") {
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        found.extend(after_each(&text, "\\label{").map(|label| label_candidate(label, name.clone())));
    }
    found
}

fn label_candidate(label: String, place: String) -> Candidate {
    Candidate {
        detail: Some(place),
        insert: label.clone(),
        kind: Kind::Label,
        label,
        source: "latex",
    }
}

/// Every entry's key in the project's `.bib` files, with its title.
fn citations(context: &Context) -> Vec<Candidate> {
    let mut found = Vec::new();
    for (text, _) in project_text(context, "bib") {
        found.extend(bib_entries(&text).into_iter().map(|(key, title)| Candidate {
            detail: title,
            insert: key.clone(),
            kind: Kind::Citation,
            label: key,
            source: "bibtex",
        }));
    }
    found
}

/// Each entry of a `.bib` file: its key and, where it has one, its title.
pub(crate) fn bib_entries(text: &str) -> Vec<(String, Option<String>)> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find('@') {
        rest = &rest[at + 1..];
        let Some(open) = rest.find(['{', '(']) else { break };
        let kind = rest[..open].trim().to_ascii_lowercase();
        let body = &rest[open + 1..];
        let Some(comma) = body.find(',') else { continue };
        let key = body[..comma].trim().to_string();
        if key.is_empty() || matches!(kind.as_str(), "comment" | "string" | "preamble") {
            continue;
        }
        let next = body.find("\n@").unwrap_or(body.len());
        let title = field(&body[..next], "title");
        found.push((key, title));
    }
    found
}

/// The value of `name` in a `.bib` entry's `body`, its braces taken off.
fn field(body: &str, name: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    let at = lower
        .match_indices(name)
        .find(|(at, _)| lower[at + name.len()..].trim_start().starts_with('='))?
        .0;
    let value = body[at + name.len()..].trim_start().trim_start_matches('=').trim_start();
    let end = value.find("},").or_else(|| value.find("\",")).or_else(|| value.find('\n'))?;
    let cleaned: String = value[..end].chars().filter(|c| !matches!(c, '{' | '}' | '"')).collect();
    Some(cleaned.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// What follows each `marker` in `text` up to the `}` closing it.
fn after_each<'a>(text: &'a str, marker: &'a str) -> impl Iterator<Item = String> + 'a {
    text.match_indices(marker).filter_map(move |(at, _)| {
        let rest = &text[at + marker.len()..];
        let end = rest.find('}')?;
        let name = rest[..end].trim();
        (!name.is_empty()).then(|| name.to_string())
    })
}

/// The text of every file with `extension` under the document's directory,
/// the document itself left out, and where each is.
fn project_text(context: &Context, extension: &str) -> Vec<(String, PathBuf)> {
    if crate::remote::parse(context.path).is_some() {
        return Vec::new();
    }
    let Some(dir) = context.path.parent() else {
        return Vec::new();
    };
    let mut files = Vec::new();
    collect(dir, extension, 0, &mut files);
    files
        .into_iter()
        .filter(|path| path != context.path)
        .filter_map(|path| Some((std::fs::read_to_string(&path).ok()?, path)))
        .collect()
}

fn collect(dir: &Path, extension: &str, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let hidden = entry.file_name().to_string_lossy().starts_with('.');
        if path.is_dir() && !hidden {
            collect(&path, extension, depth + 1, out);
        } else if path.extension().is_some_and(|found| found == extension) {
            out.push(path);
        }
        if out.len() >= MAX_FILES {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::context;
    use super::*;

    fn labels_of(reply: Option<Reply>) -> Vec<String> {
        reply.map(|r| r.candidates.into_iter().map(|c| c.label).collect()).unwrap_or_default()
    }

    #[test]
    fn test_a_command_an_environment_and_the_one_left_open_are_offered_where_each_goes() {
        let lines = vec![String::new()];
        let reply = complete(&context("see \\textb", &lines, "latex")).expect("a command");
        assert_eq!(reply.start, 5);
        assert!(reply.candidates.iter().any(|c| c.insert == "textbf{$1}"));
        assert!(labels_of(complete(&context("\\begin{ite", &lines, "latex"))).contains(&"itemize".to_string()));
        let doc = vec!["\\begin{proof}".to_string(), "\\end{".to_string()];
        let at_end = super::super::Context { row: 1, ..context("\\end{", &doc, "latex") };
        assert_eq!(labels_of(complete(&at_end))[0], "proof", "the one left open first");
    }

    #[test]
    fn test_a_reference_offers_the_labels_and_a_citation_the_keys_after_a_comma() {
        let doc = vec!["\\label{sec:intro}".to_string(), "see \\ref{se".to_string()];
        let at = super::super::Context { row: 1, ..context("see \\ref{se", &doc, "latex") };
        let reply = complete(&at).expect("labels");
        assert_eq!(reply.start, 9);
        assert!(reply.candidates.iter().any(|c| c.label == "sec:intro"));
        match spot(&"\\citep{knuth, la".chars().collect::<Vec<_>>()) {
            Some(Spot::Argument(name, at)) => assert_eq!((name.as_str(), at), ("citep", 14)),
            _ => panic!("in a citation"),
        }
        assert!(spot(&"\\textbf{done} and".chars().collect::<Vec<_>>()).is_none());
    }

    #[test]
    fn test_a_bibliography_s_keys_are_read_with_their_titles() {
        let bib = "@article{knuth84,\n  title = {Literate {P}rogramming},\n  year = 1984,\n}\n@comment{x,}\n@book{lamport, title=\"LaTeX\",\n}";
        assert_eq!(
            bib_entries(bib),
            vec![
                ("knuth84".to_string(), Some("Literate Programming".to_string())),
                ("lamport".to_string(), Some("LaTeX".to_string())),
            ]
        );
    }
}
