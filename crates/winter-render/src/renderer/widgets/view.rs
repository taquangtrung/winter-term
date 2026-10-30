//! The views the widgets are drawn from, handed over by the app each frame.

use std::sync::Arc;

// ========================================================================
// Data Structures
// ========================================================================

/// One entry shown in the picker results list.
#[derive(Clone, Debug, PartialEq)]
pub struct PickerItem {
    /// Command id or action payload dispatched when this entry is chosen.
    pub action: String,
    /// SVG drawn before the label, naming what the entry is (a file's type,
    /// a folder). Shared, so a list of a thousand `.rs` files holds one copy
    /// of the artwork and comparing views stays a pointer check.
    pub icon: Option<Arc<[u8]>>,
    /// Human-readable entry text shown in the list.
    pub label: String,
    /// Char indices in `label` that matched the query, used to highlight them.
    pub match_positions: Vec<usize>,
    /// Optional right-side hint or shortcut shown on the right (e.g. `"C-S-t"`).
    pub shortcut: String,
}

/// The picker state the renderer needs to draw its overlay.
#[derive(Clone, Debug, PartialEq)]
pub struct PickerView {
    /// The cell the list floats from, as its left edge, top edge, and
    /// bottom edge in surface pixels, for a list of what can be done at the
    /// cursor; `None` for a list centred on the window under a title bar.
    pub anchor: Option<(f32, f32, f32)>,
    /// Text shown when the filtered list is empty.
    pub empty_message: String,
    /// The entries to draw, already filtered and ranked by the caller.
    pub items: Vec<PickerItem>,
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
    /// for a picker that needs no naming, which draws no heading at all.
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
    /// [`PickerView::caret_visible`]).
    pub caret_visible: bool,
    /// The search's direction, drawn beside the field: `/` or `?`.
    pub label: String,
    /// The pane being searched, as its left edge, top edge, and width in
    /// surface pixels: the card hangs from its top-right corner.
    pub pane: (f32, f32, f32),
    /// What has been typed.
    pub query: String,
}

/// One row of a drop-down list.
#[derive(Clone, Debug, PartialEq)]
pub struct DropListRow {
    /// Whether it is drawn dim: an entry that leads nowhere.
    pub dim: bool,
    /// Whether it folds, and if so whether it is unfolded.
    pub fold: Option<bool>,
    /// What it reads as.
    pub label: String,
    /// The cell, from the card's left, its chevron is drawn on; its title
    /// starts two cells on.
    pub mark_col: usize,
    /// What is drawn dim at its right: the page an outline entry is on.
    pub note: String,
}

/// A drop-down list an editor opens over its pane, lying on its cells.
#[derive(Clone, Debug, PartialEq)]
pub struct DropListView {
    /// A cell's width and height in surface pixels.
    pub cell: (f32, f32),
    /// How many cells wide the card is.
    pub cols: usize,
    /// The card's top-left corner in surface pixels.
    pub origin: (f32, f32),
    /// The rows, one to a cell row.
    pub rows: Vec<DropListRow>,
    /// Which row is chosen, if any shows.
    pub selected: Option<usize>,
}

/// One entry of a completion popup.
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionRow {
    /// More about it, drawn dim after it.
    pub detail: Option<String>,
    /// The letter its kind is marked with.
    pub kind: char,
    /// What it reads as.
    pub label: String,
    /// The back-end that offered it, drawn dim at the right.
    pub source: String,
}

/// A completion popup, hung under the word it completes.
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionView {
    /// The word being completed's first cell, as its left edge, top edge,
    /// and bottom edge in surface pixels: the popup hangs under it, or over
    /// it where there is no room below.
    pub anchor: (f32, f32, f32),
    /// The entries, the best first.
    pub rows: Vec<CompletionRow>,
    /// Which entry is chosen.
    pub selected: usize,
}

