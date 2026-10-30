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
//! here names the run more closely: a LaTeX section's level, a Markdown
//! heading's, a commit summary's conventional type, Rust's built-in types.

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Mutex, OnceLock, PoisonError};

use syntect::highlighting::ScopeSelectors;
use syntect::parsing::{
    ParseState, Scope, ScopeStack, ScopeStackOp, SyntaxDefinition, SyntaxReference, SyntaxSet,
    SyntaxSetBuilder,
};
use winter_render::{ScopeRule, Theme, SCOPE_RULES};

use crate::model::paint::{Style, SyntaxStyle};

use self::words::{scope_words, Language, Run, Word};

mod words;

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
const OWN_GRAMMARS: [&str; 1] = [include_str!("text/grammars/kdl.sublime-syntax")];

/// The scope a commit message's summary sits in.
const COMMIT_SUBJECT: &str = "markup.heading.subject.git.commit";

/// The scopes git's `#` comment lines sit in, in a commit message and in a
/// rebase plan, which the grammars fill with scopes of their own (the
/// branch, each changed file and how it changed, the plan's headings) that
/// the theme colors as code.
const GIT_COMMENTS: [&str; 2] = ["comment.line.git.commit", "comment.line.git.rebase"];

/// The scope a summary's conventional type (`feat:`, `fix(ui)!:`) is given,
/// which the grammar leaves as part of the summary.
const COMMIT_TYPE: &str = "keyword.other.type.git.commit";

/// Words a grammar puts in too coarse a scope, and the scope each is given
/// instead: Sublime's Rust grammar scopes `u32` as it does `let`, where the
/// theme colors a built-in type as a type.
const WORD_SCOPES: &[WordScope] = &[
    WordScope {
        scope: "storage.type.rust",
        to: "support.type.primitive.rust",
        words: &[
            "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "str", "u8",
            "u16", "u32", "u64", "u128", "usize",
        ],
    },
    // Go's booleans, which Sublime scopes as it does `nil`.
    WordScope {
        scope: "constant.language.go",
        to: "constant.language.boolean.go",
        words: &["false", "true"],
    },
    // Python's `self` among a function's parameters, which VS Code colors
    // as it does `self` anywhere else.
    WordScope {
        scope: "variable.parameter.python",
        to: "variable.parameter.function.language.special.self.python",
        words: &["self"],
    },
];

/// The scopes a grammar names a definition with, rather than a use: what an
/// outline lists. A call is `variable.function`, a definition
/// `entity.name.function`, in the Sublime grammars and VS Code's alike.
const DEFINITIONS: [&str; 15] = [
    "entity.name.class",
    "entity.name.constant",
    "entity.name.enum",
    "entity.name.function",
    "entity.name.impl",
    "entity.name.interface",
    "entity.name.macro",
    "entity.name.module",
    "entity.name.namespace",
    "entity.name.section",
    "entity.name.struct",
    "entity.name.table",
    "entity.name.trait",
    "entity.name.type",
    "entity.name.union",
];

/// The scope a whole LaTeX sectioning command sits in, whatever its level.
const SECTION_META: &str = "meta.section.latex";

/// The scope of the `#`s that open a Markdown heading, whose count is its
/// level.
const HEADING_MARK: &str = "punctuation.definition.heading.begin.markdown";

/// The scope a Markdown heading sits in where Sublime does not number it,
/// as it numbers only the first two levels.
const HEADING_META: &str = "markup.heading.markdown";

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
    /// inside, for a title that runs past its command's line. While a line
    /// is walked it also holds the level of the Markdown heading it is.
    section: Option<Scope>,
    /// The set the grammar is one of.
    source: Source,
    /// The language whose unscoped words are named by what surrounds them
    /// (see [`words`]), for a grammar that leaves them so.
    words: Option<Language>,
}

