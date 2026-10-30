//! Inserting a math or Greek symbol picked from a list, as magic-vscode's
//! symbol picker does (`M-s i`): the symbol itself, or in a LaTeX file the
//! command that writes it.

use crate::model::editor::{EditorOutcome, PickPlacement, PickRequest};

use super::writing::Prose;
use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// The question the list answers.
pub(super) const ASK_SYMBOL: &str = "symbol";

/// What sets a symbol's columns apart in the list.
const GAP: &str = "   ";

/// Every symbol offered: the symbol, the LaTeX that writes it, and what it
/// is called.
const SYMBOLS: &[(&str, &str, &str)] = &[
    ("α", "\\alpha", "Greek small letter alpha"),
    ("β", "\\beta", "Greek small letter beta"),
    ("γ", "\\gamma", "Greek small letter gamma"),
    ("δ", "\\delta", "Greek small letter delta"),
    ("ε", "\\epsilon", "Greek small letter epsilon"),
    ("ζ", "\\zeta", "Greek small letter zeta"),
    ("η", "\\eta", "Greek small letter eta"),
    ("θ", "\\theta", "Greek small letter theta"),
    ("ι", "\\iota", "Greek small letter iota"),
    ("κ", "\\kappa", "Greek small letter kappa"),
    ("λ", "\\lambda", "Greek small letter lambda"),
    ("μ", "\\mu", "Greek small letter mu"),
    ("ν", "\\nu", "Greek small letter nu"),
    ("ξ", "\\xi", "Greek small letter xi"),
    ("ο", "\\omicron", "Greek small letter omicron"),
    ("π", "\\pi", "Greek small letter pi"),
    ("ρ", "\\rho", "Greek small letter rho"),
    ("σ", "\\sigma", "Greek small letter sigma"),
    ("τ", "\\tau", "Greek small letter tau"),
    ("υ", "\\upsilon", "Greek small letter upsilon"),
    ("φ", "\\phi", "Greek small letter phi"),
    ("χ", "\\chi", "Greek small letter chi"),
    ("ψ", "\\psi", "Greek small letter psi"),
    ("ω", "\\omega", "Greek small letter omega"),
    ("Γ", "\\Gamma", "Greek capital letter gamma"),
    ("Δ", "\\Delta", "Greek capital letter delta"),
    ("Θ", "\\Theta", "Greek capital letter theta"),
    ("Λ", "\\Lambda", "Greek capital letter lambda"),
    ("Ξ", "\\Xi", "Greek capital letter xi"),
    ("Π", "\\Pi", "Greek capital letter pi"),
    ("Σ", "\\Sigma", "Greek capital letter sigma"),
    ("Υ", "\\Upsilon", "Greek capital letter upsilon"),
    ("Φ", "\\Phi", "Greek capital letter phi"),
    ("Ψ", "\\Psi", "Greek capital letter psi"),
    ("Ω", "\\Omega", "Greek capital letter omega"),
    ("≈", "\\approx", "Almost equal to"),
    ("≠", "\\neq", "Not equal to"),
    ("≡", "\\equiv", "Identical to"),
    ("≤", "\\leq", "Less-than or equal to"),
    ("≥", "\\geq", "Greater-than or equal to"),
    ("≪", "\\ll", "Much less-than"),
    ("≫", "\\gg", "Much greater-than"),
    ("×", "\\times", "Multiplication sign"),
    ("÷", "\\div", "Division sign"),
    ("±", "\\pm", "Plus-minus sign"),
    ("∓", "\\mp", "Minus-plus sign"),
    ("∈", "\\in", "Element of"),
    ("∉", "\\notin", "Not an element of"),
    ("⊂", "\\subset", "Subset of"),
    ("⊃", "\\supset", "Superset of"),
    ("⊆", "\\subseteq", "Subset of or equal to"),
    ("⊇", "\\supseteq", "Superset of or equal to"),
    ("∪", "\\cup", "Union"),
    ("∩", "\\cap", "Intersection"),
    ("⇒", "\\Rightarrow", "Rightwards double arrow / implies"),
    ("⇐", "\\Leftarrow", "Leftwards double arrow / implied by"),
    ("⇔", "\\Leftrightarrow", "Left right double arrow / equivalent to"),
    ("→", "\\rightarrow", "Rightwards arrow"),
    ("←", "\\leftarrow", "Leftwards arrow"),
    ("↔", "\\leftrightarrow", "Left right arrow"),
    ("∞", "\\infty", "Infinity"),
    ("∇", "\\nabla", "Nabla / del"),
    ("∂", "\\partial", "Partial differential"),
    ("∫", "\\int", "Integral"),
    ("∬", "\\iint", "Double integral"),
    ("∭", "\\iiint", "Triple integral"),
    ("∮", "\\oint", "Contour integral"),
    ("∑", "\\sum", "N-ary summation"),
    ("∏", "\\prod", "N-ary product"),
    ("√", "\\sqrt", "Square root"),
    ("∅", "\\emptyset", "Empty set"),
    ("∀", "\\forall", "For all"),
    ("∃", "\\exists", "There exists"),
    ("∄", "\\nexists", "There does not exist"),
    ("∴", "\\therefore", "Therefore"),
    ("∵", "\\because", "Because"),
    ("∝", "\\propto", "Proportional to"),
    ("∠", "\\angle", "Angle"),
    ("°", "^\\circ", "Degree sign"),
    ("⊥", "\\bot", "Up tack / perpendicular"),
    ("∥", "\\parallel", "Parallel to"),
    ("⁰", "^{0}", "Superscript 0"),
    ("¹", "^{1}", "Superscript 1"),
    ("²", "^{2}", "Superscript 2"),
    ("³", "^{3}", "Superscript 3"),
    ("⁴", "^{4}", "Superscript 4"),
    ("⁵", "^{5}", "Superscript 5"),
    ("⁶", "^{6}", "Superscript 6"),
    ("⁷", "^{7}", "Superscript 7"),
    ("⁸", "^{8}", "Superscript 8"),
    ("⁹", "^{9}", "Superscript 9"),
    ("ᵃ", "^{a}", "Superscript a"),
    ("ᵇ", "^{b}", "Superscript b"),
    ("ᶜ", "^{c}", "Superscript c"),
    ("ᵈ", "^{d}", "Superscript d"),
    ("ᵉ", "^{e}", "Superscript e"),
    ("ᶠ", "^{f}", "Superscript f"),
    ("ᵍ", "^{g}", "Superscript g"),
    ("ʰ", "^{h}", "Superscript h"),
    ("ⁱ", "^{i}", "Superscript i"),
    ("ʲ", "^{j}", "Superscript j"),
    ("ᵏ", "^{k}", "Superscript k"),
    ("ˡ", "^{l}", "Superscript l"),
    ("ᵐ", "^{m}", "Superscript m"),
    ("ⁿ", "^{n}", "Superscript n"),
    ("ᵒ", "^{o}", "Superscript o"),
    ("ᵖ", "^{p}", "Superscript p"),
    ("ʳ", "^{r}", "Superscript r"),
    ("ˢ", "^{s}", "Superscript s"),
    ("ᵗ", "^{t}", "Superscript t"),
    ("ᵘ", "^{u}", "Superscript u"),
    ("ᵛ", "^{v}", "Superscript v"),
    ("ʷ", "^{w}", "Superscript w"),
    ("ˣ", "^{x}", "Superscript x"),
    ("ʸ", "^{y}", "Superscript y"),
    ("ᶻ", "^{z}", "Superscript z"),
    ("ᴬ", "^{A}", "Superscript A"),
    ("ᴮ", "^{B}", "Superscript B"),
    ("ᴰ", "^{D}", "Superscript D"),
    ("ᴱ", "^{E}", "Superscript E"),
    ("ᴳ", "^{G}", "Superscript G"),
    ("ᴴ", "^{H}", "Superscript H"),
    ("ᴵ", "^{I}", "Superscript I"),
    ("ᴶ", "^{J}", "Superscript J"),
    ("ᴷ", "^{K}", "Superscript K"),
    ("ᴸ", "^{L}", "Superscript L"),
    ("ᴹ", "^{M}", "Superscript M"),
    ("ᴺ", "^{N}", "Superscript N"),
    ("ᴼ", "^{O}", "Superscript O"),
    ("ᴾ", "^{P}", "Superscript P"),
    ("ᴿ", "^{R}", "Superscript R"),
    ("ᵀ", "^{T}", "Superscript T"),
    ("ᵁ", "^{U}", "Superscript U"),
    ("ⱽ", "^{V}", "Superscript V"),
    ("ᵂ", "^{W}", "Superscript W"),
    ("ᵅ", "^{\\alpha}", "Superscript alpha"),
    ("ᵝ", "^{\\beta}", "Superscript beta"),
    ("ᵞ", "^{\\gamma}", "Superscript gamma"),
    ("ᵟ", "^{\\delta}", "Superscript delta"),
    ("ᵋ", "^{\\epsilon}", "Superscript epsilon"),
    ("ᶿ", "^{\\theta}", "Superscript theta"),
    ("ᶥ", "^{\\iota}", "Superscript iota"),
    ("ᶲ", "^{\\phi}", "Superscript phi"),
    ("ᵡ", "^{\\chi}", "Superscript chi"),
    ("₀", "_{0}", "Subscript 0"),
    ("₁", "_{1}", "Subscript 1"),
    ("₂", "_{2}", "Subscript 2"),
    ("₃", "_{3}", "Subscript 3"),
    ("₄", "_{4}", "Subscript 4"),
    ("₅", "_{5}", "Subscript 5"),
    ("₆", "_{6}", "Subscript 6"),
    ("₇", "_{7}", "Subscript 7"),
    ("₈", "_{8}", "Subscript 8"),
    ("₉", "_{9}", "Subscript 9"),
    ("ₐ", "_{a}", "Subscript a"),
    ("ₑ", "_{e}", "Subscript e"),
    ("ₕ", "_{h}", "Subscript h"),
    ("ᵢ", "_{i}", "Subscript i"),
    ("ⱼ", "_{j}", "Subscript j"),
    ("ₖ", "_{k}", "Subscript k"),
    ("ₗ", "_{l}", "Subscript l"),
    ("ₘ", "_{m}", "Subscript m"),
    ("ₙ", "_{n}", "Subscript n"),
    ("ₒ", "_{o}", "Subscript o"),
    ("ₚ", "_{p}", "Subscript p"),
    ("ᵣ", "_{r}", "Subscript r"),
    ("ₛ", "_{s}", "Subscript s"),
    ("ₜ", "_{t}", "Subscript t"),
    ("ᵤ", "_{u}", "Subscript u"),
    ("ᵥ", "_{v}", "Subscript v"),
    ("ₓ", "_{x}", "Subscript x"),
    ("ᵦ", "_{\\beta}", "Subscript beta"),
    ("ᵧ", "_{\\gamma}", "Subscript gamma"),
    ("ᵨ", "_{\\rho}", "Subscript rho"),
    ("ᵩ", "_{\\phi}", "Subscript phi"),
    ("ᵪ", "_{\\chi}", "Subscript chi"),
];

