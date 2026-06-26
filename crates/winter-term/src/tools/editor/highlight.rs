//! Coloring source as it is read, with the TextMate grammars VS Code's
//! themes are written against: which scopes each run of characters sits in,
//! and which row of the theme's scope table ([`SCOPE_RULES`]) colors it.
//!
//! A grammar is chosen by the file's name, else its extension, else its first
//! line (a shebang, a modeline). It colors what the characters say and never
//! decides that something is wrong with the code.
//!
//! The grammars are two-face's (bat's set), and the few Winter carries in
//! `grammars/` for languages that set lacks, never a copy of one it has.
//! Where a grammar's scope is too coarse for a color the theme draws, a pass
//! here names the run more closely: a LaTeX section's level, a commit
//! summary's conventional type, Rust's built-in types.

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Mutex, OnceLock, PoisonError};

use syntect::highlighting::ScopeSelectors;
use syntect::parsing::{
    ParseState, Scope, ScopeStack, SyntaxDefinition, SyntaxReference, SyntaxSet, SyntaxSetBuilder,
};
use winter_render::{ScopeRule, SCOPE_RULES};

use crate::model::paint::{Style, SyntaxStyle};

// ========================================================================
// Constants
// ========================================================================

/// Extensions the grammars do not claim, and the extension each is read as.
const EXTENSIONS: &[(&str, &str)] = &[
    ("cls", "tex"),
    ("containerfile", "dockerfile"),
    ("dockerignore", "gitignore"),
    ("editorconfig", "ini"),
    ("envrc", "sh"),
    ("geojson", "json"),
    // Sublime reads a `.h` as Objective-C, which most of them are not.
    ("h", "c"),
    ("ignore", "gitignore"),
    ("ino", "cpp"),
    ("json5", "json"),
    ("jsonc", "json"),
    ("ksh", "sh"),
    ("npmignore", "gitignore"),
    ("plist", "xml"),
    ("service", "ini"),
    ("sty", "tex"),
    ("webmanifest", "json"),
    ("xsl", "xml"),
];

/// Files whose name, rather than an extension, says what they are, where the
/// grammars do not already know the name: the extension each is read as.
const FILENAMES: &[(&str, &str)] = &[
    (".npmrc", "ini"),
    ("BUILD", "py"),
    ("Containerfile", "dockerfile"),
    ("Justfile", "mk"),
    ("flake.lock", "json"),
    ("justfile", "mk"),
];

/// The longest line, in bytes, read through its grammar. A longer one, which
/// is a minified script or a data dump rather than something written to be
/// read, is painted plain and left to change nothing for the lines after it:
/// a grammar takes most of a second over a few hundred kilobytes of one
/// line, and would take it on every paint. VS Code stops at the same length.
const LONGEST_LINE: usize = 20_000;

/// How many scope stacks are remembered as colored before the memory is
/// emptied: far more than one file has, so it only empties for a session
/// that has read a great many languages.
const STYLE_CACHE_LIMIT: usize = 50_000;

/// The scope a LaTeX sectioning command's name sits in, whose word says
/// which level it is.
const SECTION_COMMAND: &str = "support.function.section.latex";

/// The grammar of text that is not in any language.
const PLAIN_TEXT: &str = "Plain Text";

/// The grammars Winter carries itself, for languages two-face lacks.
const OWN_GRAMMARS: [&str; 1] = [include_str!("grammars/kdl.sublime-syntax")];

/// The scope a commit message's summary sits in.
const COMMIT_SUBJECT: &str = "markup.heading.subject.git.commit";

/// The scope a summary's conventional type (`feat:`, `fix(ui)!:`) is given,
/// which the grammar leaves as part of the summary.
const COMMIT_TYPE: &str = "keyword.other.type.git.commit";

/// Words a grammar puts in too coarse a scope, and the scope each is given
/// instead: Sublime's Rust grammar scopes `u32` as it does `let`, where the
/// theme colors a built-in type as a type.
const WORD_SCOPES: &[WordScope] = &[WordScope {
    scope: "storage.type.rust",
    to: "support.type.primitive.rust",
    words: &[
        "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "str", "u8",
        "u16", "u32", "u64", "u128", "usize",
    ],
}];

