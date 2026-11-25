//! Winter's colors for text, through Cherry: the grammars, tokenizing, and
//! the passes that name a line's runs more closely are Cherry's and iris's;
//! what is Winter's is the theme's scope table ([`SCOPE_RULES`]), which a
//! run's scopes are matched against for its color and its weight, and the
//! numbering of the window's styles as Cherry's highlights.
//!
//! The text editor colors its document with [`grammars`]' highlighters; a
//! text shown elsewhere (a diff's side, a preview, a list's lines) with a
//! [`LineHighlighter`] from [`lines_of`] or [`lines_named`], one style a
//! char ([`line_styles`]).

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock, PoisonError};

use cherry::HighlightId;
use cherry::syntax::{LineHighlighter, ScopeStyle, TextMateGrammars};
use iris::ScopeRules;
use winter_model::paint::{Style, SyntaxStyle};
use winter_theme::{SCOPE_RULES, ScopeRule};

// ========================================================================
// Constants
// ========================================================================

/// How many scope stacks are remembered as styled before the memory is
/// emptied: far more than one file has, so it only empties for a session
/// that has read a great many languages.
const STYLE_CACHE_LIMIT: usize = 50_000;

/// The styles numbered so far, each id its index.
static NUMBERED: Mutex<Vec<Style>> = Mutex::new(Vec::new());

// ========================================================================
// Data Structures
// ========================================================================

/// Winter's style for Cherry: a run's scopes matched against the theme's
/// scope table, numbered as a highlight.
struct ThemeStyle;

// ========================================================================
// ThemeStyle Implementations
// ========================================================================

impl ScopeStyle for ThemeStyle {
    fn highlight(&self, scopes: &[&str]) -> Option<HighlightId> {
        let style = style_of(scopes);
        (style != Style::Normal).then(|| highlight_id(style))
    }

