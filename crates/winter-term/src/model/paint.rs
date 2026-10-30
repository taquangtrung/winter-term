//! What a pane paints as styled text rows: one frame's rows, the spans
//! they are made of, the semantic styles the theme resolves, and the
//! windowing that fits the rows to the pane. An editor hands the host a
//! [`Frame`]; the host lowers it into the same grid a terminal writes into.

use winter_core::RgbColor;

// ========================================================================
// Constants
// ========================================================================

/// The size a gutter mark draws at, against a full icon.
const GUTTER_MARK_SCALE: f32 = 2.0 / 3.0;

/// How many columns a tab occupies on screen. Display only: the text a pane
/// shows keeps its tab character.
pub const TAB_WIDTH: usize = 4;

// ========================================================================
// Data Structures
// ========================================================================

/// One paint of an editor: the rows it wants drawn in its pane this frame, top
/// to bottom. Only what fits the pane, already scrolled to where the editor
/// is looking, not everything the editor holds.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    /// The screen row to band as the cursor line, counted from the pane's
    /// first: past any row above it that wraps, and any screen rows skipped.
    pub cursor_line: Option<usize>,
    /// Icons to draw over the rows, as many on a row as it has.
    pub icons: Vec<Icon>,
    /// The rows to paint, the first at the top of the pane.
    pub rows: Vec<Row>,
    /// The screen rows of one row scrolled past, which are not painted.
    pub skipped: Skipped,
    /// For each row, the column its wrapped continuation starts at: a diff
    /// line reserves its marker's room, so the text it spills onto the next
    /// screen row lines up under the text it started with, not under the
    /// marker. Rows past the list, and editors that set nothing, wrap from
    /// column zero.
    pub wrap_indents: Vec<usize>,
}

/// An icon an editor wants drawn beside one of its rows.
///
/// An editor names *what* the icon is and where it goes, never how it is drawn:
/// the host resolves that to the bundled artwork and rasterizes it over the
/// row. The row's text leaves [`Icon::WIDTH`] columns clear at `col` for it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Icon {
    /// Column of the icon's left edge.
    pub col: usize,
    /// What the icon depicts.
    pub kind: IconKind,
    /// Row within the editor's returned rows.
    pub row: usize,
}

impl Icon {
    /// Columns an icon occupies: the artwork is square and a cell is roughly
    /// half as wide as it is tall, so one cell would squash it.
    pub const WIDTH: usize = 2;
}

/// What a [`Icon`] depicts, in terms the icon set can resolve.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum IconKind {
    /// A directory, whose artwork differs by whether it is expanded.
    Dir {
        /// Whether the directory's children are listed beneath it.
        expanded: bool,
        /// The directory's own name, without a path.
        name: String,
    },
    /// A file, named so the icon set can match it by name or extension.
    File {
        /// The file's name, without a path.
        name: String,
    },
    /// A Git working-tree status, named as the bundled set spells it.
    Git {
        /// The status stem, e.g. `git-modified`.
        status: String,
    },
    /// A mark drawn in a [`Gutter`], named as the bundled set spells it.
    Gutter {
        /// The mark's stem, e.g. `marked`.
        mark: &'static str,
    },
    /// A symbol from the bundled codicons, by its stem (`lightbulb`).
    Symbol {
        /// The symbol's stem, as the bundled set spells it.
        name: &'static str,
    },
    /// A terminal tab running a program, by the program's name: drawn as the
    /// program's own icon where one is bundled (Claude Code's for `claude`),
    /// else as [`IconKind::Terminal`].
    Program {
        /// The program's name, as the tab's title gives it.
        name: String,
    },
    /// A terminal tab.
    Terminal,
    /// A kind of editor, by the name it goes by (`dir`, `git`, ...).
    Editor {
        /// The editor kind's name.
        kind: &'static str,
    },
}

/// One rendered line: styled runs laid out left to right from column zero.
pub type Row = Vec<Span>;

/// The columns at one edge of an editor's rows, in lanes: each lane holds one
/// kind of mark for one purpose (a git state, a selection, a diagnostic) as
/// an icon, at most one per row, so marks for different purposes sit side by
/// side in the same columns on every row rather than pushing the text about.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Gutter {
    lanes: usize,
}

/// Which edge of an editor's rows its [`Gutter`] stands at.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum GutterSide {
    /// Ahead of each row's text, at the pane's left edge.
    #[default]
    Left,
    /// After it, at the pane's right edge.
    Right,
}

/// How far the text editor indents the rows a wrapped line continues on.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WrapIndent {
    /// Not at all: they start at the gutter.
    None,
    /// Under the line's own first character, past its indentation.
    Same,
    /// Under the line's text, past what opens it: a list item's bullet, a
    /// heading's `#`s, a quote's `>`, a comment's marker, an `\item`, and
    /// else the line's indentation.
    #[default]
    Semantic,
}

/// When the text editor hides markup's markers: the `**` of `**bold**`,
/// LaTeX's `\textbf{` and its `}`, and their kin (see
/// `editors/text/conceal.rs`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MarkupMarkers {
    /// Never.
    Visible,
    /// Everywhere but in a span the cursor or the selection touches, and a
    /// heading's on the cursor's line.
    ShowOnCursor,
    /// Always, the cursor's own span and line too.
    #[default]
    AlwaysHidden,
}

/// The rows of an editor to paint in a pane, once wrapping is accounted for:
/// where the window starts, how many rows fill the pane, which screen row
/// the cursor paints on, and which rows show where.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RowWindow {
    /// How many rows to paint; the pane clips whatever wraps past its bottom.
    pub count: usize,
    /// The screen row the cursor's row paints on, counted from the first
    /// painted screen row, in screen rows once wrapping is on.
    pub cursor: usize,
    /// The rows on the pane's top, middle, and bottom screen rows, those two
    /// kept inside the margins.
    pub shown: Shown,
    /// The same screen rows, counted from the pane's first.
    pub shown_at: Shown,
    /// How many screen rows of the first row are scrolled past, unpainted.
    pub skip: usize,
    /// The first of the editor's own rows to paint.
    pub start: usize,
}

