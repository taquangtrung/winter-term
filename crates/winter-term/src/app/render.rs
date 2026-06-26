//! Frame composition: the panes, the chrome, and the canvas's images, put
//! together for one frame.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::model::input::PendingPrefix;
use crate::model::layout::{Rect, TabId};
use crate::model::mode::Mode;
use crate::model::paint::{Icon, IconKind};
use crate::model::palette::{Palette, PaletteMode};
use winter_render::renderer::{PaneRect, PaneView};
use winter_render::{
    CellOverlay, CursorShape, FindView, Grid, ImagePlacement, InputView, PaletteItem, PaletteView,
    StatusSearch, Theme,
};

use super::blocks::{image_placements, refresh_block_textures};
use super::frame_grid::{build_frame_grid, screen_row_starts};
use super::navigation::search::{painted_match_spans, visible_match_cells};
use super::open_tabs::OpenTabs;
use super::overlay::Overlay;
use super::sequence::WHICH_KEY_DELAY;
use super::settings_grid::build_settings_grid;
use super::App;

// ========================================================================
// Constants
// ========================================================================

/// Base texture ID for rasterized pane tab strips (below chrome overlay IDs).
const PANE_STRIP_TEXTURE_BASE: u64 = u64::MAX - 200;

/// What the tab list is headed by: rows naming tools and directories do not
/// say on their own what picking one would do.
const TAB_LIST_TITLE: &str = "Tabs: open, then recently closed";

// ========================================================================
// Frame composition helpers
// ========================================================================

/// One pane's rainbow-parens marks: viewport `(row, col)` plus the resolved
/// RGB to paint that bracket glyph in.
type BracketColors = Vec<(usize, usize, (u8, u8, u8))>;

/// One frame's per-pane overlay data, in the same order as the pane rects, so
/// a pane's entry is found by its index in that list.
struct PaneOverlays {
    bracket_colors: Vec<BracketColors>,
    find_labels: Vec<(usize, usize, char)>,
    quick_select: Vec<(usize, usize, char)>,
    search_current: Vec<Vec<(usize, usize)>>,
    search_matches: Vec<Vec<(usize, usize)>>,
    sentence_spans: Vec<Vec<(usize, usize, usize, u8)>>,
}

/// Everything [`build_pane_views`] reads, taken as individual field borrows so
/// the views it returns can borrow the panes while the renderer is separately
/// held mutably for the draw.
#[derive(Clone, Copy)]
struct PaneViewInput<'a> {
    blink_phase: bool,
    /// Whether blinking text is in its shown phase.
    text_blink_visible: bool,
    /// Text an input method is composing, drawn over the focused terminal.
    preedit: Option<&'a CellOverlay>,
    config: &'a crate::config::Config,
    /// The tab whose line flashes for having just taken the keyboard, and
    /// how strongly it is lit (see [`super::focus::FocusFlash`]).
    flash: Option<(TabId, f32)>,
    focused: TabId,
    /// The focused pane's block bands, `(abs_row, rows)` in anchor order: the
    /// reserved rows a nav cursor treats as one stop, and the span the
    /// block-as-cursor outline replaces the 1-cell cursor with.
    focused_bands: &'a [(usize, usize)],
    hovered_pane: Option<TabId>,
    hovered_url: Option<&'a str>,
    overlays: &'a PaneOverlays,
    tool_paints: &'a [ToolPaint],
    /// Where each tool's caret sits, and whether it is typing there.
    tool_cursors: &'a std::collections::HashMap<TabId, PaneCaret>,
    /// The pane drawn as active: the focused one, or none while a popup has
    /// the keyboard, so every pane reads as inactive under it.
    active: Option<TabId>,
    rects: &'a [(TabId, Rect)],
    selection: Option<&'a super::Selection>,
    /// Every open tab: the grid each tool last painted into, and each
    /// terminal with its mode.
    tabs: &'a OpenTabs,
    window_focused: bool,
}

/// One tool tab's painted frame: the grid the tool rendered itself into, and
/// the row its cursor line sits on.
pub(super) struct ToolPaint {
    pub(super) cursor_line: Option<usize>,
    /// Icons to rasterize over the painted rows, each `row` already a screen
    /// row of the pane rather than the tool's own row index.
    pub(super) icons: Vec<Icon>,
    pub(super) tab: TabId,
}

/// Build one `PaneView` per laid-out pane: what the renderer should draw for
/// that pane this frame.
fn build_pane_views<'a>(input: PaneViewInput<'a>) -> Vec<PaneView<'a>> {
    let PaneViewInput {
        active,
        blink_phase,
        text_blink_visible,
        preedit,
        config,
        flash,
        focused,
        focused_bands,
        hovered_pane,
        hovered_url,
        overlays,
        tool_cursors,
        tool_paints,
        rects,
        selection,
        tabs,
        window_focused,
    } = input;
    let mut views: Vec<PaneView> = Vec::new();
    for (i, (id, rect)) in rects.iter().enumerate() {
        // A tool tab paints a grid of its own; every other tab drawn is a
        // terminal.
        if let Some(paint) = tool_paints.iter().find(|paint| paint.tab == *id) {
            if let Some(grid) = tabs.tool(id).and_then(|slot| slot.painted.as_ref()) {
                views.push(tool_pane_view(
                    paint,
                    grid,
                    *rect,
                    active == Some(*id),
                    window_focused,
                    config,
                    blink_phase,
                    selection.filter(|s| s.tab == *id),
                    tool_cursors.get(id).copied(),
                    &overlays.search_matches[i],
                    &overlays.search_current[i],
                    flash_strength(flash, *id),
                ));
            }
        } else if let Some(terminal) = tabs.terminal(id) {
            let (sel_tuple, sel_block) = match selection {
                Some(s) if s.tab == *id => (
                    Some((s.start_row, s.start_col, s.end_row, s.end_col)),
                    s.block,
                ),
                _ => (None, false),
            };
            let labels = if *id == focused && !overlays.quick_select.is_empty() {
                Some(overlays.quick_select.as_slice())
            } else {
                None
            };
            // Each terminal's cursor follows its own mode (stored per terminal), so a
            // non-focused terminal shows the configured shape for its mode rather
            // than always reverting to a stale Block the shell reported via
            // DECSCUSR.
            let pane_mode = tabs.mode(id).unwrap_or_default();
            let is_active = active == Some(*id);
            let nav_cursor = if is_active && matches!(pane_mode, Mode::Normal | Mode::Visual) {
                tabs.nav_cursor(id)
            } else {
                None
            };
            // The block-as-cursor: while the traversal cursor sits inside a
            // rich block's reserved band, the band itself is the cursor. The
            // span is clipped to the viewport so an outline never draws past
            // an edge the image itself is cropped at.
            let block_band = nav_cursor.and_then(|(nav_row, _)| {
                focused_bands.iter().find_map(|&(abs_row, band_rows)| {
                    let top = terminal.grid().to_viewport_row(abs_row);
                    let bottom = top + band_rows as isize;
                    let rows = terminal.grid().rows() as isize;
                    let (start, end) = (top.max(0), bottom.min(rows));
                    let nav = nav_row as isize;
                    (nav >= start && nav < end)
                        .then(|| (start.max(0) as usize, (end - start).max(1) as usize))
                })
            });
            // The cursor-line band is not focus-gated: a terminal left in Normal
            // mode keeps showing where its cursor is, so switching panes (and
            // back) doesn't lose your place. Only the cursor block itself is
            // drawn for the focused terminal alone.
            let cursor_line_row = cursor_line_row(pane_mode, tabs.nav_cursor(id));
            // The line the eye goes to: the traversal cursor's while one is
            // up, else the shell's cursor's, when it is scrolled into view.
            let grid = terminal.grid();
            let looked_at = tabs
                .nav_cursor(id)
                .map(|(row, _)| row)
                .unwrap_or(grid.cursor().0 + grid.scroll_offset());
            let line_flash = flash_strength(flash, *id)
                .filter(|_| looked_at < grid.rows())
                .map(|strength| (looked_at, strength));
            let config_shape = match pane_mode {
                Mode::Insert => config.cursor.insert,
                Mode::Normal => config.cursor.normal,
                Mode::Visual => config.cursor.visual,
            };
            let cursor_shape = effective_cursor_shape(
                !terminal.is_at_prompt(),
                terminal.grid().reported_cursor_shape(),
                config_shape,
            );
            let hovered_link = if hovered_pane == Some(*id) {
                hovered_url
                    .map(|url| terminal.grid().find_link_id(url))
                    .unwrap_or(0)
            } else {
                0
            };
            // Text being composed in an input method sits where the cursor
            // is, and stands in for it until it is committed.
            let pane_preedit = preedit.filter(|_| *id == focused);
            let PaneCursor {
                unfocused: cursor_unfocused,
                visible: cursor_visible,
            } = pane_cursor(
                PaneFocus {
                    pane: is_active,
                    window: window_focused,
                },
                !terminal.grid().cursor_visible() || pane_preedit.is_some(),
                &config.cursor,
                blink_phase,
            );
            views.push(PaneView {
                overlay: pane_preedit,
                blink_visible: text_blink_visible,
                bracket_colors: &overlays.bracket_colors[i],
                block_band,
                cursor_shape,
                cursor_unfocused,
                cursor_visible,
                dim: !is_active && config.dim_inactive,
                focused: is_active,
                grid: terminal.grid(),
                hovered_link,
                labels,
                find_labels: if *id == focused {
                    &overlays.find_labels
                } else {
                    &[]
                },
                nav_cursor,
                cursor_line_row,
                line_flash,
                // The Normal-mode cursor blinks on the same timer as the
                // shell's, so it's as easy to spot while navigating, but
                // holds steady, in its unfocused form, while the window
                // lacks focus.
                nav_cursor_visible: cursor_unfocused || !config.cursor.blink || blink_phase,
                rect: App::layout_rect_to_pane(*rect),
                scroll_offset: terminal.grid().scroll_offset(),
                scrollback_len: terminal.grid().scrollback_len(),
                search_matches: &overlays.search_matches[i],
                search_current: &overlays.search_current[i],
                sentence_spans: &overlays.sentence_spans[i],
                selection: sel_tuple,
                selection_block: sel_block,
                url_underline: config.url_underline,
            });
        }
    }

    views
}

