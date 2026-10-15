//! What a pane paints as styled text rows: one frame's rows, the spans
//! they are made of, the semantic styles the theme resolves, and the
//! windowing that fits the rows to the pane. A tool hands the host a
//! [`Frame`]; the host lowers it into the same grid a terminal writes into.

// ========================================================================
// Data Structures
// ========================================================================

/// One paint of a tool: the rows it wants drawn in its pane this frame, top
/// to bottom. Only what fits the pane, already scrolled to where the tool
/// is looking, not everything the tool holds.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    /// The row to band as the cursor line, counted from the first row here.
    pub cursor_line: Option<usize>,
    /// Icons to draw over the rows, one per row that has one.
    pub icons: Vec<Icon>,
    /// The rows to paint, the first at the top of the pane.
    pub rows: Vec<Row>,
    /// For each row, the column its wrapped continuation starts at: a diff
    /// line reserves its marker's room, so the text it spills onto the next
    /// screen row lines up under the text it started with, not under the
    /// marker. Rows past the list, and tools that set nothing, wrap from
    /// column zero.
    pub wrap_indents: Vec<usize>,
}

/// An icon a tool wants drawn beside one of its rows.
///
/// A tool names *what* the icon is and where it goes, never how it is drawn:
/// the host resolves that against the `icons` setting, painting a glyph into
/// the row, rasterizing the bundled artwork over it, or neither. The row's
/// text always leaves [`Icon::WIDTH`] columns clear at `col`, so every
/// column downstream sits in the same place whichever way the icon is drawn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Icon {
    /// Column of the icon's left edge.
    pub col: usize,
    /// The glyph to draw when the icon style is a font, in place of artwork.
    pub glyph: char,
    /// What the icon depicts.
    pub kind: IconKind,
    /// Row within the tool's returned rows.
    pub row: usize,
}

impl Icon {
    /// Columns an icon occupies: the artwork is square and a cell is roughly
    /// half as wide as it is tall, so one cell would squash it.
    pub const WIDTH: usize = 2;
}

/// What a [`Icon`] depicts, in terms the icon set can resolve.
#[derive(Clone, Debug, Eq, PartialEq)]
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
}

/// One rendered line: styled runs laid out left to right from column zero.
pub type Row = Vec<Span>;

/// The rows of a tool to paint in a pane, once wrapping is accounted for:
/// where the window starts, how many rows fill the pane, and which screen
/// row the cursor paints on.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RowWindow {
    /// How many rows to paint; the pane clips whatever wraps past its bottom.
    pub count: usize,
    /// The screen row the cursor's row paints on, counted from the first
    /// painted row, in screen rows once wrapping is on.
    pub cursor: usize,
    /// The first of the tool's own rows to paint.
    pub start: usize,
}

/// A run of text sharing one style.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Span {
    /// How the run is colored.
    pub style: Style,
    /// The run's text.
    pub text: String,
}

/// A semantic style, resolved against the active theme so a tool never names a
/// color of its own.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Style {
    /// An added diff line: arriving, faintly.
    Added,
    /// The words an edit added: arriving, emphatically.
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
    /// Selected by the user for an operation to act on.
    Marked,
    /// Ordinary text.
    #[default]
    Normal,
    /// A removed diff line: leaving, faintly.
    Removed,
    /// The words an edit removed: leaving, emphatically.
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
    /// Source being read: a comment, standing back from the code it explains.
    SyntaxComment,
    /// Source being read: a word of the language rather than of the program.
    SyntaxKeyword,
    /// Source being read: a literal number.
    SyntaxNumber,
    /// Source being read: a literal string.
    SyntaxString,
    /// Source being read: the name of a type.
    SyntaxType,
}

/// Where a pointer is over a tool: the row and column of the pane's own
/// content, counted from the first row the tool painted.
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

/// Where a tool's own caret sits within the row it bands as its cursor line,
/// and whether what it is doing there is typing. A tool that reports one is
/// drawn with the caret on that cell, rather than on the row's first non-blank
/// character, which for a tool with a gutter is in the gutter.
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

