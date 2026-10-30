//! Markup markers hidden, as magic-vscode hides Markdown's: the `**` of
//! `**bold**`, the `_` of `_italic_`, the backticks of `` `code` ``, the `$`
//! of `$math$`, a heading's `#`s, and a link's `[`, `](url)`, the text
//! between drawn bold or italic as the markers say. The same goes for
//! LaTeX's `\textbf{..}`, `\emph{..}`, and their kin (and `{\bf ..}`), Org's
//! `*bold*`, `/italic/`, `=verbatim=`, and `~code~`, reStructuredText's
//! `**bold**`, `*italic*`, and ` ``code`` `, AsciiDoc's and Typst's `*bold*`
//! and `_italic_`, and the Markdown in a `///`, `//!`, or `/** .. */` doc
//! comment of a language commented with `//`.
//!
//! Hidden characters stay in the text: the cursor walks over them, and they
//! take no cell on screen. Whether they hide is the `editor-markup-markers`
//! setting: never, everywhere but where the cursor or the selection touches
//! a span (a heading's on its line), or always.

use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use winter_render::SCOPE_RULES;

use crate::model::paint::{MarkupMarkers, Style, SyntaxStyle, TAB_WIDTH};

use super::comment::{self, Marker};
use super::{cell_width, TextEditor};

// ========================================================================
// Constants
// ========================================================================

/// The scopes whose rows draw text bold, and italic.
const BOLD_SCOPE: &str = "markup.bold";
const ITALIC_SCOPE: &str = "markup.italic";

/// The deepest a span is looked for inside another.
const MAX_DEPTH: usize = 8;

/// LaTeX's commands that set their argument in a style, and the style: the
/// argument is the text, and `\cmd{` and its `}` hide.
const LATEX_COMMANDS: &[(&str, Look)] = &[
    ("textbf", Look::BOLD),
    ("textit", Look::ITALIC),
    ("emph", Look::ITALIC),
    ("textsl", Look::ITALIC),
    ("texttt", Look::PLAIN),
    ("textsf", Look::PLAIN),
    ("textsc", Look::PLAIN),
    ("textrm", Look::PLAIN),
    ("textmd", Look::PLAIN),
    ("textup", Look::PLAIN),
    ("underline", Look::UNDERLINE),
    ("uline", Look::UNDERLINE),
    ("sout", Look::STRIKE),
];

/// LaTeX's declarations that set the rest of their group in a style, as
/// `{\bf ..}`: the `{\bf ` and the `}` hide.
const LATEX_DECLARATIONS: &[(&str, Look)] = &[
    ("bf", Look::BOLD),
    ("bfseries", Look::BOLD),
    ("it", Look::ITALIC),
    ("itshape", Look::ITALIC),
    ("em", Look::ITALIC),
    ("sl", Look::ITALIC),
    ("slshape", Look::ITALIC),
    ("tt", Look::PLAIN),
    ("ttfamily", Look::PLAIN),
    ("sc", Look::PLAIN),
    ("scshape", Look::PLAIN),
];

// ========================================================================
// Data Structures
// ========================================================================

/// The markup a file is written in, for its markers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Markup {
    AsciiDoc,
    /// The doc comments of a language commented with `//`, read as Markdown.
    DocComments,
    Latex,
    Markdown,
    Org,
    Rst,
    Typst,
}

/// How the text a span holds is drawn, over the colors it already has.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Look {
    pub(super) bold: bool,
    pub(super) italic: bool,
    pub(super) strike: bool,
    pub(super) underline: bool,
}

/// One span whose markers hide, as character columns of its line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Mark {
    /// The spans inside its text.
    pub(super) children: Vec<Mark>,
    /// Its whole extent, markers and all, the end one past it: the cursor
    /// anywhere from its start to its end shows it.
    pub(super) end: usize,
    /// The runs that hide.
    pub(super) hidden: Vec<(usize, usize)>,
    /// Its text, between the markers.
    pub(super) inner: (usize, usize),
    pub(super) start: usize,
    /// How its text is drawn.
    pub(super) look: Look,
}

/// A pair of markers around a span of text.
#[derive(Clone, Copy, Debug)]
struct Pair {
    close: &'static str,
    /// Whether it opens and closes only at a word's edge, as emphasis does:
    /// an HTML tag needs none.
    edges: bool,
    open: &'static str,
    /// Whether what it holds is taken as written, no span looked for in it:
    /// code and math.
    verbatim: bool,
    look: Look,
}