/// Build the view for a tool tab.
///
/// A tool has no terminal caret of its own, so the cursor is parked out of
/// bounds (suppressing it) and the cursor line marks where the tool's own
/// selection sits. A text selection over the tool is drawn here the same way a
/// terminal draws one: the rows it names are the tool's, so it has to be
/// resolved against the grid the tool painted.
/// Where a pane's caret is drawn and how it reads: the cell it is on, and
/// whether what is happening there is typing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PaneCaret {
    pub(crate) col: usize,
    pub(crate) insert: bool,
    pub(crate) row: usize,
}

impl PaneCaret {
    /// A caret on `(row, col)` that is not typing.
    fn at(row: usize, col: usize) -> Self {
        Self {
            col,
            insert: false,
            row,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn tool_pane_view<'a>(
    paint: &'a ToolPaint,
    grid: &'a Grid,
    rect: Rect,
    focused: bool,
    window_focused: bool,
    config: &crate::config::Config,
    blink_phase: bool,
    selection: Option<&'a super::Selection>,
    text_cursor: Option<PaneCaret>,
    search_matches: &'a [(usize, usize)],
    search_current: &'a [(usize, usize)],
    flash: Option<f32>,
) -> PaneView<'a> {
    let (sel_tuple, sel_block) = match selection {
        Some(s) => (
            Some((s.start_row, s.start_col, s.end_row, s.end_col)),
            s.block,
        ),
        None => (None, false),
    };
    // A caret in a tool that does not have the keyboard, because another pane
    // does or because the window lost focus, is drawn hollow.
    let caret_unfocused = text_cursor.is_some() && !(focused && window_focused);
    PaneView {
        overlay: None,
        // Tools draw no program output, so nothing in them blinks.
        blink_visible: true,
        bracket_colors: &[],
        block_band: None,
        // A tool's caret is a block over the cell it is on, as the traversal
        // cursor is everywhere else; a tool taking typed text takes the shape
        // the terminal's own cursor has while typing, so the two read apart at
        // a glance. Without the keyboard it is the block's hollow outline,
        // whatever it was, since that is the one unfocused form that cannot be
        // mistaken for an insert cursor.
        cursor_shape: match text_cursor.map(|caret| caret.insert) {
            Some(true) if !caret_unfocused => config.cursor.insert,
            _ => CursorShape::Block,
        },
        cursor_unfocused: caret_unfocused,
        cursor_visible: false,
        dim: !focused && config.dim_inactive,
        focused,
        grid,
        hovered_link: 0,
        labels: None,
        find_labels: &[],
        // Out of bounds when there is no text cursor, which suppresses it: a
        // tool has no caret of its own, so the cursor line is the only mark of
        // where the tool's own selection sits.
        nav_cursor: Some(
            text_cursor
                .map(|caret| (caret.row, caret.col))
                .unwrap_or((grid.rows(), grid.cols())),
        ),
        cursor_line_row: paint.cursor_line,
        // The caret's line while there is one, else the line the tool marks.
        line_flash: flash.and_then(|strength| {
            let row = text_cursor.map(|caret| caret.row).or(paint.cursor_line)?;
            Some((row, strength))
        }),
        // The caret blinks with the global `cursor.blink` setting, on the same
        // timer as a terminal's, and holds steady when it is drawn hollow.
        nav_cursor_visible: text_cursor.is_some()
            && (caret_unfocused || !config.cursor.blink || blink_phase),
        rect: App::layout_rect_to_pane(rect),
        scroll_offset: 0,
        scrollback_len: 0,
        search_matches,
        search_current,
        sentence_spans: &[],
        selection: sel_tuple,
        selection_block: sel_block,
        url_underline: false,
    }
}

/// The renderer's view of the command palette: its filtered items, the query
/// behind them, and the empty-state message matching what it searches over.
///
/// `icons` holds what each icon kind resolved to on earlier frames, so a long
/// list is not walked through the icon tables again every frame.
fn palette_view(
    palette: &Palette,
    match_underline: bool,
    pick: Option<&str>,
    caret_visible: bool,
    icons: &mut HashMap<IconKind, Option<Arc<[u8]>>>,
) -> PaletteView {
    let empty_message = match palette.mode {
        PaletteMode::TabList => "No matching tabs",
        PaletteMode::Files => "Nothing here by that name",
        PaletteMode::History => "No matching history",
        PaletteMode::ToolPick => "Nothing matches",
        PaletteMode::OpenWith => "No tool by that name",
        PaletteMode::Panes => "No matching panes",
        PaletteMode::TabKeys => "No key by that name",
        PaletteMode::Recent => "Nothing open or recent by that name",
        PaletteMode::RecentDirs => "No recent directories",
        PaletteMode::ProjectFiles => "No file in the project by that name",
        PaletteMode::LiveSearch => "Type a pattern (2+ characters); ` @ <file>` filters by path",
        PaletteMode::Swoop => "No matching lines",
        PaletteMode::Jump => "No matching targets",
        PaletteMode::Commands
        | PaletteMode::MuxAttachRemote
        | PaletteMode::MuxKill
        | PaletteMode::MuxNew
        | PaletteMode::MuxSessions => "No matching commands",
    };
    PaletteView {
        align_top: palette.mode == PaletteMode::Jump,
        caret: palette.caret(),
        caret_visible,
        empty_message: empty_message.to_string(),
        // A browser is headed by where it is, a list given a title by that,
        // and every other list by what it is a list of.
        title: match (&palette.title, &palette.dir) {
            (Some(title), _) => title.clone(),
            (None, Some(dir)) => dir.display().to_string(),
            (None, None) => mode_title(&palette.mode, pick),
        },
        items: palette
            .filtered
            .iter()
            .map(|&i| PaletteItem {
                action: palette.entries[i].action.clone(),
                icon: palette.entries[i]
                    .icon
                    .as_ref()
                    .and_then(|kind| palette_icon(icons, kind)),
                label: palette.entries[i].label.clone(),
                match_positions: palette.entries[i].match_positions.clone(),
                shortcut: palette.entries[i].shortcut.clone(),
            })
            .collect(),
        match_underline,
        query: palette.query.clone(),
        selected: palette.selected,
    }
}

/// The SVG for `kind`, from `icons` when an earlier frame resolved it.
fn palette_icon(
    icons: &mut HashMap<IconKind, Option<Arc<[u8]>>>,
    kind: &IconKind,
) -> Option<Arc<[u8]>> {
    if let Some(hit) = icons.get(kind) {
        return hit.clone();
    }
    let svg = crate::icons::shared_svg(kind);
    icons.insert(kind.clone(), svg.clone());
    svg
}

/// What a palette of `mode` is headed by when nothing more particular names
/// it. A popup opened by a key is read after the key is forgotten, so every one
/// says what its rows are and, where it is not plain, what picking one does.
fn mode_title(mode: &PaletteMode, pick: Option<&str>) -> String {
    let title = match mode {
        PaletteMode::Commands => "Commands",
        PaletteMode::Files => "Files",
        PaletteMode::History => "Shell history: newest first, Enter inserts it",
        PaletteMode::Jump => "Jump",
        PaletteMode::MuxAttachRemote => "Mux: attach a remote session",
        PaletteMode::MuxKill => "Mux: kill a background session",
        PaletteMode::MuxNew => "Mux: new session",
        PaletteMode::MuxSessions => "Mux sessions: Enter attaches",
        PaletteMode::OpenWith => "Open with",
        PaletteMode::Panes => "Panes: a tab's number goes straight to it",
        PaletteMode::TabKeys => "Keys",
        PaletteMode::Recent => "Recent: open tabs, then files, then directories",
        PaletteMode::RecentDirs => "Recent directories: Enter changes to one",
        PaletteMode::ProjectFiles => "Project files",
        PaletteMode::LiveSearch => "Search",
        PaletteMode::Swoop => "Swoop: lines of this pane",
        PaletteMode::TabList => TAB_LIST_TITLE,
        // A tool's question names its own list.
        PaletteMode::ToolPick => pick.unwrap_or("Pick one"),
    };
    title.to_string()
}

/// The which-key popup, once a pending prefix has been held long enough to be
/// worth explaining. `None` until then, or when the prefix has no hint.
fn which_key_view(
    pending: &crate::model::input::PendingPrefix,
    since: Option<std::time::Instant>,
) -> Option<winter_render::WhichKeyView> {
    since.filter(|s| s.elapsed() >= WHICH_KEY_DELAY)?;
    pending
        .hint()
        .map(|(title, items)| winter_render::WhichKeyView {
            items: items
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            title: key_hint_title(title),
        })
}

/// Whether a text caret is drawn this frame. It blinks with the terminal's
/// cursor: the same clock, shown again on every key press, steady when
/// blinking is off. In a window without the keyboard it is steady too, as the
/// blink clock is paused there and could have stopped on the dark phase.
fn is_caret_lit(blink: bool, window_focused: bool, blink_phase: bool) -> bool {
    !blink || !window_focused || blink_phase
}

/// The key-hint popup's title for a pending `prefix`, worded as
/// magic-vscode's editor, terminal, and webview popups word it, so the two
/// apps read as one feature.
fn key_hint_title(prefix: &str) -> String {
    format!("Key hints: ({prefix}) was pressed, wait for the next key stroke...")
}

/// Which of a pane and its window have the keyboard.
#[derive(Clone, Copy)]
struct PaneFocus {
    pane: bool,
    window: bool,
}

/// How a terminal pane's cursor is drawn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PaneCursor {
    /// Drawn as a hollow outline, whatever its shape: the pane is not
    /// receiving keystrokes.
    unfocused: bool,
    /// Drawn at all, this frame.
    visible: bool,
}

