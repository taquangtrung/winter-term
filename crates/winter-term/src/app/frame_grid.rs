//! Painting a tool's styled rows into a cell grid: each span's semantic
//! style resolved against the theme, rows folded or clipped to the pane.

use crate::model::paint::{
    self, row_height, row_text, wrap_start, wrapped_lines, Frame, SyntaxStyle,
};
use unicode_width::UnicodeWidthChar;
use winter_render::{Color, Grid, RgbColor, Style, Theme, ThemeRgb, SCOPE_RULES};

// ========================================================================
// Constants
// ========================================================================

/// How far a tool's dim style is blended toward the background.
const PAGE_DIM_MIX: f32 = 0.45;

/// How far an added or removed diff line's background is blended toward the
/// theme's green or red: a tint that reads at a glance without fighting the
/// text, the way a diff editor tints a whole inserted or deleted line.
const PAGE_DIFF_LINE_MIX: f32 = 0.15;

/// How far the edited words within a diff line blend harder than their line,
/// the way a diff editor's word highlight sits on top of its line one.
const PAGE_DIFF_EDIT_MIX: f32 = 0.38;

/// How far a hunk header's band is blended toward the theme's blue: darker
/// than a file's band, so the hunk reads as sitting under its file.
const PAGE_HUNK_MIX: f32 = 0.22;

/// How far a file's band is blended toward the theme's cyan.
///
/// Kept well short of the cyan itself: the change's name is painted on this
/// band in its own hue, and a band bright enough to read as a solid bar
/// washes those hues out, worst on the red a deletion carries. Still kept
/// clear of [`PAGE_HUNK_MIX`], so a file's band reads as the louder row where
/// one sits directly above a hunk's.
const PAGE_SECTION_MIX: f32 = 0.34;

/// The ANSI palette slots a diff's colors are drawn from: red and green for
/// the sides, blue and cyan for the header bands, yellow for a tag's ref.
const ANSI_RED: usize = 1;

const ANSI_GREEN: usize = 2;

const ANSI_YELLOW: usize = 3;

const ANSI_BLUE: usize = 4;

const ANSI_MAGENTA: usize = 5;

const ANSI_CYAN: usize = 6;

/// The palette slot a comment is drawn from: the receded gray every shell and
/// pager already writes one in.
const ANSI_BRIGHT_BLACK: usize = 8;

/// The scopes that name a type, which a theme without syntax colors draws in
/// its cyan: VS Code's spelling, and the Sublime grammars' more particular one.
const TYPE_SCOPES: [&str; 9] = [
    "entity.name.class",
    "entity.name.enum",
    "entity.name.impl",
    "entity.name.interface",
    "entity.name.struct",
    "entity.name.trait",
    "entity.name.type",
    "entity.name.union",
    "support.type",
];

// ========================================================================
// Functions
// ========================================================================

/// Paint a tool into a fresh `cols` x `rows` grid: its rows in order, each span
/// resolved from a semantic style to the theme's colors, clipped at the pane's
/// last row.
pub(super) fn build_frame_grid(
    content: &Frame,
    theme: &Theme,
    cols: usize,
    rows: usize,
    wrap: bool,
) -> Grid {
    let mut grid = Grid::new(cols, rows);
    if cols == 0 {
        return grid;
    }
    let mut screen = 0;
    'tool: for (index, spans) in content.rows.iter().enumerate() {
        // The column this row's wrapped continuations start at, reserving the
        // room its leading span gives the diff marker.
        let indent = wrap_start(content.wrap_indents.get(index).copied().unwrap_or(0), cols);
        let lead = spans.first().map(|span| span_style(span.style, theme));
        // The row flattened across its spans, each char carrying its own
        // style: wrapping works on words, which straddle spans, so the fold
        // is chosen over the whole row and each painted piece keeps the
        // color of the span it came from.
        let mut chars = Vec::new();
        let mut styles = Vec::new();
        for span in spans {
            let style = span_style(span.style, theme);
            for ch in span.text.chars() {
                chars.push(ch);
                styles.push(style);
            }
        }
        for (line, (start, end)) in wrapped_lines(&chars, cols, wrap, indent)
            .iter()
            .copied()
            .enumerate()
        {
            let mut col = 0;
            if line > 0 {
                screen += 1;
                if screen >= rows {
                    break 'tool;
                }
                col = indent;
                if indent > 0 {
                    // The indent keeps the row's own tint, from its leading
                    // span, so a wrapped line reads as one tinted line
                    // rather than one that loses its color at the fold.
                    if let Some(style) = lead {
                        let gutter = " ".repeat(indent);
                        put(&mut grid, screen, 0, &gutter, style);
                    }
                }
            }
            // Paint the line a same-style run at a time, so a span the fold
            // split still colors both of its pieces.
            let mut at = start;
            while at < end {
                let style = styles[at];
                let run_end = (at + 1..end).find(|&i| styles[i] != style).unwrap_or(end);
                let piece: String = chars[at..run_end].iter().collect();
                // The next run starts where this one's cells end, which is
                // not where its characters do once one of them is wide.
                col = put(&mut grid, screen, col, &piece, style);
                at = run_end;
            }
        }
        screen += 1;
        if screen >= rows {
            break;
        }
    }
    grid
}