/// The screen rows a cursor takes, from its first to its last, each as a row
/// and a screen row within it: the same one twice for a text editor's cursor,
/// which sits on one screen row; a list row's first and last for a list's,
/// which is the whole row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CursorRows {
    /// Its first screen row.
    pub first: (usize, usize),
    /// Its last screen row.
    pub last: (usize, usize),
}

/// How [`place_view`] moves a view before laying it out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewAim {
    /// Brought the least that shows the cursor inside the margins, its last
    /// screen row and then its first, which wins for one taller than the pane.
    Follow,
    /// Left where it is, as the wheel put it.
    Held,
    /// Started so the cursor's first screen row is there in the pane.
    Place(Place),
}

/// Where a placement (`zt`, `zz`, `zb`, `C-l`) puts the cursor's screen row
/// in the pane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Place {
    /// On the pane's last screen row.
    Bottom,
    /// On its middle one.
    Middle,
    /// On its first.
    Top,
}

/// The rows a view shows on its top, middle, and bottom screen rows, which
/// `H`, `M`, and `L` go to: a row wrapped over several screen rows is one
/// of them however many it takes. Or those screen rows themselves, counted
/// from the pane's first (see [`RowWindow::shown_at`]).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Shown {
    /// The row on the pane's last screen row with something on it.
    pub bottom: usize,
    /// The row on the middle one of those.
    pub middle: usize,
    /// The row on the first.
    pub top: usize,
}

/// The screen rows of one of a frame's rows that are scrolled past and not
/// painted: the top of a wrapped row the view starts partway down. The host
/// drops them as it lays the frame out, so the rows below come up by as many.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Skipped {
    /// The frame row they are of.
    pub row: usize,
    /// How many of its screen rows, from its first.
    pub rows: usize,
}

/// A run of text sharing one style.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Span {
    /// Whether the run is lit as an occurrence of the word under the cursor:
    /// drawn on a faint band.
    pub lit: bool,
    /// The wavy line under the run where a language server found a problem
    /// in it, in the hue of how bad the problem is.
    pub squiggle: Option<DiagnosticLevel>,
    /// Whether the run is selected: drawn on the selection's background in
    /// its own color, the way a terminal selection keeps its text's color.
    pub selected: bool,
    /// How the run is colored.
    pub style: Style,
    /// The run's text.
    pub text: String,
}

/// A semantic style, resolved against the active theme so an editor never names a
/// color of its own.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Style {
    /// An added diff line: arriving, faintly.
    Added,
    /// The words an edit added: arriving, in a stronger tint than the line.
    AddedEdit,
    /// Emphasized, in the theme's accent color.
    Accent,
    /// A file a change added, where a view names what happened to it.
    ChangeAdded,
    /// A file a change left conflicted or unmerged.
    ChangeConflict,
    /// A file a change deleted.
    ChangeDeleted,
    /// A file a change edited in place.
    ChangeModified,
    /// A file a change moved or renamed.
    ChangeRenamed,
    /// Where a program reported a problem, in the hue of how bad it said
    /// the problem was.
    Diagnostic(DiagnosticLevel),
    /// De-emphasized: metadata, hints, and inactive detail.
    Dim,
    /// A conflicted file's path on its band.
    FileConflict,
    /// A staged file's path on its band.
    FileStaged,
    /// An unstaged file's path on its band.
    FileUnstaged,
    /// An untracked file's path on its band.
    FileUntracked,
    /// A title or a column header.
    Header,
    /// A header or title emphasized in the theme's accent color, in bold.
    HeaderAccent,
    /// The heading over paths a merge left contested.
    HeadingConflict,
    /// The heading over changes that are staged.
    HeadingStaged,
    /// The heading over changes that are not staged.
    HeadingUnstaged,
    /// A heading over rows that are not a kind of change, and the labels of a
    /// header block's own lines, which read as headings of a sort.
    HeadingPlain,
    /// The heading over paths nothing tracks.
    HeadingUntracked,
    /// A hunk's header line: the band a hunk sits under.
    Hunk,
    /// The number of the line the cursor is on.
    LineNumberActive,
    /// Flagged on the selection's background: a name changed in place, not
    /// yet saved. A selection itself is a [`Span::selected`] run, which
    /// keeps its own color.
    Marked,
    /// Work that ended badly: a build that failed, said in bold red.
    Failed,
    /// Ordinary text.
    #[default]
    Normal,
    /// Work that ended well: a build that succeeded, said in bold green.
    Passed,
    /// What git is part-way through (a rebase, a merge), named in the
    /// header: the hue of a conflict, on no band, since it is a line of the
    /// header rather than a file's row.
    Operation,
    /// A removed diff line: leaving, faintly.
    Removed,
    /// The words an edit removed: leaving, in a stronger tint than the line.
    RemovedEdit,
    /// The checked-out branch, where a ref is drawn beside a commit.
    RefHead,
    /// A local branch other than the checked-out one, beside a commit.
    RefLocal,
    /// A branch on a remote, beside a commit.
    RefRemote,
    /// A tag, beside a commit.
    RefTag,
    /// Work one side of a branch pair has and the other does not: the marker
    /// on a commit the upstream has not seen, and the counts saying how far
    /// the two have drifted apart.
    Unpushed,
    /// A file's row in a diff: the band its changes sit under, bright while
    /// they show beneath it.
    Section,
    /// A file's row in a diff whose changes are folded away: the band, receded.
    SectionFolded,
    /// Source being read: the rows of the theme's scope table that color and
    /// weigh the run, which name what the run is and leave the color to the
    /// theme.
    Syntax(SyntaxStyle),
    /// Receded the way a comment stands back from the code it explains.
    SyntaxComment,
    /// A program's own output, in the colors it asked for with SGR: a build
    /// tool's red for an error is the one place a pane paints what something
    /// else chose, and a palette color still comes out of the theme's own.
    Terminal(TerminalStyle),
}