/// How to draw a terminal pane's cursor. A pane not receiving keystrokes,
/// because another pane has them or the window does not, shows a hollow
/// cursor, steady (blinking would fight that signal by hiding it half the
/// time), unless `hide-in-inactive` hides it in a pane that is not focused.
/// `hidden` is everything that takes the cursor away outright: an overlay
/// with the keyboard, a program hiding it (DECTCEM), text being composed.
fn pane_cursor(
    focus: PaneFocus,
    hidden: bool,
    cursor: &crate::config::CursorConfig,
    blink_phase: bool,
) -> PaneCursor {
    let unfocused = !focus.pane || !focus.window;
    let visible = if hidden {
        false
    } else if !focus.pane {
        !cursor.hide_in_inactive
    } else if unfocused {
        true
    } else {
        !cursor.blink || blink_phase
    };
    PaneCursor { unfocused, visible }
}

// ========================================================================
// App: rendering
// ========================================================================

/// A search being typed: what it searches, which way, what has been typed,
/// and where the caret is in it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FindTarget {
    pub(crate) caret: usize,
    /// `/` down the text, `?` back up it.
    pub(crate) label: String,
    pub(crate) query: String,
    /// The tab being searched, whose pane the popup hangs from.
    pub(crate) tab: TabId,
}

impl App {
    /// The search being typed, for the find popup at the top-right of the
    /// pane it searches: the terminal's `/` line, or a tool's search
    /// question. How far it has got stays in the status bar; every other
    /// question is asked in a dialog (see [`App::input_view`]).
    pub(crate) fn find_target(&self) -> Option<FindTarget> {
        if self.pending == PendingPrefix::SearchInput {
            let label = match self.search.reverse {
                true => "?",
                false => "/",
            };
            return Some(FindTarget {
                caret: self.search.caret(),
                label: label.to_string(),
                query: self.search.query.clone().unwrap_or_default(),
                tab: self.tabs.layout.focused(),
            });
        }
        let prompt = self.tool_prompt()?;
        prompt.typed_search()?;
        Some(FindTarget {
            caret: prompt.field.col(),
            label: prompt.request.label.clone(),
            query: prompt.field.text().to_string(),
            tab: prompt.tab,
        })
    }

