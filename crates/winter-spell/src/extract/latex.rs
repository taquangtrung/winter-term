//! LaTeX prose: drops comments, math, code, citations, labels, colors,
//! lengths, file names, environment names, option keys, and every command
//! name left over, keeping the text a reader of the PDF sees.
//!
//! The passes run in the order magic-vscode runs them, each over what the
//! earlier ones kept; the order matters wherever one construct can hide
//! inside another.

use std::iter;
use std::ops::Range;

use super::scan::{
    char_before, find_from, group_end, identifier_end, inline_math_end, is_byte_at, letters_end,
    line_end, url_end, whitespace_end, Hit, Mask,
};

// ========================================================================
// Constants
// ========================================================================

/// Environments holding code, dropped whole with their body.
const CODE_ENVIRONMENTS: [&str; 4] = ["verbatim", "lstlisting", "minted", "code"];

/// Environments holding display math, dropped whole with their body.
const MATH_ENVIRONMENTS: [&str; 7] = [
    "equation",
    "align",
    "gather",
    "multline",
    "eqnarray",
    "math",
    "displaymath",
];

/// Commands whose one argument is a key=value options list: the keys go,
/// the values stay.
const OPTIONS_MACROS: [&str; 2] = ["hypersetup", "lstset"];

/// The `listings` command naming a language before its options list.
const LANGUAGE_DEFINITION_MACRO: &str = "lstdefinelanguage";

/// Commands dropped with their arguments, by name and brace argument
/// count, in pass order: code and URLs.
const CODE_MACROS: [(&str, usize); 2] = [("texttt", 1), ("url", 1)];

/// Name fragments of the citation and cross-reference commands (`\cite`,
/// `\citep`, `\autoref`, `\Cref`, a project's own `\aicite`), whose key
/// argument is dropped with them.
const KEY_MACRO_MARKS: [&str; 2] = ["cite", "ref"];

/// Commands dropped with their arguments, by name and brace argument
/// count, in pass order: labels and color names.
const NAME_MACROS: [(&str, usize); 5] = [
    ("label", 1),
    ("definecolor", 3),
    ("colorlet", 2),
    ("pagecolor", 1),
    ("color", 1),
];

/// Commands coloring prose, by name and brace argument count: every
/// argument but the last names a color and goes.
const COLORED_TEXT_MACROS: [(&str, usize); 3] =
    [("textcolor", 2), ("colorbox", 2), ("fcolorbox", 3)];

/// Commands dropped with their arguments, by name and brace argument
/// count, in pass order: lengths, counters, and file names.
const LAYOUT_MACROS: [(&str, usize); 11] = [
    ("setlength", 2),
    ("addtolength", 2),
    ("setcounter", 2),
    ("addtocounter", 2),
    ("newlength", 1),
    ("newcounter", 1),
    ("input", 1),
    ("include", 1),
    ("bibliography", 1),
    ("bibliographystyle", 1),
    ("path", 1),
];

/// The figure command, dropped with its options and its path.
const GRAPHICS_MACRO: &str = "includegraphics";

/// Emphasis commands, unwrapped to their prose argument.
const EMPHASIS_MACROS: [&str; 8] = [
    "textbf",
    "textit",
    "underline",
    "emph",
    "textsf",
    "textsc",
    "textrm",
    "text",
];

/// Opens a command.
const BACKSLASH: u8 = b'\\';

/// Opens a line comment.
const COMMENT_MARK: u8 = b'%';

/// LaTeX's non-breaking space.
const TIE: u8 = b'~';

/// Marks the starred form of a command or environment.
const STAR: u8 = b'*';

// ========================================================================
// Passes
// ========================================================================