/// How bad a reported problem is, as the compiler, linter, or log that
/// reported it called it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DiagnosticLevel {
    /// Something that stopped the work: a compile error, a failed test.
    Error,
    /// A remark or a note, worth a look but not a fault.
    Info,
    /// Something that did not stop the work but may be wrong.
    Warning,
}

/// A color a program asked for: a slot of the terminal palette, which the
/// theme resolves, or a color of the program's own.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalColor {
    /// One of the 256 palette slots.
    Indexed(u8),
    /// A 24-bit color.
    Rgb(RgbColor),
}

/// The SGR attributes a run of a program's output was written in. A color
/// left `None` is the pane's own.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TerminalStyle {
    /// The background the program set.
    pub background: Option<TerminalColor>,
    /// Bold, which a terminal also draws brighter.
    pub bold: bool,
    /// Faint, drawn receded.
    pub faint: bool,
    /// The foreground the program set.
    pub foreground: Option<TerminalColor>,
    /// Italic.
    pub italic: bool,
    /// Foreground and background swapped.
    pub inverse: bool,
    /// Underlined, in any of the underline styles.
    pub underline: bool,
}

/// The rows of the theme's scope table (`winter_render::SCOPE_RULES`) a run
/// of source takes its color and its weight from, settled apart as a VS Code
/// theme settles them. `None` leaves the text's color or weight as it is.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct SyntaxStyle {
    /// The row the color comes from.
    pub color: Option<u16>,
    /// Whether a line is struck through it: `~~text~~` with its markers
    /// hidden.
    pub strike: bool,
    /// Whether a line is drawn under it: a link's text, or `<u>text</u>`,
    /// with their markers hidden.
    pub underline: bool,
    /// The row the weight comes from.
    pub weight: Option<u16>,
}

/// Where a pointer is over an editor: the row and column of the pane's own
/// content, counted from the first row the editor painted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Point {
    /// The column the pointer is over.
    pub col: usize,
    /// Whether the pointer is being dragged with its button held, rather than
    /// pressed where it now is.
    pub drag: bool,
    /// The row the pointer is over.
    pub row: usize,
}

/// Where an editor's own caret sits within the row it bands as its cursor line,
/// and whether what it is doing there is typing. An editor that reports one is
/// drawn with the caret on that cell, rather than on the row's first non-blank
/// character, which for an editor with a gutter is in the gutter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Caret {
    /// The column, counted in painted cells from the row's start.
    pub col: usize,
    /// Whether the next key typed goes into the text, which the caret takes
    /// its shape from the way the terminal's own does.
    pub insert: bool,
}

// ========================================================================
// Functions
// ========================================================================

/// Where a view starts after the wheel turns `lines` screen rows (positive
/// is up), as a row and how many of its screen rows are scrolled past, each
/// row `height(row)` screen rows tall: what lets the wheel read through a
/// wrapped row taller than the pane. Held inside the `total` rows so the
/// last screenful is the furthest down.
pub fn wheel_scroll_rows(
    (mut row, mut skip): (usize, usize),
    lines: isize,
    total: usize,
    screen_rows: usize,
    height: impl Fn(usize) -> usize,
) -> (usize, usize) {
    let left = lines.unsigned_abs();
    match lines < 0 {
        true => {
            skip += left;
            while row + 1 < total && skip >= height(row) {
                skip -= height(row);
                row += 1;
            }
            (row, skip).min(last_top(total, screen_rows, &height))
        }
        false => screen_rows_above((row, skip), left, height),
    }
}

/// The screen row `rows` above screen row `skip` of row `row`, as a row and
/// how many of its screen rows come before it, each row `height(row)` screen
/// rows tall: where a view starts that shows the one `rows` down its pane.
/// The very top where there are fewer above.
pub fn screen_rows_above(
    (mut row, mut skip): (usize, usize),
    rows: usize,
    height: impl Fn(usize) -> usize,
) -> (usize, usize) {
    let mut left = rows;
    while left > skip {
        if row == 0 {
            return (0, 0);
        }
        left -= skip;
        row -= 1;
        skip = height(row);
    }
    (row, skip - left)
}

/// Where a view starting at `top`, a row and how many of its screen rows are
/// scrolled past, starts once moved the least that shows screen row `within`
/// of row `cursor` with `margin` screen rows of the pane kept above and below
/// it, each row `height(row)` screen rows tall. The cursor's screen row comes
/// in at the margin from above and from below, so a move past it scrolls one
/// screen row, not a whole wrapped row. `top` itself where the cursor already
/// shows inside the margins.
pub fn follow_screen_row(
    top: (usize, usize),
    (cursor, within): (usize, usize),
    screen_rows: usize,
    margin: usize,
    height: impl Fn(usize) -> usize,
) -> (usize, usize) {
    let screen_rows = screen_rows.max(1);
    let margin = clamp_margin(margin, screen_rows);
    let low = screen_rows - 1 - margin;
    if (cursor, within) < top {
        return screen_rows_above((cursor, within), margin, height);
    }
    // The screen rows from the view's top down to the cursor's, counted
    // only as far as the low margin, so a far jump measures no more.
    let mut down = within;
    for row in top.0..cursor {
        down += height(row);
        if down > low + top.1 {
            break;
        }
    }
    let down = down - top.1;
    if down < margin {
        screen_rows_above((cursor, within), margin, height)
    } else if down > low {
        screen_rows_above((cursor, within), low, height)
    } else {
        top
    }
}

/// `margin` screen rows, held to under half a pane `screen_rows` tall, so
/// the margins above and below the cursor leave its own row between them.
pub fn clamp_margin(margin: usize, screen_rows: usize) -> usize {
    margin.min(screen_rows.saturating_sub(1) / 2)
}