    /// The cells a search lights up in tab `id`: every match, then the one
    /// the search is on, which is drawn in a color of its own. The query is
    /// what a search question has had typed so far while one is open, and
    /// the live search otherwise, so a tool's matches stay lit after `Enter`
    /// exactly as a terminal's do.
    fn search_highlight_cells(&self, id: TabId, focused: TabId) -> (Cells, Cells) {
        let typed = self
            .tool_prompt()
            .and_then(|prompt| prompt.typed_search())
            .map(|(text, _)| text);
        let Some(query) = typed
            .or(self.search.query.as_deref())
            .filter(|query| !query.is_empty())
        else {
            return Default::default();
        };
        if let Some(terminal) = self.open_tabs.terminal(&id) {
            let grid = terminal.grid();
            // The focused match's position is absolute, so it only contributes
            // cells while the row it sits on is actually on screen.
            let top = grid.to_absolute_row(0);
            let current = match self.search.current {
                Some((tab, (abs_row, col)))
                    if tab == id && (top..top + grid.rows()).contains(&abs_row) =>
                {
                    let len = query.chars().count().max(1);
                    (0..len).map(|k| (abs_row - top, col + k)).collect()
                }
                _ => Vec::new(),
            };
            return (visible_match_cells(grid, query), current);
        }
        let Some(slot) = self.open_tabs.tool(&id) else {
            return Default::default();
        };
        let Some(grid) = slot.painted.as_ref() else {
            return Default::default();
        };
        let spans = painted_match_spans(grid, query);
        // A tool's search moves its cursor, so the match the cursor is on is
        // the current one: the one under the caret where the tool has one,
        // else the one on the cursor's row nearest it, so a caret a column
        // off its match (a row's padding the tool does not count) still
        // marks one. Only the focused tab has one, as only one terminal does.
        let caret = slot.tool.caret().map(|caret| caret.col);
        let current = slot
            .cursor_line
            .filter(|_| id == focused)
            .and_then(|row| current_span(&spans, row, caret));
        let cells =
            |&(row, start, end): &(usize, usize, usize)| (start..end).map(move |col| (row, col));
        (
            spans.iter().flat_map(cells).collect(),
            current.into_iter().flat_map(cells).collect(),
        )
    }

    pub(crate) fn render_frame(&mut self) {
        // What is about to be drawn is the current state, so the pending-repaint
        // flag is spent here. Clearing it at the top rather than the bottom keeps
        // a change made while this frame is being built from being swallowed: it
        // sets the flag again and earns its own frame.
        self.dirty = false;
        self.last_frame_at = Some(Instant::now());
        // The settings page is a full-window modal; it replaces the panes, tabbar,
        // status bar, and block tiles entirely until it closes.
        if self.settings_page().is_some() {
            self.render_settings_frame();
            return;
        }

        let notice = self.status_notice();
        // A live `/` search forces the status bar on for its duration even if
        // it's configured hidden: it's the only place search feedback (query
        // text, match position) is shown, so there'd otherwise be nowhere to
        // put it. Reverts to the configured visibility as soon as the search
        // ends (`search.query` back to `None`). Shared with `viewport_rect`/
        // `resize_all_panes` via `status_bar_visible` so pane geometry and the
        // PTY's row count always agree with what's drawn here.
        let status_enabled = self.status_bar_visible();
        // Asked for only once there is a bar to show the answers on.
        self.sync_quota_monitor(status_enabled);
        // Notices float as cards in the top-right corner, or sit in the
        // status bar after its mode; never both (see `toast_cards`).
        let toasts = self.toast_cards();
        // Built before the renderer is borrowed, since it reads tab/menu state.
        let tabbar = self.build_top_tabbar();
        // Read this frame's geometry and theme through a shared borrow, so the
        // per-pane data below can be built with ordinary `&self` methods. The
        // renderer is only taken mutably once that data is in hand.
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        let (full_cols, full_rows) = renderer.grid_size();
        let (cw, ch) = renderer.cell_size();
        let viewport = self.content_band(cw, ch, full_cols, full_rows, status_enabled);
        let rects = self.pane_rects_in(viewport);
        let focused = self.tabs.layout.focused();
        let strip_frames = self.strip_frames(viewport, (cw, ch));
        let mode = self.open_tabs.mode(&focused);
        // Tools paint themselves into a grid of their own, so the renderer
        // draws a tool tab through exactly the path a terminal tab takes. A
        // tool is handed its row count first, so it can window a listing longer
        // than the pane around its own cursor.
        let mut tool_paints: Vec<ToolPaint> = Vec::new();
        for (id, rect) in &rects {
            let (cols, rows) = renderer.grid_size_for(App::layout_rect_to_pane(*rect));
            let Some(slot) = self.open_tabs.tool_mut(id) else {
                continue;
            };
            slot.tool.set_gutter_side(self.config.gutter_side);
            let content = slot.tool.content(rows, cols, self.tool_wrap);
            // Kept on the slot rather than in the paint, so that selecting over
            // the pane after this frame reads the rows the user can see.
            slot.cursor_line = content.cursor_line;
            slot.painted = Some(build_frame_grid(
                &content,
                renderer.theme(),
                cols,
                rows,
                self.tool_wrap,
            ));
            // A row that wrapped pushes every row after it down the pane, and
            // the icons beside them with it.
            let starts = screen_row_starts(&content, cols, self.tool_wrap);
            let icons = content
                .icons
                .into_iter()
                .filter_map(|icon| {
                    let row = *starts.get(icon.row)?;
                    Some(Icon { row, ..icon })
                })
                .collect();
            tool_paints.push(ToolPaint {
                cursor_line: content.cursor_line,
                icons,
                tab: *id,
            });
        }
        let overlays = self.build_pane_overlays(&rects, renderer.theme());
        let hovered_pane = self.hovered_pane(&rects);
        // A search question being typed is what the bar shows while it is
        // open, over any search already lit.
        let typed = self.tool_prompt().and_then(|prompt| prompt.typed_search());
        let search = typed
            .map(|(query, reverse)| (query.to_string(), reverse))
            .or_else(|| {
                let query = self.search.query.clone()?;
                Some((query, self.search.reverse))
            })
            .map(|(query, reverse)| StatusSearch {
                query,
                match_index: self.search.match_index,
                match_total: self.search.match_total,
                reverse,
            });
        let status =
            self.build_status_bar(mode, renderer.theme(), search, notice, &focused);
        let status = status_enabled.then_some(&status);
        let mut palette_icons = std::mem::take(&mut self.palette_icons);
        let palette_view = self.palette().map(|p| {
            palette_view(
                p,
                self.config.palette_match_underline,
                self.tool_pick().map(|pick| pick.label.as_str()),
                is_caret_lit(
                    self.config.cursor.blink,
                    self.window_focused,
                    self.blink_phase,
                ),
                &mut palette_icons,
            )
        });
        if palette_view.is_none() {
            palette_icons.clear();
        }
        let find_view = self.find_target().and_then(|target| {
            let (_, rect) = rects.iter().find(|(id, _)| *id == target.tab)?;
            let pane = App::layout_rect_to_pane(*rect);
            Some(FindView {
                caret: target.caret,
                caret_visible: is_caret_lit(
                    self.config.cursor.blink,
                    self.window_focused,
                    self.blink_phase,
                ),
                label: target.label,
                pane: (pane.x, pane.y, pane.width),
                query: target.query,
            })
        });
        let input_view = self.input_view().map(|view| InputView {
            caret_visible: is_caret_lit(
                self.config.cursor.blink,
                self.window_focused,
                self.blink_phase,
            ),
            ..view
        });
        self.palette_icons = palette_icons;
        // A tool part-way through a multi-key command says so through the
        // same card the terminal's own prefixes use, and says it at once: the
        // menus are how the tools are learned, where the terminal's hints
        // wait to see whether the typing stalls.
        // A key sequence waits the same delay as the terminal's prefixes.
        let sequence_view = self
            .sequence_hint(focused)
            .filter(|_| {
                self.pending_since
                    .is_some_and(|s| s.elapsed() >= WHICH_KEY_DELAY)
            })
            .map(|(keys, items)| winter_render::WhichKeyView {
                items,
                title: key_hint_title(&keys),
            });
        let which_key_view = sequence_view
            .or_else(|| {
                self.open_tabs
                    .tool(&focused)
                    .and_then(|slot| slot.tool.hint())
                    .map(|hint| winter_render::WhichKeyView {
                        items: hint.items,
                        title: hint.title,
                    })
            })
            .or_else(|| which_key_view(&self.pending, self.pending_since));
        // Gathered before the renderer is held mutably: the bands read app
        // state the views below borrow alongside it.
        let focused_bands = self.block_bands(focused);
        // Rasterized before the renderer is taken mutably below, since filling
        // the icon cache needs it mutably too.
        let icon_placements = self.tool_icon_placements(&tool_paints, &rects, (cw, ch));
        let shown_tools: Vec<TabId> = tool_paints.iter().map(|paint| paint.tab).collect();
        let picture_placements = self.tool_canvas_placements(&shown_tools, &rects, ch);
        // Every tool tab shows a block cursor, not only one that `v` has
        // started a text cursor in. The row band says which entry is current;
        // the cursor says which cell a selection would start from, and without
        // it a tool looks like it has no cursor at all.
        let tool_cursors: std::collections::HashMap<TabId, PaneCaret> = self
            .open_tabs
            .tools()
            .filter_map(|(id, slot)| {
                if let Some(cursor) = self.tool_cursor.as_ref().filter(|c| c.tab == *id) {
                    return Some((*id, PaneCaret::at(cursor.row, cursor.col)));
                }
                let row = slot.cursor_line?;
                // A tool that edits text says which cell it is on; one whose
                // cursor is a whole row leaves it to the row's first painted
                // character, which is where the eye starts reading it.
                if let Some(caret) = slot.tool.caret() {
                    return Some((
                        *id,
                        PaneCaret {
                            col: caret.col,
                            insert: caret.insert,
                            row,
                        },
                    ));
                }
                let grid = slot.painted.as_ref()?;
                Some((
                    *id,
                    PaneCaret::at(row, super::tool_cursor::first_non_blank(grid, row)),
                ))
            })
            .collect();

        let preedit = self.preedit_overlay(focused);
        let active = self.active_pane();

        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        refresh_block_textures(&mut self.image_blocks, &rects, ch, renderer);
        let mut placements = image_placements(
            &self.image_blocks,
            &self.open_tabs,
            &rects,
            ch,
            &tool_paints,
        );
        placements.extend(picture_placements);
        placements.extend(icon_placements);
        for (i, frame) in strip_frames.iter().enumerate() {
            placements.push(renderer.rasterize_pane_strip(
                PANE_STRIP_TEXTURE_BASE + i as u64,
                &frame.tabs,
                &frame.layout,
                frame.rect.width as u32,
                frame.rect.height as u32,
                frame.hover,
                frame.edges,
                frame.drop_target,
                frame.rect.x,
                frame.rect.y,
            ));
        }
        let views = build_pane_views(PaneViewInput {
            active,
            blink_phase: self.blink_phase,
            text_blink_visible: self.text_blink.is_visible(),
            preedit: preedit.as_ref(),
            config: &self.config,
            flash: self
                .focus_flash
                .and_then(|flash| Some((flash.tab, flash.strength(Instant::now())?))),
            focused,
            focused_bands: &focused_bands,
            hovered_pane,
            hovered_url: self.pointer.hovered_url.as_deref(),
            overlays: &overlays,
            tool_cursors: &tool_cursors,
            tool_paints: &tool_paints,
            rects: &rects,
            selection: self.selection.span.as_ref(),
            tabs: &self.open_tabs,
            window_focused: self.window_focused,
        });
        renderer.render(
            &views,
            status,
            Some(&tabbar),
            &placements,
            palette_view.as_ref(),
            input_view.as_ref(),
            find_view.as_ref(),
            &toasts,
            which_key_view.as_ref(),
        );

        self.update_ime_anchor(focused);
    }