/// What a line is read for: the columns of it scanned for spans, or
/// nothing for a line of code or math.
pub(super) type Scan = Option<(usize, usize)>;

/// The spans found in a document, kept while its text stands: what each
/// line is read for, and the spans of the lines asked about, by the text
/// they were found in.
#[derive(Debug, Default)]
pub(super) struct Cache {
    /// The document, its revision, and its line count the scans are of.
    key: Option<(usize, usize, usize)>,
    lines: HashMap<usize, (String, Rc<Vec<Mark>>)>,
    markup: Option<Markup>,
    scans: Vec<Scan>,
}

/// What shows of one line: which of its characters hide, and how each is
/// drawn.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct Concealed {
    pub(super) hidden: Vec<bool>,
    pub(super) looks: Vec<Look>,
}

// ========================================================================
// Look
// ========================================================================

impl Look {
    /// As the text was: code, math.
    const PLAIN: Look = Look {
        bold: false,
        italic: false,
        strike: false,
        underline: false,
    };
    const BOLD: Look = Look { bold: true, ..Look::PLAIN };
    const ITALIC: Look = Look { italic: true, ..Look::PLAIN };
    const BOLD_ITALIC: Look = Look { bold: true, italic: true, ..Look::PLAIN };
    const STRIKE: Look = Look { strike: true, ..Look::PLAIN };
    const UNDERLINE: Look = Look { underline: true, ..Look::PLAIN };

    /// This look with `other` laid over it: bold inside italic is both.
    fn with(self, other: Look) -> Look {
        Look {
            bold: self.bold || other.bold,
            italic: self.italic || other.italic,
            strike: self.strike || other.strike,
            underline: self.underline || other.underline,
        }
    }

    /// The theme's scope row that draws text as heavy and as slanted as
    /// this, if any does.
    pub(super) fn row(self) -> Option<u16> {
        let by_scope = |scope: &str| SCOPE_RULES.iter().position(|rule| rule.scope == scope);
        let at = match (self.bold, self.italic) {
            (false, false) => None,
            (true, false) => by_scope(BOLD_SCOPE),
            (false, true) => by_scope(ITALIC_SCOPE),
            (true, true) => SCOPE_RULES
                .iter()
                .position(|rule| rule.weighs() && rule.bold && rule.italic)
                .or_else(|| by_scope(BOLD_SCOPE)),
        };
        at.and_then(|at| u16::try_from(at).ok())
    }
}

// ========================================================================
// Concealed
// ========================================================================

impl Concealed {
    /// Whether character `col` hides.
    pub(super) fn hides(&self, col: usize) -> bool {
        self.hidden.get(col).copied().unwrap_or(false)
    }

    /// `style` with the look character `col` is drawn with laid over it:
    /// bold or italic through the theme's rows for them, a line under it or
    /// through it, its color kept.
    pub(super) fn styled(&self, col: usize, style: Style) -> Style {
        let look = self.looks.get(col).copied().unwrap_or_default();
        if look == Look::default() {
            return style;
        }
        let syntax = match style {
            Style::Syntax(syntax) => syntax,
            Style::Normal => SyntaxStyle::default(),
            other => return other,
        };
        Style::Syntax(SyntaxStyle {
            strike: syntax.strike || look.strike,
            underline: syntax.underline || look.underline,
            weight: look.row().or(syntax.weight),
            ..syntax
        })
    }
}

// ========================================================================
// TextEditor: hidden markers
// ========================================================================

impl TextEditor {
    /// Hide markup's markers as `markers` says, from the next paint.
    pub(super) fn take_markup_markers(&mut self, markers: MarkupMarkers) {
        self.markup_markers = markers;
    }

    /// What shows of line `index`: `None` where nothing of it hides.
    pub(super) fn concealed(&self, index: usize) -> Option<Concealed> {
        if self.markup_markers == MarkupMarkers::Visible || self.doc().unread {
            return None;
        }
        let marks = self.marks_of(index)?;
        if marks.is_empty() {
            return None;
        }
        let length = self.buffer().line_len_of(index);
        let (hidden, looks) = match self.markup_markers {
            MarkupMarkers::ShowOnCursor => {
                let cursor = (self.buffer().row(), self.buffer().col());
                let selected = self.selected_on(index, length);
                apply(&marks, length, &|mark| {
                    let touched = cursor.0 == index && (mark.start..=mark.end).contains(&cursor.1);
                    let overlaps = selected.is_some_and(|(from, to)| from < mark.end && to > mark.start);
                    touched || overlaps
                })
            }
            _ => apply(&marks, length, &|_| false),
        };
        hidden.iter().any(|hide| *hide).then_some(Concealed { hidden, looks })
    }

