//! Color swatches: a `#rrggbb` or `#rrggbbaa` written in a file is drawn on
//! the color it names, as magic-vscode's swatches and VS Code's color
//! decorators show one, so a theme or a style sheet reads as what it looks
//! like. The text keeps its place: only its cells' colors change, so a click
//! and the cursor land where they would without it.

use winter_core::RgbColor;

use crate::model::paint::{Style, TerminalColor, TerminalStyle};

// ========================================================================
// Constants
// ========================================================================

/// The luminance past which a swatch's color is light, and its text drawn
/// dark to stay readable on it.
const LIGHT_LUMINANCE: f32 = 0.55;

/// The hex digits a swatch may have after its `#`: a color, or a color and
/// its opacity.
const DIGITS: [usize; 2] = [6, 8];

// ========================================================================
// Data Structures
// ========================================================================

/// One color written in a line: the characters it covers, `start..end`, and
/// the color it names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Swatch {
    pub(super) color: RgbColor,
    pub(super) end: usize,
    pub(super) start: usize,
}

// ========================================================================
// Swatch
// ========================================================================

impl Swatch {
    /// How the swatch's characters are drawn: on its color, in black or white
    /// as the color is light or dark.
    pub(super) fn style(&self) -> Style {
        let RgbColor { r, g, b } = self.color;
        let luminance = (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) / 255.0;
        let text = match luminance > LIGHT_LUMINANCE {
            true => RgbColor { r: 0, g: 0, b: 0 },
            false => RgbColor {
                r: 255,
                g: 255,
                b: 255,
            },
        };
        Style::Terminal(TerminalStyle {
            background: Some(TerminalColor::Rgb(self.color)),
            foreground: Some(TerminalColor::Rgb(text)),
            ..TerminalStyle::default()
        })
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Every color `line` writes as `#` and six or eight hex digits, standing as
/// a word of its own so an anchor (`#readme`) or a longer hex run is left
/// alone. Positions are in characters.
pub(super) fn swatches(line: &str) -> Vec<Swatch> {
    let chars: Vec<char> = line.chars().collect();
    let mut found = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        if chars[at] != '#' || at > 0 && chars[at - 1].is_alphanumeric() {
            at += 1;
            continue;
        }
        let run = chars[at + 1..]
            .iter()
            .take_while(|c| c.is_ascii_hexdigit())
            .count();
        let ends_word = chars
            .get(at + 1 + run)
            .is_none_or(|c| !c.is_alphanumeric() && *c != '_');
        if DIGITS.contains(&run) && ends_word {
            let hex: String = chars[at + 1..at + 7].iter().collect();
            let channel = |from: usize| u8::from_str_radix(&hex[from..from + 2], 16).unwrap_or(0);
            found.push(Swatch {
                color: RgbColor {
                    r: channel(0),
                    g: channel(2),
                    b: channel(4),
                },
                end: at + 1 + run,
                start: at,
            });
        }
        at += 1 + run;
    }
    found
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_color_written_as_a_word_is_found_and_nothing_else() {
        let found = swatches(r##"cursor "#51afef" band "#14708acc" "##);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].start, found[0].end), (8, 15));
        assert_eq!(found[0].color, RgbColor { r: 0x51, g: 0xaf, b: 0xef });
        assert_eq!(found[1].end - found[1].start, 9, "with its opacity");
        assert!(swatches("see #readme and #fff and #1234567").is_empty());
        assert!(swatches("a#ffffff").is_empty(), "inside a word");
        assert!(swatches("#ffffffff0").is_empty(), "a longer run");
    }

    #[test]
    fn test_a_swatch_writes_dark_on_a_light_color_and_light_on_a_dark_one() {
        let text_of = |hex: &str| match swatches(hex)[0].style() {
            Style::Terminal(style) => style.foreground,
            other => panic!("a terminal style, got {other:?}"),
        };
        assert_eq!(text_of("#f2f2f2"), Some(TerminalColor::Rgb(RgbColor { r: 0, g: 0, b: 0 })));
        assert_eq!(
            text_of("#1b2229"),
            Some(TerminalColor::Rgb(RgbColor { r: 255, g: 255, b: 255 }))
        );
    }
}
