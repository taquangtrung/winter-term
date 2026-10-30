//! Writing LaTeX, as magic-vscode and AUCTeX help with it: moving between
//! sectioning commands, starting an environment and renaming the one the
//! cursor is in, turning a `\[...\]` display into an `equation` and back,
//! and wrapping a word in a font command.
//!
//! Everything here reads and returns lines, and never the editor, so it is
//! tested on text alone. What a comment holds is left out of every reading.

// ========================================================================
// Constants
// ========================================================================

/// The sectioning commands, shallowest first, so a command's place here is
/// its depth.
const SECTIONS: [&str; 7] = [
    "part",
    "chapter",
    "section",
    "subsection",
    "subsubsection",
    "paragraph",
    "subparagraph",
];

/// What opens and closes an environment.
const BEGIN: &str = "\\begin{";
const END: &str = "\\end{";

/// What opens and closes a display the short way.
const DISPLAY_OPEN: &str = "\\[";
const DISPLAY_CLOSE: &str = "\\]";

/// The environment a short display turns into.
const EQUATION: &str = "equation";

// ========================================================================
// Data Structures
// ========================================================================

/// An environment: its name, and the rows of its `\begin` and its `\end`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Environment {
    pub(super) begin: usize,
    pub(super) end: usize,
    pub(super) name: String,
}

// ========================================================================
// Sections
// ========================================================================

/// How deep the sectioning command `line` opens with is (`\part` is 0),
/// starred or not.
pub(super) fn section_depth(line: &str) -> Option<usize> {
    let rest = code_of(line).trim_start().strip_prefix('\\')?;
    SECTIONS.iter().position(|name| {
        rest.strip_prefix(name)
            .map(|after| after.strip_prefix('*').unwrap_or(after))
            .is_some_and(|after| after.starts_with('{') || after.starts_with('['))
    })
}

/// The row of the sectioning command after `row`, or before it.
pub(super) fn step_section(lines: &[String], row: usize, forward: bool) -> Option<usize> {
    let is_section = |at: &usize| section_depth(&lines[*at]).is_some();
    match forward {
        true => (row + 1..lines.len()).find(is_section),
        false => (0..row).rev().find(is_section),
    }
}

// ========================================================================
// Environments
// ========================================================================

/// The innermost environment `row` is in, its own `\begin` and `\end`
/// lines included.
pub(super) fn enclosing(lines: &[String], row: usize) -> Option<Environment> {
    let mut open: Vec<(String, usize)> = Vec::new();
    for (at, line) in lines.iter().enumerate().take(row + 1) {
        for (is_begin, name) in markers(code_of(line)) {
            match is_begin {
                true => open.push((name, at)),
                // The `\end` on `row` itself still closes something `row` is in.
                false if at == row => {}
                false => {
                    if let Some(found) = open.iter().rposition(|(open, _)| *open == name) {
                        open.truncate(found);
                    }
                }
            }
        }
    }
    let (name, begin) = open.pop()?;
    let end = closing(lines, begin, &name)?;
    (end >= row).then_some(Environment { begin, end, name })
}

/// The row of the `\end` closing the environment `name` opened on `begin`.
fn closing(lines: &[String], begin: usize, name: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (at, line) in lines.iter().enumerate().skip(begin) {
        for (is_begin, found) in markers(code_of(line)) {
            if found != name {
                continue;
            }
            match is_begin {
                true => depth += 1,
                false => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(at);
                    }
                }
            }
        }
    }
    None
}

/// The `\begin{name}` and `\end{name}` on `line`, in order, as whether each
/// begins and the name.
fn markers(line: &str) -> Vec<(bool, String)> {
    let mut found = Vec::new();
    let mut rest = line;
    loop {
        let begin = rest.find(BEGIN).map(|at| (at, true));
        let end = rest.find(END).map(|at| (at, false));
        let Some((at, is_begin)) = [begin, end].into_iter().flatten().min() else {
            return found;
        };
        let open = at + if is_begin { BEGIN.len() } else { END.len() };
        let Some(close) = rest[open..].find('}') else {
            return found;
        };
        found.push((is_begin, rest[open..open + close].to_string()));
        rest = &rest[open + close + 1..];
    }
}