/// Drops everything in `mask` that is LaTeX markup rather than prose.
pub(super) fn drop_latex_markup(mask: &mut Mask) {
    mask.apply(|hay, at| environment(hay, at, &CODE_ENVIRONMENTS).map(Hit::dropping));
    // Options lists go before comments: a `listings` block often ends
    // every line with `%`, and dropping those first would split the block
    // into pieces whose braces no longer balance.
    for name in OPTIONS_MACROS {
        mask.apply(|hay, at| {
            let open = macro_end(hay, at, name).filter(|&end| is_byte_at(hay, end, b'{'))?;
            options_macro(hay, at, open)
        });
    }
    mask.apply(|hay, at| {
        let name_end = macro_end(hay, at, LANGUAGE_DEFINITION_MACRO)?;
        let open = group_end(hay, name_end, b'{', b'}')?;
        let is_plain_language = !hay[name_end + 1..open - 1].contains('{');
        (is_plain_language && is_byte_at(hay, open, b'{'))
            .then(|| options_macro(hay, at, open))
            .flatten()
    });
    mask.apply(bracket_options_macro);
    mask.apply(|hay, at| {
        (is_byte_at(hay, at, COMMENT_MARK) && char_before(hay, at) != Some('\\'))
            .then(|| Hit::dropping(at..line_end(hay, at)))
    });
    drop_math(mask);
    drop_code_and_keys(mask);
    for (name, count) in NAME_MACROS {
        mask.apply(|hay, at| {
            fixed_macro(hay, at, name, count).map(|(range, _)| Hit::dropping(range))
        });
    }
    for (name, count) in COLORED_TEXT_MACROS {
        mask.apply(|hay, at| {
            let (range, arguments) = fixed_macro(hay, at, name, count)?;
            Some(Hit::sparing(range, arguments.last().cloned()))
        });
    }
    for (name, count) in LAYOUT_MACROS {
        mask.apply(|hay, at| {
            fixed_macro(hay, at, name, count).map(|(range, _)| Hit::dropping(range))
        });
    }
    // `\includegraphics[...]` comes before the generic `[...]` rule below,
    // which would leave its path behind as prose.
    mask.apply(|hay, at| {
        let name_end = macro_end(hay, at, GRAPHICS_MACRO)?;
        let options_end = group_end(hay, name_end, b'[', b']').unwrap_or(name_end);
        group_end(hay, options_end, b'{', b'}').map(|end| Hit::dropping(at..end))
    });
    mask.apply(|hay, at| {
        (is_byte_at(hay, at, TIE) && char_before(hay, at) != Some('\\'))
            .then(|| Hit::dropping(at..at + 1).spaced())
    });
    mask.apply(|hay, at| {
        let item_end = macro_end(hay, at, "item")?;
        let at_line_start = char_before(hay, at).is_none_or(|c| c == '\n');
        at_line_start.then(|| Hit::dropping(at..whitespace_end(hay, item_end)).spaced())
    });
    for name in EMPHASIS_MACROS {
        mask.apply(|hay, at| {
            let (range, arguments) = fixed_macro(hay, at, name, 1)?;
            Some(Hit::sparing(range, arguments))
        });
    }
    drop_structure(mask);
}

/// Display math, math environments, and inline math.
fn drop_math(mask: &mut Mask) {
    mask.apply(|hay, at| delimited(hay, at, "$$", "$$").map(Hit::dropping));
    mask.apply(|hay, at| delimited(hay, at, "\\[", "\\]").map(Hit::dropping));
    mask.apply(|hay, at| environment(hay, at, &MATH_ENVIRONMENTS).map(Hit::dropping));
    mask.apply(|hay, at| inline_math_end(hay, at).map(|end| Hit::dropping(at..end).spaced()));
    mask.apply(|hay, at| {
        delimited(hay, at, "\\(", "\\)")
            .filter(|range| !hay[range.clone()].contains('\n'))
            .map(|range| Hit::dropping(range).spaced())
    });
}