// ========================================================================
// TextEditor: symbols
// ========================================================================

impl TextEditor {
    /// The symbols, to pick one to put in at the cursor.
    pub(super) fn ask_symbol(&mut self) -> EditorOutcome {
        EditorOutcome::Pick(PickRequest {
            icons: Vec::new(),
            items: SYMBOLS
                .iter()
                .map(|(symbol, latex, name)| format!("{symbol}{GAP}{latex}{GAP}{name}"))
                .collect(),
            label: "Insert symbol".to_string(),
            placement: PickPlacement::Centered,
            tag: ASK_SYMBOL,
        })
    }

    /// Put the symbol `chosen` names in at the cursor: its LaTeX in a LaTeX
    /// file, else the symbol.
    pub(super) fn symbol_chosen(&mut self, chosen: &str) -> EditorOutcome {
        let Some((symbol, latex, _)) = SYMBOLS
            .iter()
            .find(|(symbol, latex, name)| format!("{symbol}{GAP}{latex}{GAP}{name}") == chosen)
        else {
            return EditorOutcome::Consumed;
        };
        let text = match self.prose() {
            Some(Prose::Latex) => latex,
            _ => symbol,
        };
        self.paste_text(text.to_string())
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_every_symbol_is_listed_once_with_its_latex() {
        let mut seen = std::collections::HashSet::new();
        for (symbol, latex, _) in SYMBOLS {
            assert!(seen.insert(*symbol), "{symbol} twice");
            assert!(!latex.is_empty() && !symbol.is_empty());
        }
    }
}