/// The screen row each of a tool's own rows starts on in a pane `cols` wide,
/// folded the way [`build_frame_grid`] folds them. An icon names its row among
/// the tool's, so below a row that wrapped it lands only through this.
pub(super) fn screen_row_starts(content: &Frame, cols: usize, wrap: bool) -> Vec<usize> {
    let mut starts = Vec::with_capacity(content.rows.len());
    let mut screen = 0;
    for (index, spans) in content.rows.iter().enumerate() {
        starts.push(screen);
        let indent = content.wrap_indents.get(index).copied().unwrap_or(0);
        screen += row_height(&row_text(spans), cols, wrap, indent);
    }
    starts
}

/// Resolve a tool's semantic style against the active theme. The diff styles
/// tint the background with the theme's palette the way a diff editor does,
/// rather than carrying colors of their own, so every theme gets bands that
/// belong to it.
fn span_style(style: paint::Style, theme: &Theme) -> Style {
    match style {
        paint::Style::Accent => Style {
            foreground: theme_rgb(theme.cursor_bg),
            ..Style::default()
        },
        paint::Style::Added => Style {
            background: git_band(
                theme,
                theme.git.inserted_line,
                ANSI_GREEN,
                PAGE_DIFF_LINE_MIX,
            ),
            foreground: theme_rgb(theme.foreground),
            ..Style::default()
        },
        paint::Style::AddedEdit => Style {
            background: git_band(
                theme,
                theme.git.inserted_text,
                ANSI_GREEN,
                PAGE_DIFF_EDIT_MIX,
            ),
            bold: true,
            foreground: theme_rgb(theme.foreground),
            ..Style::default()
        },
        // What a change did to a file is told by hue and carried in bold, the
        // colors magic-vscode's change list uses resolved against the theme's
        // own palette rather than pinned to its hex: green for what arrived,
        // red for what left, blue for what was edited where it stands, yellow
        // for what moved, magenta for what is still contested.
        paint::Style::ChangeAdded => change_style(theme, theme.git.added, ANSI_GREEN),
        paint::Style::ChangeConflict => change_style(theme, theme.git.unmerged, ANSI_MAGENTA),
        paint::Style::ChangeDeleted => change_style(theme, theme.git.deleted, ANSI_RED),
        paint::Style::ChangeModified => change_style(theme, theme.git.modified, ANSI_BLUE),
        paint::Style::ChangeRenamed => change_style(theme, theme.git.renamed, ANSI_YELLOW),
        paint::Style::Dim => Style {
            foreground: mix_rgb(theme.foreground, theme.background, PAGE_DIM_MIX),
            ..Style::default()
        },
        // A file's path on its band wears its section's hue, the colors
        // magic-vscode's semantic tokens give staged, unstaged, untracked and
        // conflicted files, with the theme's own cyan, yellow, green and red
        // where it sets none.
        paint::Style::FileConflict => file_path_style(theme, theme.git.file_conflict, ANSI_RED),
        paint::Style::FileStaged => file_path_style(theme, theme.git.file_staged, ANSI_CYAN),
        paint::Style::FileUnstaged => file_path_style(theme, theme.git.file_unstaged, ANSI_YELLOW),
        paint::Style::FileUntracked => file_path_style(theme, theme.git.file_untracked, ANSI_GREEN),
        paint::Style::Header => Style {
            bold: true,
            foreground: theme_rgb(theme.foreground),
            ..Style::default()
        },
        paint::Style::HeaderAccent => Style {
            bold: true,
            foreground: theme_rgb(theme.cursor_bg),
            ..Style::default()
        },
        // A section's heading wears the hue of what sits under it, the way
        // magic-vscode's own headings do: cyan for what is staged, yellow for
        // what is not, green for what is untracked, red for what a merge left
        // contested. The count beside it stays receded, so the color says
        // which section without the row shouting.
        paint::Style::HeadingConflict => heading_style(theme, None, ANSI_RED),
        paint::Style::HeadingStaged => heading_style(theme, theme.git.header_staged, ANSI_CYAN),
        paint::Style::HeadingUnstaged => {
            heading_style(theme, theme.git.header_unstaged, ANSI_YELLOW)
        }
        paint::Style::HeadingUntracked => {
            heading_style(theme, theme.git.header_untracked, ANSI_GREEN)
        }
        // Everything else a git view heads a block with (the recent commits,
        // a log's title, and the labels the header block is read down) is
        // blue, which is where magic-vscode's grammar sends every heading it
        // has no change color for.
        paint::Style::HeadingPlain => heading_style(theme, theme.git.header, ANSI_BLUE),
        paint::Style::Hunk => Style {
            background: git_band(theme, theme.git.hunk_band, ANSI_BLUE, PAGE_HUNK_MIX),
            foreground: mix_rgb(theme.foreground, theme.background, PAGE_DIM_MIX),
            ..Style::default()
        },
        // The theme's selection text, unless it would not read on the
        // selection (the default dark theme's black on dark blue), when it
        // turns black or white, whichever does.
        paint::Style::Marked => Style {
            background: theme_rgb(theme.selection_bg),
            bold: true,
            foreground: theme_rgb(theme.selection_fg.readable_on(theme.selection_bg)),
            ..Style::default()
        },
        paint::Style::Operation => heading_style(theme, theme.git.unmerged, ANSI_MAGENTA),
        paint::Style::Normal => Style {
            foreground: theme_rgb(theme.foreground),
            ..Style::default()
        },
        paint::Style::Removed => Style {
            background: git_band(theme, theme.git.removed_line, ANSI_RED, PAGE_DIFF_LINE_MIX),
            foreground: theme_rgb(theme.foreground),
            ..Style::default()
        },
        paint::Style::RemovedEdit => Style {
            background: git_band(theme, theme.git.removed_text, ANSI_RED, PAGE_DIFF_EDIT_MIX),
            bold: true,
            foreground: theme_rgb(theme.foreground),
            ..Style::default()
        },
        // The checked-out branch and the remote ones (`origin/main`) share a
        // bold magenta, so the eye lands on where the work is and where it
        // was shared to; other local branches are the same hue unbolded, and
        // tags cyan, the hues magic-vscode uses.
        paint::Style::RefHead => Style {
            bold: true,
            foreground: color_or(theme, theme.git.ref_head, theme.ansi[ANSI_MAGENTA]),
            ..Style::default()
        },
        paint::Style::RefLocal => Style {
            foreground: color_or(theme, theme.git.ref_local, theme.ansi[ANSI_MAGENTA]),
            ..Style::default()
        },
        paint::Style::RefRemote => Style {
            bold: true,
            foreground: color_or(theme, theme.git.ref_remote, theme.ansi[ANSI_MAGENTA]),
            ..Style::default()
        },
        paint::Style::RefTag => Style {
            foreground: color_or(theme, theme.git.ref_tag, theme.ansi[ANSI_CYAN]),
            ..Style::default()
        },
        paint::Style::Section => Style {
            background: section_band(theme),
            foreground: theme_rgb(theme.foreground),
            ..Style::default()
        },
        // Source is colored by the theme's scope table, and where the theme
        // sets no color for a row, out of its own palette: the comment hue
        // the shells and the pagers already use, and one hue per kind of
        // thing after that.
        paint::Style::Syntax(rows) => syntax_style(theme, rows),
        paint::Style::SyntaxComment => Style {
            foreground: color_or(
                theme,
                theme.syntax.get("comment"),
                theme.ansi[ANSI_BRIGHT_BLACK],
            ),
            ..Style::default()
        },
        paint::Style::Unpushed => Style {
            bold: true,
            foreground: color_or(theme, theme.git.unpushed, theme.ansi[ANSI_RED]),
            ..Style::default()
        },
        paint::Style::SectionFolded => Style {
            background: section_band(theme),
            foreground: mix_rgb(theme.foreground, theme.background, PAGE_DIM_MIX),
            ..Style::default()
        },
    }
}