/// The scope a whole LaTeX sectioning command sits in, whatever its level.
const SECTION_META: &str = "meta.section.latex";

// ========================================================================
// Data Structures
// ========================================================================

/// The grammar a file is read with, and the set it is one of, which its
/// lines are parsed against.
#[derive(Clone, Copy, Debug)]
pub struct Grammar {
    source: Source,
    syntax: &'static SyntaxReference,
}

/// Where a line begins in its grammar: what the lines above it left open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LineState {
    /// The grammar's own state: the contexts it is inside.
    parse: ParseState,
    /// The scopes the line begins inside.
    scopes: ScopeStack,
    /// The level of the LaTeX sectioning command whose title the line begins
    /// inside, for a title that runs past its command's line.
    section: Option<Scope>,
    /// The set the grammar is one of.
    source: Source,
}

/// Which set of grammars a grammar comes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Source {
    /// two-face's, bat's set.
    Bundled,
    /// The ones Winter carries itself.
    Own,
}

/// A grammar's scope that holds words the theme colors apart, and the scope
/// those words are given instead.
struct WordScope {
    /// The scope the grammar gives the words.
    scope: &'static str,
    /// The scope they are given instead.
    to: &'static str,
    /// The words.
    words: &'static [&'static str],
}

// ========================================================================
// Grammar
// ========================================================================

impl Grammar {
    /// The grammar's name, as the grammar itself gives it (`Rust`, `LaTeX`).
    pub fn name(&self) -> &'static str {
        &self.syntax.name
    }
}

// ========================================================================
// Source
// ========================================================================

impl Source {
    /// The set itself, unpacked the first time a grammar of it is wanted.
    fn syntaxes(self) -> &'static SyntaxSet {
        match self {
            Self::Bundled => bundled_syntaxes(),
            Self::Own => own_syntaxes(),
        }
    }
}

// ========================================================================
// LineState
// ========================================================================

