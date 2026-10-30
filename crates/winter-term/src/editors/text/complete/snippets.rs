//! Snippets: templates a short word expands to, each language's own set
//! and the reader's, in snippet syntax (`$1`, `${1:default}`, `$0`; see
//! [`super::expand`]).
//!
//! The reader's are files under `snippets/<language>/` in the config
//! directory, one a snippet, named by the word that expands it, its body
//! the file's text; a first line starting `# ` describes it instead.

use std::path::Path;

use super::{Candidate, Context, Kind, Reply};

/// What starts a reader's snippet file's description line.
const DESCRIPTION_MARK: &str = "# ";

/// One built-in snippet: the word, the body, and what it is.
type Snippet = (&'static str, &'static str, &'static str);

/// Each language's own snippets.
const BUILT_IN: &[(&str, &[Snippet])] = &[
    ("rust", &[
        ("fn", "fn ${1:name}($2) {\n    $0\n}", "a function"),
        ("pfn", "pub fn ${1:name}($2) -> ${3:Type} {\n    $0\n}", "a public function"),
        ("impl", "impl ${1:Type} {\n    $0\n}", "an impl block"),
        ("struct", "struct ${1:Name} {\n    $0\n}", "a struct"),
        ("enum", "enum ${1:Name} {\n    $0\n}", "an enum"),
        ("match", "match ${1:value} {\n    $0\n}", "a match"),
        ("test", "#[test]\nfn ${1:test_name}() {\n    $0\n}", "a test"),
        ("for", "for ${1:item} in ${2:items} {\n    $0\n}", "a for loop"),
    ]),
    ("python", &[
        ("def", "def ${1:name}($2):\n    $0", "a function"),
        ("class", "class ${1:Name}:\n    def __init__(self$2):\n        $0", "a class"),
        ("main", "if __name__ == \"__main__\":\n    $0", "the main guard"),
        ("for", "for ${1:item} in ${2:items}:\n    $0", "a for loop"),
    ]),
    ("latex", &[
        ("begin", "\\begin{${1:environment}}\n    $0\n\\end{environment}", "an environment"),
        ("fig", "\\begin{figure}[t]\n    \\centering\n    \\includegraphics[width=\\linewidth]{${1:file}}\n    \\caption{$2}\n    \\label{fig:$3}\n\\end{figure}", "a figure"),
        ("tab", "\\begin{table}[t]\n    \\centering\n    \\caption{$2}\n    \\label{tab:$3}\n    \\begin{tabular}{${1:ll}}\n        $0\n    \\end{tabular}\n\\end{table}", "a table"),
        ("itemize", "\\begin{itemize}\n    \\item $0\n\\end{itemize}", "a bulleted list"),
        ("enumerate", "\\begin{enumerate}\n    \\item $0\n\\end{enumerate}", "a numbered list"),
        ("eq", "\\begin{equation}\n    $0\n\\end{equation}", "an equation"),
        ("sec", "\\section{${1:title}}\n\\label{sec:$2}\n$0", "a section with its label"),
    ]),
    ("markdown", &[
        ("code", "```${1:language}\n$0\n```", "a fenced code block"),
        ("link", "[${1:text}](${2:url})$0", "a link"),
        ("img", "![${1:alt}](${2:path})$0", "an image"),
        ("table", "| ${1:A} | ${2:B} |\n|---|---|\n| $0 |  |", "a table"),
    ]),
];

/// The snippets of the language whose word ends at the cursor.
pub(crate) fn complete(context: &Context) -> Option<Reply> {
    let start = context.word_start();
    if start == context.col {
        return None;
    }
    let mut candidates = user_snippets(context.language);
    if let Some((_, own)) = BUILT_IN.iter().find(|(language, _)| *language == context.language) {
        candidates.extend(own.iter().map(|(word, body, what)| snippet(word, body, what)));
    }
    Some(Reply { candidates, start })
}

/// A snippet offered by `word`, putting in `body`.
fn snippet(word: &str, body: &str, what: &str) -> Candidate {
    Candidate {
        detail: Some(what.to_string()),
        insert: body.to_string(),
        kind: Kind::Snippet,
        label: word.to_string(),
        source: "snippet",
    }
}

/// The reader's own snippets of `language`, ahead of the built-in ones.
fn user_snippets(language: &str) -> Vec<Candidate> {
    read_snippets(&crate::config::config_dir().join("snippets").join(language))
}

/// The snippet files in `dir`.
fn read_snippets(dir: &Path) -> Vec<Candidate> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Candidate> = entries
        .flatten()
        .filter(|entry| entry.path().is_file())
        .filter_map(|entry| {
            let word = entry.file_name().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(entry.path()).ok()?;
            let (what, body) = match text.split_once('\n') {
                Some((first, rest)) if first.starts_with(DESCRIPTION_MARK) => {
                    (first[DESCRIPTION_MARK.len()..].trim().to_string(), rest)
                }
                _ => ("yours".to_string(), text.as_str()),
            };
            Some(snippet(&word, body.trim_end_matches('\n'), &what))
        })
        .collect();
    found.sort_by(|a, b| a.label.cmp(&b.label));
    found
}

#[cfg(test)]
mod tests {
    use super::super::tests::context;
    use super::*;

    #[test]
    fn test_a_language_offers_its_snippets_where_a_word_is_typed() {
        let lines = vec![String::new()];
        let reply = complete(&context("    fi", &lines, "latex")).expect("snippets");
        let fig = reply.candidates.iter().find(|c| c.label == "fig").expect("the figure");
        assert_eq!(fig.kind, Kind::Snippet);
        assert!(complete(&context("    ", &lines, "latex")).is_none());
    }

    #[test]
    fn test_a_reader_s_snippet_file_is_its_word_and_body() {
        let dir = std::env::temp_dir().join(format!("winter-snippets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory");
        std::fs::write(dir.join("todo"), "# a reminder\n// TODO(me): $0\n").expect("a snippet");
        let found = read_snippets(&dir);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].label.as_str(), found[0].insert.as_str()), ("todo", "// TODO(me): $0"));
        assert_eq!(found[0].detail.as_deref(), Some("a reminder"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
