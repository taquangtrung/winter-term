//! Pure interaction model: modes, layout geometry, key resolution, the palette,
//! the settings page, and tool pages.
//!
//! These modules share no external dependencies (std-only) and no side effects.

pub mod history;
pub mod input;
pub mod layout;
pub mod mode;
pub mod page;
pub mod path;
pub mod palette;
pub mod settings_page;
pub mod units;
pub mod vim;
