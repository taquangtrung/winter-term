//! Modal text editing, from [`winter_textedit`]: Emacs keys while typing,
//! Vim keys for everything else, over text that knows nothing of files or
//! panes. Re-exported here so every editor reaches it through the model.

pub use winter_textedit::{buffer, field, graphemes, kill, modal, recall, search};