/// The furthest down a view over `total` rows may start, as a row and the
/// screen rows of it scrolled past: where the last `screen_rows` screen rows
/// begin, each row `height(row)` tall, or the very top when all of them fit.
pub fn last_top(total: usize, screen_rows: usize, height: impl Fn(usize) -> usize) -> (usize, usize) {
    let mut filled = 0;
    for row in (0..total).rev() {
        filled += height(row);
        if filled >= screen_rows {
            return (row, filled - screen_rows);
        }
    }
    (0, 0)
}

/// Lay a view out over `total` rows, each `height(row)` screen rows tall,
/// in `screen_rows` from `top` (a row and how many of its screen rows are
/// scrolled past), the way every pane that scrolls by screen rows does:
/// `top` held to the rows there are now, turned `wheel` screen rows
/// (positive upwards), moved as `aim` says round the cursor's screen rows
/// `cursor` with `margin` screen rows kept round it, held inside the last
/// screenful, then laid out (see [`lay_out_rows`]).
#[allow(clippy::too_many_arguments)]
pub fn place_view(
    top: (usize, usize),
    wheel: isize,
    aim: ViewAim,
    cursor: CursorRows,
    total: usize,
    screen_rows: usize,
    margin: usize,
    height: impl Fn(usize) -> usize,
) -> RowWindow {
    if total == 0 {
        return RowWindow::default();
    }
    // Every row takes a screen row at least, painted or not.
    let height = |row: usize| height(row).max(1);
    // A fold changed, or rows went, since the view was last laid out.
    let start = top.0.min(total - 1);
    let top = (start, top.1.min(height(start) - 1));
    let top = wheel_scroll_rows(top, wheel, total, screen_rows, height);
    let top = match aim {
        ViewAim::Follow => {
            let top = follow_screen_row(top, cursor.last, screen_rows, margin, height);
            follow_screen_row(top, cursor.first, screen_rows, margin, height)
        }
        ViewAim::Held => top,
        ViewAim::Place(place) => screen_rows_above(cursor.first, place.rows_above(screen_rows, margin), height),
    };
    let top = top.min(last_top(total, screen_rows, height));
    lay_out_rows(top, cursor.first.0, total, screen_rows, margin, height)
}

/// The rows a view lays out in `screen_rows` from `top`, a row and how many
/// of its screen rows are scrolled past, over `total` rows each `height(row)`
/// screen rows tall: enough of them to fill the pane, the screen row row
/// `cursor` paints on, and the rows on the top, middle, and bottom screen
/// rows, the top and bottom kept `margin` screen rows in from an edge the
/// view can still scroll past, so going to them scrolls nothing. Only the
/// rows that show are measured.
pub fn lay_out_rows(
    (start, skip): (usize, usize),
    cursor: usize,
    total: usize,
    screen_rows: usize,
    margin: usize,
    height: impl Fn(usize) -> usize,
) -> RowWindow {
    let mut window = RowWindow {
        skip,
        start,
        ..RowWindow::default()
    };
    // Each painted row, with the screen row just past its last.
    let mut ends: Vec<(usize, usize)> = Vec::new();
    let mut filled = 0;
    for row in start..total {
        if filled >= screen_rows {
            break;
        }
        if row == cursor {
            window.cursor = filled;
        }
        filled += height(row).saturating_sub(window.skip_of(row));
        ends.push((row, filled));
    }
    window.count = ends.len();
    let last = filled.min(screen_rows).saturating_sub(1);
    let margin = clamp_margin(margin, screen_rows);
    // No margin at the very top or end, past which nothing scrolls.
    let top_margin = match start == 0 && skip == 0 {
        true => 0,
        false => margin.min(last),
    };
    let bottom_margin = match start + window.count == total && filled <= screen_rows {
        true => 0,
        false => margin,
    };
    window.shown_at = Shown {
        bottom: last.saturating_sub(bottom_margin),
        middle: last / 2,
        top: top_margin,
    };
    let row_on = |screen: usize| ends.iter().find(|(_, end)| *end > screen).map_or(start, |(row, _)| *row);
    window.shown = Shown {
        bottom: row_on(window.shown_at.bottom),
        middle: row_on(window.shown_at.middle),
        top: row_on(window.shown_at.top),
    };
    window
}

/// What a row puts on screen, with its styles dropped: the text a search reads.
pub fn row_text(row: &Row) -> String {
    row.iter().map(|span| span.text.as_str()).collect()
}

/// `text` with each tab opened out to [`TAB_WIDTH`] spaces, for a pane that
/// paints text it did not lay out itself: a raw tab reaches the grid as a
/// control character and draws as the font's missing glyph. A byte a
/// program wrote that is not UTF-8 is shown as its value for the same
/// reason (see [`crate::bytes_text::display`]).
pub fn expand_tabs(text: &str) -> String {
    crate::bytes_text::display(text).replace('\t', &" ".repeat(TAB_WIDTH))
}

/// The column a wrapped row's continuation starts at, clamped so at least one
/// column of text remains however narrow the pane or wide the indent.
pub fn wrap_start(indent: usize, cols: usize) -> usize {
    indent.min(cols.saturating_sub(1))
}

