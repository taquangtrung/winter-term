//! Pure interaction model: modes, layout geometry, key resolution, the palette,
//! the settings page, the tools tabs hold, and the styled rows they paint.
//!
//! These modules share no external dependencies (std-only) and no side effects.

pub mod history;
pub mod input;
pub mod layout;
pub mod mode;
pub mod paint;
pub mod palette;
pub mod path;
pub mod process;
pub mod settings_page;
pub mod system;
pub mod textedit;
pub mod tool;
pub mod units;
pub mod vim;