    /// The cells each character of line `index` takes on screen: none for
    /// one that hides.
    pub(super) fn char_widths(&self, index: usize) -> Vec<usize> {
        let line = self.line_of(index);
        let concealed = self.concealed(index);
        line.chars()
            .enumerate()
            .map(|(col, c)| match concealed.as_ref().is_some_and(|shown| shown.hides(col)) {
                true => 0,
                false => cell_width(c),
            })
            .collect()
    }

    /// The cells the characters of line `index` before `col` take as a row
    /// is painted: a tab opened out, one for any other, none for one that
    /// hides.
    pub(super) fn painted_cells_before(&self, index: usize, col: usize) -> usize {
        let concealed = self.concealed(index);
        self.line_of(index)
            .chars()
            .take(col)
            .enumerate()
            .map(|(at, c)| match (concealed.as_ref().is_some_and(|shown| shown.hides(at)), c) {
                (true, _) => 0,
                (false, '\t') => TAB_WIDTH,
                (false, _) => 1,
            })
            .sum()
    }

    /// The columns of line `index` (`length` long) the selection covers.
    fn selected_on(&self, index: usize, length: usize) -> Option<(usize, usize)> {
        let selection = self.selection()?;
        if index < selection.start.0 || index > selection.end.0 {
            return None;
        }
        if selection.linewise {
            return Some((0, length.max(1)));
        }
        let from = if index == selection.start.0 { selection.start.1 } else { 0 };
        let to = if index == selection.end.0 { selection.end.1 + 1 } else { length.max(1) };
        Some((from, to))
    }

    /// The spans of line `index`, found once for its text.
    fn marks_of(&self, index: usize) -> Option<Rc<Vec<Mark>>> {
        let mut cache = self.conceal_cache.borrow_mut();
        let key = (self.at, self.buffer().revision(), self.buffer().lines().len());
        if cache.key != Some(key) {
            let language = self.language();
            let slashes = self.doc().grammar.and_then(|grammar| comment::marker_of(grammar.name())) == Some(Marker::Line("//"));
            let markup = markup_of(&self.doc().path, &language, slashes);
            *cache = Cache {
                key: Some(key),
                lines: HashMap::new(),
                markup,
                scans: markup.map(|markup| scans(markup, self.buffer().lines())).unwrap_or_default(),
            };
        }
        let markup = cache.markup?;
        let scan = (*cache.scans.get(index)?)?;
        let line = self.buffer().lines().get(index)?;
        if let Some((text, marks)) = cache.lines.get(&index) {
            if text == line {
                return Some(Rc::clone(marks));
            }
        }
        let found = Rc::new(marks(markup, line, scan));
        cache.lines.insert(index, (line.clone(), Rc::clone(&found)));
        Some(found)
    }
}

// ========================================================================
// Functions: which markup
// ========================================================================

/// The markup `path` is written in, for one whose markers hide; `language`
/// is the editor's name for it, and `slashes` whether its comments open
/// with `//`.
pub(super) fn markup_of(path: &Path, language: &str, slashes: bool) -> Option<Markup> {
    let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or_default().to_ascii_lowercase();
    match (language, ext.as_str()) {
        ("markdown", _) | (_, "md" | "markdown" | "mdx") => Some(Markup::Markdown),
        ("latex", _) | (_, "tex" | "ltx") => Some(Markup::Latex),
        (_, "org") => Some(Markup::Org),
        (_, "rst") => Some(Markup::Rst),
        (_, "adoc" | "asciidoc") => Some(Markup::AsciiDoc),
        (_, "typ") => Some(Markup::Typst),
        _ if slashes => Some(Markup::DocComments),
        _ => None,
    }
}

