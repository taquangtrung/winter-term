//! The misty theme a widget draws in: the one the renderer keeps, built
//! when Winter's theme last changed, or misty's for the dark preset before
//! there is a renderer.

use std::sync::OnceLock;

use crate::render::renderer::GpuRenderer;
use crate::render::{Theme, Themes};

// ========================================================================
// Functions
// ========================================================================

/// Misty's theme as `renderer` keeps it, or the dark preset's without one.
pub(in crate::window) fn misty_theme(renderer: Option<&GpuRenderer>) -> &misty::Theme {
    static DARK: OnceLock<Themes> = OnceLock::new();
    match renderer {
        Some(renderer) => renderer.misty_theme(),
        None => DARK.get_or_init(|| Themes::new(Theme::dark())).misty(),
    }
}