    /// Whether a row of the table that colors text matches: one that only
    /// weighs it leaves its color the text's.
    fn colors(&self, scopes: &[&str]) -> bool {
        closest_row(scopes, ScopeRule::colors).is_some()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The grammars every text is colored with, styled by the theme's scope
/// table.
pub fn grammars() -> &'static TextMateGrammars {
    static GRAMMARS: OnceLock<TextMateGrammars> = OnceLock::new();
    GRAMMARS.get_or_init(|| TextMateGrammars::new().with_style(ThemeStyle))
}

/// A highlighter of the lines of a text in the file at `path` whose first
/// line is `first_line`; `None` for a text in no language with a grammar.
pub fn lines_of(path: &Path, first_line: &str) -> Option<LineHighlighter> {
    grammars().line_highlighter(path, first_line)
}

/// A highlighter of lines in the language a Markdown fence names (`rust`,
/// `py`); `None` for a name of no language with a grammar.
pub fn lines_named(name: &str) -> Option<LineHighlighter> {
    grammars().line_highlighter_named(name)
}

/// The style of each char of `line`, the next line `lines` reads.
pub fn line_styles(lines: &mut LineHighlighter, line: &str) -> Vec<Style> {
    let mut styles = vec![Style::Normal; line.chars().count()];
    // The line colored first: a style it numbers is in the table after.
    let spans = lines.line(line);
    let numbered = highlight_styles();
    for (columns, id) in spans {
        let style = numbered
            .get(usize::from(id.index()))
            .copied()
            .unwrap_or(Style::Normal);
        let end = columns.end.min(styles.len());
        for slot in &mut styles[columns.start.min(end)..end] {
            *slot = style;
        }
    }
    styles
}

/// The style text inside the scopes `names` takes, outermost first: what a
/// view drawing a document of its own (a rendered Markdown preview) colors
/// a heading or a bold run with, as the source is colored.
pub fn scoped(names: &[&str]) -> Style {
    style_of(names)
}

/// Makes `keywords` the todo keywords colored in every file, from the next
/// line colored on; the same list again changes nothing.
pub fn set_todo_keywords(keywords: &[String]) {
    grammars().set_todo_keywords(keywords);
}

/// The window's styles as numbered so far, each at its highlight's index:
/// what the views draw a highlight in.
pub fn highlight_styles() -> Vec<Style> {
    NUMBERED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// The number `style` is drawn by, numbering it if it has none yet: a
/// run's, or any other text a view shows in one of the window's styles (a
/// gutter's marks, a diff's lines).
pub fn highlight_id(style: Style) -> HighlightId {
    let mut numbered = NUMBERED.lock().unwrap_or_else(PoisonError::into_inner);
    let index = match numbered.iter().position(|known| *known == style) {
        Some(index) => index,
        None => {
            numbered.push(style);
            numbered.len() - 1
        }
    };
    HighlightId::new(u16::try_from(index).unwrap_or(u16::MAX))
}

/// The style of text inside `stack`, outermost first: its color from the
/// row of the scope table that sets one and matches the stack most closely,
/// its weight from the closest that sets one, a later row winning a tie, as
/// a later rule does in a VS Code theme. Text no row matches is plain.
fn style_of(stack: &[&str]) -> Style {
    static CACHE: OnceLock<Mutex<HashMap<String, Style>>> = OnceLock::new();
    let key = stack.join(" ");
    let cache = CACHE.get_or_init(Mutex::default);
    if let Some(style) = cache
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return *style;
    }
    let rows = SyntaxStyle {
        color: closest_row(stack, ScopeRule::colors),
        weight: closest_row(stack, ScopeRule::weighs),
        ..SyntaxStyle::default()
    };
    let style = match rows == SyntaxStyle::default() {
        true => Style::Normal,
        false => Style::Syntax(rows),
    };
    let mut cache = cache.lock().unwrap_or_else(PoisonError::into_inner);
    if cache.len() >= STYLE_CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(key, style);
    style
}

/// The row of the scope table, among those `wanted` keeps, whose selector
/// matches `stack` most closely, a later row winning a tie.
fn closest_row(stack: &[&str], wanted: impl Fn(&ScopeRule) -> bool) -> Option<u16> {
    scope_rules()
        .best_among(stack, |index| wanted(&SCOPE_RULES[index]))
        .map(|index| index as u16)
}

/// The scope table's selectors, as iris matches them, parsed once.
fn scope_rules() -> &'static ScopeRules {
    static RULES: OnceLock<ScopeRules> = OnceLock::new();
    RULES.get_or_init(|| ScopeRules::new(SCOPE_RULES.iter().map(|rule| rule.scope)))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_theme::{Rgb, Theme};

    use super::*;

    /// The style the scope table's row for `scope` paints, where that row
    /// both colors and weighs the text.
    fn row(scope: &str) -> Style {
        Style::Syntax(SyntaxStyle {
            color: color_row(scope),
            weight: SCOPE_RULES
                .iter()
                .position(|rule| rule.scope == scope && rule.weighs())
                .map(|index| index as u16),
            ..SyntaxStyle::default()
        })
    }

    /// The row the color of text in `scope` comes from.
    fn color_row(scope: &str) -> Option<u16> {
        let index = SCOPE_RULES
            .iter()
            .position(|rule| rule.scope == scope)
            .expect("a row of the scope table");
        Some(index as u16)
    }

    /// The row a style takes its color from.
    fn color(style: Style) -> Option<u16> {
        match style {
            Style::Syntax(rows) => rows.color,
            _ => None,
        }
    }

    /// The color the dark theme draws `style` in, where it sets one.
    fn hue(style: Style) -> Option<Rgb> {
        color(style).and_then(|row| Theme::dark().syntax.at(usize::from(row)))
    }

    /// The color the dark theme draws text in `scope`'s row in.
    fn hue_of(scope: &str) -> Option<Rgb> {
        color_row(scope).and_then(|row| Theme::dark().syntax.at(usize::from(row)))
    }