/// What each of `lines` is read for, by what the lines above it opened: a
/// fenced code block or a `$$` block skipped in Markdown, a comment's text
/// alone in LaTeX, a doc comment's text alone in code.
pub(super) fn scans(markup: Markup, lines: &[String]) -> Vec<Scan> {
    let mut out = Vec::with_capacity(lines.len());
    let mut fenced = false;
    let mut math = false;
    let mut doc = false;
    for line in lines {
        let length = line.chars().count();
        let trimmed = line.trim_start();
        let scan = match markup {
            Markup::Markdown => {
                if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                    fenced = !fenced;
                    None
                } else if !fenced && trimmed.trim_end() == "$$" {
                    math = !math;
                    None
                } else if fenced || math {
                    None
                } else {
                    Some((0, length))
                }
            }
            Markup::Org => {
                let lower = trimmed.to_ascii_lowercase();
                if lower.starts_with("#+begin_src") || lower.starts_with("#+begin_example") {
                    fenced = true;
                    None
                } else if lower.starts_with("#+end_src") || lower.starts_with("#+end_example") {
                    fenced = false;
                    None
                } else if fenced || trimmed.starts_with("#+") {
                    None
                } else {
                    Some((0, length))
                }
            }
            Markup::AsciiDoc => {
                if trimmed.starts_with("----") || trimmed.starts_with("....") {
                    fenced = !fenced;
                    None
                } else if fenced {
                    None
                } else {
                    Some((0, length))
                }
            }
            Markup::Rst | Markup::Typst => {
                if trimmed.starts_with("```") {
                    fenced = !fenced;
                    None
                } else if fenced {
                    None
                } else {
                    Some((0, length))
                }
            }
            Markup::Latex => {
                let lower = trimmed.to_ascii_lowercase();
                if lower.starts_with("\\begin{verbatim}") || lower.starts_with("\\begin{lstlisting}") || lower.starts_with("\\begin{minted}") {
                    fenced = true;
                    None
                } else if lower.starts_with("\\end{verbatim}") || lower.starts_with("\\end{lstlisting}") || lower.starts_with("\\end{minted}") {
                    fenced = false;
                    None
                } else if fenced {
                    None
                } else {
                    Some((0, latex_code_end(line)))
                }
            }
            Markup::DocComments => doc_scan(line, &mut doc),
        };
        out.push(scan);
    }
    out
}

/// The part of a line of code a doc comment's text takes, and whether a
/// `/** .. */` comment runs on past it.
fn doc_scan(line: &str, open: &mut bool) -> Scan {
    let chars: Vec<char> = line.chars().collect();
    let text: String = chars.iter().collect();
    if !*open {
        let lead = chars.iter().take_while(|c| c.is_whitespace()).count();
        let rest: String = chars[lead..].iter().collect();
        if (rest.starts_with("///") && !rest.starts_with("////")) || rest.starts_with("//!") {
            return Some((lead + 3, chars.len()));
        }
        let opens = text.find("/**").filter(|at| !text[*at..].starts_with("/**/"))?;
        let from = text[..opens].chars().count() + 3;
        *open = true;
        return Some(doc_until_close(&chars, from, open));
    }
    Some(doc_until_close(&chars, 0, open))
}

/// From `from` to where a `/** .. */` comment closes on this line, or the
/// line's end where it runs on.
fn doc_until_close(chars: &[char], from: usize, open: &mut bool) -> (usize, usize) {
    for at in from..chars.len().saturating_sub(1) {
        if chars[at] == '*' && chars[at + 1] == '/' {
            *open = false;
            return (from, at);
        }
    }
    (from, chars.len())
}

/// Where a line of LaTeX's comment starts: its first `%` not escaped.
fn latex_code_end(line: &str) -> usize {
    let chars: Vec<char> = line.chars().collect();
    let mut at = 0;
    while at < chars.len() {
        match chars[at] {
            '\\' => at += 2,
            '%' => return at,
            _ => at += 1,
        }
    }
    chars.len()
}

// ========================================================================
// Functions: the spans of a line
// ========================================================================

/// The spans of `line` within `scan` whose markers hide.
pub(super) fn marks(markup: Markup, line: &str, scan: (usize, usize)) -> Vec<Mark> {
    let chars: Vec<char> = line.chars().collect();
    let (from, to) = (scan.0.min(chars.len()), scan.1.min(chars.len()));
    let mut found = Vec::new();
    let mut from = from;
    if matches!(markup, Markup::Markdown) {
        if let Some(heading) = heading(&chars, from, to) {
            from = heading.inner.0;
            found.push(heading);
        }
    }
    match markup {
        Markup::Latex => found.extend(latex_spans(&chars, from, to, 0)),
        _ => found.extend(spans(&chars, from, to, pairs(markup), markup, 0)),
    }
    found
}

