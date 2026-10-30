//! Pure interaction model: modes, layout geometry, key resolution, the palette,
//! the settings page, the editors tabs hold, and the styled rows they paint.
//!
//! These modules share no external dependencies (std-only) and no side effects.

pub mod button;
pub mod editor;
pub mod guard;
pub mod history;
pub mod input;
pub mod layout;
pub mod mode;
pub mod notify;
pub mod paint;
pub mod picker;
pub mod path;
pub mod picture;
pub mod process;
pub mod settings_page;
pub mod spinner;
pub mod system;
pub mod textedit;
pub mod units;
pub mod vim;
