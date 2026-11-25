//! Winter's colors: the [`Theme`] every part of the window draws in, the
//! KDL theme files it is read from (with the built-in dark and light
//! themes), and the TextMate syntax scopes a theme colors.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod syntax;
mod theme;
mod theme_file;

pub use syntax::{SCOPE_COUNT, SCOPE_RULES, ScopeRule, SyntaxColors};
pub use theme::{GitColors, PAGE_DIM_MIX, Rgb, Theme};
pub use theme_file::{
    ColorOverrides, KdlColors, SCALAR_COLORS, ScalarColor, kdl_string, parse_theme_file, theme_kdl,
};

/// How far non-focused pane colors are lerped toward the background (0 = full,
/// 1 = fully dimmed). Public so what the host draws over a pane itself, such
/// as an editor's icons, can fade by the same amount.
pub const DIM_FACTOR: f32 = 0.4;

/// How opaque the two alternating tones of a sentence band are, over the
/// background in the cursor line's hue: low and close together, so the bands
/// read as a subtle rhythm rather than a zebra stripe. A terminal's sentences
/// and an editor's take the same two.
pub const SENTENCE_TINT_ALPHAS: [f32; 2] = [0.28, 0.14];