/// How text the scope table's rows color and weigh is drawn: in the theme's
/// color for the color row, else in the palette color of the kind of thing
/// that row names, else in the text color, and as heavy as the weight row
/// says.
fn syntax_style(theme: &Theme, rows: SyntaxStyle) -> Style {
    let row = |index: Option<u16>| index.and_then(|index| SCOPE_RULES.get(index as usize));
    let foreground = match rows.color {
        Some(index) => {
            let fallback = row(Some(index))
                .and_then(|rule| palette_slot(rule.scope))
                .map_or(theme.foreground, |slot| theme.ansi[slot]);
            color_or(theme, theme.syntax.at(index as usize), fallback)
        }
        None => theme_rgb(theme.foreground),
    };
    let weight = row(rows.weight);
    Style {
        bold: weight.is_some_and(|rule| rule.bold),
        foreground,
        italic: weight.is_some_and(|rule| rule.italic),
        ..Style::default()
    }
}

/// The palette slot for what a scope selector names, for the kinds of thing
/// a theme without syntax colors of its own still tells apart. The selector's
/// last word is the scope it colors: the ones before it only say where.
fn palette_slot(selector: &str) -> Option<usize> {
    let scope = selector.rsplit(' ').next().unwrap_or(selector);
    let is = |kind: &str| scope == kind || scope.starts_with(&format!("{kind}."));
    let slot = |kinds: &[&str]| kinds.iter().any(|kind| is(kind));
    if slot(&["comment", "punctuation.definition.comment"]) {
        return Some(ANSI_BRIGHT_BLACK);
    }
    if slot(&["string", "punctuation.definition.string", "markup.raw"]) {
        return Some(ANSI_GREEN);
    }
    if slot(&["constant.numeric"]) {
        return Some(ANSI_YELLOW);
    }
    if slot(&["keyword", "storage"]) {
        return Some(ANSI_MAGENTA);
    }
    if slot(&TYPE_SCOPES) {
        return Some(ANSI_CYAN);
    }
    None
}