    /// The styles of each of `lines`, the lines of a file at `path` from its
    /// first.
    fn styles(path: &str, lines: &[&str]) -> Vec<Vec<Style>> {
        let first = lines.first().copied().unwrap_or_default();
        let mut highlighter = lines_of(Path::new(path), first).expect("a grammar");
        lines
            .iter()
            .map(|line| line_styles(&mut highlighter, line))
            .collect()
    }

    /// Each run of `line`, the only line of a file at `path`, as the style
    /// it is painted in and its text.
    fn runs(path: &str, line: &str) -> Vec<(Style, String)> {
        let styles = styles(path, &[line]).remove(0);
        let mut runs: Vec<(Style, String)> = Vec::new();
        for (style, c) in styles.into_iter().zip(line.chars()) {
            match runs.last_mut() {
                Some((last, text)) if *last == style => text.push(c),
                _ => runs.push((style, c.to_string())),
            }
        }
        runs
    }

    /// The style each word of `line` is painted in, by its text trimmed.
    fn words(path: &str, line: &str) -> HashMap<String, Style> {
        runs(path, line)
            .into_iter()
            .map(|(style, text)| (text.trim().to_string(), style))
            .collect()
    }

    /// The language iris reads the file at `path` as.
    fn read_as(path: &str) -> Option<&'static str> {
        grammars().language_id(path, "")
    }

    #[test]
    fn test_files_are_known_by_name_extension_alias_and_first_line() {
        assert_eq!(read_as(".dockerignore"), Some("ignore"));
        assert_eq!(read_as(".gitignore"), Some("ignore"));
        assert_eq!(read_as("app.socket"), Some("ini"));
        assert_eq!(read_as("a.service"), Some("ini"));
        assert_eq!(read_as("App.csproj"), Some("xml"));
        assert_eq!(read_as("events.jsonl"), Some("jsonl"));
        assert_eq!(read_as("notes.mdx"), Some("markdown"));
        assert_eq!(read_as("Justfile"), Some("makefile"));
        assert_eq!(read_as("BUILD"), Some("python"));
        assert_eq!(read_as("lib.mli"), Some("ocaml.interface"));
        assert_eq!(read_as("Makefile"), Some("makefile"));
        assert_eq!(read_as("x/COMMIT_EDITMSG"), Some("git-commit"));
        assert_eq!(read_as("x/git-rebase-todo"), Some("git-rebase"));
        assert_eq!(read_as("main.RS"), Some("rust"));
        assert_eq!(read_as("lib.h"), Some("c"), "not Objective-C");
        assert_eq!(read_as("Containerfile"), Some("dockerfile"));
        assert_eq!(read_as("notes.txt"), None);
        assert_eq!(
            grammars().language_id("script", "#!/usr/bin/env python3"),
            Some("python")
        );
        assert!(lines_named("py").is_some());
        assert!(lines_named("sh").is_some());
        assert!(lines_named("nope").is_none());
    }

    #[test]
    fn test_each_latex_section_level_is_styled_apart() {
        let title = |line: &str| words("a.tex", line)["Intro"];
        assert_eq!(title(r"\section{Intro}"), row("entity.name.section.latex"));
        let subsection = row("meta.function.section.subsection.latex entity.name.section.latex");
        assert_eq!(title(r"\subsection{Intro}"), subsection);
        assert_eq!(
            title(r"\subsubsection*{Intro}"),
            row("meta.function.section.subsubsection.latex entity.name.section.latex"),
            "a starred command keeps its level"
        );
    }

    #[test]
    fn test_a_latex_title_keeps_its_level_onto_the_next_line() {
        let found = styles("a.tex", &[r"\subsection{A long", "title}"]);
        assert_eq!(
            found[1][0],
            row("meta.function.section.subsection.latex entity.name.section.latex")
        );
    }

    #[test]
    fn test_each_markdown_heading_level_is_styled_apart() {
        let heading = |line: &str| runs("a.md", line);
        let four = heading("#### Four");
        assert_eq!(
            four[0],
            (
                row("heading.4.markdown punctuation.definition.heading.markdown"),
                "####".to_string()
            )
        );
        assert_eq!(four[1].0, row("heading.4.markdown"));
        assert_ne!(heading("### Three")[1].0, four[1].0, "each level its own");
    }

    #[test]
    fn test_latex_math_labels_and_citations_take_their_own_colors() {
        let styles = words("a.tex", r"$x$ \label{a} \cite{b}");
        assert_ne!(color(styles["x"]), None, "math");
        assert_ne!(styles["a"], Style::Normal, "a label");
        assert_eq!(styles["b"], row("constant.other.reference.citation.latex"));
    }

    #[test]
    fn test_a_closer_scope_wins_over_a_broader_one() {
        let rust = words("a.rs", "struct Foo; const N: u8 = 42;");
        assert_eq!(color(rust["Foo"]), color_row("entity.name.type"));
        assert_eq!(color(rust["42"]), color_row("constant.numeric"));
        assert_ne!(
            color(rust["42"]),
            color_row("constant"),
            "not the broader row"
        );
    }

    #[test]
    fn test_kdl_and_wit_are_read() {
        let kdl = words(
            "config.kdl",
            r##"colors { git { modified "#8fb1f5" } } // x"##,
        );
        assert_eq!(color(kdl["colors"]), color_row("entity.name.tag"));
        assert_eq!(hue(kdl["\"#8fb1f5\""]), hue_of("string"));
        assert_eq!(color(kdl["// x"]), color_row("comment"));
        let wit = words("winter.wit", "package winter:extension@0.1.0;");
        assert_eq!(color(wit["winter"]), color_row("entity.name.type"));
    }

    #[test]
    fn test_a_commit_summary_s_conventional_type_is_a_keyword() {
        let summary = |line: &str| {
            let (style, text) = runs("x/COMMIT_EDITMSG", line)[0].clone();
            (color(style), text)
        };
        assert_eq!(
            summary("fix(ui)!: tidy"),
            (color_row("keyword"), "fix(ui)!:".to_string())
        );
        assert_eq!(
            summary("feat: x"),
            (color_row("keyword"), "feat:".to_string())
        );
        // Prose that only looks like one at a glance is left as the summary.
        for line in ["Fix the parser", "v2: notes", "(ui): x", "fix(ui x"] {
            assert_ne!(summary(line).0, color_row("keyword"), "{line}");
        }
    }

    #[test]
    fn test_git_s_comments_are_one_color_whatever_they_name() {
        // Regression: the grammars scope each changed file and the kind of
        // change inside git's comment lines, and the theme colored them as
        // code.
        let commit = [
            "fix: x",
            "",
            "# On branch main",
            "# Changes to be committed:",
            "#\tmodified:   src/a.rs",
            "#\tnew file:   b.rs",
        ];
        let rebase = [
            "pick 1a2b3c4 fix: x",
            "",
            "# Rebase 1a2b..3c4d onto 1a2b (1 command)",
            "#",
            "# Commands:",
            "# p, pick <commit> = use commit",
        ];
        for (path, lines) in [
            ("x/COMMIT_EDITMSG", &commit),
            ("x/git-rebase-todo", &rebase),
        ] {
            let comments: Vec<Vec<Style>> = styles(path, lines)
                .into_iter()
                .zip(lines.iter())
                .filter(|(_, line)| line.starts_with('#'))
                .map(|(styles, _)| styles)
                .collect();
            let comment = comments[0][0];
            assert_eq!(color(comment), color_row("comment"), "{path}");
            for styles in &comments {
                assert!(
                    styles.iter().all(|style| *style == comment),
                    "{path}: every character of every comment line alike"
                );
            }
        }
    }

    #[test]
    fn test_a_line_too_long_to_read_is_plain_and_leaves_the_next_line_be() {
        let long = format!("/* {}", "x".repeat(20_000));
        let found = styles("a.rs", &[&long, "fn f() {}"]);
        assert!(found[0].iter().all(|style| *style == Style::Normal));
        assert_eq!(found[0].len(), long.chars().count());
        assert_ne!(
            color(found[1][0]),
            color_row("comment"),
            "the comment it opens is not read"
        );
    }

    #[test]
    fn test_a_function_call_is_colored_as_a_function() {
        for (path, line, call) in [
            ("a.py", "obj.method(1)", "method"),
            ("a.js", "foo(1);", "foo"),
            ("a.go", "fmt.Println(x)", "Println"),
            ("a.rs", "obj.method(1);", "method"),
        ] {
            let call_style = words(path, line).get(call).copied().map(hue);
            assert_eq!(call_style, Some(hue_of("entity.name.function")), "{path}");
        }
    }

    #[test]
    fn test_task_boxes_and_todo_keywords_are_colored_where_they_count() {
        // Grammars of its own: the shared ones' keywords are every editor's,
        // which other tests set as they configure editors.
        let grammars = TextMateGrammars::new().with_style(ThemeStyle);
        grammars.set_todo_keywords(&["TODO".to_owned(), "NEXT".to_owned()]);
        let colors = |path: &str, line: &str| -> Vec<Option<u16>> {
            let mut highlighter = grammars.line_highlighter(path, line).expect("a grammar");
            line_styles(&mut highlighter, line)
                .into_iter()
                .map(color)
                .collect()
        };
        let task = colors("a.md", "- [ ] NEXT call TODOS");
        let checkbox = color_row("markup.checkbox");
        assert_eq!(
            task[2..5],
            [checkbox, color_row("markup.checkbox.checked"), checkbox]
        );
        let keyword = color_row("keyword.todo");
        assert_eq!(task[6], keyword, "NEXT in prose");
        assert_ne!(task[16], keyword, "TODOS is another word");
        let code = colors("a.md", "Use `TODO` here");
        assert_ne!(code[5], keyword, "Markdown's code is left alone");
        let rust = colors("a.rs", "let TODO = 1; // TODO: x");
        assert_ne!(rust[4], keyword, "a name in code");
        assert_eq!(rust[17], keyword, "in a comment");
    }

    #[test]
    fn test_markdown_front_matter_math_and_marks_are_colored() {
        let doc = [
            "---",
            "title: Winter",
            "---",
            "Some $x^2$ and [[Page]] and ==hot==",
            "$$",
            "a + b",
            "$$",
            "> [!NOTE] Mind",
        ];
        let lines: Vec<Vec<Option<u16>>> = styles("a.md", &doc)
            .into_iter()
            .map(|styles| styles.into_iter().map(color).collect())
            .collect();
        let comment = color_row("comment");
        assert_eq!(lines[0], [comment; 3], "the opening fence");
        assert_eq!(lines[2], [comment; 3], "the closing fence, not a heading's");
        assert_ne!(
            lines[1][0],
            color_row("markup.heading"),
            "YAML, not a heading"
        );
        assert!(lines[1].iter().any(Option::is_some), "colored by YAML");
        let math = color_row("markup.math");
        assert_eq!(lines[3][6], math, "inline math");
        assert_eq!(
            lines[3][15],
            color_row("markup.underline.link"),
            "a wiki link"
        );
        assert_eq!(lines[3][30], color_row("markup.highlight"));
        assert!(lines[5].iter().all(|row| *row == math), "display math");
        assert_eq!(lines[7][3], color_row("keyword"), "a callout's label");
    }

    #[test]
    fn test_rust_names_the_grammar_leaves_plain_are_named_from_the_line() {
        let rust = words("a.rs", "obj.field.method(1);");
        assert_eq!(color(rust["field"]), color_row("variable.other.member"));
    }
}
