//! The views the widgets are drawn from, handed over by the app each frame.

use std::ops::Range;
use std::sync::Arc;

use crate::renderer::view::NoticeKind;
use crate::theme::Rgb;

// ========================================================================
// Data Structures
// ========================================================================

/// One entry shown in the picker results list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PickerItem {
    /// Command id or action payload dispatched when this entry is chosen.
    pub action: String,
    /// The icons of the entry's actions that are buttons, drawn at the
    /// selected entry's right and clicked to run them.
    pub buttons: Vec<Arc<[u8]>>,
    /// Drawn dimmer after the label, on its line. Empty for none.
    pub description: String,
    /// Char indices in `description` that matched the query.
    pub description_matches: Vec<usize>,
    /// A second line under the label. Empty for an entry of one line.
    pub detail: String,
    /// Char indices in `detail` that matched the query.
    pub detail_matches: Vec<usize>,
    /// A lightweight hint drawn dim on the entry's right: a command's key
    /// binding (`"C-S-t"`), a pane's digit, `"up"`. Empty for none.
    pub hint: String,
    /// SVG drawn before the label, naming what the entry is (a file's type,
    /// a folder). Shared, so a list of a thousand `.rs` files holds one copy
    /// of the artwork and comparing views stays a pointer check.
    pub icon: Option<Arc<[u8]>>,
    /// Human-readable entry text shown in the list.
    pub label: String,
    /// Char indices in `label` that matched the query, used to highlight them.
    pub match_positions: Vec<usize>,
}

/// What the picker's side panel shows: lines of text in their colours, one
/// of them the line the selected entry is about.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PanelView {
    /// The index in `lines` of the line the entry is about, drawn tinted.
    pub focus: Option<usize>,
    /// The lines.
    pub lines: Vec<PanelLine>,
}

/// One line of the side panel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PanelLine {
    /// The char ranges of the line the query matched, drawn marked.
    pub marks: Vec<Range<usize>>,
    /// The line's runs of text, each in its colour.
    pub spans: Vec<PanelSpan>,
}

/// A run of text on a side panel line, in one colour.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelSpan {
    /// The colour it is drawn in.
    pub color: Rgb,
    /// The text.
    pub text: String,
}

/// Where the picker floating window is anchored on screen.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum PickerPlacement {
    /// Window-wise picker: centered horizontally and vertically in the main window.
    #[default]
    WindowCenter,
    /// Editor-wise picker: anchored at the bottom of the active editor pane rectangle `(x, y, w, h)`.
    EditorBottom((f32, f32, f32, f32)),
}

/// The picker state the renderer needs to draw its overlay.
#[derive(Clone, Debug, Default, PartialEq)]
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
    /// The keys the picker offers, drawn dim on a line under the entries:
    /// `F4 actions · F6 panel`. Empty for no line.
    pub hints: String,
    /// Draw an underline under highlighted match characters.
    pub match_underline: bool,
    /// The action menu over the selected entry, while it is open: a short
    /// list hung from the entry, drawn as the list at the cursor is.
    pub menu: Option<Box<PickerView>>,
    /// The side panel's preview of the selected entry, when it is shown.
    pub panel: Option<PanelView>,
    /// The filter text as typed.
    pub query: String,
    /// What the list is of, drawn as a heading above the input line. Empty
    /// for a picker that needs no naming, which draws no heading at all.
    pub title: String,
    /// Where the selection stands, `2/10`, and whether more rows are
    /// coming, drawn dim at the title bar's right: never cut short to fit, as
    /// a long title is, and still as the selection moves.
    pub title_count: String,
    /// Index into `items` of the highlighted entry.
    pub selected: usize,
    /// Drag offset `(dx, dy)` from default placement.
    pub offset: Option<(f32, f32)>,
    /// Custom size `(width, height)` in pixels, overriding default fraction of surface.
    pub size: Option<(f32, f32)>,
    /// Placement mode of the picker: centered in the window or anchored at the bottom of an editor pane.
    pub placement: PickerPlacement,
}

impl PickerView {
    /// Set the placement mode of the picker.
    pub fn with_placement(mut self, placement: PickerPlacement) -> Self {
        self.placement = placement;
        self
    }
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

/// Which edge of its column a [`ToolbarColumn`]'s name stands against.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColumnAlign {
    /// The column's left edge: a name over text.
    Left,
    /// The column's right edge: a name over numbers.
    Right,
}