    // --------------------------------------------------------------------
    // Frame data
    // --------------------------------------------------------------------

    /// The band of the window the panes are laid out in.
    fn content_band(
        &self,
        cw: f32,
        ch: f32,
        full_cols: usize,
        full_rows: usize,
        status_enabled: bool,
    ) -> Rect {
        // Panes sit below the top tabbar (tabbar/menubar) and, when enabled,
        // above the status bar; the grid is centered in whatever space remains
        // below the tabbar, whether or not the status bar eats into it.
        let top_rows = winter_render::tabbar_rows(self.config.menu_style);
        let window_size = self.window.as_ref().map(|w| w.inner_size());
        let h = window_size
            .map(|s| s.height as f32)
            .unwrap_or(full_rows as f32 * ch);
        let w = window_size
            .map(|s| s.width as f32)
            .unwrap_or(full_cols as f32 * cw);

        let top_h_on_screen = if self.config.menu_style == winter_render::MenuStyle::Modern {
            winter_render::modern_titlebar_height_px(ch)
        } else {
            top_rows as f32 * ch
        };
        let status_h = if status_enabled {
            winter_render::STATUS_BAR_HEIGHT * ch
        } else {
            0.0
        };

        // Every pixel between the title bar and the status bar belongs to the
        // pane band (see `viewport_rect`).
        Rect::new(
            0.0,
            top_h_on_screen,
            w,
            (h - top_h_on_screen - status_h).max(1.0),
        )
    }

    /// Precompute the per-pane overlay data the pane views borrow as slices:
    /// labels, search highlights, sentence tints, and bracket colors. Built up
    /// front, in pane order, so the view loop can hand out slices of it.
    fn build_pane_overlays(&self, rects: &[(TabId, Rect)], theme: &Theme) -> PaneOverlays {
        let qs_labels: Vec<(usize, usize, char)> = self
            .quick_select
            .as_ref()
            .map(|labels| labels.iter().map(|ql| (ql.row, ql.col, ql.label)).collect())
            .unwrap_or_default();
        // The `f`/`t` jump overlay's labels, for the focused pane only (that's
        // where the cursor being moved lives).
        let find_label_data: Vec<(usize, usize, char)> = self
            .find_labels
            .as_ref()
            .map(|labels| labels.iter().map(|fl| (fl.row, fl.col, fl.label)).collect())
            .unwrap_or_default();

        // Precompute search match cell positions per pane so PaneView can borrow
        // them as slices. Built before the view loop to satisfy the borrow checker.
        let focused = self.tabs.layout.focused();
        let (search_match_data, search_current_data): (Vec<_>, Vec<_>) = rects
            .iter()
            .map(|(id, _)| self.search_highlight_cells(*id, focused))
            .unzip();

        let sentence_span_data = if self.sentence_highlight() {
            rects
                .iter()
                .map(|(id, _)| {
                    if let Some(terminal) = self.open_tabs.terminal(id) {
                        super::navigation::reading::sentence_spans(terminal.grid())
                            .into_iter()
                            .map(|s| (s.row, s.col_start, s.col_end, s.tone))
                            .collect()
                    } else {
                        vec![]
                    }
                })
                .collect()
        } else {
            vec![vec![]; rects.len()]
        };

        let bracket_color_data = if self.rainbow_parens() {
            rects
                .iter()
                .map(|(id, _)| {
                    if let Some(terminal) = self.open_tabs.terminal(id) {
                        let marks = super::navigation::reading::bracket_marks(terminal.grid());
                        super::navigation::reading::resolve_bracket_colors(&marks, theme)
                    } else {
                        vec![]
                    }
                })
                .collect()
        } else {
            vec![vec![]; rects.len()]
        };

        PaneOverlays {
            bracket_colors: bracket_color_data,
            find_labels: find_label_data,
            quick_select: qs_labels,
            search_current: search_current_data,
            search_matches: search_match_data,
            sentence_spans: sentence_span_data,
        }
    }