/// The band a file's row is painted on, the same whichever way the row is
/// folded: the fold triangle at its head says which, and a band that changed
/// color with it would break the row into two tones, since the change's name
/// beside the triangle carries a hue of its own.
fn section_band(theme: &Theme) -> Color {
    git_band(theme, theme.git.file_band, ANSI_CYAN, PAGE_SECTION_MIX)
}

/// How a section's heading is painted: its own hue, in bold, since a heading
/// is the loudest row of the block it opens.
fn heading_style(theme: &Theme, hue: Option<ThemeRgb>, ansi: usize) -> Style {
    Style {
        bold: true,
        foreground: color_or(theme, hue, theme.ansi[ansi]),
        ..Style::default()
    }
}

/// How a change's name is painted where it heads a file's band: its own hue,
/// in bold, over the band the rest of the row sits on, so the row reads as one
/// bar with the change called out on it.
fn change_style(theme: &Theme, hue: Option<ThemeRgb>, ansi: usize) -> Style {
    Style {
        background: section_band(theme),
        bold: true,
        foreground: color_or(theme, hue, theme.ansi[ansi]),
        ..Style::default()
    }
}

/// A file's path on its band: the section's hue over the same band the change's
/// name sits on, so the row reads as one bar.
fn file_path_style(theme: &Theme, hue: Option<ThemeRgb>, ansi: usize) -> Style {
    Style {
        background: section_band(theme),
        foreground: color_or(theme, hue, theme.ansi[ansi]),
        ..Style::default()
    }
}

/// A themed color: the theme's own, laid over the background when it is
/// translucent, or `fallback` out of the theme's palette when it sets none.
fn color_or(theme: &Theme, color: Option<ThemeRgb>, fallback: ThemeRgb) -> Color {
    theme_rgb(color.map_or(fallback, |c| c.over(theme.background)))
}