/// The slice of `total` rows to show so that `cursor` stays visible, given a
/// pane that can draw `rows` of them below `header` pinned rows. `scroll` is
/// the current first visible row, and is moved the least amount that reveals
/// the cursor, so paging down a long listing does not jump the view around.
pub fn scroll_to_cursor(scroll: usize, cursor: usize, total: usize, visible: usize) -> usize {
    if visible == 0 {
        return 0;
    }
    let last_start = total.saturating_sub(visible);
    let mut start = scroll.min(last_start);
    if cursor < start {
        start = cursor;
    } else if cursor >= start + visible {
        start = cursor + 1 - visible;
    }
    start.min(last_start)
}

/// Where a view starts after the wheel turns `lines` rows (positive is up),
/// held inside the `total` rows so the last screenful is the furthest down.
pub fn wheel_scroll(scroll: usize, lines: isize, total: usize, visible: usize) -> usize {
    let last_start = total.saturating_sub(visible) as isize;
    (scroll as isize - lines).clamp(0, last_start.max(0)) as usize
}

/// The row to lay a window out around. Normally the cursor; while the wheel
/// holds the view, the cursor only if it still sits inside the `visible` rows
/// from `scroll`, else the first of them, so the window stays where the wheel
/// put it instead of being pulled back to a cursor it has scrolled away from.
/// A tool paints its cursor band only when this is the cursor itself.
pub fn window_anchor(held: bool, scroll: usize, cursor: usize, visible: usize) -> usize {
    match held && !(scroll..scroll + visible).contains(&cursor) {
        true => scroll,
        false => cursor,
    }
}