/// A column's name on a pane's toolbar, drawn over the column it names,
/// wherever on the bar that is.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolbarColumn {
    /// Which edge of the column it stands against.
    pub align: ColumnAlign,
    /// The column's left edge, in surface pixels.
    pub left: f32,
    /// The column's right edge, in surface pixels.
    pub right: f32,
    /// The name.
    pub text: String,
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
    /// The names of the pane's columns, each over its column; the left items
    /// give way before the first of them.
    pub columns: Vec<ToolbarColumn>,
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
    /// Optional notice kind (e.g. Warning, Error) showing an icon in the title bar.
    pub kind: Option<NoticeKind>,
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

/// A command action in a transient popup window.
#[derive(Clone, Debug, PartialEq)]
pub struct TransientCommand {
    /// Shortcut key that triggers the command (e.g. `"c"`, `"b"`).
    pub key: String,
    /// Human-readable description of what the command does.
    pub label: String,
}

impl TransientCommand {
    /// Create a new transient command entry.
    pub fn new(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
        }
    }
}

/// The visual presentation of a switch parameter in a transient window.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SwitchKind {
    /// Standard square checkbox (`[✓]` / `[ ]`).
    #[default]
    Checkbox,
    /// Circular radio / bullet button (`(•)` / `( )`).
    Radio,
}

/// A switch parameter in a transient popup window with a toggleable checkbox or radio bullet.
#[derive(Clone, Debug, PartialEq)]
pub struct TransientSwitch {
    /// Whether the switch is currently checked.
    pub checked: bool,
    /// CLI argument or flag associated with the switch (e.g. `"--all"`).
    pub flag: String,
    /// Shortcut key or prefix that toggles the switch (e.g. `"-a"`).
    pub key: String,
    /// The visual style of the switch (square checkbox or circular radio bullet).
    pub kind: SwitchKind,
    /// Human-readable label for the parameter switch.
    pub label: String,
}

impl TransientSwitch {
    /// Create a new transient switch entry (defaulting to square checkbox).
    pub fn new(
        key: impl Into<String>,
        label: impl Into<String>,
        flag: impl Into<String>,
        checked: bool,
    ) -> Self {
        Self {
            checked,
            flag: flag.into(),
            key: key.into(),
            kind: SwitchKind::Checkbox,
            label: label.into(),
        }
    }

    /// Set the switch kind (e.g. `SwitchKind::Radio` for a bullet checked button).
    pub fn with_kind(mut self, kind: SwitchKind) -> Self {
        self.kind = kind;
        self
    }

    /// Create a new circular radio / bullet switch entry.
    pub fn radio(
        key: impl Into<String>,
        label: impl Into<String>,
        flag: impl Into<String>,
        checked: bool,
    ) -> Self {
        Self::new(key, label, flag, checked).with_kind(SwitchKind::Radio)
    }
}

/// A transient popup window containing action commands and parameter switches.
#[derive(Clone, Debug, PartialEq)]
pub struct TransientView {
    /// Bounding rectangle `(x, y, width, height)` of the active pane/editor to anchor within.
    pub anchor_rect: Option<(f32, f32, f32, f32)>,
    /// Action commands in this transient popup.
    pub commands: Vec<TransientCommand>,
    /// Drag offset `(dx, dy)` from default placement.
    pub offset: Option<(f32, f32)>,
    /// Index of the selected switch or command row.
    pub selected: usize,
    /// Parameter switches with checkboxes.
    pub switches: Vec<TransientSwitch>,
    /// Window title shown in the header.
    pub title: String,
}

impl TransientView {
    /// Create a new transient popup view.
    pub fn new(
        title: impl Into<String>,
        commands: Vec<TransientCommand>,
        switches: Vec<TransientSwitch>,
    ) -> Self {
        Self {
            anchor_rect: None,
            commands,
            offset: None,
            selected: 0,
            switches,
            title: title.into(),
        }
    }

    /// Set the anchor bounding rectangle (e.g. active pane bounds) for default centering.
    pub fn with_anchor_rect(mut self, anchor_rect: Option<(f32, f32, f32, f32)>) -> Self {
        self.anchor_rect = anchor_rect;
        self
    }

    /// Set the selected item index of this transient view.
    pub fn with_selected(mut self, selected: usize) -> Self {
        self.selected = selected;
        self
    }

    /// Set the drag offset of this transient view.
    pub fn with_offset(mut self, offset: Option<(f32, f32)>) -> Self {
        self.offset = offset;
        self
    }
}