    /// The GPU texture for one icon at one pixel box, rasterizing it the first
    /// time it is asked for.
    ///
    /// The whole cache is dropped when `full`, the box a full-size icon fills,
    /// changes, which happens only on a font-size or DPI change: every texture
    /// in it was rasterized for the old box, and an icon scaled from the wrong
    /// size is exactly the blur that rasterizing per size exists to avoid.
    fn icon_texture(
        &mut self,
        name: &str,
        full: (u32, u32),
        box_w: u32,
        box_h: u32,
    ) -> Option<u64> {
        if self.icon_texture_box != full {
            for id in self.icon_textures.values().flatten() {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.free_image(*id);
                }
            }
            self.icon_textures.clear();
            self.icon_texture_box = full;
        }
        let key = (name.to_string(), box_w, box_h);
        if let Some(cached) = self.icon_textures.get(&key) {
            return *cached;
        }
        let id = self.next_image_id;
        let uploaded = crate::icons::named_svg(name).is_some_and(|svg| {
            self.renderer
                .as_mut()
                .is_some_and(|renderer| renderer.upload_svg_in_box(id, &svg, box_w, box_h))
        });
        let slot = uploaded.then(|| {
            self.next_image_id += 1;
            id
        });
        self.icon_textures.insert(key, slot);
        slot
    }

    /// Quads for every icon the tools want drawn this frame.
    ///
    /// Recomputed from scratch each frame rather than anchored, because a tool
    /// repaints its rows from its own model every frame anyway: there is no
    /// scrollback row for an icon to drift away from.
    fn tool_icon_placements(
        &mut self,
        paints: &[ToolPaint],
        rects: &[(TabId, Rect)],
        cell: (f32, f32),
    ) -> Vec<ImagePlacement> {
        let (cw, ch) = cell;
        let focused = self.layout().focused();
        let dim_inactive = self.config.dim_inactive;
        let slot_w = cw * Icon::WIDTH as f32;
        let full = (slot_w.round().max(1.0) as u32, ch.round().max(1.0) as u32);
        let mut placements = Vec::new();
        for paint in paints {
            let Some((_, rect)) = rects.iter().find(|(id, _)| *id == paint.tab) else {
                continue;
            };
            // Rows sit in the grid area, centred in the pane.
            let pane_rect = App::layout_rect_to_pane(*rect).grid_area(ch);
            // Faded with the rest of an inactive pane: drawn over the pane's
            // background, an alpha of one less the dim factor lands each pixel
            // where the pane's own colors are blended to.
            let alpha = match dim_inactive && paint.tab != focused {
                true => 1.0 - winter_render::DIM_FACTOR,
                false => 1.0,
            };
            for icon in &paint.icons {
                let Some(name) = crate::icons::name_for(&icon.kind) else {
                    continue;
                };
                // A smaller icon is rasterized at its own size, not shrunk
                // from the full one, and centred in the slot it leaves.
                let scale = icon.kind.scale();
                let box_w = (slot_w * scale).round().max(1.0) as u32;
                let box_h = (ch * scale).round().max(1.0) as u32;
                let Some(id) = self.icon_texture(&name, full, box_w, box_h) else {
                    continue;
                };
                let slot_x = pane_rect.x + icon.col as f32 * cw;
                let slot_y = pane_rect.y + icon.row as f32 * ch;
                let x = slot_x + ((slot_w - box_w as f32) / 2.0).round();
                let y = slot_y + ((ch - box_h as f32) / 2.0).round();
                // Clipped out rather than drawn past the pane's last row.
                if slot_y + ch > pane_rect.y + pane_rect.height
                    || slot_x + cw > pane_rect.x + pane_rect.width
                {
                    continue;
                }
                placements.push(ImagePlacement {
                    alpha,
                    height: box_h as f32,
                    id,
                    u_max: 1.0,
                    u_min: 0.0,
                    v_max: 1.0,
                    v_min: 0.0,
                    width: box_w as f32,
                    x,
                    y,
                });
            }
        }
        placements
    }

    /// Which pane the mouse pointer is over, if any.
    fn hovered_pane(&self, rects: &[(TabId, Rect)]) -> Option<TabId> {
        let (cx, cy) = self.pointer.cursor_pos;
        let hovered_pane: Option<TabId> = rects
            .iter()
            .find(|(_, r)| {
                let pr = Self::layout_rect_to_pane(*r);
                cx >= pr.x && cx < pr.x + pr.width && cy >= pr.y && cy < pr.y + pr.height
            })
            .map(|(id, _)| *id);

        hovered_pane
    }

    /// Draw the settings page as a single full-window grid: no tabbar, status
    /// bar, or panes, just the modal overlay.
    fn render_settings_frame(&mut self) {
        // The field rather than the accessor, so the renderer beside it can
        // be held mutably while the page is read.
        let Some(Overlay::Settings(tool)) = &self.overlay else {
            return;
        };
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let (cols, rows) = renderer.grid_size();
        let (cw, ch) = renderer.cell_size();
        let grid = build_settings_grid(tool, renderer.theme(), cols, rows);
        let view = PaneView {
            overlay: None,
            blink_visible: true,
            bracket_colors: &[],
            block_band: None,
            cursor_shape: CursorShape::Block,
            cursor_unfocused: false,
            cursor_visible: true,
            dim: false,
            focused: true,
            grid: &grid,
            hovered_link: 0,
            labels: None,
            find_labels: &[],
            // An out-of-bounds nav cursor suppresses the terminal cursor (the
            // settings grid has no caret of its own) without drawing a nav block.
            nav_cursor: Some((rows, cols)),
            nav_cursor_visible: false,
            cursor_line_row: None,
            line_flash: None,
            rect: PaneRect {
                height: rows as f32 * ch,
                width: cols as f32 * cw,
                x: 0.0,
                y: 0.0,
            },
            scroll_offset: 0,
            scrollback_len: 0,
            search_matches: &[],
            search_current: &[],
            sentence_spans: &[],
            selection: None,
            selection_block: false,
            url_underline: false,
        };
        renderer.render(
            std::slice::from_ref(&view),
            None,
            None,
            &[],
            None,
            None,
            None,
            &[],
            None,
        );
    }
}

// ========================================================================
// Data Structures
// ========================================================================

/// Viewport cells, as `(row, col)`, that one overlay paints over a pane.
type Cells = Vec<(usize, usize)>;

// ========================================================================
// Functions
// ========================================================================

/// How strongly tab `tab`'s line is lit by `flash`, if it is the tab flashing.
fn flash_strength(flash: Option<(TabId, f32)>, tab: TabId) -> Option<f32> {
    flash
        .filter(|(flashing, _)| *flashing == tab)
        .map(|(_, strength)| strength)
}