/// A Markdown heading's `#`s and the space after them.
fn heading(chars: &[char], from: usize, to: usize) -> Option<Mark> {
    let hashes = chars[from..to].iter().take_while(|c| **c == '#').count();
    let after = chars.get(from + hashes);
    if !(1..=6).contains(&hashes) || after != Some(&' ') || from != 0 {
        return None;
    }
    let text = from + hashes + 1;
    Some(Mark {
        children: Vec::new(),
        end: to,
        hidden: vec![(from, text)],
        inner: (text, to),
        start: from,
        look: Look::PLAIN,
    })
}

/// The marker pairs of a markup, the longest first wherever two start
/// alike.
fn pairs(markup: Markup) -> &'static [Pair] {
    const fn pair(open: &'static str, close: &'static str, look: Look, verbatim: bool) -> Pair {
        Pair {
            close,
            edges: true,
            open,
            verbatim,
            look,
        }
    }
    const fn tag(open: &'static str, close: &'static str, look: Look) -> Pair {
        Pair {
            edges: false,
            ..pair(open, close, look, false)
        }
    }
    const MARKDOWN: &[Pair] = &[
        tag("<u>", "</u>", Look::UNDERLINE),
        tag("<ins>", "</ins>", Look::UNDERLINE),
        tag("<del>", "</del>", Look::STRIKE),
        tag("<s>", "</s>", Look::STRIKE),
        pair("```", "```", Look::PLAIN, true),
        pair("``", "``", Look::PLAIN, true),
        pair("`", "`", Look::PLAIN, true),
        pair("$$", "$$", Look::PLAIN, true),
        pair("$", "$", Look::PLAIN, true),
        pair("***", "***", Look::BOLD_ITALIC, false),
        pair("**", "**", Look::BOLD, false),
        pair("__", "__", Look::BOLD, false),
        pair("~~", "~~", Look::STRIKE, false),
        pair("*", "*", Look::ITALIC, false),
        pair("_", "_", Look::ITALIC, false),
    ];
    const ORG: &[Pair] = &[
        pair("=", "=", Look::PLAIN, true),
        pair("~", "~", Look::PLAIN, true),
        pair("*", "*", Look::BOLD, false),
        pair("/", "/", Look::ITALIC, false),
        pair("_", "_", Look::UNDERLINE, false),
        pair("+", "+", Look::STRIKE, false),
    ];
    const RST: &[Pair] = &[
        pair("``", "``", Look::PLAIN, true),
        pair("**", "**", Look::BOLD, false),
        pair("*", "*", Look::ITALIC, false),
    ];
    const ASCIIDOC: &[Pair] = &[
        pair("``", "``", Look::PLAIN, true),
        pair("`", "`", Look::PLAIN, true),
        pair("**", "**", Look::BOLD, false),
        pair("__", "__", Look::ITALIC, false),
        pair("*", "*", Look::BOLD, false),
        pair("_", "_", Look::ITALIC, false),
    ];
    const TYPST: &[Pair] = &[
        pair("`", "`", Look::PLAIN, true),
        pair("$", "$", Look::PLAIN, true),
        pair("*", "*", Look::BOLD, false),
        pair("_", "_", Look::ITALIC, false),
    ];
    match markup {
        Markup::Markdown | Markup::DocComments => MARKDOWN,
        Markup::Org => ORG,
        Markup::Rst => RST,
        Markup::AsciiDoc => ASCIIDOC,
        Markup::Typst => TYPST,
        Markup::Latex => &[],
    }
}

/// The spans from `from` up to `to`, each with those inside it.
fn spans(chars: &[char], from: usize, to: usize, pairs: &[Pair], markup: Markup, depth: usize) -> Vec<Mark> {
    let mut found = Vec::new();
    let mut at = from;
    while at < to {
        if chars[at] == '\\' {
            at += 2;
            continue;
        }
        match span_at(chars, at, to, pairs, markup, depth) {
            Some(mark) => {
                at = mark.end;
                found.push(mark);
            }
            None => at += 1,
        }
    }
    found
}

/// The span opening at `at`, if one does and closes before `to`.
fn span_at(chars: &[char], at: usize, to: usize, pairs: &[Pair], markup: Markup, depth: usize) -> Option<Mark> {
    if depth > MAX_DEPTH {
        return None;
    }
    if matches!(markup, Markup::Markdown | Markup::DocComments) {
        if let Some(link) = link_at(chars, at, to, pairs, markup, depth) {
            return Some(link);
        }
    }
    pairs.iter().find_map(|pair| pair_at(chars, at, to, *pair, pairs, markup, depth))
}