/// Inline code, URLs, and the keys of citations and cross-references.
fn drop_code_and_keys(mask: &mut Mask) {
    mask.apply(|hay, at| {
        let name_end = macro_end(hay, at, "verb")?;
        let delimiter = *hay.as_bytes().get(name_end)?;
        if delimiter != b'|' && delimiter != b'+' {
            return None;
        }
        group_end(hay, name_end, delimiter, delimiter).map(|end| Hit::dropping(at..end).spaced())
    });
    mask.apply(|hay, at| {
        let name_end = macro_end(hay, at, "lstinline")?;
        group_end(hay, name_end, b'{', b'}')
            .or_else(|| group_end(hay, name_end, b'|', b'|'))
            .map(|end| Hit::dropping(at..end).spaced())
    });
    for (name, count) in CODE_MACROS {
        mask.apply(|hay, at| {
            fixed_macro(hay, at, name, count).map(|(range, _)| Hit::dropping(range).spaced())
        });
    }
    mask.apply(|hay, at| url_end(hay, at).map(|end| Hit::dropping(at..end).spaced()));
    mask.apply(|hay, at| {
        if !is_byte_at(hay, at, BACKSLASH) {
            return None;
        }
        let name_end = letters_end(hay, at + 1);
        let name = hay[at + 1..name_end].to_ascii_lowercase();
        if !KEY_MACRO_MARKS.iter().any(|mark| name.contains(mark)) {
            return None;
        }
        group_end(hay, name_end, b'{', b'}').map(|end| Hit::dropping(at..end).spaced())
    });
}

/// Environment names and their arguments, leftover `[...]` options and
/// option keys, then every command name still standing.
fn drop_structure(mask: &mut Mask) {
    mask.apply(|hay, at| {
        let mut end = environment_name_end(hay, macro_end(hay, at, "begin")?)?;
        end = group_end(hay, end, b'[', b']').unwrap_or(end);
        while let Some(argument_end) = group_end(hay, end, b'{', b'}') {
            end = argument_end;
        }
        Some(Hit::dropping(at..end))
    });
    mask.apply(|hay, at| {
        environment_name_end(hay, macro_end(hay, at, "end")?).map(|end| Hit::dropping(at..end))
    });
    mask.apply(|hay, at| {
        let name_end = command_end(hay, at)?;
        group_end(hay, name_end, b'[', b']').map(|end| Hit::dropping(name_end..end))
    });
    // An identifier right after `{` or `,` and before `=` is an option key
    // of some options list, whatever command takes it.
    mask.apply(|hay, at| {
        if !matches!(char_before(hay, at), Some(',' | '{')) {
            return None;
        }
        let key_end = identifier_end(hay, whitespace_end(hay, at))?;
        let equals = whitespace_end(hay, key_end);
        is_byte_at(hay, equals, b'=').then(|| Hit::new(iter::once(at..key_end), equals + 1))
    });
    mask.apply(|hay, at| command_end(hay, at).map(|end| Hit::dropping(at..end)));
    // Bare grouping braces join what they split, as `{P}rogram` reads
    // `Program`; magic-vscode leaves them, splitting such words.
    mask.apply(|hay, at| {
        let is_brace = is_byte_at(hay, at, b'{') || is_byte_at(hay, at, b'}');
        (is_brace && char_before(hay, at) != Some('\\')).then(|| Hit::dropping(at..at + 1))
    });
}

// ========================================================================
// Options Lists
// ========================================================================

/// An options command from `at` whose list opens with the `{` at `open`:
/// the command and its opening brace go, and so does every key in the
/// list; the values, separators, and closing brace stay.
fn options_macro(hay: &str, at: usize, open: usize) -> Option<Hit> {
    let close = matching_brace(hay, open)?;
    Some(options_hit(hay, at, open + 1..close))
}

/// A command from `at` with a `[...]` options argument. A list of nothing
/// but key=value pairs loses the command, its `[`, and its keys; any other
/// argument is skipped whole, for the generic `[...]` rule to drop later.
fn bracket_options_macro(hay: &str, at: usize) -> Option<Hit> {
    let open = command_end(hay, at).filter(|&end| is_byte_at(hay, end, b'['))?;
    let close = matching_bracket(hay, open)?;
    let body = open + 1..close;
    if !is_options_list(&hay[body.clone()]) {
        return Some(Hit::new(iter::empty(), close + 1));
    }
    Some(options_hit(hay, at, body))
}