/// The cursor shape to render for a pane: the active program's own DECSCUSR
/// report when it can be trusted, otherwise the host's configured per-mode
/// shape.
///
/// A DECSCUSR report is trusted only inside a full-screen app (`is_alt_screen`,
/// e.g. vim/nvim's block-in-normal / bar-in-insert signalling), where the
/// program owns the cursor for as long as it holds the alternate screen. At
/// the shell prompt (`is_alt_screen` false) the configured per-mode shape is
/// always authoritative: shells (notably zsh) re-emit a default Block cursor
/// (`\e[2 q`) when they redraw on a resize: e.g. after a pane is split or
/// closed, which would otherwise leak a stale Block into Insert mode and
/// clobber the user's configured Bar.
///
/// Trusting the report as soon as the alt screen is entered (rather than
/// waiting for a Bar to prove the program signals modality) matters because a
/// full-screen app's very first frame is already meaningful: vim/nvim opens
/// straight into Normal mode and reports Block immediately, before the user
/// ever presses `i`, waiting for a Bar sighting would show the host's Insert
/// shape instead for that entire opening stretch.
fn effective_cursor_shape(
    is_alt_screen: bool,
    reported: Option<CursorShape>,
    config_shape: CursorShape,
) -> CursorShape {
    if is_alt_screen {
        reported.unwrap_or(config_shape)
    } else {
        config_shape
    }
}

/// The match among `spans` (`(row, start, end)` in painted cells) a tool's
/// cursor is on, on cursor row `row`: the one under the caret at column
/// `caret`, else the one nearest it, so a caret a column off its match (a
/// row's padding the tool does not count) still marks one; with no caret,
/// the row's first.
fn current_span(
    spans: &[(usize, usize, usize)],
    row: usize,
    caret: Option<usize>,
) -> Option<&(usize, usize, usize)> {
    let on_row = spans.iter().filter(|span| span.0 == row);
    match caret {
        Some(col) => on_row
            .clone()
            .find(|&&(_, start, end)| (start..end).contains(&col))
            .or_else(|| on_row.min_by_key(|&&(_, start, _)| start.abs_diff(col))),
        None => on_row.min_by_key(|&&(_, start, _)| start),
    }
}

