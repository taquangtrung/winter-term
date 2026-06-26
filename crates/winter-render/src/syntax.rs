//! The colors source is read in: a table of TextMate scope selectors, each
//! with the color and weight magic-vscode's One Color theme gives it, and the
//! per-theme colors laid over that table.
//!
//! The table is the vocabulary a grammar's scopes are matched against, so a
//! file is colored the way VS Code colors it under the same theme. Most rows
//! are the theme's own `tokenColors`; the rest, at the end, name the same
//! things the way the Sublime grammars the editor reads with spell them
//! (`entity.name.struct` where VS Code says `entity.name.type`).

#[cfg(test)]
use crate::theme::contrast_ratio;
use crate::theme::Rgb;

// ========================================================================
// Constants
// ========================================================================

/// The least contrast a color made legible on a background keeps against it,
/// as WCAG measures it: the bar its level AA sets for body text.
const LEGIBLE_CONTRAST: f64 = 4.5;

/// How many rows the scope table has.
pub const SCOPE_COUNT: usize = SCOPE_RULES.len();

/// The names the syntax colors went by before they were scopes, and the
/// scope each now stands for, so a config written then still reads.
const ALIASES: [(&str, &str); 2] = [("number", "constant.numeric"), ("type", "entity.name.type")];

/// Every scope the editor tells apart, in the order the theme lists them. A
/// later row wins over an earlier one that matches a scope just as closely,
/// as a later rule does in a VS Code theme.
pub const SCOPE_RULES: &[ScopeRule] = &[
    fg("comment", "#718b9f"),
    fg("comment.block.documentation", "#919186"),
    fg(
        "comment.block.documentation punctuation.definition.comment",
        "#919186",
    ),
    fg("comment.documentation", "#819d71"),
    fg("comment.inline-code", "#c18b4f"),
    fg("comment.line.documentation", "#919186"),
    fg(
        "comment.line.documentation punctuation.definition.comment",
        "#919186",
    ),
    fg("constant", "#8da0df"),
    fg("constant.other.reference.citation.latex", "#ada348"),
    fg("constant.other.reference.label.latex", "#56b6c2"),
    fg("constant.character.math.tex", "#98be65"),
    fg("constant.character.escape.tex", "#b189f7"),
    fg("constant.numeric.math.tex", "#98be65"),
    fg("constant.language.boolean", "#da8548"),
    fg("constant.language.python", "#da8548"),
    fg("constant.numeric", "#da8548"),
    fg("constant.other.language", "#709bb1"),
    plain("emphasis", false, true),
    fg("entity.name.function", "#bf73df"),
    fg("entity.name.function.call", "#c678dd"),
    fg("entity.name.function.definition", "#c678dd"),
    fg("entity.name.function.math.tex", "#98be65"),
    fg("entity.name.namespace", "#a9a1e1"),
    fg("entity.name.operator", "#c678dd"),
    fg("entity.name.scope-resolution", "#a9a1e1"),
    bold("entity.name.section.latex", "#5fa21f"),
    fg("entity.name.tag.environment.latex", "#2aa9bf"),
    fg("entity.name.tag", "#2aa9bf"),
    fg("entity.name.type", "#cf517a"),
    fg("entity.name.type.class", "#cf517a"),
    fg("entity.name.type.module", "#a9a1e1"),
    fg("entity.name.type.template", "#ffcb6b"),
    fg("entity.other.attribute-name", "#6abfba"),
    fg("fenced_code.block.language", "#709bb1"),
    bold("heading.1.markdown", "#4fafef"),
    bold("heading.2.markdown", "#5fa21f"),
    bold("heading.3.markdown", "#b1951d"),
    bold("heading.4.markdown", "#a57cdf"),
    bold("heading.5.markdown", "#b59e75"),
    bold("heading.6.markdown", "#2d9574"),
    fg("keyword", "#51afef"),
    fg("keyword.control.block", "#709bb1"),
    fg("keyword.cmake", "#c678dd"),
    fg("keyword.control", "#51afef"),
    fg("keyword.control.directive", "#4bc4f7"),
    fg("keyword.operator", "#baaf51"),
    fg("keyword.operator.new", "#51afef"),
    fg("keyword.other.operator", "#baaf51"),
    fg("keyword.other.unit", "#da8548"),
    bold("markup.bold", "#a071d9"),
    fg("markup.checkbox", "#5b6268"),
    fg("markup.checkbox.checked", "#c18b4f"),
    fg("markup.deleted.diff", "#ff6c6b"),
    fg("markup.fenced_code", "#98be65"),
    fg("markup.fenced_code.block", "#bfa587"),
    fg("markup.heading", "#ff5f5f"),
    bold("markup.heading.setext.1", "#4fafef"),
    fg("markup.inline.raw.string", "#c18b4f"),
    fg("markup.inserted.diff", "#98be65"),
    italic("markup.italic", "#b581f1"),
    fg("markup.link.latex", "#2d9574"),
    fg("markup.list", "#bbc2cf"),
    fg(
        "markup.list.numbered.markdown punctuation.definition.list.begin.markdown",
        "#da8548",
    ),
    fg("markup.math", "#98be65"),
    fg("markup.underline.link", "#51afef"),
    fg("meta.attribute.rust", "#b38f5f"),
    fg("meta.diff", "#519bdf"),
    bold("meta.diff.range", "#519bdf"),
    fg("meta.diff.header.from-file", "#bb9bf1"),
    fg("meta.diff.header.to-file", "#bb9bf1"),
    fg("meta.embedded.block", "#b0997f"),
    fg("meta.function.python", "#c678dd"),
    fg("meta.interpolation.rust", "#75b91f"),
    fg("meta.math", "#98be65"),
    fg("meta.parameter", "#bbc2cf"),
    fg("meta.preprocessor", "#4bc4f7"),
    fg("meta.macro.section.paragraph.latex", "#51afef"),
    bold(
        "meta.macro.section.subsection.latex entity.name.section.latex",
        "#aba23f",
    ),
    bold(
        "meta.macro.section.subsubsection.latex entity.name.section.latex",
        "#a57cdf",
    ),
    fg("meta.template", "#c678dd"),
    fg("meta.template.call", "#98be65"),
    fg("punctuation.definition.arguments.begin.math.tex", "#98be65"),
    fg("punctuation.definition.arguments.end.math.tex", "#98be65"),
    fg("punctuation.definition.begin.bracket.square", "#98be65"),
    fg("punctuation.definition.bold", "#709bb1"),
    fg("punctuation.definition.comment", "#718b9f"),
    fg("punctuation.definition.decorator.python", "#ffcb6b"),
    fg("punctuation.definition.deleted.diff", "#ff2f15"),
    fg("punctuation.definition.directive", "#4bc4f7"),
    fg("punctuation.definition.end.bracket.square", "#98be65"),
    fg("punctuation.definition.function.math.tex", "#98be65"),
    bold("punctuation.definition.heading.markdown", "#f77a4f"),
    fg("punctuation.definition.inserted.diff", "#5fd113"),
    fg("punctuation.definition.link", "#5b6268"),
    fg("punctuation.definition.list", "#51afef"),
    fg("punctuation.definition.markdown", "#c18b4f"),
    fg("punctuation.definition.metadata.markdown", "#5b6268"),
    fg("punctuation.definition.range.diff", "#51afef"),
    fg("punctuation.definition.string", "#2d9574"),
    fg("punctuation.section.angle-brackets.end", "#98be65"),
    fg("punctuation.section.angle-brackets.start", "#98be65"),
    fg("punctuation.section.block.begin.bracket.curly", "#c678dd"),
    fg(
        "punctuation.section.block.begin.bracket.curly.namespace",
        "#51afef",
    ),
    fg("punctuation.section.block.begin.bracket.round", "#98be65"),
    fg("punctuation.section.block.end.bracket.curly", "#c678dd"),
    fg(
        "punctuation.section.block.end.bracket.curly.namespace",
        "#51afef",
    ),
    fg("punctuation.section.block.end.bracket.round", "#98be65"),
    fg("punctuation.section.parameters", "#98be65"),
    fg("punctuation.section.parens.begin.bracket.round", "#98be65"),
    fg("punctuation.section.parens.end.bracket.round", "#98be65"),
    fg("punctuation.separator.delimiter", "#bbc2cf"),
    fg("punctuation.separator.scope-resolution", "#bbc2cf"),
    fg("punctuation.vararg-ellipses", "#c678dd"),
    fg("storage.modifier", "#51afef"),
    fg("storage.modifier.reference", "#bbc2cf"),
    fg("storage.type", "#51afef"),
    fg("storage.type.builtin", "#ffcb6b"),
    fg("storage.type.primitive", "#ffcb6b"),
    fg("string", "#2d9574"),
    fg("string.other.link", "#51afef"),
    plain("strong", true, false),
    fg("support.function", "#bf73df"),
    fg("keyword.control.macro.latex", "#51afef"),
    fg("support.function.general.tex", "#51afef"),
    fg("support.type.primitive", "#cf517a"),
    fg("support.type.property-name.json", "#51afef"),
    fg("variable.declaration", "#7590db"),
    fg("variable.language", "#51afef"),
    fg("variable.other", "#7590db"),
    fg("variable.other.enummember", "#bf9169"),
    fg("variable.other.global", "#bbc2cf"),
    fg("variable.other.local", "#bbc2cf"),
    fg("variable.other.property", "#7590db"),
    fg("variable.parameter.function.language.python", "#bbc2cf"),
    fg("variable.parameter.macro.latex", "#bf73df"),
    fg("variable.parameter.key.latex", "#7590db"),
    fg("variable.parameter.value.latex", "#d1b89f"),
    fg("variable.parameter.macro.generic.latex", "#d1b89f"),
    fg("entity.name.section.toml", "#51afef"),
    fg("punctuation.definition.table.toml", "#47abb8"),
    fg("variable.other.key.toml", "#8192dd"),
    fg("punctuation.separator.key-value.toml", "#c678dd"),
    fg("punctuation.separator.dot.toml", "#7a8299"),
    // The same things as the Sublime grammars name them. Types: what VS Code
    // calls `entity.name.type`, and the types a language comes with.
    fg("entity.name.class", "#cf517a"),
    fg("entity.name.enum", "#cf517a"),
    fg("entity.name.impl", "#cf517a"),
    fg("entity.name.interface", "#cf517a"),
    fg("entity.name.struct", "#cf517a"),
    fg("entity.name.trait", "#cf517a"),
    fg("entity.name.union", "#cf517a"),
    fg("support.type", "#cf517a"),
    // A function where it is called, which VS Code's grammars name where
    // the Sublime ones say `variable.function`.
    fg("variable.function", "#bf73df"),
    // What VS Code's semantic tokens color: macros, members, attributes.
    fg("support.macro", "#bd91f7"),
    fg("variable.other.member", "#7590db"),
    fg("meta.annotation", "#b38f5f"),
    fg("constant.other.placeholder", "#75b91f"),
    fg("constant.language", "#da8548"),
    // Markdown, whose headings Sublime numbers on `markup.heading`.
    bold("markup.heading.1", "#4fafef"),
    bold("markup.heading.2", "#5fa21f"),
    bold("markup.heading.3", "#b1951d"),
    bold("markup.heading.4", "#a57cdf"),
    bold("markup.heading.5", "#b59e75"),
    bold("markup.heading.6", "#2d9574"),
    bold("punctuation.definition.heading", "#f77a4f"),
    fg("markup.raw.inline", "#c18b4f"),
    fg("markup.raw.block", "#bfa587"),
    fg("markup.raw.code-fence", "#bfa587"),
    fg("meta.link.inline.description", "#51afef"),
    // A commit message's summary is drawn in the text's own color and weight,
    // not in `markup.heading`'s red, which would read as a warning.
    fg("markup.heading.subject.git.commit", "#bbc2cf"),
    // LaTeX: an environment's name, math, and what a label, a reference, a
    // citation and a link hold.
    fg("variable.parameter.function.latex", "#2aa9bf"),
    fg("meta.environment.math", "#98be65"),
    fg("string.other.math", "#98be65"),
    fg("variable.other.math", "#98be65"),
    fg("support.function.math.tex", "#98be65"),
    fg("keyword.operator.math.tex", "#98be65"),
    fg("entity.name.label.latex", "#56b6c2"),
    fg("constant.other.reference.latex", "#56b6c2"),
    fg("constant.other.citation.latex", "#ada348"),
    fg("markup.underline.link.latex", "#2d9574"),
    fg("support.function.general.latex", "#51afef"),
    fg("constant.character.newline.latex", "#b189f7"),
    fg("entity.name.newcommand.latex", "#bf73df"),
];