impl LineState {
    /// Where the first line of a file in `grammar` begins.
    pub fn start(grammar: Grammar) -> Self {
        Self {
            parse: ParseState::new(grammar.syntax),
            scopes: ScopeStack::new(),
            section: None,
            source: grammar.source,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The grammar `path` names by its file name or its extension, or `None` for
/// a file whose language this does not know.
pub fn of_path(path: &Path) -> Option<Grammar> {
    let name = path.file_name()?.to_str()?;
    if let Some((_, extension)) = FILENAMES.iter().find(|(file, _)| *file == name) {
        return by_extension(extension);
    }
    // The grammars list whole names among their extensions (`Makefile`,
    // `COMMIT_EDITMSG`, `.bashrc`).
    if let Some(grammar) = by_extension(name) {
        return Some(grammar);
    }
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let extension = EXTENSIONS
        .iter()
        .find(|(from, _)| *from == extension)
        .map_or(extension.as_str(), |(_, to)| to);
    by_extension(extension)
}

/// The grammar a file's first line names: a shebang (`#!/usr/bin/env
/// python3`), a modeline, an XML declaration.
pub fn of_first_line(line: &str) -> Option<Grammar> {
    find(|set| set.find_syntax_by_first_line(line))
}

/// The grammar called `name`.
pub fn named(name: &str) -> Option<Grammar> {
    find(|set| set.find_syntax_by_name(name))
}

/// The style of each character of `line`, and where the next line begins.
pub fn highlight(line: &str, state: &LineState) -> (Vec<Style>, LineState) {
    let mut styles = Vec::with_capacity(line.len());
    let next = scan(line, state, Some(&mut styles));
    (styles, next)
}

/// Where the line after `line` begins, without working out its colors: the
/// same walk as [`highlight`], for the lines between the top of a file and
/// the part of it being looked at.
pub fn next_state(line: &str, state: &LineState) -> LineState {
    scan(line, state, None)
}

/// What `line` is made of at character `col`: the scopes its grammar puts
/// the character in, innermost last, and the rows of the scope table that
/// color and weigh it. What a user names in the `syntax` colors to recolor it.
pub fn describe_at(line: &str, state: &LineState, col: usize) -> String {
    let mut scopes = state.scopes.clone();
    let mut parse = state.parse.clone();
    let text = format!("{line}\n");
    let at = line
        .char_indices()
        .nth(col)
        .map_or(line.len(), |(at, _)| at);
    if line.len() <= LONGEST_LINE {
        if let Ok(ops) = parse.parse_line(&text, state.source.syntaxes()) {
            for (end, op) in ops {
                if end > at || scopes.apply(&op).is_err() {
                    break;
                }
            }
        }
    }
    let names: Vec<String> = scopes
        .as_slice()
        .iter()
        .map(|scope| scope.build_string())
        .collect();
    let (styles, _) = highlight(line, state);
    let rows = match styles.get(col) {
        Some(Style::Syntax(rows)) => *rows,
        _ => SyntaxStyle::default(),
    };
    let rule = |index: Option<u16>| {
        index
            .and_then(|index| SCOPE_RULES.get(index as usize))
            .map_or("none", |rule| rule.scope)
    };
    format!(
        "{} (color: {}, weight: {})",
        names.join(" "),
        rule(rows.color),
        rule(rows.weight)
    )
}

/// Walk a line through its grammar, styling it where somewhere to put the
/// styles is given. One walk rather than two, so what a line leaves open
/// cannot come out different from what it was painted as. A line the
/// grammar fails on, or one too long to read through it, is left plain, and
/// the next begins where it did.
fn scan(line: &str, state: &LineState, mut styles: Option<&mut Vec<Style>>) -> LineState {
    let plain = |styles: Option<&mut Vec<Style>>| {
        if let Some(styles) = styles {
            push_style(styles, Style::Normal, line);
        }
        state.clone()
    };
    if line.len() > LONGEST_LINE {
        return plain(styles);
    }
    let mut parse = state.parse.clone();
    let mut scopes = state.scopes.clone();
    // The grammars are the ones that expect each line to end in its newline.
    let text = format!("{line}\n");
    let Ok(ops) = parse.parse_line(&text, state.source.syntaxes()) else {
        return plain(styles);
    };
    let mut section = state.section;
    let mut at = 0;
    for (end, op) in ops {
        let run = &text[at..end.min(line.len())];
        note_section(run, scopes.as_slice(), &mut section);
        if let Some(styles) = styles.as_deref_mut() {
            paint_run(styles, run, at == 0, &scopes, section);
        }
        at = end.min(line.len());
        if scopes.apply(&op).is_err() {
            break;
        }
    }
    note_section(&line[at..], scopes.as_slice(), &mut section);
    if let Some(styles) = styles {
        paint_run(styles, &line[at..], at == 0, &scopes, section);
    }
    // A title still open at the end of the line carries its level on.
    let section = section.filter(|_| scopes.as_slice().contains(&scope_named(SECTION_META)));
    LineState {
        parse,
        scopes,
        section,
        source: state.source,
    }
}

/// Remember the level a LaTeX sectioning command's name says, for its title.
fn note_section(run: &str, stack: &[Scope], section: &mut Option<Scope>) {
    let top = stack.last().copied().unwrap_or_default();
    if !run.is_empty()
        && scope_named(SECTION_COMMAND).is_prefix_of(top)
        && run.chars().all(char::is_alphabetic)
    {
        *section = Scope::new(&format!("meta.macro.section.{run}.latex")).ok();
    }
}

/// Style the run `text`, which sits inside `scopes` and starts the line when
/// `line_start` says so, with the passes that name it more closely than its
/// grammar does.
fn paint_run(
    styles: &mut Vec<Style>,
    text: &str,
    line_start: bool,
    scopes: &ScopeStack,
    section: Option<Scope>,
) {
    if text.is_empty() {
        return;
    }
    let stack = scopes.as_slice();
    let top = stack.last().copied().unwrap_or_default();
    // A commit summary's conventional type, the way `git log` readers and
    // the forges pick it out.
    if line_start && scope_named(COMMIT_SUBJECT).is_prefix_of(top) {
        if let Some(end) = conventional_type_end(text) {
            let mut typed = stack.to_vec();
            typed.push(scope_named(COMMIT_TYPE));
            push_style(styles, style_of(&typed), &text[..end]);
            push_style(styles, style_of(stack), &text[end..]);
            return;
        }
    }
    let style = match (leveled(stack, section), worded(text, stack)) {
        (Some(stack), _) | (None, Some(stack)) => style_of(&stack),
        (None, None) => style_of(stack),
    };
    push_style(styles, style, text);
}

/// The stack a LaTeX section's title is styled in.
///
/// Sublime's LaTeX grammar puts every sectioning command in one scope, where
/// the theme colors each level apart as VS Code's grammar names them. So the
/// level a command's name says is remembered for the title after it (see
/// [`note_section`]), and the title is styled as sitting in
/// `meta.macro.section.<level>.latex`.
fn leveled(stack: &[Scope], section: Option<Scope>) -> Option<Vec<Scope>> {
    let level = section?;
    let meta = scope_named(SECTION_META);
    if !stack.contains(&meta) {
        return None;
    }
    Some(
        stack
            .iter()
            .map(|scope| match *scope == meta {
                true => level,
                false => *scope,
            })
            .collect(),
    )
}

/// The stack a word is styled in where [`WORD_SCOPES`] names it more closely
/// than its grammar does.
fn worded(text: &str, stack: &[Scope]) -> Option<Vec<Scope>> {
    let (top, rest) = stack.split_last()?;
    let rule = WORD_SCOPES
        .iter()
        .find(|rule| *top == scope_named(rule.scope) && rule.words.contains(&text))?;
    let mut stack = rest.to_vec();
    stack.push(scope_named(rule.to));
    Some(stack)
}

/// Where a summary's conventional type ends, its colon included: a word, an
/// optional `(scope)`, an optional `!`, then `:`.
fn conventional_type_end(text: &str) -> Option<usize> {
    let word = text
        .find(|c: char| !c.is_ascii_alphabetic())
        .filter(|end| *end > 0)?;
    let mut end = word;
    if text[end..].starts_with('(') {
        end += text[end..].find(')')? + 1;
    }
    if text[end..].starts_with('!') {
        end += 1;
    }
    text[end..].starts_with(':').then_some(end + 1)
}

/// Style every character of `text` as `style`.
fn push_style(styles: &mut Vec<Style>, style: Style, text: &str) {
    styles.extend(std::iter::repeat_n(style, text.chars().count()));
}

/// The style of text inside `stack`: its color from the row of the scope
/// table that sets one and matches the stack most closely, its weight from
/// the closest that sets one, a later row winning a tie, as a later rule
/// does in a VS Code theme. Text no row matches is plain.
fn style_of(stack: &[Scope]) -> Style {
    static CACHE: OnceLock<Mutex<HashMap<Vec<Scope>, Style>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Mutex::default);
    let mut cache = cache.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(style) = cache.get(stack) {
        return *style;
    }
    let rows = SyntaxStyle {
        color: closest_row(stack, |rule| rule.colors()),
        weight: closest_row(stack, |rule| rule.weighs()),
    };
    let style = match rows == SyntaxStyle::default() {
        true => Style::Normal,
        false => Style::Syntax(rows),
    };
    if cache.len() >= STYLE_CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(stack.to_vec(), style);
    style
}

/// The row of the scope table, among those `wanted` keeps, whose selector
/// matches `stack` most closely, a later row winning a tie.
fn closest_row(stack: &[Scope], wanted: impl Fn(&ScopeRule) -> bool) -> Option<u16> {
    let mut best = None;
    for (index, (rule, selector)) in SCOPE_RULES.iter().zip(selectors()).enumerate() {
        if !wanted(rule) {
            continue;
        }
        let Some(power) = selector.as_ref().and_then(|s| s.does_match(stack)) else {
            continue;
        };
        if best.is_none_or(|(_, best_power)| power >= best_power) {
            best = Some((index, power));
        }
    }
    best.map(|(index, _)| index as u16)
}

/// The grammar that claims `extension`. Plain text has a grammar too, which
/// colors nothing and is not worth reading a file through.
fn by_extension(extension: &str) -> Option<Grammar> {
    find(|set| {
        set.find_syntax_by_extension(extension)
            .filter(|syntax| syntax.name != PLAIN_TEXT)
    })
}

/// The first grammar `lookup` finds, in Winter's own set before two-face's.
fn find(
    lookup: impl Fn(&'static SyntaxSet) -> Option<&'static SyntaxReference>,
) -> Option<Grammar> {
    [Source::Own, Source::Bundled]
        .into_iter()
        .find_map(|source| lookup(source.syntaxes()).map(|syntax| Grammar { source, syntax }))
}

/// two-face's grammars, unpacked the first time one is wanted.
fn bundled_syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(two_face::syntax::extra_newlines)
}

/// The grammars Winter carries itself, read the first time one is wanted.
/// One that does not read is left out, as if it were not carried; a test
/// reads them all.
fn own_syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(|| {
        let mut builder = SyntaxSetBuilder::new();
        for grammar in OWN_GRAMMARS {
            if let Ok(definition) = SyntaxDefinition::load_from_str(grammar, true, None) {
                builder.add(definition);
            }
        }
        builder.build()
    })
}

/// The scope table's selectors, parsed once. A row whose selector does not
/// parse matches nothing.
fn selectors() -> &'static [Option<ScopeSelectors>] {
    static SELECTORS: OnceLock<Vec<Option<ScopeSelectors>>> = OnceLock::new();
    SELECTORS.get_or_init(|| {
        SCOPE_RULES
            .iter()
            .map(|rule| ScopeSelectors::from_str(rule.scope).ok())
            .collect()
    })
}

