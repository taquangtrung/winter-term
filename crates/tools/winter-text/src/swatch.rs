//! Color swatches: each color a file writes as `#rrggbb` or `#rrggbbaa`
//! drawn on the color it names, its text dark on a light color and light
//! on a dark one. Each document's colors are found again only once its
//! text changes, and laid on it as Cherry's washes, one host style for
//! each color, numbered past the highlighter's own.

use cherry::{Decoration, DecorationKind, DocumentId, HighlightId};
use misty::color::Rgb;
use misty_cherry::HighlightStyle;

use super::session::Session;

// ========================================================================
// Constants
// ========================================================================

/// The owner of the washes the swatches are laid on a document as.
const SWATCHES: &str = "swatches";

/// How light a color is (its relative luminance, from 0 to 1) past which
/// its text is dark.
const LIGHT_LUMINANCE: f32 = 0.55;

/// The text of a swatch on a light color, and on a dark one.
const DARK_TEXT: Rgb = Rgb::new(0, 0, 0);
const LIGHT_TEXT: Rgb = Rgb::new(255, 255, 255);

/// How many hex digits follow the `#` of a color: its channels, and its
/// channels with an opacity.
const DIGITS: [usize; 2] = [6, 8];

// ========================================================================
// Data Structures
// ========================================================================

/// The swatches laid on a document: the text's version they were found in,
/// the first highlight id they are numbered from, and their colors by id.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Swatched {
    base: usize,
    colors: Vec<Rgb>,
    version: Option<u64>,
}

/// A color written in a line: the chars from column `start` up to column
/// `end`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Swatch {
    color: Rgb,
    end: usize,
    start: usize,
}

// ========================================================================
// Session: swatches
// ========================================================================

impl Session {
    /// Lays the swatches on `document`'s colors, where they are on, their
    /// ids numbered from `base`: the styles to draw those ids by, in order.
    pub(crate) fn lay_swatches(
        &mut self,
        document: DocumentId,
        base: usize,
    ) -> Vec<HighlightStyle> {
        let version = self.editor.document_version(document);
        let on = self.swatches_on;
        let Some(file) = self.files.get_mut(&document) else {
            return Vec::new();
        };
        if !on {
            if file.swatched.take().is_some() {
                self.editor.clear_decorations(document, SWATCHES);
            }
            return Vec::new();
        }
        if let Some(laid) = &file.swatched {
            if laid.version == version && laid.base == base {
                return laid.colors.iter().copied().map(style_of).collect();
            }
        }
        let mut colors: Vec<Rgb> = Vec::new();
        let mut washes = Vec::new();
        for (line, text) in self.lines(document).iter().enumerate() {
            for found in swatches(text) {
                let at = match colors.iter().position(|color| *color == found.color) {
                    Some(at) => at,
                    None => {
                        colors.push(found.color);
                        colors.len() - 1
                    }
                };
                let Ok(id) = u16::try_from(base + at) else {
                    continue;
                };
                let start = self.offset_of(document, line, found.start);
                let range = start..start + (found.end - found.start);
                let kind = DecorationKind::Background(HighlightId::new(id));
                washes.push(Decoration::new(range, kind));
            }
        }
        self.editor.set_decorations(document, SWATCHES, washes);
        let styles = colors.iter().copied().map(style_of).collect();
        if let Some(file) = self.files.get_mut(&document) {
            file.swatched = Some(Swatched {
                base,
                colors,
                version,
            });
        }
        styles
    }

    /// Turns the swatches off where they are on, and on where they are
    /// off, in every document. Whether they are on now.
    pub(crate) fn toggle_swatches(&mut self) -> bool {
        self.swatches_on = !self.swatches_on;
        self.swatches_on
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The colors `line` writes, each a `#` and six or eight hex digits that
/// make a word of their own.
fn swatches(line: &str) -> Vec<Swatch> {
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
                color: Rgb::new(channel(0), channel(2), channel(4)),
                end: at + 1 + run,
                start: at,
            });
        }
        at += 1 + run;
    }
    found
}

/// How a swatch of `color` is drawn: on it, in the text color that reads
/// on it.
fn style_of(color: Rgb) -> HighlightStyle {
    let luminance =
        (0.2126 * f32::from(color.r) + 0.7152 * f32::from(color.g) + 0.0722 * f32::from(color.b))
            / 255.0;
    let text = match luminance > LIGHT_LUMINANCE {
        true => DARK_TEXT,
        false => LIGHT_TEXT,
    };
    HighlightStyle {
        background: Some(color),
        foreground: Some(text),
        ..HighlightStyle::default()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use crate::session;

    #[test]
    fn test_a_color_written_as_a_word_is_found_and_nothing_else() {
        let found = swatches(r##"cursor "#51afef" band "#14708acc" "##);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].start, found[0].end), (8, 15));
        assert_eq!(found[0].color, Rgb::new(0x51, 0xaf, 0xef));
        assert_eq!(found[1].end - found[1].start, 9, "with its opacity");
        assert!(swatches("see #readme and #fff and #1234567").is_empty());
        assert!(swatches("a#ffffff").is_empty(), "inside a word");
        assert!(swatches("#ffffffff0").is_empty(), "a longer run");
    }

    #[test]
    fn test_a_swatch_writes_dark_on_a_light_color_and_light_on_a_dark_one() {
        assert_eq!(
            style_of(Rgb::new(0xf2, 0xf2, 0xf2)).foreground,
            Some(DARK_TEXT)
        );
        assert_eq!(
            style_of(Rgb::new(0x1b, 0x22, 0x29)).foreground,
            Some(LIGHT_TEXT)
        );
    }

    #[test]
    fn test_each_color_is_washed_by_its_own_id_past_the_base_and_toggled_off() {
        let path = std::env::temp_dir().join(format!("winter-swatch-{}.css", std::process::id()));
        std::fs::write(
            &path,
            "a { color: #ff0000; }\nb { color: #00ff00; border: #ff0000; }\n",
        )
        .expect("written");
        let (document, view) = session::with(|s| s.open_view(&path)).expect("opens");
        let washes = |s: &Session| -> Vec<(std::ops::Range<usize>, DecorationKind)> {
            s.editor
                .decorations(document, SWATCHES)
                .iter()
                .map(|wash| (wash.range.clone(), wash.kind))
                .collect()
        };
        session::with(|s| {
            let styles = s.lay_swatches(document, 10);
            let backgrounds: Vec<_> = styles.iter().map(|style| style.background).collect();
            assert_eq!(
                backgrounds,
                [Some(Rgb::new(255, 0, 0)), Some(Rgb::new(0, 255, 0))],
                "one style per color, in the order first written"
            );
            let id = |at| DecorationKind::Background(HighlightId::new(at));
            let second = s.offset_of(document, 1, 11);
            let third = s.offset_of(document, 1, 28);
            assert_eq!(
                washes(s),
                [
                    (11..18, id(10)),
                    (second..second + 7, id(11)),
                    (third..third + 7, id(10))
                ]
            );
            assert!(!s.toggle_swatches());
            assert!(s.lay_swatches(document, 10).is_empty());
            assert!(washes(s).is_empty(), "off, the washes go");
            assert!(s.toggle_swatches());
            assert_eq!(s.lay_swatches(document, 10).len(), 2, "and come back on");
            s.close_view(document, view);
        });
        let _ = std::fs::remove_file(path);
    }
}