/// The hit for an options list `body` opened by a command at `at`: the
/// command through the list's opener, and every key, ending past the
/// list's closer.
fn options_hit(hay: &str, at: usize, body: Range<usize>) -> Hit {
    let keys = option_keys(&hay[body.clone()])
        .into_iter()
        .map(|key| body.start + key.start..body.start + key.end);
    Hit::new(iter::once(at..body.start).chain(keys), body.end + 1)
}

/// The keys of an options list body: each identifier that opens the body
/// or follows `,` or `{`, past whitespace and `%` comments, and comes
/// before `=`.
fn option_keys(body: &str) -> Vec<Range<usize>> {
    let mut keys = Vec::new();
    let mut at = 0;
    while at < body.len() {
        let key = (at == 0)
            .then(|| option_key_after(body, 0))
            .flatten()
            .or_else(|| {
                matches!(body.as_bytes()[at], b',' | b'{')
                    .then(|| option_key_after(body, at + 1))
                    .flatten()
            });
        match key {
            Some((range, end)) => {
                keys.push(range);
                at = end;
            }
            None => at += body[at..].chars().next().map_or(1, char::len_utf8),
        }
    }
    keys
}

/// The key starting past the whitespace and comments at `at`, and the end
/// of its `=`.
fn option_key_after(body: &str, at: usize) -> Option<(Range<usize>, usize)> {
    let key_start = space_and_comments_end(body, at);
    let key_end = identifier_end(body, key_start)?;
    let equals = whitespace_end(body, key_end);
    is_byte_at(body, equals, b'=').then_some((key_start..key_end, equals + 1))
}

/// Whether `body`, its braced values erased, is one or more `key =`
/// entries separated by optional commas, whitespace, and comments.
fn is_options_list(body: &str) -> bool {
    if !body.contains('=') {
        return false;
    }
    let skeleton = without_brace_groups(body);
    let mut at = space_and_comments_end(&skeleton, 0);
    let mut entries = 0;
    while at < skeleton.len() {
        let Some(key_end) = identifier_end(&skeleton, at) else {
            return false;
        };
        let equals = whitespace_end(&skeleton, key_end);
        if !is_byte_at(&skeleton, equals, b'=') {
            return false;
        }
        at = space_and_comments_end(&skeleton, equals + 1);
        if is_byte_at(&skeleton, at, b',') {
            at = space_and_comments_end(&skeleton, at + 1);
        }
        entries += 1;
    }
    entries > 0
}

/// `text` with every balanced `{...}` group removed, braces included.
fn without_brace_groups(text: &str) -> String {
    let mut depth = 0_usize;
    let mut kept = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => kept.push(c),
            _ => {}
        }
    }
    kept
}

/// Past the whitespace and whole `%` comment lines starting at `at`.
fn space_and_comments_end(hay: &str, at: usize) -> usize {
    let mut at = at;
    loop {
        let after_space = whitespace_end(hay, at);
        if !is_byte_at(hay, after_space, COMMENT_MARK) {
            return after_space;
        }
        let comment_end = line_end(hay, after_space);
        if comment_end == hay.len() {
            return after_space;
        }
        at = comment_end + 1;
    }
}