/// One run of a line as the grammar scoped it, kept until the whole line is
/// walked, so a word can be told by what stands after it as well as before.
struct LineRun {
    /// Its bytes in the line.
    range: std::ops::Range<usize>,
    /// The scopes it sits inside.
    scopes: ScopeStack,
    /// The section level it is under (see [`LineState::section`]).
    section: Option<Scope>,
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
            words: Language::of(&grammar.syntax.scope.build_string()),
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

/// Whether `line`, beginning where `state` says, names something it defines:
/// a function, a type, a module, a section. What an outline lists.
pub fn defines(line: &str, state: &LineState) -> bool {
    if line.len() > LONGEST_LINE {
        return false;
    }
    let mut parse = state.parse.clone();
    let Ok(ops) = parse.parse_line(&format!("{line}\n"), state.source.syntaxes()) else {
        return false;
    };
    ops.iter().any(|(_, op)| match op {
        ScopeStackOp::Push(scope) => DEFINITIONS
            .iter()
            .any(|definition| scope_named(definition).is_prefix_of(*scope)),
        _ => false,
    })
}

/// Where the line after `line` begins, without working out its colors: the
/// same walk as [`highlight`], for the lines between the top of a file and
/// the part of it being looked at.
pub fn next_state(line: &str, state: &LineState) -> LineState {
    scan(line, state, None)
}

/// Inspect token details under `(row, col)` in `line` with grammar `grammar_name`.
pub fn inspect_token(
    line: &str,
    state: &LineState,
    col: usize,
    row: usize,
    grammar_name: &str,
) -> crate::model::editor::TokenInspectData {
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

    let ch = line.chars().nth(col);
    let character = crate::model::editor::TokenInspectData::format_char_or_newline(ch);
    let position = crate::model::editor::TokenInspectData::format_position(row, col);

    let (styles, _) = highlight(line, state);
    let rows = match styles.get(col) {
        Some(Style::Syntax(rows)) => *rows,
        _ => SyntaxStyle::default(),
    };

    let color_rule_obj = rows.color.and_then(|idx| SCOPE_RULES.get(idx as usize));
    let weight_rule_obj = rows.weight.and_then(|idx| SCOPE_RULES.get(idx as usize));

    let color_rule = color_rule_obj.map(|r| r.scope.to_string());
    let foreground_hex = rows
        .color
        .and_then(|idx| Theme::dark().syntax.at(idx as usize))
        .map(|color| color.to_hex());

    let style = match (
        weight_rule_obj.map(|r| (r.bold, r.italic)),
        color_rule_obj.map(|r| (r.bold, r.italic)),
    ) {
        (Some((true, true)), _) | (_, Some((true, true))) => Some("bold italic".to_string()),
        (Some((true, false)), _) | (_, Some((true, false))) => Some("bold".to_string()),
        (Some((false, true)), _) | (_, Some((false, true))) => Some("italic".to_string()),
        _ => None,
    };

    crate::model::editor::TokenInspectData {
        character,
        color_rule,
        foreground_hex,
        language: grammar_name.to_string(),
        position,
        scopes: names,
        style,
    }
}

/// Walk a line through its grammar, styling it where somewhere to put the
/// styles is given. One walk rather than two, so what a line leaves open
/// cannot come out different from what it was painted as. A line the
/// grammar fails on, or one too long to read through it, is left plain, and
/// the next begins where it did.
fn scan(line: &str, state: &LineState, styles: Option<&mut Vec<Style>>) -> LineState {
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
    // The runs are painted once the line is walked, when what follows a
    // word is known too.
    let mut runs = Vec::new();
    for (end, op) in ops {
        let run = &text[at..end.min(line.len())];
        note_section(run, scopes.as_slice(), &mut section);
        if styles.is_some() {
            runs.push(LineRun { range: at..end.min(line.len()), scopes: scopes.clone(), section });
        }
        at = end.min(line.len());
        if scopes.apply(&op).is_err() {
            break;
        }
    }
    note_section(&line[at..], scopes.as_slice(), &mut section);
    if let Some(styles) = styles {
        runs.push(LineRun { range: at..line.len(), scopes: scopes.clone(), section });
        paint_runs(styles, line, &runs, state.words);
    }
    // A title still open at the end of the line carries its level on.
    let section = section.filter(|_| scopes.as_slice().contains(&scope_named(SECTION_META)));
    LineState {
        parse,
        scopes,
        section,
        source: state.source,
        words: state.words,
    }
}

/// Paint the runs of `line`, a word the grammar left plain in a language
/// `words` names in the scope what surrounds it gives it (see [`words`]).
fn paint_runs(styles: &mut Vec<Style>, line: &str, runs: &[LineRun], words: Option<Language>) {
    let named: Vec<Word> = match words {
        Some(language) => {
            let views: Vec<Run> = runs
                .iter()
                .map(|run| Run {
                    plain: closest_row(run.scopes.as_slice(), |rule| rule.colors()).is_none(),
                    range: run.range.clone(),
                    scopes: run.scopes.as_slice(),
                })
                .collect();
            scope_words(language, line, &views)
        }
        None => Vec::new(),
    };
    for run in runs {
        let mut at = run.range.start;
        for word in named.iter().filter(|word| run.range.contains(&word.range.start)) {
            paint_run(styles, &line[at..word.range.start], at == 0, &run.scopes, run.section);
            let mut stack = run.scopes.as_slice().to_vec();
            stack.push(scope_named(word.scope));
            push_style(styles, style_of(&stack), &line[word.range.clone()]);
            at = word.range.end;
        }
        paint_run(styles, &line[at..run.range.end], at == 0, &run.scopes, run.section);
    }
}

/// Remember the level a LaTeX sectioning command's name says, starred or
/// not, for its title, and the level a Markdown heading's `#`s say.
fn note_section(run: &str, stack: &[Scope], section: &mut Option<Scope>) {
    let top = stack.last().copied().unwrap_or_default();
    let name = run.strip_suffix('*').unwrap_or(run);
    if !name.is_empty()
        && scope_named(SECTION_COMMAND).is_prefix_of(top)
        && name.chars().all(char::is_alphabetic)
    {
        *section = Scope::new(&format!("meta.macro.section.{name}.latex")).ok();
    } else if !run.is_empty()
        && scope_named(HEADING_MARK).is_prefix_of(top)
        && run.chars().all(|c| c == '#')
    {
        *section = Scope::new(&format!("markup.heading.{}.markdown", run.len())).ok();
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
    // Git's comments in a commit message or a rebase plan are its notes on
    // what is being done, not part of it: all of each line one comment,
    // whatever the grammar picks out inside it.
    let git_comments = GIT_COMMENTS.map(scope_named);
    if let Some(comment) = stack.iter().position(|scope| git_comments.contains(scope)) {
        push_style(styles, style_of(&stack[..=comment]), text);
        return;
    }
    let style = match (leveled(stack, section), worded(text, stack)) {
        (Some(stack), _) | (None, Some(stack)) => style_of(&stack),
        (None, None) => style_of(stack),
    };
    push_style(styles, style, text);
}

/// The stack a LaTeX section's title, or a Markdown heading, is styled in.
///
/// Sublime's LaTeX grammar puts every sectioning command in one scope, and
/// its Markdown grammar every heading past the second level, where the theme
/// colors each level apart as VS Code's grammars name them. So the level a
/// command's name or a heading's `#`s say is remembered for what follows (see
/// [`note_section`]), and it is styled as sitting in
/// `meta.macro.section.<level>.latex` or `markup.heading.<level>.markdown`.
fn leveled(stack: &[Scope], section: Option<Scope>) -> Option<Vec<Scope>> {
    let level = section?;
    let metas = [scope_named(SECTION_META), scope_named(HEADING_META)];
    if !stack.iter().any(|scope| metas.contains(scope)) {
        return None;
    }
    Some(
        stack
            .iter()
            .map(|scope| match metas.contains(scope) {
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

/// The style text inside the scopes `names` takes, outermost first: what a
/// view drawing a document of its own (a rendered Markdown preview) colors
/// a heading or a bold run with, as the source is colored.
pub fn scoped(names: &[&str]) -> Style {
    let stack: Vec<Scope> = names.iter().map(|name| scope_named(name)).collect();
    style_of(&stack)
}

/// The grammar a code fence's language token names (`rust`, `py`, `sh`).
pub fn of_token(token: &str) -> Option<Grammar> {
    find(|set| {
        set.find_syntax_by_token(token)
            .filter(|syntax| syntax.name != PLAIN_TEXT)
    })
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
        ..SyntaxStyle::default()
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

/// The scope `name` stands for, or the empty scope for a name that is not
/// one.
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
        assert_eq!(
            title(r"\subsubsection*{Intro}").0,
            row("meta.macro.section.subsubsection.latex entity.name.section.latex"),
            "a starred command keeps its level"
        );
    }

    #[test]
    fn test_a_definition_is_told_from_a_use() {
        let defines_in = |path: &str, line: &str| {
            let grammar = of_path(Path::new(path)).expect("a grammar");
            defines(line, &LineState::start(grammar))
        };
        assert!(defines_in("a.rs", "pub fn new() -> Self {"));
        assert!(defines_in("a.rs", "struct Foo;"));
        assert!(defines_in("a.rs", "impl Foo {"));
        assert!(!defines_in("a.rs", "let x = new();"), "a call");
        assert!(defines_in("a.py", "def run(self):"));
        assert!(defines_in("a.py", "class A(B):"));
        assert!(!defines_in("a.py", "run(x)"));
    }

    #[test]
    fn test_each_markdown_heading_level_is_styled_apart() {
        // Sublime numbers only the first two levels; past them the `#`s say it.
        let heading = |line: &str| runs("a.md", line);
        assert!(heading("### Three").contains(&(row("markup.heading.3"), " Three".to_string())));
        let four = heading("#### Four");
        assert_eq!(
            four[0],
            (
                row("markup.heading.4 punctuation.definition.heading"),
                "####".to_string()
            )
        );
        assert_eq!(four[1], (row("markup.heading.4"), " Four".to_string()));
        assert_eq!(
            heading("## Two")[0].0,
            row("punctuation.definition.heading"),
            "the first levels keep their own marks"
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
        assert_eq!(styles["#8fb1f5"], row("string.quoted"));
        assert_eq!(styles["x"], row("comment"));

        let property = runs("a.kdl", "node key=1 #true");
        assert!(property.contains(&(row("entity.other.attribute-name"), "key".to_string())));
        assert!(property.contains(&(row("constant.numeric"), "1".to_string())));
        assert!(property.contains(&(row("constant.language.boolean"), "#true".to_string())));
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

    /// The styles of the `#` lines of `lines`, in a file at `path`.
    fn comment_styles(path: &str, lines: &[&str]) -> Vec<Vec<Style>> {
        let grammar = of_path(Path::new(path)).expect("a grammar");
        let mut state = LineState::start(grammar);
        let mut comments = Vec::new();
        for line in lines {
            let (styles, next) = highlight(line, &state);
            state = next;
            if line.starts_with('#') {
                comments.push(styles);
            }
        }
        comments
    }

    #[test]
    fn test_git_s_comments_are_one_color_whatever_they_name() {
        // Regression: the grammars scope the branch, each changed file, the
        // kind of change, and a rebase plan's headings inside git's comment
        // lines, and the theme colored them all as code.
        let commit = ["fix: x", "", "# On branch main", "# Changes to be committed:", "#\tmodified:   src/a.rs", "#\tnew file:   b.rs"];
        let rebase = ["pick 1a2b3c4 fix: x", "", "# Rebase 1a2b..3c4d onto 1a2b (1 command)", "#", "# Commands:", "# p, pick <commit> = use commit"];
        for (path, lines) in [("x/COMMIT_EDITMSG", &commit), ("x/git-rebase-todo", &rebase)] {
            let comments = comment_styles(path, lines);
            let comment = comments[0][0];
            for styles in &comments {
                assert!(styles.iter().all(|style| *style == comment), "{path}: every character of every comment line alike");
            }
            assert_ne!(comment, runs(path, lines[0])[0].0, "{path}: and not the first line's");
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
    fn test_inspect_token_reports_character_scopes_and_theme_color() {
        let grammar = of_path(Path::new("a.rs")).expect("a grammar");
        let data = inspect_token("let x = 1;", &LineState::start(grammar), 0, 0, grammar.name());
        assert_eq!(data.character, "'l' (U+006C)");
        assert_eq!(data.position, "ln 1, col 1");
        assert_eq!(data.language, "Rust");
        assert!(data.scopes.contains(&"source.rust".to_string()));
        assert!(data.scopes.contains(&"storage.type.rust".to_string()));
        assert_eq!(data.color_rule.as_deref(), Some("storage.type"));
        assert!(data.foreground_hex.is_some());
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