/// The half-open char ranges a row's text paints as, one per screen row,
/// wrapped to a pane `cols` wide. A fold lands before the last word that
/// starts inside the line, and only when the part of it overflowing the
/// margin would fit beside the continuation's indent — the same choice the
/// terminal grid's own word wrap makes — so words move down whole, a diff
/// marker keeps its line, and only a word with no room at all breaks
/// mid-word. Continuations after the first start at the row's wrap indent
/// (clamped by [`wrap_start`], so every line holds at least one column and
/// wrapping always terminates). With `wrap` off, the whole text is one line
/// whatever the pane's width.
pub fn wrapped_lines(
    chars: &[char],
    cols: usize,
    wrap: bool,
    indent: usize,
) -> Vec<(usize, usize)> {
    if !wrap || cols == 0 || chars.len() <= cols {
        return vec![(0, chars.len())];
    }
    let cont = wrap_start(indent, cols);
    let mut lines = Vec::new();
    let mut pos = 0;
    let mut room = cols;
    while pos < chars.len() {
        let end = if chars.len() - pos <= room {
            chars.len()
        } else {
            // The line must fold. The fold lands before the last word that
            // starts inside the line, and only when the part overflowing the
            // margin fits beside the continuation's indent — the same choice
            // the grid's own word wrap makes, so a diff marker keeps its
            // place and a word too long for the indent stays split at the
            // margin. Leading whitespace is never a break. With no word
            // boundary in reach, the line breaks exactly at the margin, so
            // wrapping always terminates.
            let limit = pos + room;
            let mut end = limit;
            for w in (pos + 1..=limit).rev() {
                if !chars[w - 1].is_whitespace() || chars[w].is_whitespace() {
                    continue;
                }
                if limit - w <= cols - cont {
                    end = w;
                    break;
                }
            }
            end
        };
        lines.push((pos, end));
        if end == chars.len() {
            break;
        }
        pos = end;
        while pos < chars.len() && chars[pos].is_whitespace() {
            pos += 1;
        }
        room = cols - cont;
    }
    lines
}

/// How many screen rows a painted row takes in a pane `cols` wide: one,
/// unless it wraps, in which case one per line it spills onto — the first
/// filling the pane, the rest starting at the row's wrap indent and so
/// holding less. Counts the lines [`wrapped_lines`] paints, so the height a
/// editor measures its rows by is the height they paint at.
pub fn row_height(text: &str, cols: usize, wrap: bool, indent: usize) -> usize {
    if !wrap || cols == 0 || text.chars().count() <= cols {
        return 1;
    }
    let chars: Vec<char> = text.chars().collect();
    wrapped_lines(&chars, cols, wrap, indent).len()
}

// ========================================================================
// Frame
// ========================================================================

impl Frame {
    /// A page of `rows` with no cursor line.
    pub fn new(rows: Vec<Row>) -> Self {
        Self {
            cursor_line: None,
            icons: Vec::new(),
            rows,
            skipped: Skipped::default(),
            wrap_indents: Vec::new(),
        }
    }

    /// Attach the icons the host should draw over these rows.
    pub fn with_icons(mut self, icons: Vec<Icon>) -> Self {
        self.icons = icons;
        self
    }

    /// Attach the column each row's wrapped continuation starts at, one per
    /// row.
    pub fn with_wrap_indents(mut self, wrap_indents: Vec<usize>) -> Self {
        self.wrap_indents = wrap_indents;
        self
    }

    /// Band screen row `row` as the cursor line, counted from the pane's first.
    pub fn with_cursor_line(mut self, row: usize) -> Self {
        self.cursor_line = Some(row);
        self
    }

    /// Leave the first `rows` screen rows of frame row `row` unpainted: the
    /// view starts that far down it.
    pub fn with_skipped(mut self, row: usize, rows: usize) -> Self {
        self.skipped = Skipped { row, rows };
        self
    }
}

// ========================================================================
// Place
// ========================================================================

impl Place {
    /// How many screen rows of a pane `screen_rows` tall are above the
    /// cursor's once it is placed, `margin` of them kept at the edge it goes
    /// to.
    pub fn rows_above(self, screen_rows: usize, margin: usize) -> usize {
        let screen_rows = screen_rows.max(1);
        let margin = clamp_margin(margin, screen_rows);
        match self {
            Place::Bottom => screen_rows - 1 - margin,
            Place::Middle => screen_rows / 2,
            Place::Top => margin,
        }
    }
}

// ========================================================================
// RowWindow
// ========================================================================

impl RowWindow {
    /// How many screen rows of row `row` are scrolled past.
    pub fn skip_of(&self, row: usize) -> usize {
        match row == self.start {
            true => self.skip,
            false => 0,
        }
    }

    /// Whether row `row` is painted.
    pub fn holds(&self, row: usize) -> bool {
        (self.start..self.start + self.count).contains(&row)
    }
}

// ========================================================================
// Skipped
// ========================================================================

impl Skipped {
    /// How many screen rows of frame row `row` are not painted.
    pub fn rows_of(&self, row: usize) -> usize {
        match row == self.row {
            true => self.rows,
            false => 0,
        }
    }
}

// ========================================================================
// Gutter
// ========================================================================

impl Gutter {
    /// A gutter of `lanes` icon lanes.
    pub fn new(lanes: usize) -> Self {
        Self { lanes }
    }

    /// How many columns the gutter takes: [`Icon::WIDTH`] for each lane.
    pub fn width(&self) -> usize {
        self.lanes * Icon::WIDTH
    }

    /// One row of `cols` columns: the editor's own `content` and the `icons`
    /// over it (their columns counted from where the content starts), with
    /// the gutter holding `marks`, one per lane in order (`None` for a lane
    /// left blank), at `side`. At the left the gutter leads and the content
    /// follows it; at the right the content is padded out so the gutter takes
    /// the row's last columns, or follows it at once where the content is too
    /// wide for that.
    pub fn place(
        &self,
        side: GutterSide,
        cols: usize,
        content: Vec<Span>,
        icons: Vec<Icon>,
        marks: &[Option<IconKind>],
    ) -> (Vec<Span>, Vec<Icon>) {
        // The gutter's own columns stay blank: the host draws its icons over.
        let blank = Span::plain(" ".repeat(self.width()));
        let (spans, start, content_shift) = match side {
            GutterSide::Left => {
                let spans = std::iter::once(blank).chain(content).collect();
                (spans, 0, self.width())
            }
            GutterSide::Right => {
                let used = row_text(&content).chars().count();
                let start = used.max(cols.saturating_sub(self.width()));
                let mut spans = content;
                if start > used {
                    spans.push(Span::plain(" ".repeat(start - used)));
                }
                spans.push(blank);
                (spans, start, 0)
            }
        };
        let own = icons.into_iter().map(|icon| Icon {
            col: icon.col + content_shift,
            ..icon
        });
        let marked = marks
            .iter()
            .take(self.lanes)
            .enumerate()
            .filter_map(|(lane, mark)| {
                Some(Icon {
                    col: start + lane * Icon::WIDTH,
                    kind: mark.clone()?,
                    row: 0,
                })
            });
        (spans, own.chain(marked).collect())
    }
}