// ========================================================================
// Data Structures
// ========================================================================

/// One row of the scope table: the scopes it colors, and how.
///
/// A row may set the text's color, its weight, or both, and the two are
/// settled apart, as VS Code settles a theme's `foreground` and `fontStyle`:
/// text takes its color from the closest row that sets one and its weight
/// from the closest row that sets one, which need not be the same row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScopeRule {
    /// Whether the text is drawn bold, for a row that sets the weight.
    pub bold: bool,
    /// Whether the text is drawn italic, for a row that sets the weight.
    pub italic: bool,
    /// The color One Color gives it, or `None` for a row that only changes
    /// the weight of the text.
    magic: Option<&'static str>,
    /// The TextMate scope selector, written as a VS Code theme writes one.
    pub scope: &'static str,
    /// Whether the row sets the weight, rather than leaving it to a broader
    /// row: a row naming only a color leaves the text as heavy as it was.
    weighs: bool,
}

// ========================================================================
// ScopeRule
// ========================================================================

impl ScopeRule {
    /// Whether the row sets the text's color.
    pub fn colors(&self) -> bool {
        self.magic.is_some()
    }

    /// Whether the row sets the text's weight.
    pub fn weighs(&self) -> bool {
        self.weighs
    }
}

/// The color a theme gives each row of [`SCOPE_RULES`]. A row left `None` is
/// colored from the theme's own palette by whatever draws it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyntaxColors {
    colors: [Option<Rgb>; SCOPE_COUNT],
}