/// A git view's background band, or a mix of the background toward `ansi` when
/// the theme sets none.
fn git_band(theme: &Theme, band: Option<ThemeRgb>, ansi: usize, mix: f32) -> Color {
    match band {
        Some(band) => theme_rgb(band.over(theme.background)),
        None => mix_rgb(theme.background, theme.ansi[ansi], mix),
    }
}

/// Write `text` into `grid` starting at `(row, col)`, in `style`, truncated to
/// the grid width so it never wraps onto the next row. Returns the column the
/// text ends at, counted in cells as the grid lays them out: a wide character
/// takes two, a combining mark none.
pub(super) fn put(grid: &mut Grid, row: usize, col: usize, text: &str, style: Style) -> usize {
    let cols = grid.cols();
    if col >= cols {
        return col;
    }
    grid.move_to(row, col);
    grid.set_style(style);
    let mut end = col;
    for ch in text.chars() {
        let width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if end + width > cols {
            break;
        }
        grid.print(ch);
        end += width;
    }
    end
}

/// Convert a theme color into an explicit grid cell color.
pub(super) fn theme_rgb(c: ThemeRgb) -> Color {
    Color::Rgb(RgbColor {
        r: c.r,
        g: c.g,
        b: c.b,
    })
}

/// Blend `a` toward `b` by `t` in `[0, 1]`, e.g. to derive a muted text color
/// partway between the foreground and the background.
pub(super) fn mix_rgb(a: ThemeRgb, b: ThemeRgb, t: f32) -> Color {
    let blend = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t).round() as u8;
    Color::Rgb(RgbColor {
        r: blend(a.r, b.r),
        g: blend(a.g, b.g),
        b: blend(a.b, b.b),
    })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::paint::{Row, Span};

    #[test]
    fn test_frame_grid_lays_spans_out_left_to_right() {
        let content = Frame::new(vec![vec![
            Span::plain("Zoom Pane"),
            Span::new(paint::Style::Accent, "M-S-="),
        ]]);
        let grid = build_frame_grid(&content, &Theme::dark(), 40, 4, false);
        let first = grid
            .to_text()
            .lines()
            .next()
            .unwrap_or_default()
            .to_string();
        assert_eq!(first.trim_end(), "Zoom PaneM-S-=");
    }

    #[test]
    fn test_a_run_after_wide_characters_starts_past_their_cells() {
        // Regression: a run started as many columns on as the one before it
        // had characters, so after a wide one it overwrote the second half
        // of it, and every run after drifted further left of its text.
        let content = Frame::new(vec![vec![
            Span::plain("\u{65e5}\u{672c}"),
            Span::new(paint::Style::Accent, "x"),
        ]]);
        let grid = build_frame_grid(&content, &Theme::dark(), 10, 1, false);
        let accent = span_style(paint::Style::Accent, &Theme::dark());
        let cell = grid
            .cell(0, 4)
            .expect("the cell after both wide characters");
        assert_eq!(cell.ch, 'x', "the run lands past the wide characters");
        assert_eq!(cell.style, accent);
        assert_eq!(
            grid.cell(0, 2).map(|cell| cell.ch),
            Some('\u{672c}'),
            "and the second wide character is whole"
        );
    }

    #[test]
    fn test_text_too_wide_for_the_row_stops_rather_than_wrapping() {
        let mut grid = Grid::new(3, 2);
        let end = put(&mut grid, 0, 0, "\u{65e5}\u{672c}", Style::default());
        assert_eq!(end, 2, "the second wide character does not fit");
        assert!(grid
            .to_text()
            .lines()
            .nth(1)
            .unwrap_or_default()
            .trim()
            .is_empty());
    }

    #[test]
    fn test_frame_grid_clips_rows_past_the_pane_height() {
        // A tool longer than its pane must lose the overflow, not write past
        // the grid's last row.
        let rows: Vec<Row> = (0..10)
            .map(|i| vec![Span::plain(format!("row{i}"))])
            .collect();
        let grid = build_frame_grid(&Frame::new(rows), &Theme::dark(), 20, 3, false);
        let text = grid.to_text();
        assert!(text.contains("row2"), "the last visible row is painted");
        assert!(
            !text.contains("row3"),
            "rows past the pane height are dropped"
        );
    }

    #[test]
    fn test_a_row_wider_than_the_pane_wraps_onto_the_next_screen_row() {
        let content = Frame::new(vec![vec![Span::plain("abcdefghij")]]);
        let wrapped = build_frame_grid(&content, &Theme::dark(), 5, 2, true);
        let text = wrapped.to_text();
        // Five cells a line: the row's text spills onto the screen row under
        // it rather than being lost at the pane's edge.
        assert!(text.contains("abcde"), "the first line, got {text:?}");
        assert!(text.contains("fghij"), "the wrapped rest, got {text:?}");
        let clipped = build_frame_grid(&content, &Theme::dark(), 5, 2, false);
        let text = clipped.to_text();
        assert!(text.contains("abcde"), "the first line, got {text:?}");
        assert!(!text.contains("fghij"), "the edge clips the rest");
    }

    #[test]
    fn test_a_wrapped_row_folds_at_its_last_word_boundary() {
        // "the quick brown fox" in ten cells folds after "quick", the last
        // word that fits, and the space the fold lands on vanishes with it.
        let content = Frame::new(vec![vec![Span::plain("the quick brown fox")]]);
        let grid = build_frame_grid(&content, &Theme::dark(), 10, 2, true);
        let text = grid.to_text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0].trim_end(), "the quick", "folds at the space");
        assert_eq!(lines[1].trim_end(), "brown fox", "carries the word down");
    }

    #[test]
    fn test_what_git_is_part_way_through_wears_a_conflict_s_hue_on_no_band() {
        let theme = Theme::dark();
        let operation = span_style(paint::Style::Operation, &theme);
        let conflict = span_style(paint::Style::ChangeConflict, &theme);
        assert_eq!(operation.foreground, conflict.foreground);
        assert_eq!(operation.background, Style::default().background, "no band");
        assert!(operation.bold);
    }

    #[test]
    fn test_a_fold_splits_spans_but_keeps_each_pieces_style() {
        // The fold lands inside the second span, and both of its pieces keep
        // the span's color: words wrap, colors follow the words. The break
        // lands before "bbb", not after it: the space past the margin belongs
        // to the fold, exactly where the grid's own wrap would take it.
        let content = Frame::new(vec![vec![
            Span::plain("aaa "),
            Span::new(paint::Style::Accent, "bbb ccc"),
        ]]);
        let theme = Theme::dark();
        let grid = build_frame_grid(&content, &theme, 7, 2, true);
        let text = grid.to_text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0].trim_end(), "aaa");
        assert_eq!(lines[1].trim_end(), "bbb ccc");
        let plain = span_style(paint::Style::Normal, &theme);
        let accent = span_style(paint::Style::Accent, &theme);
        assert_eq!(grid.cell(0, 0).map(|c| c.style), Some(plain));
        assert_eq!(
            grid.cell(0, 3).map(|c| c.style),
            Some(plain),
            "the span's own space"
        );
        assert_eq!(
            grid.cell(1, 0).map(|c| c.style),
            Some(accent),
            "the piece after the fold"
        );
        assert_eq!(
            grid.cell(1, 6).map(|c| c.style),
            Some(accent),
            "the span's tail on the same line"
        );
    }

    #[test]
    fn test_wrapping_pushes_the_rows_under_a_wrapped_one_down() {
        // Two spans in one row: the second must follow the first onto its
        // wrapped line, and a whole second row must land below the lines the
        // first took, not on top of them.
        let content = Frame::new(vec![
            vec![Span::plain("aaa"), Span::plain("bbb")],
            vec![Span::plain("zz")],
        ]);
        let grid = build_frame_grid(&content, &Theme::dark(), 4, 3, true);
        let text = grid.to_text();
        assert!(text.contains("aaab"), "the spans share the wrapped line");
        assert!(text.contains("bb"), "the row's wrapped tail");
        let zz_line = text
            .lines()
            .position(|line| line.contains("zz"))
            .expect("the second row");
        let tail_line = text
            .lines()
            .position(|line| line.trim() == "bb")
            .expect("the wrapped tail");
        assert!(zz_line > tail_line, "the second row paints under the wrap");
    }

    #[test]
    fn test_screen_row_starts_follow_the_rows_the_grid_wrapped() {
        // An icon is placed by the start this returns, so it has to name the
        // screen row the grid actually painted its row on: counting the tool's
        // rows instead puts every icon under a wrapped row one line too high.
        let content = Frame::new(vec![
            vec![Span::plain("aaaa bbbb")],
            vec![Span::plain("zz")],
        ]);
        let text = build_frame_grid(&content, &Theme::dark(), 5, 4, true).to_text();
        let zz_line = text
            .lines()
            .position(|line| line.contains("zz"))
            .expect("the second row");

        assert_eq!(screen_row_starts(&content, 5, true), [0, zz_line]);
        assert_eq!(zz_line, 2, "the first row took two lines");
        assert_eq!(screen_row_starts(&content, 5, false), [0, 1]);
    }

    #[test]
    fn test_a_clipped_row_still_takes_its_own_screen_row() {
        // With wrapping off, a row wider than the pane loses its tail, but it
        // still occupies one screen row, and the row under it must paint on
        // the next screen row rather than over the clipped one.
        let content = Frame::new(vec![
            vec![Span::plain("0123456789")],
            vec![Span::plain("next")],
        ]);
        let grid = build_frame_grid(&content, &Theme::dark(), 5, 2, false);
        let text = grid.to_text();
        assert!(text.contains("01234"), "the clipped row keeps its start");
        assert!(text.contains("next"), "the next row paints under it");
    }

    #[test]
    fn test_a_wrapped_diff_line_continues_past_its_marker() {
        // The continuation of a wrapped diff line starts past the marker's
        // column, so the spilled text lines up under the text it follows, not
        // under the marker, and the room it leaves keeps the line's own
        // tint, so a wrapped line reads as one tinted line rather than one
        // that loses its color at the fold.
        let content = Frame::new(vec![vec![
            Span::new(paint::Style::Removed, "    -"),
            Span::new(paint::Style::RemovedEdit, "old".repeat(4)),
        ]])
        .with_wrap_indents(vec![5]);
        let theme = Theme::dark();
        let grid = build_frame_grid(&content, &theme, 10, 3, true);
        let text = grid.to_text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "    -oldol", "the marker and the first cells");
        assert_eq!(
            lines[1], "     doldo",
            "the continuation starts past the marker"
        );
        assert_eq!(lines[2], "     ld", "the last of the spilled text");
        assert_eq!(
            grid.cell(1, 0).map(|cell| cell.style),
            Some(span_style(paint::Style::Removed, &theme)),
            "the gutter keeps the line's tint"
        );
        assert_eq!(
            grid.cell(1, 5).map(|cell| cell.style),
            Some(span_style(paint::Style::RemovedEdit, &theme)),
            "the spilled text keeps its own style"
        );
    }

    #[test]
    fn test_a_truncated_diff_line_keeps_its_row_under_the_next_line() {
        // The shape the bug came from: a removed line long enough to be
        // clipped, with the added line under it. Before the fix the added
        // line painted over the removed one's screen row, so the decorator
        // showed one line wearing the wrong side's tint.
        let content = Frame::new(vec![
            vec![
                Span::new(paint::Style::Removed, "-"),
                Span::new(paint::Style::RemovedEdit, "old".repeat(10)),
            ],
            vec![
                Span::new(paint::Style::Added, "+"),
                Span::new(paint::Style::AddedEdit, "new"),
            ],
        ]);
        let theme = Theme::dark();
        let grid = build_frame_grid(&content, &theme, 6, 2, false);
        let text = grid.to_text();
        assert!(
            text.contains("-oldol"),
            "the truncated removed line keeps its row, got {text:?}"
        );
        assert!(
            text.contains("+new"),
            "the added line paints on its own row, got {text:?}"
        );
        assert_eq!(
            grid.cell(0, 0).map(|cell| cell.style),
            Some(span_style(paint::Style::Removed, &theme)),
            "the removed row's cells keep the removed tint"
        );
        assert_eq!(
            grid.cell(1, 0).map(|cell| cell.style),
            Some(span_style(paint::Style::Added, &theme)),
            "the added row's cells carry the added tint"
        );
    }
}