impl GutterSide {
    /// The side `settings.kdl` names: `"right"`, else the default left.
    pub fn from_value(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "right" => Self::Right,
            _ => Self::Left,
        }
    }

    /// What `settings.kdl` calls this side.
    pub fn as_value(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

impl MarkupMarkers {
    /// The setting `settings.kdl` names: `"visible"`, `"show-on-cursor"`,
    /// else always hidden.
    pub fn from_value(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "visible" => Self::Visible,
            "show-on-cursor" => Self::ShowOnCursor,
            _ => Self::AlwaysHidden,
        }
    }

    /// What `settings.kdl` calls this setting.
    pub fn as_value(self) -> &'static str {
        match self {
            Self::Visible => "visible",
            Self::ShowOnCursor => "show-on-cursor",
            Self::AlwaysHidden => "always-hidden",
        }
    }
}

impl WrapIndent {
    /// The indent `settings.kdl` names: `"none"`, `"same"`, else semantic.
    pub fn from_value(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" => Self::None,
            "same" => Self::Same,
            _ => Self::Semantic,
        }
    }

    /// What `settings.kdl` calls this indent.
    pub fn as_value(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Same => "same",
            Self::Semantic => "semantic",
        }
    }
}

// ========================================================================
// IconKind
// ========================================================================

impl IconKind {
    /// How large the artwork draws within its slot, as a fraction of the
    /// row's height: a gutter mark stays small, so it reads as a mark beside
    /// the text rather than competing with the entry's own icon.
    pub fn scale(&self) -> f32 {
        match self {
            Self::Git { .. } | Self::Gutter { .. } => GUTTER_MARK_SCALE,
            Self::Dir { .. }
            | Self::File { .. }
            | Self::Program { .. }
            | Self::Symbol { .. }
            | Self::Terminal
            | Self::Editor { .. } => 1.0,
        }
    }
}

// ========================================================================
// Span
// ========================================================================

impl Span {
    /// A run drawn in `style`.
    pub fn new(style: Style, text: impl Into<String>) -> Self {
        Self {
            lit: false,
            squiggle: None,
            selected: false,
            style,
            text: text.into(),
        }
    }

    /// The same run, with a problem's wavy line under it.
    pub fn squiggled(mut self, level: Option<DiagnosticLevel>) -> Self {
        self.squiggle = level;
        self
    }

    /// The same run, lit as an occurrence when `lit` is set.
    pub fn lit(mut self, lit: bool) -> Self {
        self.lit = lit;
        self
    }

    /// The same run, selected.
    pub fn selected(mut self) -> Self {
        self.selected = true;
        self
    }