// ========================================================================
// SyntaxColors
// ========================================================================

impl Default for SyntaxColors {
    fn default() -> Self {
        Self {
            colors: [None; SCOPE_COUNT],
        }
    }
}

impl SyntaxColors {
    /// magic-vscode's One Color token colors, for the dark preset.
    pub fn magic() -> Self {
        let mut colors = Self::default();
        for (slot, rule) in colors.colors.iter_mut().zip(SCOPE_RULES) {
            *slot = rule.magic.and_then(Rgb::parse_hex);
        }
        colors
    }

    /// These colors, each darkened or lightened toward the far end from
    /// `background` as little as reads on it: the dark preset's hues, kept
    /// recognisable, for a light background.
    pub fn legible_on(mut self, background: Rgb) -> Self {
        for color in self.colors.iter_mut().flatten() {
            *color = color.legible_on(background, LEGIBLE_CONTRAST);
        }
        self
    }

    /// Whether `name` names a row, as a scope selector or an old name.
    pub fn knows(name: &str) -> bool {
        row_of(name).is_some()
    }

    /// The config name of every color: the scope selector of each row.
    pub fn names() -> impl Iterator<Item = &'static str> {
        SCOPE_RULES.iter().map(|rule| rule.scope)
    }

    /// The color of row `index` of [`SCOPE_RULES`].
    pub fn at(&self, index: usize) -> Option<Rgb> {
        self.colors.get(index).copied().flatten()
    }

    /// The color called `name` in the config.
    pub fn get(&self, name: &str) -> Option<Rgb> {
        self.at(row_of(name)?)
    }

    /// Set the color called `name`; a name no row has is ignored.
    pub fn set(&mut self, name: &str, color: Option<Rgb>) {
        if let Some(index) = row_of(name) {
            self.colors[index] = color;
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The row a config name stands for: a scope selector, or one of the names
/// the colors went by before.
fn row_of(name: &str) -> Option<usize> {
    let scope = ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map_or(name, |(_, scope)| scope);
    SCOPE_RULES.iter().position(|rule| rule.scope == scope)
}

/// A row that sets the color alone.
const fn fg(scope: &'static str, hex: &'static str) -> ScopeRule {
    ScopeRule {
        bold: false,
        italic: false,
        magic: Some(hex),
        scope,
        weighs: false,
    }
}

const fn bold(scope: &'static str, hex: &'static str) -> ScopeRule {
    ScopeRule {
        bold: true,
        weighs: true,
        ..fg(scope, hex)
    }
}

const fn italic(scope: &'static str, hex: &'static str) -> ScopeRule {
    ScopeRule {
        italic: true,
        weighs: true,
        ..fg(scope, hex)
    }
}

/// A row that keeps the text's color and only changes its weight.
const fn plain(scope: &'static str, bold: bool, italic: bool) -> ScopeRule {
    ScopeRule {
        bold,
        italic,
        magic: None,
        scope,
        weighs: true,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_every_scope_is_named_once_and_every_color_parses() {
        for (index, rule) in SCOPE_RULES.iter().enumerate() {
            assert_eq!(row_of(rule.scope), Some(index), "{} twice", rule.scope);
            if let Some(hex) = rule.magic {
                assert!(Rgb::parse_hex(hex).is_some(), "{hex}");
            }
        }
    }

    #[test]
    fn test_colors_made_legible_keep_their_contrast_and_leave_readable_ones_be() {
        let light = Rgb::parse_hex("#f2f2f2").expect("a color");
        let fitted = SyntaxColors::magic().legible_on(light);
        for (index, rule) in SCOPE_RULES.iter().enumerate() {
            if let Some(color) = fitted.at(index) {
                assert!(
                    contrast_ratio(color, light) >= LEGIBLE_CONTRAST - 0.05,
                    "{}",
                    rule.scope
                );
            }
        }
        let dark = Rgb::parse_hex("#262a32").expect("a color");
        let keyword = SyntaxColors::magic().get("keyword");
        assert_eq!(
            SyntaxColors::magic().legible_on(dark).get("keyword"),
            keyword
        );
    }

    #[test]
    fn test_the_old_names_still_reach_their_scopes() {
        let magic = SyntaxColors::magic();
        assert_eq!(magic.get("type"), Rgb::parse_hex("#cf517a"));
        assert_eq!(magic.get("number"), magic.get("constant.numeric"));

        let mut colors = SyntaxColors::default();
        colors.set("type", Rgb::parse_hex("#010203"));
        assert_eq!(colors.get("entity.name.type"), Rgb::parse_hex("#010203"));
        colors.set("no.such.scope", Rgb::parse_hex("#010203"));
        assert_eq!(colors.get("no.such.scope"), None);
    }
}