/// What a row puts on screen, with its styles dropped: the text a search reads.
pub fn row_text(row: &Row) -> String {
    row.iter().map(|span| span.text.as_str()).collect()
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
/// tool measures its rows by is the height they paint at.
pub fn row_height(text: &str, cols: usize, wrap: bool, indent: usize) -> usize {
    if !wrap || cols == 0 || text.chars().count() <= cols {
        return 1;
    }
    let chars: Vec<char> = text.chars().collect();
    wrapped_lines(&chars, cols, wrap, indent).len()
}

/// The window of a tool's rows to paint in a pane, so the cursor stays
/// visible: the same cursor-following window [`scroll_to_cursor`] gives, but
/// measured in screen rows, since a row wider than the pane wraps onto
/// several of them and leaves room for fewer of the tool's own rows. Each
/// row's height is read from `height`, which decides how a row wider than the
/// pane is counted, and only for the rows the walk visits — so a tool whose
/// rows are built as they are painted is not asked to build them all. With
/// every row one screen row this is the plain window.
pub fn wrap_window(
    scroll: usize,
    cursor: usize,
    total: usize,
    screen_rows: usize,
    height: impl Fn(usize) -> usize,
) -> RowWindow {
    // The logical clamp first: it lands jumps (a long way to the bottom, back
    // to the top) with the cursor inside a window of plain rows, without
    // measuring every row to get there.
    let mut start = scroll_to_cursor(scroll, cursor, total, screen_rows);
    // Enough rows from `start` to fill the pane, counting wrapped heights.
    let fill_from = |start: usize| {
        let mut filled = 0;
        let mut count = 0;
        for index in start..total {
            filled += height(index);
            count += 1;
            if filled >= screen_rows {
                break;
            }
        }
        count
    };
    let mut count = fill_from(start);
    // Wrapping shrinks the window below the cursor the logical clamp put
    // inside it, so when it did, the cursor's own row goes to the top — the
    // one place it is certainly on screen, even if its own tail wraps past
    // the pane's bottom.
    if !(start..start + count).contains(&cursor) {
        start = cursor.min(total.saturating_sub(1));
        count = fill_from(start);
    }
    // The cursor's screen row, counted from the first painted row.
    let mut cursor_row = 0;
    for index in start..cursor.min(start + count) {
        cursor_row += height(index);
    }
    RowWindow {
        count,
        cursor: cursor_row,
        start,
    }
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

    /// Band `row` as the cursor line, counted from the first returned row.
    pub fn with_cursor_line(mut self, row: usize) -> Self {
        self.cursor_line = Some(row);
        self
    }
}

// ========================================================================
// Span
// ========================================================================

impl Span {
    /// A run drawn in `style`.
    pub fn new(style: Style, text: impl Into<String>) -> Self {
        Self {
            style,
            text: text.into(),
        }
    }

    /// A run drawn in the tool's ordinary text style.
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
    fn test_the_wheel_stops_at_the_first_and_last_screenful() {
        // Up is positive. The furthest down is the last `visible` rows, not
        // the last row, so the view never scrolls into blank space.
        assert_eq!(wheel_scroll(10, 3, 100, 20), 7);
        assert_eq!(wheel_scroll(2, 3, 100, 20), 0);
        assert_eq!(wheel_scroll(70, -3, 100, 20), 73);
        assert_eq!(wheel_scroll(79, -3, 100, 20), 80);
        assert_eq!(wheel_scroll(0, -3, 5, 20), 0, "a short page does not move");
    }

    #[test]
    fn test_a_held_view_keeps_its_window_off_a_cursor_it_has_left() {
        // The cursor in view is the anchor either way; off screen, only a
        // held view lets go of it, which is what lets the wheel leave the
        // cursor behind like an editor does.
        assert_eq!(window_anchor(true, 40, 45, 20), 45);
        assert_eq!(window_anchor(true, 40, 5, 20), 40);
        assert_eq!(window_anchor(true, 40, 90, 20), 40);
        assert_eq!(window_anchor(false, 40, 5, 20), 5);
    }

    #[test]
    fn test_a_cursor_already_in_view_does_not_move_the_window() {
        assert_eq!(scroll_to_cursor(10, 15, 100, 20), 10);
    }

    #[test]
    fn test_the_window_follows_the_cursor_by_the_least_it_can() {
        // Stepping one row past the bottom must scroll one row, not a page.
        assert_eq!(scroll_to_cursor(0, 20, 100, 20), 1);
        assert_eq!(scroll_to_cursor(10, 9, 100, 20), 9);
    }

    #[test]
    fn test_the_window_never_scrolls_past_the_last_row() {
        // Otherwise a listing that shrank under a scrolled view (a reload after
        // deleting files) would paint a screen of blank rows.
        assert_eq!(scroll_to_cursor(90, 5, 10, 20), 0);
        assert_eq!(scroll_to_cursor(0, 99, 100, 20), 80);
    }

    #[test]
    fn test_a_pane_with_no_room_shows_the_first_row() {
        assert_eq!(scroll_to_cursor(5, 9, 100, 0), 0);
    }

    #[test]
    fn test_without_wrapping_the_window_is_one_row_per_row() {
        // Every row one screen row: the window is the plain cursor-following
        // one, and the count is the pane's rows.
        let window = wrap_window(10, 15, 100, 20, |_| 1);
        assert_eq!(
            window,
            RowWindow {
                count: 20,
                cursor: 5,
                start: 10
            }
        );
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
    fn test_a_wrapped_row_pushes_the_cursor_down_a_screen_row() {
        // A row five panes wide takes five screen rows, so the row under it
        // paints on screen row five, not one.
        let heights = [5, 1, 1, 1];
        let window = wrap_window(0, 1, heights.len(), 10, |index| heights[index]);
        assert_eq!(window.cursor, 5);
        // Four rows heights 5+1+1+1 fill eight of ten screen rows.
        assert_eq!(window.count, 4);
    }

    #[test]
    fn test_a_cursor_squeezed_out_by_wrapping_goes_to_the_top() {
        // Rows tall enough that the filled window ends before the cursor:
        // the cursor's own row becomes the first painted one, where it is
        // certainly on screen.
        let heights = [4, 4, 4, 4, 4];
        let window = wrap_window(0, 4, heights.len(), 10, |index| heights[index]);
        assert_eq!(window.start, 4);
        assert_eq!(window.cursor, 0);
    }

    #[test]
    fn test_wrapping_fits_fewer_rows_than_the_pane_is_tall() {
        // Ten screen rows of two-row-tall rows hold five of the tool's rows.
        let heights = [2; 30];
        let window = wrap_window(0, 0, heights.len(), 10, |index| heights[index]);
        assert_eq!(window.count, 5);
    }

    #[test]
    fn test_an_empty_tool_windows_to_nothing() {
        let window = wrap_window(3, 0, 0, 10, |_| 1);
        assert_eq!(
            window,
            RowWindow {
                count: 0,
                cursor: 0,
                start: 0
            }
        );
    }
}