/// The scope `name` stands for. Only ever called with the constants above,
/// which are scopes.
fn scope_named(name: &str) -> Scope {
    Scope::new(name).unwrap_or_default()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
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

    /// Each run of `line`, as the style it is painted in and its text.
    fn runs(path: &str, line: &str) -> Vec<(Style, String)> {
        let grammar = of_path(Path::new(path)).expect("a grammar");
        let (styles, _) = highlight(line, &LineState::start(grammar));
        let mut runs: Vec<(Style, String)> = Vec::new();
        for (c, style) in line.chars().zip(styles) {
            match runs.last_mut() {
                Some((last, text)) if *last == style => text.push(c),
                _ => runs.push((style, c.to_string())),
            }
        }
        runs
    }

    #[test]
    fn test_each_latex_section_level_is_styled_apart() {
        let title = |line: &str| {
            runs("a.tex", line)
                .into_iter()
                .find(|(_, text)| text == "Intro")
                .expect("the title is a run of its own")
        };
        assert_eq!(
            title(r"\section{Intro}"),
            (row("entity.name.section.latex"), "Intro".to_string())
        );
        assert_eq!(
            title(r"\subsection{Intro}").0,
            row("meta.macro.section.subsection.latex entity.name.section.latex")
        );
        assert_eq!(
            title(r"\subsubsection{Intro}").0,
            row("meta.macro.section.subsubsection.latex entity.name.section.latex")
        );
    }

    #[test]
    fn test_latex_math_labels_and_citations_take_their_own_colors() {
        let line = r"$x$ \label{a} \cite{b}";
        let styles: HashMap<String, Style> = runs("a.tex", line)
            .into_iter()
            .map(|(style, text)| (text, style))
            .collect();
        assert_eq!(styles["x"], row("variable.other.math"));
        assert_eq!(styles["a"], row("entity.name.label.latex"));
        assert_eq!(styles["b"], row("constant.other.citation.latex"));
    }

    #[test]
    fn test_a_closer_scope_wins_over_a_broader_one() {
        // `constant.numeric` is more specific than `constant`, and Sublime's
        // `entity.name.struct` is a type, as VS Code's `entity.name.type` is.
        let rust = runs("a.rs", "struct Foo; const N: u8 = 42;");
        assert!(rust.contains(&(row("entity.name.struct"), "Foo".to_string())));
        assert!(rust.contains(&(row("constant.numeric"), "42".to_string())));
    }

    #[test]
    fn test_kdl_is_read_with_winter_s_own_grammar() {
        assert_eq!(
            own_syntaxes().syntaxes().len(),
            OWN_GRAMMARS.len(),
            "every grammar Winter carries reads"
        );
        let styles: HashMap<String, Style> = runs(
            "config.kdl",
            r##"colors { git { modified "#8fb1f5" } } // x"##,
        )
        .into_iter()
        .map(|(style, text)| (text.trim().to_string(), style))
        .collect();
        assert_eq!(styles["colors"], row("entity.name.tag"));
        assert_eq!(styles["modified"], row("entity.name.tag"));
        assert_eq!(styles["#8fb1f5"], row("string"));
        assert_eq!(styles["x"], row("comment"));

        let property = runs("a.kdl", "node key=1 #true");
        assert!(property.contains(&(row("entity.other.attribute-name"), "key".to_string())));
        assert!(property.contains(&(row("constant.numeric"), "1".to_string())));
        assert!(property.contains(&(row("constant.language"), "#true".to_string())));
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
            assert_eq!(summary(line).0, color_row(COMMIT_SUBJECT), "{line}");
        }
    }

    #[test]
    fn test_rust_s_built_in_types_are_types_and_let_is_not() {
        let rust = runs("a.rs", "let x: u32 = 1;");
        assert!(rust.contains(&(row("storage.type"), "let".to_string())));
        assert!(rust.contains(&(row("support.type.primitive"), "u32".to_string())));
    }

    #[test]
    fn test_a_line_too_long_to_read_is_plain_and_leaves_the_next_line_be() {
        let grammar = of_path(Path::new("a.rs")).expect("a grammar");
        let start = LineState::start(grammar);
        let long = format!("/* {}", "x".repeat(LONGEST_LINE));
        let (styles, next) = highlight(&long, &start);
        assert!(styles.iter().all(|style| *style == Style::Normal));
        assert_eq!(styles.len(), long.chars().count());
        assert_eq!(next, start, "the comment it opens is not read");
    }

    #[test]
    fn test_a_latex_title_keeps_its_level_onto_the_next_line() {
        let grammar = of_path(Path::new("a.tex")).expect("a grammar");
        let first = next_state(r"\subsection{A long", &LineState::start(grammar));
        let (styles, after) = highlight("title}", &first);
        assert_eq!(
            styles[0],
            row("meta.macro.section.subsection.latex entity.name.section.latex")
        );
        assert_eq!(after.section, None, "the title closed on this line");
    }

    #[test]
    fn test_a_function_call_is_colored_as_a_function() {
        for (path, line, call) in [
            ("a.py", "obj.method(1)", "method"),
            ("a.js", "foo(1);", "foo"),
            ("a.go", "fmt.Println(x)", "Println"),
        ] {
            let call_style = runs(path, line)
                .into_iter()
                .find(|(_, text)| text == call)
                .map(|(style, _)| color(style));
            assert_eq!(call_style, Some(color_row("variable.function")), "{path}");
        }
    }

    #[test]
    fn test_a_character_is_described_by_its_scopes_and_the_rows_that_paint_it() {
        let grammar = of_path(Path::new("a.rs")).expect("a grammar");
        let said = describe_at("let x = 1;", &LineState::start(grammar), 0);
        assert!(said.starts_with("source.rust storage.type.rust"), "{said}");
        assert!(said.contains("color: storage.type,"), "{said}");
        assert!(said.ends_with("weight: none)"), "{said}");
    }

    #[test]
    fn test_files_are_known_by_name_extension_alias_and_first_line() {
        let name = |path: &str| of_path(Path::new(path)).map(|grammar| grammar.name());
        assert_eq!(name("Makefile"), Some("Makefile"));
        assert_eq!(name("x/COMMIT_EDITMSG"), Some("Git Commit"));
        assert_eq!(name("main.RS"), Some("Rust"));
        assert_eq!(name("lib.h"), Some("C"), "not Objective-C");
        assert_eq!(name("Containerfile"), Some("Dockerfile"));
        assert_eq!(name("notes.txt"), None);
        assert_eq!(
            of_first_line("#!/usr/bin/env python3").map(|g| g.name()),
            Some("Python")
        );
    }
}
