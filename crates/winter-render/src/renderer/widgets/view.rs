//! The views the widgets are drawn from, handed over by the app each frame.

use std::sync::Arc;

// ========================================================================
// Data Structures
// ========================================================================

/// One entry shown in the command palette results list.
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteItem {
    /// Command id dispatched when this entry is chosen.
    pub action: String,
    /// SVG drawn before the label, naming what the entry is (a file's type,
    /// a folder). Shared, so a list of a thousand `.rs` files holds one copy
    /// of the artwork and comparing views stays a pointer check.
    pub icon: Option<Arc<[u8]>>,
    /// Human-readable entry text shown in the list.
    pub label: String,
    /// Char indices in `label` that matched the query, used to highlight them.
    pub match_positions: Vec<usize>,
    /// Keyboard shortcut hint shown on the right (e.g. `"C-S-t"`).
    pub shortcut: String,
}
/// The command palette state the renderer needs to draw its overlay.
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteView {
    /// Whether the palette is docked to the top of the window with 1/2 window height.
    pub align_top: bool,
    /// Text shown when the filtered list is empty.
    pub empty_message: String,
    /// The entries to draw, already filtered and ranked by the caller.
    pub items: Vec<PaletteItem>,
    /// How many characters of the query sit before the caret, which is drawn
    /// between them and the rest.
    pub caret: usize,
    /// Whether the caret is in its shown phase: it blinks with the terminal's
    /// cursor, and is always shown when cursor blinking is off.
    pub caret_visible: bool,
    /// Draw an underline under highlighted match characters.
    pub match_underline: bool,
    /// The filter text as typed.
    pub query: String,
    /// What the list is of, drawn as a heading above the input line. Empty
    /// for a palette that needs no naming, which draws no heading at all.
    pub title: String,
    /// Index into `items` of the highlighted entry.
    pub selected: usize,
}
/// A search being typed, drawn on a small card at the top-right of the pane
/// it searches.
#[derive(Clone, Debug, PartialEq)]
pub struct FindView {
    /// How many characters of the query sit before the caret.
    pub caret: usize,
    /// Whether the caret is in its shown phase (see
    /// [`PaletteView::caret_visible`]).
    pub caret_visible: bool,
    /// The search's direction, drawn beside the field: `/` or `?`.
    pub label: String,
    /// The pane being searched, as its left edge, top edge, and width in
    /// surface pixels: the card hangs from its top-right corner.
    pub pane: (f32, f32, f32),
    /// What has been typed.
    pub query: String,
}

/// A question being answered in a floating dialog: what is being asked, what
/// has been typed so far, and how to answer it.
#[derive(Clone, Debug, PartialEq)]
pub struct InputView {
    /// How many characters of the input sit before the caret, which is drawn
    /// between them and the rest.
    pub caret: usize,
    /// Whether the caret is in its shown phase (see
    /// [`PaletteView::caret_visible`]).
    pub caret_visible: bool,
    /// What has been typed, drawn in a field with a caret after it.
    /// `None` for a question answered by one key, which has no line to type
    /// on.
    pub input: Option<String>,
    /// The keys that answer, as `(key, what it does)`, drawn as keycaps
    /// along the dialog's foot. Empty draws nothing.
    pub keys: Vec<(String, String)>,
    /// What is being asked, drawn as the dialog's heading.
    pub label: String,
}

/// The which-key hint overlay state shown when the user pauses mid-prefix.
#[derive(Clone, Debug, PartialEq)]
pub struct WhichKeyView {
    /// The continuations available from the pending prefix, as
    /// `(key, description)`.
    pub items: Vec<(String, String)>,
    /// The prefix already typed, shown as the popup's heading.
    pub title: String,
}