/// The `}` closing the `{` at `open`, counting nested braces.
fn matching_brace(hay: &str, open: usize) -> Option<usize> {
    let mut depth = 0_usize;
    for (index, byte) in hay.bytes().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// The `]` closing the `[` at `open`, counting nested brackets outside
/// braced values only.
fn matching_bracket(hay: &str, open: usize) -> Option<usize> {
    let mut brackets = 0_usize;
    let mut braces = 0_usize;
    for (index, byte) in hay.bytes().enumerate().skip(open) {
        match byte {
            b'{' => braces += 1,
            b'}' => braces = braces.saturating_sub(1),
            b'[' if braces == 0 => brackets += 1,
            b']' if braces == 0 => {
                brackets -= 1;
                if brackets == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

// ========================================================================
// Commands
// ========================================================================

/// Past `\name` at `at`, for an exact `name` (a longer name still matches,
/// as in the source regexes; callers check what follows).
fn macro_end(hay: &str, at: usize, name: &str) -> Option<usize> {
    let name_start = at + 1;
    (is_byte_at(hay, at, BACKSLASH) && hay.get(name_start..)?.starts_with(name))
        .then_some(name_start + name.len())
}

/// Past any command name at `at`, with its star if starred.
fn command_end(hay: &str, at: usize) -> Option<usize> {
    if !is_byte_at(hay, at, BACKSLASH) {
        return None;
    }
    let name_end = letters_end(hay, at + 1);
    if name_end == at + 1 {
        return None;
    }
    Some(name_end + usize::from(is_byte_at(hay, name_end, STAR)))
}

/// `\name` at `at` followed straight by `count` brace arguments: the
/// whole match and each argument's inside.
fn fixed_macro(
    hay: &str,
    at: usize,
    name: &str,
    count: usize,
) -> Option<(Range<usize>, Vec<Range<usize>>)> {
    let mut end = macro_end(hay, at, name)?;
    let mut arguments = Vec::with_capacity(count);
    for _ in 0..count {
        let argument_end = group_end(hay, end, b'{', b'}')?;
        arguments.push(end + 1..argument_end - 1);
        end = argument_end;
    }
    Some((at..end, arguments))
}

/// Past the `{name}` of an environment at `at`: letters and stars.
fn environment_name_end(hay: &str, at: usize) -> Option<usize> {
    if !is_byte_at(hay, at, b'{') {
        return None;
    }
    let name_end = hay[at + 1..]
        .bytes()
        .position(|byte| !byte.is_ascii_alphabetic() && byte != STAR)
        .map_or(hay.len(), |index| at + 1 + index);
    (name_end > at + 1 && is_byte_at(hay, name_end, b'}')).then_some(name_end + 1)
}

/// A whole `\begin{name}...\end{name}` at `at` for a name in `names`,
/// either end optionally starred, closed by the first matching end.
fn environment(hay: &str, at: usize, names: &[&str]) -> Option<Range<usize>> {
    let name_start = macro_end(hay, at, "begin{")?;
    let name_end = letters_end(hay, name_start);
    let name = &hay[name_start..name_end];
    if !names.contains(&name) {
        return None;
    }
    let star_end = name_end + usize::from(is_byte_at(hay, name_end, STAR));
    if !is_byte_at(hay, star_end, b'}') {
        return None;
    }
    let closer = format!("\\end{{{name}");
    let mut from = star_end + 1;
    while let Some(index) = find_from(hay, from, &closer) {
        let after = index + closer.len();
        let after = after + usize::from(is_byte_at(hay, after, STAR));
        if is_byte_at(hay, after, b'}') {
            return Some(at..after + 1);
        }
        from = index + 1;
    }
    None
}

/// `open ... close` at `at`, closed by the first `close` after it.
fn delimited(hay: &str, at: usize, open: &str, close: &str) -> Option<Range<usize>> {
    if !hay[at..].starts_with(open) {
        return None;
    }
    find_from(hay, at + open.len(), close).map(|index| at..index + close.len())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::{extract_prose_text, Prose};

    fn kept(text: &str) -> String {
        extract_prose_text(text, Prose::Latex).text().to_string()
    }

    #[test]
    fn test_latex_drops_texttt_but_unwraps_emphasis() {
        let kept = kept("Uses \\texttt{circomlib} and \\textbf{important} notes.");
        assert!(!kept.contains("circomlib"));
        assert!(!kept.contains("textbf"));
        assert!(kept.contains("important"));
    }

    #[test]
    fn test_latex_drops_any_cite_family_key() {
        assert!(!kept("guarding value~\\aicite{sok-snark-vulns}.").contains("sok-snark-vulns"));
        let kept = kept("Real citation \\cite{knuth1984} still works.");
        assert!(!kept.contains("knuth1984"));
        assert!(kept.contains("Real citation"));
        assert!(kept.contains("still works"));
    }

    #[test]
    fn test_latex_drops_any_ref_family_key() {
        for command in ["ref", "cref", "Cref", "autoref", "pageref", "eqref"] {
            let text = format!("See Section~\\{command}{{sec:intro}} for details.");
            assert!(!kept(&text).contains("sec:intro"), "\\{command} key kept");
        }
    }

    #[test]
    fn test_latex_drops_label_key() {
        let kept = kept("As shown in \\label{sec:intro} the results hold.");
        assert!(!kept.contains("sec:intro"));
        assert!(kept.contains("the results hold"));
    }

    #[test]
    fn test_latex_drops_environment_names_and_arguments() {
        let kept_array = kept("See \\begin{array}{lll} a & b & c \\end{array} for the layout.");
        assert!(!kept_array.contains("lll"));
        assert!(kept_array.contains("for the layout"));
        let kept_tabular = kept("See \\begin{tabular}[t]{lll} a & b \\end{tabular} here.");
        assert!(!kept_tabular.contains("[t]"));
        assert!(!kept_tabular.contains("lll"));
        let kept_tabularx = kept("See \\begin{tabularx}{\\linewidth}{lll} a \\end{tabularx} here.");
        assert!(!kept_tabularx.contains("linewidth"));
        assert!(!kept_tabularx.contains("lll"));
        assert!(kept_tabularx.contains("here"));
        let kept_custom = kept("A \\begin{workhighlights} entry \\end{workhighlights} here.");
        assert!(!kept_custom.contains("workhighlights"));
        assert!(kept_custom.contains("entry"));
    }

    #[test]
    fn test_latex_keeps_items_and_section_titles() {
        let kept_list = kept("A normal \\begin{itemize} \\item one \\end{itemize} list.");
        assert!(kept_list.contains("one"));
        assert!(kept_list.contains("list"));
        assert!(!kept_list.contains("item"));
        assert!(kept("The \\section{Introduction} stays.").contains("Introduction"));
    }

    #[test]
    fn test_latex_drops_hypersetup_keys_but_keeps_values() {
        let kept = kept(
            "\\hypersetup{\n  pdfauthor={Ta, Quang Trung},\n  pdftitle={CV - Ta Quang Trung},\n}",
        );
        assert!(!kept.contains("pdfauthor"));
        assert!(!kept.contains("pdftitle"));
        assert!(kept.contains("Ta, Quang Trung"));
        assert!(kept.contains("CV - Ta Quang Trung"));
    }

    #[test]
    fn test_latex_drops_every_lstset_key_despite_trailing_comments() {
        let kept = kept(
            "\\lstset{%\n  language=Circom,%\n  basicstyle=\\ttfamily\\small,%\n  \
             keywordstyle=\\color{circomkw}\\bfseries,%\n  numbersep=8pt,%\n  xrightmargin=0em%\n}",
        );
        for key in [
            "language",
            "basicstyle",
            "keywordstyle",
            "numbersep",
            "xrightmargin",
        ] {
            assert!(!kept.contains(key), "{key} kept");
        }
        assert!(kept.contains("Circom"));
        assert!(!kept.contains("circomkw"));
    }

    #[test]
    fn test_latex_drops_lstdefinelanguage_name_and_keys() {
        let kept = kept("\\lstdefinelanguage{Circom}{\n  morekeywords={template, signal},\n  sensitive=true,\n}");
        assert!(!kept.contains("Circom"));
        assert!(!kept.contains("morekeywords"));
        assert!(!kept.contains("sensitive"));
        assert!(kept.contains("template"));
        assert!(kept.contains("signal"));
        assert!(kept.contains("true"));
    }

    #[test]
    fn test_latex_drops_bracket_list_keys_but_keeps_values() {
        let kept = kept(
            "\\award[\n  name = {Ph.D. Research Scholarship},\n  year = {2013--2017},\n  \
             org  = {Ministry of Education, Singapore},\n  desc = {Fully funded doctoral fellowship at NUS.},\n]",
        );
        assert!(!kept.contains("name ="));
        assert!(!kept.contains("org"));
        assert!(!kept.contains("desc"));
        assert!(kept.contains("Ph.D. Research Scholarship"));
        assert!(kept.contains("Ministry of Education, Singapore"));
        assert!(kept.contains("Fully funded doctoral fellowship at NUS."));
    }

    #[test]
    fn test_latex_drops_mixed_bracket_options_whole() {
        let kept = kept("\\begin{enumerate}[leftmargin=2em, nosep]\nSome text.\n\\end{enumerate}");
        assert!(!kept.contains("leftmargin"));
        assert!(!kept.contains("nosep"));
        assert!(kept.contains("Some text."));
    }

    #[test]
    fn test_latex_drops_brace_option_keys_of_any_command() {
        let kept_setlist = kept(
            "\\setlist[itemize]{leftmargin=1.2em, itemsep=3.5pt, topsep=0pt, partopsep=0pt, parsep=0pt}\n\
             \\setlist[enumerate]{leftmargin=1.2em, itemsep=3.5pt, topsep=2pt, parsep=0pt}",
        );
        for word in [
            "leftmargin",
            "itemsep",
            "topsep",
            "parsep",
            "itemize",
            "enumerate",
        ] {
            assert!(!kept_setlist.contains(word), "{word} kept");
        }
        assert!(kept_setlist.contains("1.2em"));
        let kept_multiline = kept(
            "\\pgfplotsset{\n  width=7cm,\n  height=5cm,\n  legendstyle=none,\n}\nReal prose here.",
        );
        assert!(!kept_multiline.contains("width"));
        assert!(!kept_multiline.contains("legendstyle"));
        assert!(kept_multiline.contains("Real prose here."));
    }

    #[test]
    fn test_latex_keeps_equation_phrase_in_prose() {
        assert!(kept("We assume alpha = beta throughout the proof.").contains("alpha = beta"));
    }

    #[test]
    fn test_latex_drops_color_names_but_keeps_colored_prose() {
        let kept_names = kept(
            "\\definecolor{circomkw}{HTML}{0969DA} % modern clear blue\n\
             \\colorlet{circomkw2}{circomkw}\n\\pagecolor{circombg}\n{\\color{circomstr} inline}",
        );
        for word in [
            "circomkw",
            "circombg",
            "circomstr",
            "HTML",
            "0969DA",
            "modern",
        ] {
            assert!(!kept_names.contains(word), "{word} kept");
        }
        assert!(kept_names.contains("inline"));
        let kept_text = kept(
            "A \\textcolor{circomkw}{highlighted phrase} and a \\colorbox{circombg}{boxed note} \
             and a \\fcolorbox{circomborder}{circombg}{framed note}.",
        );
        for word in ["circomkw", "circombg", "circomborder"] {
            assert!(!kept_text.contains(word), "{word} kept");
        }
        for phrase in ["highlighted phrase", "boxed note", "framed note"] {
            assert!(kept_text.contains(phrase), "{phrase} dropped");
        }
    }

    #[test]
    fn test_latex_drops_registers_and_file_names() {
        let kept = kept(
            "\\setlength{\\parskip}{1em}\n\\newcounter{figcount}\n\\setcounter{figcount}{0}\n\
             \\addtocounter{figcount}{1}\n\\input{chapters/intro}\n\
             \\includegraphics[width=5cm]{figures/diagram.png}\n\\bibliography{refs}\n\\bibliographystyle{plain}",
        );
        for word in [
            "parskip", "figcount", "1em", "chapters", "diagram", "refs", "plain", "width",
        ] {
            assert!(!kept.contains(word), "{word} kept");
        }
    }

    #[test]
    fn test_latex_drops_math_comments_and_code() {
        let kept = kept(
            "Inline $x_iy$ and \\(a+b\\) and\n$$ disp $$\n\\begin{align*} q \\end{align*}\n\
             % hidden note\n\\begin{verbatim}raw code\\end{verbatim}\nSee \\url{http://a.b} 100\\% sure.",
        );
        for word in ["x_iy", "a+b", "disp", " q ", "hidden", "raw code", "a.b"] {
            assert!(!kept.contains(word), "{word} kept");
        }
        assert!(kept.contains("Inline"));
        assert!(kept.contains("sure."));
    }
}