/// `lines` with the environment `env` renamed to `name`: the rows of its
/// `\begin` and its `\end`, as they now read.
pub(super) fn renamed(lines: &[String], env: &Environment, name: &str) -> Vec<(usize, String)> {
    let begin_from = format!("{BEGIN}{}}}", env.name);
    let end_from = format!("{END}{}}}", env.name);
    let begin_to = format!("{BEGIN}{name}}}");
    let end_to = format!("{END}{name}}}");
    if env.begin == env.end {
        let line = lines[env.begin]
            .replacen(&begin_from, &begin_to, 1)
            .replacen(&end_from, &end_to, 1);
        return vec![(env.begin, line)];
    }
    vec![
        (env.begin, lines[env.begin].replacen(&begin_from, &begin_to, 1)),
        (env.end, lines[env.end].replacen(&end_from, &end_to, 1)),
    ]
}

/// The lines a new environment `name` takes, indented as `indent`, with
/// an empty line inside for what goes in it.
pub(super) fn new_environment(name: &str, indent: &str) -> Vec<String> {
    vec![
        format!("{indent}{BEGIN}{name}}}"),
        indent.to_string(),
        format!("{indent}{END}{name}}}"),
    ]
}

/// The display `row` is in turned the other way: a `\[...\]` into an
/// `equation` environment, an `equation` into `\[...\]`. The rows of its
/// delimiters and what they now read, or `None` outside either.
pub(super) fn toggled_display(lines: &[String], row: usize) -> Option<Vec<(usize, String)>> {
    if let Some(env) = enclosing(lines, row).filter(|env| env.name.trim_end_matches('*') == EQUATION) {
        let begin = lines[env.begin].replacen(&format!("{BEGIN}{}}}", env.name), DISPLAY_OPEN, 1);
        let end = lines[env.end].replacen(&format!("{END}{}}}", env.name), DISPLAY_CLOSE, 1);
        return Some(match env.begin == env.end {
            true => vec![(env.begin, begin.replacen(&format!("{END}{}}}", env.name), DISPLAY_CLOSE, 1))],
            false => vec![(env.begin, begin), (env.end, end)],
        });
    }
    let open = (0..=row).rev().find(|at| code_of(&lines[*at]).contains(DISPLAY_OPEN))?;
    let close = (open..lines.len()).find(|at| {
        let code = code_of(&lines[*at]);
        match *at == open {
            true => code
                .find(DISPLAY_OPEN)
                .is_some_and(|start| code[start..].contains(DISPLAY_CLOSE)),
            false => code.contains(DISPLAY_CLOSE),
        }
    })?;
    if close < row {
        return None;
    }
    let begin_to = format!("{BEGIN}{EQUATION}}}");
    let end_to = format!("{END}{EQUATION}}}");
    if open == close {
        let line = lines[open]
            .replacen(DISPLAY_OPEN, &begin_to, 1)
            .replacen(DISPLAY_CLOSE, &end_to, 1);
        return Some(vec![(open, line)]);
    }
    Some(vec![
        (open, lines[open].replacen(DISPLAY_OPEN, &begin_to, 1)),
        (close, lines[close].replacen(DISPLAY_CLOSE, &end_to, 1)),
    ])
}

// ========================================================================
// Fonts
// ========================================================================