/// The span `pair` opens at `at`: its opener with a word's edge before it
/// and text after, and its closer the first after it with text before it
/// and a word's edge after, past any span inside.
fn pair_at(
    chars: &[char],
    at: usize,
    to: usize,
    pair: Pair,
    pairs: &[Pair],
    markup: Markup,
    depth: usize,
) -> Option<Mark> {
    let open: Vec<char> = pair.open.chars().collect();
    let close: Vec<char> = pair.close.chars().collect();
    if !starts_with(chars, at, to, &open) || (pair.edges && !edge_before(chars, at, open[0])) {
        return None;
    }
    let text = at + open.len();
    let first = *chars.get(text).filter(|_| text < to)?;
    if pair.edges && (first.is_whitespace() || first == open[0]) {
        return None;
    }
    let mut end = text;
    while end < to {
        if !pair.verbatim && chars[end] == '\\' {
            end += 2;
            continue;
        }
        let edged = !pair.edges
            || (!chars[end - 1].is_whitespace()
                && edge_after(chars, end + close.len(), to, close[0])
                && (pair.verbatim || chars[end - 1] != close[0]));
        let closes = end > text && starts_with(chars, end, to, &close) && edged;
        if closes {
            let inner = (text, end);
            let children = match pair.verbatim {
                true => Vec::new(),
                false => spans(chars, text, end, pairs, markup, depth + 1),
            };
            return Some(Mark {
                children,
                end: end + close.len(),
                hidden: vec![(at, text), (end, end + close.len())],
                inner,
                start: at,
                look: pair.look,
            });
        }
        if !pair.verbatim {
            if let Some(inside) = span_at(chars, end, to, pairs, markup, depth + 1) {
                end = inside.end;
                continue;
            }
        }
        end += 1;
    }
    None
}

/// A Markdown link `[text](url)` at `at`: the brackets and the URL hide,
/// the text stays; an image's `![` is left alone.
fn link_at(chars: &[char], at: usize, to: usize, pairs: &[Pair], markup: Markup, depth: usize) -> Option<Mark> {
    if chars[at] != '[' || (at > 0 && chars[at - 1] == '!') {
        return None;
    }
    let close = (at + 1..to).find(|i| chars[*i] == ']' && chars[*i - 1] != '\\')?;
    if close == at + 1 || chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let paren = (close + 2..to).find(|i| chars[*i] == ')')?;
    if chars[close + 2..paren].iter().any(|c| c.is_whitespace() && *c != ' ') {
        return None;
    }
    Some(Mark {
        children: spans(chars, at + 1, close, pairs, markup, depth + 1),
        end: paren + 1,
        hidden: vec![(at, at + 1), (close, paren + 1)],
        inner: (at + 1, close),
        start: at,
        look: Look::UNDERLINE,
    })
}

/// LaTeX's spans from `from` up to `to`: `\textbf{..}` and its kin, and
/// `{\bf ..}`, each with those inside it.
fn latex_spans(chars: &[char], from: usize, to: usize, depth: usize) -> Vec<Mark> {
    let mut found = Vec::new();
    let mut at = from;
    while at < to {
        if depth <= MAX_DEPTH {
            if let Some(mark) = latex_command_at(chars, at, to, depth).or_else(|| latex_group_at(chars, at, to, depth)) {
                at = mark.end;
                found.push(mark);
                continue;
            }
        }
        at += match chars[at] {
            '\\' => 2,
            _ => 1,
        };
    }
    found
}

/// `\cmd{..}` at `at`, for a command of [`LATEX_COMMANDS`].
fn latex_command_at(chars: &[char], at: usize, to: usize, depth: usize) -> Option<Mark> {
    if chars[at] != '\\' {
        return None;
    }
    let name_end = (at + 1..to).find(|i| !chars[*i].is_ascii_alphabetic()).unwrap_or(to);
    let name: String = chars[at + 1..name_end].iter().collect();
    let look = LATEX_COMMANDS.iter().find(|(command, _)| *command == name)?.1;
    if chars.get(name_end) != Some(&'{') || name_end >= to {
        return None;
    }
    let close = closing_brace(chars, name_end, to)?;
    Some(Mark {
        children: latex_spans(chars, name_end + 1, close, depth + 1),
        end: close + 1,
        hidden: vec![(at, name_end + 1), (close, close + 1)],
        inner: (name_end + 1, close),
        start: at,
        look,
    })
}