/// A card of text hung over the word it is about: what a language server
/// says of a name, or the problems found there.
#[derive(Clone, Debug, PartialEq)]
pub struct InfoView {
    /// The word's first cell, as its left edge, top edge, and bottom edge in
    /// surface pixels: the card hangs over it, or under it where there is no
    /// room above.
    pub anchor: (f32, f32, f32),
    /// What kind of thing it is about, drawn apart at the card's foot under
    /// a hairline, dim, as VS Code says what found a problem: `None` for a
    /// card with no foot.
    pub footer: Option<String>,
    /// What it says, one line per line.
    pub text: String,
}

/// How one item of a pane's toolbar is drawn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolbarTone {
    /// In the accent color: a button that is on.
    Accent,
    /// Receded: a separator, or a label with nothing to press.
    Dim,
    /// In the text color.
    Normal,
    /// In the text color, heavier: a button that does something.
    Strong,
}

/// One item of a pane's toolbar: a button, a label, or a separator.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolbarItem {
    /// What pressing it is told back as, for a button; `None` for the rest.
    pub button: Option<u16>,
    /// Whether it gives up characters, ending in an ellipsis, where the bar
    /// is short of room: a title, rather than a button that must stay whole.
    pub shrinks: bool,
    /// What it says.
    pub text: String,
    /// How it is drawn.
    pub tone: ToolbarTone,
}

/// A bar over the top of a pane, drawn in the window's own font rather than
/// in the pane's cells: an editor's toolbar.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolbarView {
    /// The items from the left edge.
    pub left: Vec<ToolbarItem>,
    /// Whose bar it is, told back with a press so the host knows whose
    /// button was pressed.
    pub owner: u64,
    /// The band it covers, as left edge, top edge, width, and height in
    /// surface pixels.
    pub rect: (f32, f32, f32, f32),
    /// The items against the right edge, in order from the left.
    pub right: Vec<ToolbarItem>,
}

/// A button shown in a dialog or toolbar.
#[derive(Clone, Debug, PartialEq)]
pub struct ButtonView {
    /// Whether this button is currently focused by keyboard navigation.
    pub focused: bool,
    /// Whether this button represents the primary/default action.
    pub primary: bool,
    /// Keyboard shortcut hint shown inside the button (e.g. "y", "n", "Esc").
    pub shortcut: Option<String>,
    /// Button label text (e.g. "Yes", "No", "Cancel").
    pub text: String,
}

/// A question being answered in a floating dialog: what is being asked, what
/// has been typed so far, and how to answer it.
#[derive(Clone, Debug, PartialEq)]
pub struct InputView {
    /// Buttons drawn along the dialog foot, if any.
    pub buttons: Vec<ButtonView>,
    /// How many characters of the input sit before the caret, which is drawn
    /// between them and the rest.
    pub caret: usize,
    /// Whether the caret is in its shown phase (see
    /// [`PickerView::caret_visible`]).
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
    /// Optional title shown centered above the message for confirmations.
    pub title: Option<String>,
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

/// The token and TextMate scopes under the cursor to display in the inspect popup.
#[derive(Clone, Debug, PartialEq)]
pub struct TokenInspectView {
    /// The cursor's cell, as its left edge, top edge, and bottom edge in
    /// surface pixels: the tooltip hangs under it, or over it where there
    /// is no room below.
    pub anchor: (f32, f32, f32),
    /// The character and its Unicode codepoint (e.g. `'a' (U+0061)`).
    pub character: String,
    /// The scope selector rule that matched the token's foreground color.
    pub color_rule: Option<String>,
    /// The hex foreground color from the matching theme rule (e.g. `"#c678dd"`).
    pub foreground_hex: Option<String>,
    /// The syntax grammar name (e.g. `"Rust"`).
    pub language: String,
    /// Line and column in the document (e.g. `"ln 42, col 15"`).
    pub position: String,
    /// The TextMate scope stack from outermost (root) to innermost (leaf).
    pub scopes: Vec<String>,
    /// The font style (e.g. `"bold"`, `"italic"`, `"normal"`).
    pub style: Option<String>,
}