/// `line` with the characters `start..end` wrapped in `\command{...}`, or
/// unwrapped where that command already wraps exactly them, and where the
/// same characters now are. Counts are in characters.
pub(super) fn toggled_command(
    line: &str,
    start: usize,
    end: usize,
    command: &str,
) -> (String, usize, usize) {
    let chars: Vec<char> = line.chars().collect();
    let open: Vec<char> = format!("\\{command}{{").chars().collect();
    let wrapped = start >= open.len()
        && chars[start - open.len()..start] == open[..]
        && chars.get(end) == Some(&'}');
    let text = |parts: &[&[char]]| parts.iter().flat_map(|part| part.iter()).collect::<String>();
    match wrapped {
        true => (
            text(&[&chars[..start - open.len()], &chars[start..end], &chars[end + 1..]]),
            start - open.len(),
            end - open.len(),
        ),
        false => (
            text(&[&chars[..start], &open, &chars[start..end], &['}'], &chars[end..]]),
            start + open.len(),
            end + open.len(),
        ),
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `line` up to its comment: the first `%` no backslash escapes.
fn code_of(line: &str) -> &str {
    let mut escaped = false;
    for (at, c) in line.char_indices() {
        match c {
            '\\' => escaped = !escaped,
            '%' if !escaped => return &line[..at],
            _ => escaped = false,
        }
    }
    line
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    #[test]
    fn test_sectioning_commands_are_found_starred_or_not_and_not_in_comments() {
        assert_eq!(section_depth("\\section{Intro}"), Some(2));
        assert_eq!(section_depth("  \\subsubsection*{x}"), Some(4));
        assert_eq!(section_depth("\\sectionmark{x}"), None);
        assert_eq!(section_depth("% \\section{x}"), None);
        let doc = lines("\\section{A}\ntext\n% \\section{no}\n\\subsection{B}");
        assert_eq!(step_section(&doc, 0, true), Some(3));
        assert_eq!(step_section(&doc, 3, false), Some(0));
        assert_eq!(step_section(&doc, 3, true), None);
    }

    #[test]
    fn test_the_innermost_environment_is_found_through_nesting() {
        let doc = lines("\\begin{a}\n\\begin{b}\nx\n\\end{b}\ny\n\\end{a}");
        let at = |row| enclosing(&doc, row).map(|env| (env.name, env.begin, env.end));
        assert_eq!(at(2), Some(("b".to_string(), 1, 3)));
        assert_eq!(at(3), Some(("b".to_string(), 1, 3)), "its own \\end");
        assert_eq!(at(4), Some(("a".to_string(), 0, 5)));
        assert_eq!(at(0), Some(("a".to_string(), 0, 5)));
        assert_eq!(enclosing(&lines("x\n\\begin{a}\\end{a}"), 0), None);
    }

    #[test]
    fn test_an_environment_is_renamed_at_both_ends() {
        let doc = lines("\\begin{itemize}\n\\item x\n\\end{itemize}");
        let env = enclosing(&doc, 1).expect("in it");
        assert_eq!(
            renamed(&doc, &env, "enumerate"),
            vec![
                (0, "\\begin{enumerate}".to_string()),
                (2, "\\end{enumerate}".to_string())
            ]
        );
        assert_eq!(new_environment("proof", "  ")[0], "  \\begin{proof}");
    }

    #[test]
    fn test_a_display_turns_into_an_equation_and_back() {
        let doc = lines("\\[\n  x = 1\n\\]");
        let toggled = toggled_display(&doc, 1).expect("in a display");
        assert_eq!(
            toggled,
            vec![
                (0, "\\begin{equation}".to_string()),
                (2, "\\end{equation}".to_string())
            ]
        );
        let back = lines("\\begin{equation}\n  x = 1\n\\end{equation}");
        assert_eq!(toggled_display(&back, 1).expect("in one")[0].1, "\\[");
        assert_eq!(
            toggled_display(&lines("a \\[ x \\] b"), 0).expect("on one line")[0].1,
            "a \\begin{equation} x \\end{equation} b"
        );
        assert_eq!(toggled_display(&lines("\\[ x \\]\ntext"), 1), None);
    }

    #[test]
    fn test_a_word_is_wrapped_in_a_font_command_and_unwrapped() {
        let (line, start, end) = toggled_command("a word here", 2, 6, "textbf");
        assert_eq!((line.as_str(), start, end), ("a \\textbf{word} here", 10, 14));
        let (line, start, end) = toggled_command(&line, start, end, "textbf");
        assert_eq!((line.as_str(), start, end), ("a word here", 2, 6));
        assert_eq!(code_of("50\\% off % note"), "50\\% off ");
    }
}