/// `{\bf ..}` at `at`, for a declaration of [`LATEX_DECLARATIONS`].
fn latex_group_at(chars: &[char], at: usize, to: usize, depth: usize) -> Option<Mark> {
    if chars[at] != '{' || chars.get(at + 1) != Some(&'\\') {
        return None;
    }
    let name_end = (at + 2..to).find(|i| !chars[*i].is_ascii_alphabetic()).unwrap_or(to);
    let name: String = chars[at + 2..name_end].iter().collect();
    let look = LATEX_DECLARATIONS.iter().find(|(declaration, _)| *declaration == name)?.1;
    if chars.get(name_end) != Some(&' ') {
        return None;
    }
    let close = closing_brace(chars, at, to)?;
    let text = name_end + 1;
    Some(Mark {
        children: latex_spans(chars, text, close, depth + 1),
        end: close + 1,
        hidden: vec![(at, text), (close, close + 1)],
        inner: (text, close),
        start: at,
        look,
    })
}

/// The `}` closing the `{` at `open`, before `to`, past escaped braces and
/// groups inside.
fn closing_brace(chars: &[char], open: usize, to: usize) -> Option<usize> {
    let mut depth = 0;
    let mut at = open;
    while at < to {
        match chars[at] {
            '\\' => {
                at += 2;
                continue;
            }
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
        at += 1;
    }
    None
}

/// Whether `chars` from `at` (and before `to`) read `run`.
fn starts_with(chars: &[char], at: usize, to: usize, run: &[char]) -> bool {
    at + run.len() <= to && chars[at..at + run.len()] == *run
}

/// Whether a marker opening at `at` stands at a word's edge: the line's
/// start, or after something that is neither a letter nor a digit nor the
/// marker itself.
fn edge_before(chars: &[char], at: usize, marker: char) -> bool {
    match at.checked_sub(1).map(|before| chars[before]) {
        None => true,
        Some(before) => !before.is_alphanumeric() && before != marker,
    }
}

/// Whether a marker closing just before `at` stands at a word's edge: the
/// scan's end, or before something that is neither a letter nor a digit
/// nor the marker itself.
fn edge_after(chars: &[char], at: usize, to: usize, marker: char) -> bool {
    match chars.get(at).filter(|_| at < to) {
        None => true,
        Some(after) => !after.is_alphanumeric() && *after != marker,
    }
}

// ========================================================================
// Functions: what shows
// ========================================================================

/// Lay `marks` onto a line `length` long: which characters hide and how
/// each is drawn, `revealed` saying which spans show their markers.
pub(super) fn apply(marks: &[Mark], length: usize, revealed: &dyn Fn(&Mark) -> bool) -> (Vec<bool>, Vec<Look>) {
    let mut hidden = vec![false; length];
    let mut looks = vec![Look::PLAIN; length];
    lay(marks, Look::PLAIN, revealed, &mut hidden, &mut looks);
    (hidden, looks)
}

/// Lay `marks`, inside text of look `outer`, onto `hidden` and
/// `looks`: a span shown leaves its markers and its spans inside alone,
/// as magic-vscode leaves them.
fn lay(marks: &[Mark], outer: Look, revealed: &dyn Fn(&Mark) -> bool, hidden: &mut [bool], looks: &mut [Look]) {
    for mark in marks {
        if revealed(mark) {
            continue;
        }
        for (from, to) in &mark.hidden {
            for at in *from..(*to).min(hidden.len()) {
                hidden[at] = true;
            }
        }
        let look = outer.with(mark.look);
        for at in mark.inner.0..mark.inner.1.min(looks.len()) {
            looks[at] = looks[at].with(look);
        }
        lay(&mark.children, look, revealed, hidden, looks);
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// `line` with the characters `markup` hides taken out, everything
    /// shown.
    fn shown(markup: Markup, line: &str) -> String {
        let length = line.chars().count();
        let found = marks(markup, line, (0, length));
        let (hidden, _) = apply(&found, length, &|_| false);
        line.chars().zip(hidden).filter(|(_, hide)| !hide).map(|(c, _)| c).collect()
    }

    fn looks(markup: Markup, line: &str) -> Vec<Look> {
        let length = line.chars().count();
        apply(&marks(markup, line, (0, length)), length, &|_| false).1
    }

    #[test]
    fn test_markdown_s_emphasis_code_math_and_links_hide_their_markers() {
        let line = "# A **bold** and _it_ with `x*y` and $a_1$ see [the docs](https://x.y/z).";
        assert_eq!(shown(Markup::Markdown, line), "A bold and it with x*y and a_1 see the docs.");
    }

    #[test]
    fn test_links_underlines_and_strikes_draw_their_lines() {
        let line = "[docs](u) <u>under</u> ~~gone~~";
        assert_eq!(shown(Markup::Markdown, line), "docs under gone");
        let looks = looks(Markup::Markdown, line);
        assert!(looks[1].underline, "a link's text");
        assert!(looks[13].underline, "<u>'s");
        assert!(looks[26].strike, "~~'s");
        assert_eq!(shown(Markup::Latex, r"\underline{a} \sout{b}"), "a b");
        assert_eq!(shown(Markup::Org, "_a_ +b+"), "a b");
    }

    #[test]
    fn test_what_is_no_emphasis_keeps_its_markers() {
        for line in ["snake_case_name and 2*3*4", "a * b * c", "**not closed", "![img](a.png)", "`"] {
            assert_eq!(shown(Markup::Markdown, line), line, "{line}");
        }
    }

    #[test]
    fn test_the_text_between_is_drawn_as_the_markers_say_nested_too() {
        let line = "*a **b** c*";
        let found = looks(Markup::Markdown, line);
        assert_eq!(found[1], Look::ITALIC);
        assert_eq!(found[5], Look::BOLD_ITALIC, "the b");
        assert_eq!(shown(Markup::Markdown, line), "a b c");
    }

    #[test]
    fn test_latex_s_style_commands_and_declarations_hide_to_their_argument() {
        let line = r"See \textbf{bold \emph{and it}} or {\bf old} but not \textbf{open";
        assert_eq!(shown(Markup::Latex, line), r"See bold and it or old but not \textbf{open");
        let line = r"\emph{a} % \emph{x}";
        let scan = scans(Markup::Latex, &[line.to_string()])[0];
        let length = line.chars().count();
        let (hidden, _) = apply(&marks(Markup::Latex, line, scan.expect("read")), length, &|_| false);
        let after: String = line.chars().zip(hidden).filter(|(_, hide)| !hide).map(|(c, _)| c).collect();
        assert_eq!(after, r"a % \emph{x}", "a comment's text is left alone");
    }

    #[test]
    fn test_org_rst_and_asciidoc_have_their_own_markers() {
        assert_eq!(shown(Markup::Org, "a *b* /c/ =d= e/f"), "a b c d e/f");
        assert_eq!(shown(Markup::Rst, "a **b** *c* ``d``"), "a b c d");
        assert_eq!(shown(Markup::AsciiDoc, "a *b* _c_ `d`"), "a b c d");
    }

    #[test]
    fn test_code_blocks_and_math_blocks_are_not_read() {
        let lines: Vec<String> = ["*a*", "```", "*b*", "```", "$$", "x_1", "$$", "*c*"].iter().map(|line| line.to_string()).collect();
        let read: Vec<bool> = scans(Markup::Markdown, &lines).iter().map(Option::is_some).collect();
        assert_eq!(read, [true, false, false, false, false, false, false, true]);
    }

    #[test]
    fn test_doc_comments_alone_are_read_in_code() {
        let lines: Vec<String> = ["/// A **doc** line", "let x = a * b * c; // *no*", "/** Some", " * `code` */ y"]
            .iter()
            .map(|line| line.to_string())
            .collect();
        let read = scans(Markup::DocComments, &lines);
        assert_eq!(read[0], Some((3, 18)));
        assert_eq!(read[1], None, "a plain comment is code's");
        assert_eq!(read[2], Some((3, 8)));
        assert_eq!(read[3], Some((0, 10)), "up to the close");
    }

    #[test]
    fn test_a_span_revealed_shows_its_markers_and_those_inside() {
        let line = "**a _b_** c";
        let length = line.chars().count();
        let found = marks(Markup::Markdown, line, (0, length));
        let (hidden, _) = apply(&found, length, &|mark| mark.start == 0);
        assert!(hidden.iter().all(|hide| !hide), "the cursor's span shows, inner ones too");
    }
}