    /// A run drawn in the editor's ordinary text style.
    pub fn plain(text: impl Into<String>) -> Self {
        Self::new(Style::Normal, text)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_wheel_reads_through_a_row_taller_than_the_pane() {
        // A row of 30 screen rows in a 20-row pane: counted whole, the view
        // could start at it but never show its end.
        let height = |row: usize| if row == 4 { 30 } else { 1 };
        assert_eq!(last_top(5, 20, height), (4, 10), "its last 20 screen rows");
        assert_eq!(wheel_scroll_rows((4, 0), -3, 5, 20, height), (4, 3));
        assert_eq!(wheel_scroll_rows((4, 9), -3, 5, 20, height), (4, 10), "held at the end");
        assert_eq!(wheel_scroll_rows((4, 2), 3, 5, 20, height), (3, 0), "up out of it");
        assert_eq!(wheel_scroll_rows((0, 0), 3, 5, 20, height), (0, 0), "the top stays");
        // Plain rows move a row a step, as the wheel always did.
        assert_eq!(wheel_scroll_rows((10, 0), -3, 100, 20, |_| 1), (13, 0));
        assert_eq!(wheel_scroll_rows((79, 0), -3, 100, 20, |_| 1), (80, 0));
    }

    #[test]
    fn test_the_view_follows_the_cursor_a_screen_row_at_a_time() {
        // Rows of four screen rows in a 10-row pane. Counted whole, a cursor
        // stepping a screen row past the bottom pulled the view down by the
        // whole top row, four screen rows at once.
        let height = |_: usize| 4;
        assert_eq!(follow_screen_row((0, 0), (2, 1), 10, 0, height), (0, 0), "shows on the bottom row");
        assert_eq!(follow_screen_row((0, 0), (2, 2), 10, 0, height), (0, 1), "one screen row down");
        assert_eq!(follow_screen_row((0, 1), (2, 3), 10, 0, height), (0, 2));
        assert_eq!(follow_screen_row((0, 3), (3, 0), 10, 0, height), (0, 3), "still on the bottom row");
        assert_eq!(follow_screen_row((0, 3), (3, 1), 10, 0, height), (1, 0), "past the top row's end");
        assert_eq!(follow_screen_row((1, 2), (1, 1), 10, 0, height), (1, 1), "up into the rows scrolled past");
        assert_eq!(follow_screen_row((5, 0), (2, 3), 10, 0, height), (2, 3), "from above, at the top");
        assert_eq!(follow_screen_row((0, 0), (90, 0), 10, 0, height), (87, 3), "a far jump, at the bottom");
        // Plain rows scroll a row at a time, as before.
        assert_eq!(follow_screen_row((0, 0), (20, 0), 10, 0, |_| 1), (11, 0));
    }

    #[test]
    fn test_the_view_keeps_a_margin_of_screen_rows_round_the_cursor() {
        let plain = |_: usize| 1;
        assert_eq!(follow_screen_row((0, 0), (7, 0), 10, 2, plain), (0, 0), "on the low margin's edge");
        assert_eq!(follow_screen_row((0, 0), (8, 0), 10, 2, plain), (1, 0), "past it, a row down");
        assert_eq!(follow_screen_row((5, 0), (6, 0), 10, 2, plain), (4, 0), "into the top margin, a row up");
        assert_eq!(follow_screen_row((5, 0), (3, 0), 10, 2, plain), (1, 0), "from above, the margin over it");
        assert_eq!(follow_screen_row((0, 0), (0, 0), 10, 2, plain), (0, 0), "none above the first row");
        // More than half the pane is held to under half, so the cursor moves.
        assert_eq!(follow_screen_row((0, 0), (5, 0), 10, 9, plain), (0, 0));
        assert_eq!(follow_screen_row((0, 0), (6, 0), 10, 9, plain), (1, 0));
        // Wrapped rows: the margin is screen rows, partway into a row.
        assert_eq!(follow_screen_row((0, 0), (1, 2), 6, 1, |_| 3), (0, 1));
    }

    #[test]
    fn test_placing_a_view_follows_holds_or_places_its_cursor() {
        let plain = |_: usize| 1;
        let row = |at: usize| CursorRows { first: (at, 0), last: (at, 0) };
        let start = |top, wheel, aim, cursor| place_view(top, wheel, aim, cursor, 100, 10, 2, plain).start;
        assert_eq!(start((0, 0), 0, ViewAim::Follow, row(20)), 13, "followed, the margin below it");
        assert_eq!(start((0, 0), 0, ViewAim::Held, row(20)), 0, "held where it was");
        assert_eq!(start((0, 0), -5, ViewAim::Held, row(20)), 5, "turned by the wheel first");
        assert_eq!(start((0, 0), 0, ViewAim::Place(Place::Top), row(20)), 18, "zt leaves the margin above");
        assert_eq!(start((95, 0), 0, ViewAim::Held, row(99)), 90, "never past the last screenful");
        // A list row three screen rows tall, under the pane's bottom: it comes
        // in whole, its last screen row the margin above the bottom.
        let tall = CursorRows { first: (4, 0), last: (4, 2) };
        let window = place_view((0, 0), 0, ViewAim::Follow, tall, 10, 6, 1, |row| if row == 4 { 3 } else { 1 });
        assert_eq!((window.start, window.skip), (2, 0));
        assert_eq!(place_view((7, 3), 0, ViewAim::Follow, row(0), 0, 10, 2, plain), RowWindow::default(), "no rows");
    }

    #[test]
    fn test_a_placement_and_the_screen_rows_keep_the_margin() {
        assert_eq!(Place::Top.rows_above(10, 3), 3);
        assert_eq!(Place::Bottom.rows_above(10, 3), 6);
        assert_eq!(Place::Middle.rows_above(10, 3), 5);
        assert_eq!((Place::Top.rows_above(4, 9), Place::Bottom.rows_above(4, 9)), (1, 2), "held under half");
        // `H` and `L` land inside the margins, where going to them scrolls
        // nothing, except at the very top and end.
        let shown = |top| lay_out_rows(top, 0, 100, 20, 3, |_| 1).shown;
        assert_eq!(shown((10, 0)), Shown { bottom: 26, middle: 19, top: 13 });
        assert_eq!(shown((0, 0)).top, 0, "nothing above to scroll to");
        assert_eq!(shown((80, 0)).bottom, 99, "nothing below");
    }

    #[test]
    fn test_screen_rows_above_cross_into_the_rows_before() {
        let height = |row: usize| if row == 1 { 3 } else { 1 };
        assert_eq!(screen_rows_above((2, 0), 1, height), (1, 2), "the last row of the one before");
        assert_eq!(screen_rows_above((2, 0), 3, height), (1, 0));
        assert_eq!(screen_rows_above((2, 0), 4, height), (0, 0));
        assert_eq!(screen_rows_above((2, 0), 9, height), (0, 0), "no further than the top");
        assert_eq!(screen_rows_above((1, 2), 1, height), (1, 1), "within its own row");
    }

    #[test]
    fn test_the_last_top_is_measured_in_screen_rows() {
        // Ten rows of four screen rows fill a 20-row pane from the sixth on;
        // counted as one each, the view stopped at the first.
        assert_eq!(last_top(10, 20, |_| 4), (5, 0));
        assert_eq!(last_top(100, 20, |_| 1), (80, 0));
        assert_eq!(last_top(3, 20, |_| 1), (0, 0), "all of it fits");
    }

    #[test]
    fn test_a_window_counts_the_rows_that_fill_the_pane_past_those_skipped() {
        let heights = [5, 1, 1, 1];
        let window = lay_out_rows((0, 0), 1, heights.len(), 10, 0, |index| heights[index]);
        // A row five screen rows tall pushes the next down to screen row five.
        assert_eq!((window.count, window.cursor), (4, 5));
        assert_eq!(window.shown, Shown { bottom: 3, middle: 0, top: 0 }, "eight rows painted, the middle in the tall one");
        // Three of the tall row's screen rows scrolled past bring the rest up.
        let window = lay_out_rows((0, 3), 1, heights.len(), 4, 0, |index| heights[index]);
        assert_eq!((window.count, window.cursor, window.skip), (3, 2, 3));
        assert_eq!(window.shown, Shown { bottom: 2, middle: 0, top: 0 }, "the middle still in the tall one");
        // Plain rows: one each, as many as the pane is tall.
        let window = lay_out_rows((10, 0), 15, 100, 20, 0, |_| 1);
        assert_eq!((window.count, window.cursor), (20, 5));
        assert_eq!(window.shown, Shown { bottom: 29, middle: 19, top: 10 });
        assert_eq!(lay_out_rows((0, 0), 0, 0, 10, 0, |_| 1).count, 0, "nothing to paint");
    }

    #[test]
    fn test_an_indented_wrap_holds_less_per_continuation_line() {
        // A 17-cell row of one unbreakable word in a 10-cell pane: without an
        // indent it spills seven cells onto a second line; with the
        // continuation starting five cells in, that line holds only five, so
        // the row takes three.
        assert_eq!(row_height(&"x".repeat(17), 10, true, 0), 2);
        assert_eq!(row_height(&"x".repeat(17), 10, true, 5), 3);
    }

    #[test]
    fn test_a_wrap_indent_never_leaves_no_room_for_text() {
        // An indent as wide as the pane is clamped to leave one column of
        // text, so wrapping still terminates.
        assert_eq!(wrap_start(10, 10), 9);
        assert_eq!(wrap_start(4, 10), 4);
        assert_eq!(row_height(&"x".repeat(12), 10, true, 10), 3);
    }

    #[test]
    fn test_a_row_folds_at_its_last_space_that_fits() {
        // "aa bb ccc" in eight cells: the fold lands before "ccc", the last
        // word that fits to start a line, so the first line holds "aa bb" and
        // carries "ccc" down whole rather than splitting it across the margin.
        let chars: Vec<char> = "aa bb ccc".chars().collect();
        assert_eq!(wrapped_lines(&chars, 8, true, 0), vec![(0, 6), (6, 9)]);
        assert_eq!(row_height("aa bb ccc", 8, true, 0), 2);
    }

    #[test]
    fn test_a_word_too_long_for_the_indent_stays_at_the_margin() {
        // The grid's own word wrap carries a word down only when the part
        // overflowing the margin fits beside the continuation's indent:
        // "-oldoldoldold" beside a five-cell indent does not, so the fold
        // falls back to the margin and the marker keeps its line.
        let chars: Vec<char> = "    -oldoldoldold".chars().collect();
        assert_eq!(
            wrapped_lines(&chars, 10, true, 5),
            vec![(0, 10), (10, 15), (15, 17)]
        );
    }

    #[test]
    fn test_a_word_longer_than_a_line_breaks_at_the_margin() {
        // No space in reach: the word breaks mid-word exactly at the margin,
        // the way the terminal grid's own wrap falls back to.
        let chars: Vec<char> = "aaaaaa".chars().collect();
        assert_eq!(wrapped_lines(&chars, 4, true, 0), vec![(0, 4), (4, 6)]);
    }

    #[test]
    fn test_the_fold_lands_before_the_word_not_on_the_spaces() {
        // A fold never starts a continuation on whitespace: the spaces end
        // the line they fold at, and a row ending in spaces does not spill
        // them onto a line of their own.
        let chars: Vec<char> = "a  b".chars().collect();
        assert_eq!(wrapped_lines(&chars, 1, true, 0), vec![(0, 1), (3, 4)]);
        let trailing: Vec<char> = "word ".chars().collect();
        assert_eq!(wrapped_lines(&trailing, 4, true, 0), vec![(0, 4)]);
    }

    #[test]
    fn test_wrapping_off_keeps_one_line_however_wide() {
        let chars: Vec<char> = "aaa bbb ccc".chars().collect();
        assert_eq!(wrapped_lines(&chars, 4, false, 0), vec![(0, 11)]);
        assert_eq!(row_height("aaa bbb ccc", 4, false, 0), 1);
    }

    #[test]
    fn test_a_gutter_leads_the_row_at_the_left_and_ends_it_at_the_right() {
        let gutter = Gutter::new(2);
        let git = IconKind::Git {
            status: "git-added".to_string(),
        };
        let own = Icon {
            col: 1,
            kind: IconKind::Terminal,
            row: 0,
        };
        let marks = [Some(git), Some(IconKind::Gutter { mark: "marked" })];
        let cols = |icons: &[Icon]| icons.iter().map(|icon| icon.col).collect::<Vec<_>>();

        let (spans, icons) = gutter.place(
            GutterSide::Left,
            10,
            vec![Span::plain("abc")],
            vec![own.clone()],
            &marks,
        );
        assert_eq!(row_text(&spans), "    abc");
        assert_eq!(
            cols(&icons),
            [5, 0, 2],
            "the row's own icon moves past the gutter"
        );

        let (spans, icons) = gutter.place(
            GutterSide::Right,
            10,
            vec![Span::plain("abc")],
            vec![own.clone()],
            &marks,
        );
        assert_eq!(
            row_text(&spans),
            "abc       ",
            "the gutter in the last columns"
        );
        assert_eq!(cols(&icons), [1, 6, 8], "the row's own icon stays put");

        let (spans, icons) = gutter.place(
            GutterSide::Right,
            4,
            vec![Span::plain("abcdef")],
            Vec::new(),
            &marks,
        );
        assert_eq!(
            row_text(&spans),
            "abcdef    ",
            "too wide: the gutter follows at once"
        );
        assert_eq!(cols(&icons), [6, 8]);
    }

    #[test]
    fn test_a_blank_lane_keeps_its_columns_so_the_next_lane_stays_put() {
        // Marks for different purposes stay in their own columns, so a row
        // with one and a row with the other still line their text up.
        let gutter = Gutter::new(2);
        let (spans, icons) = gutter.place(
            GutterSide::Left,
            10,
            vec![Span::plain("x")],
            Vec::new(),
            &[None, Some(IconKind::Gutter { mark: "marked" })],
        );
        assert_eq!(row_text(&spans), "    x");
        assert_eq!(icons.iter().map(|icon| icon.col).collect::<Vec<_>>(), [2]);
    }
}
