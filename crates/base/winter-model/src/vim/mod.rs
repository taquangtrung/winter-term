//! The Vim foundation: the layers every Vim surface in Winter stands on.
//!
//! The architecture is layered bottom-up:
//!
//! 1. **This module** — the vocabulary ([`motion::CursorMove`]), the text
//!    primitives ([`words`], [`objects`]), the key mapping ([`nav`]), and the
//!    grammar both modal surfaces resolve through ([`grammar`]: counts,
//!    operators, registers, marks, the `g` and `z` motions).
//!    Nothing here knows about grids, panes, or editors.
//! 2. **The grid surface** — `model::input` resolves the full Vim grammar
//!    (counts, operators, text objects) into these motions for the terminal,
//!    and `window::terminal::navigation` executes them over the live grid.
//! 3. **The editors**: `editors::dir`, `editors::git`, and their kin consume the
//!    same foundation through [`nav::VimNav`], interpreting the motions over
//!    their own content.
//!
//! The override contract: a surface's own key handling runs *first*, so any
//! binding an editor claims (dir's `l` expands, its `G` reloads; git's `g`
//! sequence jumps sections) wins over the foundation's default; what it does
//! not claim falls through, and the surface interprets the resulting motion.

pub mod grammar;
pub mod jumps;
pub mod motion;
pub mod nav;
pub mod objects;
pub mod words;
