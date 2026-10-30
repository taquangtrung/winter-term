//! The colors source is read in: a table of TextMate scope selectors, each
//! with the weight magic-vscode's One Color theme gives it and whether that
//! theme colors it, and the per-theme colors laid over that table.
//!
//! The table is the vocabulary a grammar's scopes are matched against, so a
//! file is colored the way VS Code colors it under the same theme. Most rows
//! are the theme's own `tokenColors`; the rest, at the end, name the same
//! things the way the Sublime grammars the editor reads with spell them
//! (`entity.name.struct` where VS Code says `entity.name.type`).

use crate::theme::Rgb;

// ========================================================================
// Constants
// ========================================================================

/// How many rows the scope table has.
pub const SCOPE_COUNT: usize = SCOPE_RULES.len();

/// The names the syntax colors went by before they were scopes, and the
/// scope each now stands for, so a config written then still reads.
const ALIASES: [(&str, &str); 2] = [("number", "constant.numeric"), ("type", "entity.name.type")];

/// Every scope the editor tells apart, in the order the theme lists them. A
/// later row wins over an earlier one that matches a scope just as closely,
/// as a later rule does in a VS Code theme.
pub const SCOPE_RULES: &[ScopeRule] = &[
    fg("comment"),
    fg("comment.block.documentation"),
    fg("comment.block.documentation punctuation.definition.comment"),
    fg("comment.documentation"),
    fg("comment.inline-code"),
    fg("comment.line.documentation"),
    fg("comment.line.documentation punctuation.definition.comment"),
    fg("constant"),
    fg("constant.other.reference.citation.latex"),
    fg("constant.other.reference.label.latex"),
    fg("constant.character.math.tex"),
    fg("constant.character.escape.tex"),
    fg("constant.numeric.math.tex"),
    fg("constant.language.boolean"),
    fg("constant.language.python"),
    fg("constant.numeric"),
    fg("constant.other.language"),
    plain("emphasis", false, true),
    fg("entity.name.function"),
    fg("entity.name.function.call"),
    fg("entity.name.function.definition"),
    fg("entity.name.function.math.tex"),
    fg("entity.name.namespace"),
    fg("entity.name.operator"),
    fg("entity.name.scope-resolution"),
    bold("entity.name.section.latex"),
    fg("entity.name.tag.environment.latex"),
    fg("entity.name.tag"),
    fg("entity.name.type"),
    fg("entity.name.type.class"),
    fg("entity.name.type.module"),
    fg("entity.name.type.template"),
    fg("entity.other.attribute-name"),
    fg("fenced_code.block.language"),
    bold("heading.1.markdown"),
    bold("heading.2.markdown"),
    bold("heading.3.markdown"),
    bold("heading.4.markdown punctuation.definition.heading.markdown"),
    bold("heading.4.markdown"),
    bold("heading.5.markdown punctuation.definition.heading.markdown"),
    bold("heading.5.markdown"),
    bold("heading.6.markdown punctuation.definition.heading.markdown"),
    bold("heading.6.markdown"),
    fg("keyword"),
    fg("keyword.control.block"),
    fg("keyword.cmake"),
    fg("keyword.control"),
    fg("keyword.control.directive"),
    fg("keyword.operator"),
    fg("keyword.operator.new"),
    fg("keyword.other.operator"),
    fg("keyword.other.unit"),
    bold("markup.bold"),
    fg("markup.checkbox"),
    fg("markup.checkbox.checked"),
    fg("markup.deleted.diff"),
    fg("markup.fenced_code"),
    fg("markup.fenced_code.block"),
    fg("markup.heading"),
    bold("markup.heading.setext.1"),
    fg("markup.inline.raw.string"),
    fg("markup.inserted.diff"),
    italic("markup.italic"),
    fg("markup.link.latex"),
    fg("markup.list"),
    fg("markup.list.numbered.markdown punctuation.definition.list.begin.markdown"),
    fg("markup.math"),
    fg("markup.underline.link"),
    fg("meta.attribute.rust"),
    fg("meta.diff"),
    bold("meta.diff.range"),
    fg("meta.diff.header.from-file"),
    fg("meta.diff.header.to-file"),
    fg("meta.embedded.block"),
    fg("meta.function.python"),
    fg("meta.interpolation.rust"),
    fg("meta.math"),
    fg("meta.parameter"),
    fg("meta.preprocessor"),
    fg("meta.macro.section.paragraph.latex"),
    bold("meta.macro.section.subsection.latex entity.name.section.latex"),
    bold("meta.macro.section.subsubsection.latex entity.name.section.latex"),
    fg("meta.structure.array.json"),
    fg("meta.structure.dictionary.json"),
    fg("meta.template"),
    fg("meta.template.call"),
    fg("punctuation.definition.arguments.begin.math.tex"),
    fg("punctuation.definition.arguments.end.math.tex"),
    fg("punctuation.definition.begin.bracket.square"),
    fg("punctuation.definition.bold"),
    fg("punctuation.definition.comment"),
    fg("punctuation.definition.decorator.python"),
    fg("punctuation.definition.deleted.diff"),
    fg("punctuation.definition.directive"),
    fg("punctuation.definition.end.bracket.square"),
    fg("punctuation.definition.function.math.tex"),
    bold("punctuation.definition.heading.markdown"),
    fg("punctuation.definition.inserted.diff"),
    fg("punctuation.definition.link"),
    fg("punctuation.definition.list"),
    fg("punctuation.definition.markdown"),
    fg("punctuation.definition.metadata.markdown"),
    fg("punctuation.definition.range.diff"),
    fg("punctuation.definition.string"),
    fg("punctuation.section.angle-brackets.end"),
    fg("punctuation.section.angle-brackets.start"),
    fg("punctuation.section.block.begin.bracket.curly"),
    fg("punctuation.section.block.begin.bracket.curly.class"),
    fg("punctuation.section.block.begin.bracket.curly.namespace"),
    fg("punctuation.section.block.begin.bracket.round"),
    fg("punctuation.section.block.end.bracket.curly"),
    fg("punctuation.section.block.end.bracket.curly.class"),
    fg("punctuation.section.block.end.bracket.curly.namespace"),
    fg("punctuation.section.block.end.bracket.round"),
    fg("punctuation.section.parameters"),
    fg("punctuation.section.parens.begin.bracket.round"),
    fg("punctuation.section.parens.end.bracket.round"),
    fg("punctuation.separator.array.json"),
    fg("punctuation.separator.delimiter"),
    fg("punctuation.separator.dictionary.key-value.json"),
    fg("punctuation.separator.dictionary.pair.json"),
    fg("punctuation.separator.scope-resolution"),
    fg("punctuation.vararg-ellipses"),
    fg("storage.modifier"),
    fg("storage.modifier.reference"),
    fg("storage.type"),
    fg("storage.type.builtin"),
    fg("storage.type.class"),
    fg("storage.type.modifier"),
    fg("storage.type.namespace.definition"),
    fg("storage.type.primitive"),
    fg("storage.type.template"),
    fg("string"),
    fg("string.other.link"),
    fg("string.quoted"),
    fg("string.template"),
    plain("strong", true, false),
    fg("support.function"),
    fg("keyword.control.macro.latex"),
    fg("support.function.builtin"),
    fg("support.function.general.tex"),
    fg("support.type.primitive"),
    fg("support.type.property-name.json"),
    fg("variable.declaration"),
    fg("variable.language"),
    fg("variable.language.special.self.python"),
    fg("variable.other"),
    fg("variable.other.enummember"),
    fg("variable.other.global"),
    fg("variable.other.local"),
    fg("variable.other.property"),
    fg("variable.parameter.function.language.python"),
    fg("variable.parameter.function.language.special.self.python"),
    fg("variable.parameter.macro.latex"),
    fg("variable.parameter.key.latex"),
    fg("variable.parameter.value.latex"),
    fg("variable.parameter.macro.generic.latex"),
    fg("entity.name.section.toml"),
    fg("punctuation.definition.table.toml"),
    fg("punctuation.definition.table.array.toml"),
    fg("variable.other.key.toml"),
    fg("punctuation.separator.key-value.toml"),
    fg("punctuation.separator.dot.toml"),
    // The same things as the Sublime grammars name them. Types: what VS Code
    // calls `entity.name.type`, and the types a language comes with.
    fg("entity.name.class"),
    fg("entity.name.enum"),
    fg("entity.name.impl"),
    fg("entity.name.interface"),
    fg("entity.name.struct"),
    fg("entity.name.trait"),
    fg("entity.name.union"),
    fg("support.type"),
    // A function where it is called, which VS Code's grammars name where
    // the Sublime ones say `variable.function`.
    fg("variable.function"),
    // What VS Code's semantic tokens color: macros, members, attributes.
    fg("support.macro"),
    fg("variable.other.member"),
    fg("meta.annotation"),
    fg("constant.other.placeholder"),
    // Rust's booleans, which rust-analyzer colors as VS Code's grammars do
    // `constant.language.boolean`; every other language constant keeps
    // `constant`'s color, as in VS Code.
    fg("constant.language.rust"),
    // Markdown, whose headings Sublime numbers on `markup.heading`.
    bold("markup.heading.1"),
    bold("markup.heading.2"),
    bold("markup.heading.3"),
    bold("markup.heading.4"),
    bold("markup.heading.5"),
    bold("markup.heading.6"),
    bold("punctuation.definition.heading"),
    bold("markup.heading.4 punctuation.definition.heading"),
    bold("markup.heading.5 punctuation.definition.heading"),
    bold("markup.heading.6 punctuation.definition.heading"),
    fg("markup.raw.inline"),
    fg("markup.raw.block"),
    fg("markup.raw.code-fence"),
    // The code a fence holds in a language it names, which VS Code colors
    // as `meta.embedded.block` where that language does not color it.
    fg("markup.raw.code-fence source"),
    fg("punctuation.definition.raw.code-fence"),
    fg("constant.other.language-name.markdown"),
    fg("meta.link.inline.description"),
    fg("punctuation.definition.metadata"),
    fg("punctuation.definition.list_item.markdown"),
    fg("markup.list.numbered.bullet.markdown"),
    fg("markup.list.numbered.bullet.markdown punctuation.definition.list_item.markdown"),
    // A commit message's summary is drawn in the text's own color and weight,
    // not in `markup.heading`'s red, which would read as a warning.
    fg("markup.heading.subject.git.commit"),
    // LaTeX: every command, which VS Code's grammar names
    // `keyword.control.macro.latex`, a package's options, an environment's
    // name, math, and what a label, a reference, a citation and a link hold.
    fg("text.tex.latex support.function"),
    fg("meta.preamble.usepackage.latex meta.group.bracket.latex - punctuation"),
    fg("variable.parameter.function.latex"),
    fg("meta.environment.math"),
    fg("string.other.math"),
    fg("variable.other.math"),
    fg("support.function.math.tex"),
    fg("keyword.operator.math.tex"),
    fg("meta.environment.math keyword"),
    fg("meta.environment.math support.function"),
    fg("meta.environment.math punctuation.definition.string"),
    fg("entity.name.label.latex"),
    fg("constant.other.reference.latex"),
    fg("constant.other.citation.latex"),
    fg("markup.underline.link.latex"),
    fg("support.function.general.latex"),
    fg("constant.character.newline.latex"),
    fg("entity.name.newcommand.latex"),
    // JSON: a key, and the brackets of an object and of an array.
    fg("meta.mapping.key.json string.quoted"),
    fg("meta.mapping.key.json punctuation.definition.string"),
    fg("source.json punctuation.section.mapping"),
    fg("source.json punctuation.section.sequence"),
    // TOML: a key, which VS Code's TOML grammar leaves in the text's color,
    // the dot inside one, a table's brackets (not an array of tables'), and
    // a boolean.
    fg("entity.name.tag.toml"),
    fg("punctuation.separator.key.toml"),
    fg("punctuation.definition.table.begin.toml"),
    fg("punctuation.definition.table.end.toml"),
    fg("constant.language.toml"),
    // Python: a docstring, which VS Code reads as a string, a decorator, the
    // punctuation of a function's parameters, an f-string's braces, and the
    // base of a number.
    fg("comment.block.documentation.python"),
    fg("comment.block.documentation.python punctuation.definition.comment"),
    fg("meta.annotation.python punctuation.definition.annotation"),
    fg("meta.annotation.python variable.annotation"),
    fg("meta.function.parameters.python punctuation.section.parameters"),
    fg("meta.function.parameters.python punctuation.separator"),
    fg("source.python punctuation.separator.annotation"),
    fg("meta.interpolation.python punctuation.section.interpolation"),
    fg("source.python punctuation.definition.numeric.base"),
    // Shell: a command, an option, a variable's `$`, a bare argument, which
    // VS Code reads as a string, and the separators between commands, which
    // it leaves in the text's color.
    fg("variable.function.shell"),
    fg("variable.parameter.option.shell"),
    fg("meta.group.expansion.parameter.shell punctuation.definition.variable"),
    fg("meta.function-call.arguments.shell"),
    fg("keyword.operator.logical.and.shell"),
    fg("keyword.operator.logical.continue.shell"),
    fg("keyword.operator.logical.or.shell"),
    fg("support.function.double-brace"),
    // Make: assignments and bare words in the text's color, and a variable
    // a recipe expands.
    fg("keyword.operator.assignment.makefile"),
    fg("string.unquoted.makefile"),
    fg("variable.parameter.makefile"),
    fg("variable.parameter.makefile keyword.other.block"),
    // CSS: a combinator, a pseudo-class, and a property's name.
    fg("punctuation.separator.combinator.css"),
    fg("entity.other.pseudo-class.css"),
    fg("entity.other.pseudo-element.css"),
    fg("support.type.property-name.css"),
    // HTML's doctype, as VS Code reads it: a tag and its attribute.
    fg("keyword.declaration.doctype.html"),
    fg("constant.language.doctype.html"),
    // JavaScript's `.`, which VS Code leaves in the text's color.
    fg("keyword.operator.accessor.js"),
    // What a language server's semantic tokens color in VS Code, as Sublime's
    // grammars name it: a base class, a library class, and a generic's type
    // parameter as types, a declared constant, a module as a namespace, a
    // macro's name, a parameter where it is declared (but Python's, which
    // the theme leaves in the text's color), an object's property, a TOML
    // table's name as its section, and a YAML anchor as VS Code's grammar
    // reads it.
    fg("entity.other.inherited-class"),
    fg("support.class.java"),
    fg("support.class.import.java"),
    fg("support.class.builtin"),
    fg("variable.parameter.type.java"),
    fg("entity.name.other.anchor.yaml"),
    fg("entity.name.constant"),
    fg("support.other.module"),
    fg("entity.name.module"),
    fg("entity.name.macro"),
    fg("variable.parameter"),
    fg("variable.parameter.python"),
    fg("variable.object.property"),
    fg("entity.name.table.toml"),
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
    /// The TextMate scope selector, written as a VS Code theme writes one.
    pub scope: &'static str,
    /// Whether the row sets the text's color, rather than only its weight.
    sets_color: bool,
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
        self.sets_color
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
const fn fg(scope: &'static str) -> ScopeRule {
    ScopeRule {
        bold: false,
        italic: false,
        scope,
        sets_color: true,
        weighs: false,
    }
}

const fn bold(scope: &'static str) -> ScopeRule {
    ScopeRule {
        bold: true,
        weighs: true,
        ..fg(scope)
    }
}

const fn italic(scope: &'static str) -> ScopeRule {
    ScopeRule {
        italic: true,
        weighs: true,
        ..fg(scope)
    }
}

/// A row that keeps the text's color and only changes its weight.
const fn plain(scope: &'static str, bold: bool, italic: bool) -> ScopeRule {
    ScopeRule {
        bold,
        italic,
        scope,
        sets_color: false,
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
    fn test_every_scope_is_named_once() {
        for (index, rule) in SCOPE_RULES.iter().enumerate() {
            assert_eq!(row_of(rule.scope), Some(index), "{} twice", rule.scope);
        }
    }

    #[test]
    fn test_the_old_names_still_reach_their_scopes() {
        let mut colors = SyntaxColors::default();
        colors.set("type", Rgb::parse_hex("#010203"));
        assert_eq!(colors.get("entity.name.type"), Rgb::parse_hex("#010203"));
        colors.set("number", Rgb::parse_hex("#040506"));
        assert_eq!(colors.get("constant.numeric"), Rgb::parse_hex("#040506"));
        colors.set("no.such.scope", Rgb::parse_hex("#010203"));
        assert_eq!(colors.get("no.such.scope"), None);
    }
}