/// The row to band as the cursor line for a pane: wherever its traversal cursor
/// sits while it's being navigated.
///
/// Deliberately takes no notion of focus: a pane left in Normal or Visual mode
/// keeps its band while another pane is focused, so switching panes and coming
/// back doesn't hide where the cursor was. The cursor block itself stays
/// focus-only (see `PaneView::nav_cursor`).
fn cursor_line_row(mode: Mode, nav_cursor: Option<(usize, usize)>) -> Option<usize> {
    matches!(mode, Mode::Normal | Mode::Visual)
        .then(|| nav_cursor.map(|(row, _)| row))
        .flatten()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tool;

    #[test]
    fn test_a_lit_search_marks_a_tools_matches_and_the_one_its_cursor_is_on() {
        let dir = std::env::temp_dir().join(format!("winter-lit-tool-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let mut app = App::new();
        let tab = app.show_tool(
            super::super::tool::ToolKind::Dir,
            Box::new(crate::tools::dir::DirTool::new(dir.clone())),
        );
        let mut grid = Grid::new(10, 3);
        for (row, line) in ["a mod", "mod mod", "x"].iter().enumerate() {
            grid.move_to(row, 0);
            line.chars().for_each(|ch| grid.print(ch));
        }
        let slot = app.open_tabs.tool_mut(&tab).expect("the tool");
        slot.painted = Some(grid);
        slot.cursor_line = Some(1);
        app.search.query = Some("mod".to_string());

        let (matches, current) = app.search_highlight_cells(tab, tab);
        let row = |row, cols: std::ops::Range<usize>| cols.map(move |col| (row, col));
        let every: Vec<_> = row(0, 2..5)
            .chain(row(1, 0..3))
            .chain(row(1, 4..7))
            .collect();
        assert_eq!(matches, every);
        assert_eq!(
            current,
            row(1, 0..3).collect::<Vec<_>>(),
            "first on the cursor's row"
        );
        let (_, unfocused) = app.search_highlight_cells(tab, TabId(u64::MAX));
        assert!(
            unfocused.is_empty(),
            "only the focused tab has a current match"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_the_current_match_is_under_the_caret_or_else_nearest_it() {
        let spans = [(0, 2, 5), (1, 0, 3), (1, 8, 11)];
        assert_eq!(current_span(&spans, 1, Some(9)), Some(&(1, 8, 11)), "under it");
        assert_eq!(current_span(&spans, 1, Some(6)), Some(&(1, 8, 11)), "nearest");
        assert_eq!(current_span(&spans, 1, None), Some(&(1, 0, 3)), "the first");
        assert_eq!(current_span(&spans, 2, Some(0)), None, "none on the row");
    }

    #[test]
    fn test_a_tool_caret_without_the_keyboard_is_a_hollow_block() {
        let grid = Grid::new(10, 3);
        let paint = ToolPaint {
            cursor_line: Some(0),
            icons: Vec::new(),
            tab: TabId(1),
        };
        let mut config = crate::config::Config::default();
        config.cursor.insert = CursorShape::Bar;
        let rect = Rect::new(0.0, 0.0, 100.0, 60.0);
        let typing = Some(PaneCaret {
            col: 1,
            insert: true,
            row: 0,
        });
        let view = |focused: bool, window_focused: bool, caret: Option<PaneCaret>| {
            let view = tool_pane_view(
                &paint,
                &grid,
                rect,
                focused,
                window_focused,
                &config,
                true,
                None,
                caret,
                &[],
                &[],
                None,
            );
            (view.cursor_unfocused, view.cursor_shape)
        };

        assert_eq!(
            view(true, true, typing),
            (false, CursorShape::Bar),
            "has the keyboard"
        );
        assert_eq!(
            view(false, true, typing),
            (true, CursorShape::Block),
            "another pane has it"
        );
        assert_eq!(
            view(true, false, typing),
            (true, CursorShape::Block),
            "the window lost it"
        );
        assert!(
            !view(false, true, None).0,
            "a page with no caret draws none"
        );
    }

    #[test]
    fn test_a_tool_caret_blinks_with_the_global_setting() {
        let grid = Grid::new(10, 3);
        let paint = ToolPaint {
            cursor_line: Some(0),
            icons: Vec::new(),
            tab: TabId(1),
        };
        let rect = Rect::new(0.0, 0.0, 100.0, 60.0);
        let caret = Some(PaneCaret::at(0, 1));
        let mut config = crate::config::Config::default();
        let shown = |config: &crate::config::Config, focused, phase, caret| {
            tool_pane_view(
                &paint,
                &grid,
                rect,
                focused,
                true,
                config,
                phase,
                None,
                caret,
                &[],
                &[],
                None,
            )
            .nav_cursor_visible
        };

        assert!(config.cursor.blink, "blinking is the default");
        assert!(shown(&config, true, true, caret), "on phase");
        assert!(!shown(&config, true, false, caret), "off phase");
        assert!(
            shown(&config, false, false, caret),
            "hollow outline holds steady"
        );
        assert!(
            !shown(&config, true, true, None),
            "no caret, nothing to show"
        );

        config.cursor.blink = false;
        assert!(
            shown(&config, true, false, caret),
            "steady when blink is off"
        );
    }

    #[test]
    fn test_cursor_line_row_is_kept_for_unfocused_panes() {
        // The band follows the pane's mode and cursor only, no focus term, so a
        // pane you navigated stays banded after you switch to another pane.
        assert_eq!(cursor_line_row(Mode::Normal, Some((4, 2))), Some(4));
        assert_eq!(cursor_line_row(Mode::Visual, Some((0, 0))), Some(0));
        // Insert (or a pane that was never navigated) shows no band.
        assert_eq!(cursor_line_row(Mode::Insert, Some((4, 2))), None);
        assert_eq!(cursor_line_row(Mode::Normal, None), None);
    }

    #[test]
    fn test_alt_screen_app_reports_shape_from_its_first_frame() {
        // Regression: vim/nvim opens straight into Normal mode and reports
        // Block immediately, before the user ever presses `i`. The shape must
        // come from that first report, not fall back to the host's Insert
        // config shape while waiting for a Bar to appear.
        assert_eq!(
            effective_cursor_shape(true, Some(CursorShape::Block), CursorShape::Bar),
            CursorShape::Block
        );
    }

    #[test]
    fn test_alt_screen_app_with_no_report_yet_falls_back_to_config() {
        assert_eq!(
            effective_cursor_shape(true, None, CursorShape::Bar),
            CursorShape::Bar
        );
    }

    #[test]
    fn test_shell_prompt_always_uses_configured_shape() {
        // Outside the alt screen, a shell's stray DECSCUSR (e.g. zsh
        // re-emitting Block on a resize) must never override the host's
        // configured per-mode shape.
        assert_eq!(
            effective_cursor_shape(false, Some(CursorShape::Block), CursorShape::Bar),
            CursorShape::Bar
        );
        assert_eq!(
            effective_cursor_shape(false, None, CursorShape::Bar),
            CursorShape::Bar
        );
    }

    #[test]
    fn test_a_pending_prefix_is_explained_only_once_it_has_sat_unresolved() {
        let pending = crate::model::input::PendingPrefix::G;
        assert!(which_key_view(&pending, Some(Instant::now())).is_none());
        assert!(which_key_view(&pending, None).is_none(), "nothing pending");
        let view = which_key_view(&pending, Some(Instant::now() - WHICH_KEY_DELAY))
            .expect("shown once the delay is up");
        assert!(view.title.contains("(g)"), "title: {}", view.title);
        assert!(!view.items.is_empty());
    }

    #[test]
    fn test_every_palette_is_headed_by_its_title_its_directory_or_its_kind() {
        // The command palette and the other lists opened by a key used to
        // draw no heading at all, so nothing on screen said what they were.
        let title = |palette: &Palette, pick: Option<&str>| {
            palette_view(palette, false, pick, true, &mut HashMap::new()).title
        };
        let keymap = crate::model::input::Keymap::default();
        assert_eq!(title(&Palette::open(&keymap), None), "Commands");
        assert_eq!(
            title(
                &Palette::open_pick(Vec::new(), Vec::new()),
                Some("Checkout")
            ),
            "Checkout",
            "a tool's question names its own list"
        );

        let browsing = Palette::open_files(std::env::temp_dir());
        assert_eq!(
            title(&browsing, None),
            std::env::temp_dir().display().to_string(),
            "a browser says where it is"
        );
        let jump = Palette::open_jump(Vec::new(), Some("Jump: notes.md".to_string()));
        assert_eq!(title(&jump, None), "Jump: notes.md", "a given title wins");
    }

    #[test]
    fn test_the_palette_caret_stays_lit_in_a_window_without_the_keyboard() {
        // Regression: the blink clock pauses when the window loses focus, and
        // a caret that paused on its dark phase stayed hidden until it came
        // back.
        assert!(is_caret_lit(true, false, false));
        assert!(
            !is_caret_lit(true, true, false),
            "blinking with the keyboard"
        );
        assert!(
            is_caret_lit(false, true, false),
            "steady when blinking is off"
        );
    }

    #[test]
    fn test_an_inactive_pane_shows_a_hollow_steady_cursor() {
        let cursor = crate::config::CursorConfig::default();
        let inactive = PaneFocus {
            pane: false,
            window: true,
        };
        // Hollow, and shown whatever the blink phase: it marks where the
        // pane stands without claiming the keys.
        for blink_phase in [true, false] {
            assert_eq!(
                pane_cursor(inactive, false, &cursor, blink_phase),
                PaneCursor {
                    unfocused: true,
                    visible: true,
                }
            );
        }
        // Unless the user asked for none there.
        let hiding = crate::config::CursorConfig {
            hide_in_inactive: true,
            ..cursor
        };
        assert!(!pane_cursor(inactive, false, &hiding, true).visible);
        // The focused pane keeps its filled, blinking cursor.
        let focused = PaneFocus {
            pane: true,
            window: true,
        };
        assert_eq!(
            pane_cursor(focused, false, &cursor, false),
            PaneCursor {
                unfocused: false,
                visible: false,
            }
        );
    }

    #[test]
    fn test_questions_are_asked_in_the_dialog_and_searches_in_the_find_popup() {
        let mut app = App::new();
        assert!(app.find_target().is_none());
        assert!(app.input_view().is_none());

        let prompt = tool::ActivePrompt {
            field: crate::model::textedit::field::TextField::line("hello"),
            recall: crate::model::textedit::recall::Recall::default(),
            tab: TabId(0),
            request: crate::model::tool::PromptRequest {
                initial: String::new(),
                label: "find".to_string(),
                mode: crate::model::tool::PromptMode::Text,
                tag: "search",
            },
        };
        app.overlay = Some(Overlay::ToolPrompt(prompt));
        assert!(app.find_target().is_none(), "a question is not a search");
        let dialog = app.input_view().expect("a tool's question");
        assert_eq!(dialog.label, "find");
        assert_eq!(dialog.input.as_deref(), Some("hello"));
        assert_eq!(dialog.caret, 5);

        app.overlay = Some(Overlay::ThemeName(
            crate::model::textedit::field::TextField::line("dracula"),
        ));
        assert!(app.find_target().is_none());
        let dialog = app.input_view().expect("the theme name question");
        assert_eq!(dialog.label, "New theme name");
        assert_eq!(dialog.input.as_deref(), Some("dracula"));

        // A tool's search is typed in the find popup, not a dialog.
        app.overlay = Some(Overlay::ToolPrompt(tool::ActivePrompt {
            field: crate::model::textedit::field::TextField::line("needle"),
            recall: crate::model::textedit::recall::Recall::default(),
            tab: TabId(3),
            request: crate::model::tool::PromptRequest {
                initial: String::new(),
                label: "?".to_string(),
                mode: crate::model::tool::PromptMode::Search { reverse: true },
                tag: "search-back",
            },
        }));
        assert!(app.input_view().is_none(), "no dialog for a search");
        let target = app.find_target().expect("the tool's search");
        assert_eq!(
            (target.tab, target.label.as_str(), target.query.as_str(), target.caret),
            (TabId(3), "?", "needle", 6)
        );

        // And so is the terminal's own `/` line.
        app.overlay = None;
        app.pending = PendingPrefix::SearchInput;
        app.search.reverse = false;
        app.search.query = Some("term".to_string());
        let target = app.find_target().expect("forward search");
        assert_eq!(target.label, "/");
        assert_eq!(target.query, "term");
        assert_eq!(target.caret, 4);

        app.search.reverse = true;
        assert_eq!(app.find_target().expect("reverse search").label, "?");
    }
}
