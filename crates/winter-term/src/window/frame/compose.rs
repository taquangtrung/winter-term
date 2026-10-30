//! Frame composition: the panes, the chrome, and the canvas's images, put
//! together for one frame.

use std::time::Instant;

use crate::editors::dir::DirEditor;
use crate::editors::text::TextEditor;
use crate::model::editor::PickPlacement;
use crate::model::input::PendingPrefix;
use crate::model::layout::{Rect, TabId};
use crate::model::mode::Mode;
use crate::model::paint::Icon;
use winter_core::{CursorShape, Grid};
use winter_render::renderer::{GpuRenderer, PaneRect, PaneView};
use winter_render::{
    CellOverlay, CompletionRow, CompletionView, DropListView, FindView, InfoView, ImagePlacement, InputView, PickerView,
    StatusSearch, ToolbarView, WhichKeyView,
};

use super::frame_grid::{build_outlined_grid, screen_row_starts};
use super::inspect::token_inspect_popup;
use super::popups::{key_hint_title, palette_view, which_key_view};
use crate::window::editor::buffers::ScreenSkip;
use crate::window::input::overlay::Overlay;
use crate::window::input::sequence::WHICH_KEY_DELAY;
use crate::window::layout::open_tabs::OpenTabs;
use crate::window::settings::grid::build_settings_grid;
use crate::window::strip::StripFrame;
use crate::window::terminal::blocks::native::{image_placements, refresh_block_textures};
use crate::window::terminal::navigation::search::{painted_match_spans, visible_match_cells};
use crate::window::{App, Selection};

// ========================================================================
// Constants
// ========================================================================

/// Base texture ID for rasterized pane tab strips, counting down from it:
/// below the renderer's chrome IDs, which counting up from here reached once
/// a window had enough panes.
const PANE_STRIP_TEXTURE_BASE: u64 = u64::MAX - 200;

// ========================================================================
// Frame composition helpers
// ========================================================================

/// Each editor on screen painted into a grid of its own, so the renderer draws
/// an editor tab through exactly the path a terminal tab takes. An editor is
/// handed its row count first, so it can window a listing longer than the pane
/// around its own cursor.
fn paint_editors(
    open_tabs: &mut OpenTabs,
    renderer: &GpuRenderer,
    config: &crate::config::Config,
    rects: &[(TabId, Rect)],
) -> Vec<EditorPaint> {
    let mut paints = Vec::new();
    for (id, rect) in rects {
        let (cols, rows) = renderer.grid_size_for(App::layout_rect_to_pane(*rect));
        let Some(slot) = open_tabs.editor_mut(id) else {
            continue;
        };
        // The settings reach the kinds they bear on, before each paint.
        slot.editor.with_kind_mut(|text: &mut TextEditor| text.configure(config));
        slot.editor.with_kind_mut(|dir: &mut DirEditor| dir.set_gutter_side(config.gutter_side));
        slot.editor.set_scroll_margin(config.scroll_margin);
        let wrap = slot.view.wrap;
        let content = slot.editor.content(rows, cols, wrap);
        // Kept on the slot rather than in the paint, so that selecting over
        // the pane after this frame reads the rows the user can see.
        slot.view.cursor_line = content.cursor_line;
        let (grid, outlines) = build_outlined_grid(&content, renderer.theme(), cols, rows, wrap);
        slot.view.painted = Some(grid);
        // A row that wrapped pushes every row after it down the pane, and
        // the icons beside them with it.
        let starts = screen_row_starts(&content, cols, wrap);
        // A click lands on the screen row it is over, which the editor counts
        // as though none of a row's screen rows were skipped.
        let skipped = content.skipped;
        slot.view.skipped = ScreenSkip::new(starts.get(skipped.row).copied(), skipped.rows);
        // An icon sits on its row's first screen row, gone with it where
        // that is scrolled past.
        let icons = content
            .icons
            .into_iter()
            .filter(|icon| skipped.rows_of(icon.row) == 0)
            .filter_map(|icon| {
                let row = *starts.get(icon.row)?;
                Some(Icon { row, ..icon })
            })
            .collect();
        let (cell_w, cell_h) = renderer.cell_size();
        let pane = App::layout_rect_to_pane(*rect);
        // What floats over a cell is placed from where the renderer draws
        // the cell (see `PaneRect::cell_box`), so it sits on its row.
        let cell_box = |row: usize, col: usize| pane.cell_box((row, col), (cell_w, cell_h));
        // The popup hangs under the word it completes, on the screen row the
        // cursor's line starts on.
        let popups = slot.editor.popups();
        let popup = popups.completion.and_then(|popup| {
            let caret = slot.editor.caret()?;
            let row = content.cursor_line?;
            let col = caret.col.saturating_sub(popup.typed);
            Some(CompletionView {
                anchor: cell_box(row, col),
                rows: popup
                    .items
                    .into_iter()
                    .map(|item| CompletionRow {
                        detail: item.detail,
                        kind: item.kind,
                        label: item.label,
                        source: item.source.to_string(),
                    })
                    .collect(),
                selected: popup.selected,
            })
        });
        let caret_cell = slot.editor.caret().and_then(|caret| {
            let row = content.cursor_line?;
            Some(cell_box(row, caret.col))
        });
        // A card hangs over the word it is about: the pointer's cell, or
        // the cursor's.
        let info = popups.card.and_then(|card| {
            let (row, col) = match card.cell {
                Some(cell) => cell,
                None => {
                    let caret = slot.editor.caret()?;
                    (content.cursor_line?, caret.col)
                }
            };
            Some(InfoView {
                anchor: cell_box(row, col),
                footer: card.footer,
                text: card.text,
            })
        });
        let drop_list = popups.drop_list.map(|list| {
            let (left, top, _) = cell_box(list.cell.0, list.cell.1);
            DropListView {
            cell: (cell_w, cell_h),
            cols: list.cols,
            origin: (left, top),
            rows: list.rows,
            selected: list.selected,
            }
        });
        let toolbar = slot.editor.toolbar().map(|bar| ToolbarView {
            left: bar.left,
            owner: id.0,
            rect: (pane.x, pane.y, pane.width, bar.rows as f32 * cell_h),
            right: bar.right,
        });
        paints.push(EditorPaint {
            caret_cell,
            completion: popup,
            cursor_line: content.cursor_line,
            drop_list,
            icons,
            info,
            outlines,
            tab: *id,
            toolbar,
        });
    }
    paints
}

/// Each pane's tab strip, rasterized into a texture placed along the pane's top.
fn rasterize_strips(
    renderer: &mut GpuRenderer,
    strip_frames: &[StripFrame],
) -> Vec<ImagePlacement> {
    strip_frames
        .iter()
        .enumerate()
        .map(|(i, frame)| {
            renderer.rasterize_pane_strip(
                PANE_STRIP_TEXTURE_BASE - i as u64,
                &frame.tabs,
                &frame.layout,
                frame.rect.width as u32,
                frame.rect.height as u32,
                frame.hover,
                frame.drop_target,
                frame.rect.x,
                frame.rect.y,
            )
        })
        .collect()
}

/// One pane's rainbow-parens marks: viewport `(row, col)` plus the resolved
/// RGB to paint that bracket glyph in.
pub(super) type BracketColors = Vec<(usize, usize, (u8, u8, u8))>;

/// One frame's per-pane overlay data, in the same order as the pane rects, so
/// a pane's entry is found by its index in that list.
pub(super) struct PaneOverlays {
    pub(super) bracket_colors: Vec<BracketColors>,
    pub(super) find_labels: Vec<(usize, usize, char)>,
    pub(super) quick_select: Vec<(usize, usize, char)>,
    pub(super) search_current: Vec<Vec<(usize, usize)>>,
    pub(super) search_matches: Vec<Vec<(usize, usize)>>,
    pub(super) sentence_spans: Vec<Vec<(usize, usize, usize, u8)>>,
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
    /// how strongly it is lit (see [`crate::window::input::focus::FocusFlash`]).
    flash: Option<(TabId, f32)>,
    focused: TabId,
    /// The focused pane's block bands, `(abs_row, rows)` in anchor order: the
    /// reserved rows a nav cursor treats as one stop, and the span the
    /// block-as-cursor outline replaces the 1-cell cursor with.
    focused_bands: &'a [(usize, usize)],
    hovered_pane: Option<TabId>,
    hovered_url: Option<&'a str>,
    overlays: &'a PaneOverlays,
    editor_paints: &'a [EditorPaint],
    /// Where each editor's caret sits, and whether it is typing there.
    editor_cursors: &'a std::collections::HashMap<TabId, PaneCaret>,
    /// The pane drawn as active: the focused one, or none while a popup has
    /// the keyboard, so every pane reads as inactive under it.
    active: Option<TabId>,
    rects: &'a [(TabId, Rect)],
    /// Each pane's whole area, its strip of tabs included, which its borders
    /// run along.
    areas: &'a [(TabId, Rect)],
    selection: Option<&'a Selection>,
    /// Every open tab: the grid each editor last painted into, and each
    /// terminal with its mode.
    tabs: &'a OpenTabs,
    window_focused: bool,
}

/// One editor tab's painted frame: the grid the editor rendered itself into, and
/// the row its cursor line sits on.
pub(in crate::window) struct EditorPaint {
    /// The cursor's cell, as its left edge, top edge, and bottom edge in
    /// surface pixels: what a list of what can be done there floats from.
    pub(in crate::window) caret_cell: Option<(f32, f32, f32)>,
    pub(in crate::window) cursor_line: Option<usize>,
    /// Icons to rasterize over the painted rows, each `row` already a screen
    /// row of the pane rather than the editor's own row index.
    pub(in crate::window) icons: Vec<Icon>,
    pub(in crate::window) tab: TabId,
    /// The bar the editor has drawn over its top rows, in the window's font.
    pub(in crate::window) toolbar: Option<ToolbarView>,
    /// The completion popup open at the editor's cursor.
    pub(in crate::window) completion: Option<CompletionView>,
    /// The info card open over the editor's text.
    pub(in crate::window) info: Option<InfoView>,
    /// The drop-down list open over the editor's pane.
    pub(in crate::window) drop_list: Option<DropListView>,
    /// The runs of cells drawn with a border: the occurrences of the word
    /// under the cursor.
    pub(in crate::window) outlines: Vec<(usize, usize, usize)>,
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
        editor_cursors,
        editor_paints,
        rects,
        areas,
        selection,
        tabs,
        window_focused,
    } = input;
    let mut views: Vec<PaneView> = Vec::new();
    for (i, (id, rect)) in rects.iter().enumerate() {
        let area = areas.iter().find(|(pane, _)| pane == id).map_or(*rect, |(_, area)| *area);
        // An editor tab paints a grid of its own; every other tab drawn is a
        // terminal.
        if let Some(paint) = editor_paints.iter().find(|paint| paint.tab == *id) {
            if let Some(grid) = tabs.editor(id).and_then(|slot| slot.view.painted.as_ref()) {
                views.push(editor_pane_view(
                    paint,
                    grid,
                    *rect,
                    area,
                    active == Some(*id),
                    window_focused,
                    config,
                    blink_phase,
                    selection.filter(|s| s.tab == *id),
                    editor_cursors.get(id).copied(),
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
                area: App::layout_rect_to_pane(area),
                scroll_offset: terminal.grid().scroll_offset(),
                scrollback_len: terminal.grid().scrollback_len(),
                search_matches: &overlays.search_matches[i],
                search_current: &overlays.search_current[i],
                sentence_spans: &overlays.sentence_spans[i],
                outlines: &[],
                selection: sel_tuple,
                selection_block: sel_block,
                completion: None,
            drop_list: None,
                info: None,
                toolbar: None,
                url_underline: config.url_underline,
            });
        }
    }

    views
}

/// Build the view for an editor tab.
///
/// An editor has no terminal caret of its own, so the cursor is parked out of
/// bounds (suppressing it) and the cursor line marks where the editor's own
/// selection sits. A text selection over the editor is drawn here the same way a
/// terminal draws one: the rows it names are the editor's, so it has to be
/// resolved against the grid the editor painted.
/// Where a pane's caret is drawn and how it reads: the cell it is on, and
/// whether what is happening there is typing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) struct PaneCaret {
    pub(in crate::window) col: usize,
    pub(in crate::window) insert: bool,
    pub(in crate::window) row: usize,
}

impl PaneCaret {
    /// A caret on `(row, col)` that is not typing.
    pub(super) fn at(row: usize, col: usize) -> Self {
        Self {
            col,
            insert: false,
            row,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn editor_pane_view<'a>(
    paint: &'a EditorPaint,
    grid: &'a Grid,
    rect: Rect,
    area: Rect,
    focused: bool,
    window_focused: bool,
    config: &crate::config::Config,
    blink_phase: bool,
    selection: Option<&'a Selection>,
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
    // A caret in an editor that does not have the keyboard, because another pane
    // does or because the window lost focus, is drawn hollow.
    let caret_unfocused = text_cursor.is_some() && !(focused && window_focused);
    PaneView {
        overlay: None,
        // Editors draw no program output, so nothing in them blinks.
        blink_visible: true,
        bracket_colors: &[],
        block_band: None,
        // An editor's caret is a block over the cell it is on, as the traversal
        // cursor is everywhere else; an editor taking typed text takes the shape
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
        // editor has no caret of its own, so the cursor line is the only mark of
        // where the editor's own selection sits.
        nav_cursor: Some(
            text_cursor
                .map(|caret| (caret.row, caret.col))
                .unwrap_or((grid.rows(), grid.cols())),
        ),
        cursor_line_row: paint.cursor_line,
        // The caret's line while there is one, else the line the editor marks.
        line_flash: flash.and_then(|strength| {
            let row = text_cursor.map(|caret| caret.row).or(paint.cursor_line)?;
            Some((row, strength))
        }),
        // The caret blinks with the global `cursor.blink` setting, on the same
        // timer as a terminal's, and holds steady when it is drawn hollow.
        nav_cursor_visible: text_cursor.is_some()
            && (caret_unfocused || !config.cursor.blink || blink_phase),
        rect: App::layout_rect_to_pane(rect),
        area: App::layout_rect_to_pane(area),
        scroll_offset: 0,
        scrollback_len: 0,
        search_matches,
        search_current,
        sentence_spans: &[],
        outlines: &paint.outlines,
        selection: sel_tuple,
        selection_block: sel_block,
        // Only where the keys go: a popup in a pane typed into no longer
        // would answer nothing.
        completion: paint.completion.as_ref().filter(|_| focused && window_focused),
        info: paint.info.as_ref(),
        drop_list: paint.drop_list.as_ref(),
        toolbar: paint.toolbar.as_ref(),
        url_underline: false,
    }
}

/// Whether a text caret is drawn this frame. It blinks with the terminal's
/// cursor: the same clock, shown again on every key press, steady when
/// blinking is off. In a window without the keyboard it is steady too, as the
/// blink clock is paused there and could have stopped on the dark phase.
fn is_caret_lit(blink: bool, window_focused: bool, blink_phase: bool) -> bool {
    !blink || !window_focused || blink_phase
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
pub(in crate::window) struct FindTarget {
    pub(in crate::window) caret: usize,
    /// `/` down the text, `?` back up it.
    pub(in crate::window) label: String,
    pub(in crate::window) query: String,
    /// The tab being searched, whose pane the popup hangs from.
    pub(in crate::window) tab: TabId,
}

impl App {
    /// The search being typed, for the find popup at the top-right of the
    /// pane it searches: the terminal's `/` line, or an editor's search
    /// question. How far it has got stays in the status bar; every other
    /// question is asked in a dialog (see [`App::input_view`]).
    pub(in crate::window) fn find_target(&self) -> Option<FindTarget> {
        if self.input.keys.prefix == PendingPrefix::SearchInput {
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
        let prompt = self.chrome.editor_prompt()?;
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
    /// the live search otherwise, so an editor's matches stay lit after `Enter`
    /// exactly as a terminal's do. A search is of the focused tab alone: the
    /// other panes in view light nothing.
    pub(super) fn search_highlight_cells(&self, id: TabId, focused: TabId) -> (Cells, Cells) {
        if id != focused {
            return Default::default();
        }
        let typed = self
            .chrome.editor_prompt()
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
        let Some(slot) = self.open_tabs.editor(&id) else {
            return Default::default();
        };
        let Some(grid) = slot.view.painted.as_ref() else {
            return Default::default();
        };
        let spans = painted_match_spans(grid, query);
        // An editor's search moves its cursor, so the match the cursor is on is
        // the current one: the one under the caret where the editor has one,
        // else the one on the cursor's row nearest it, so a caret a column
        // off its match (a row's padding the editor does not count) still
        // marks one.
        let caret = slot.editor.caret().map(|caret| caret.col);
        let current = slot
            .view
            .cursor_line
            .and_then(|row| current_span(&spans, row, caret));
        let cells =
            |&(row, start, end): &(usize, usize, usize)| (start..end).map(move |col| (row, col));
        (
            spans.iter().flat_map(cells).collect(),
            current.into_iter().flat_map(cells).collect(),
        )
    }

    pub(in crate::window) fn render_frame(&mut self) {
        // What is about to be drawn is the current state, so the pending-repaint
        // flag is spent here. Clearing it at the top rather than the bottom keeps
        // a change made while this frame is being built from being swallowed: it
        // sets the flag again and earns its own frame.
        self.dirty = false;
        self.runtime.last_frame_at = Some(Instant::now());
        // The settings page is a full-window modal; it replaces the panes, tabbar,
        // status bar, and block tiles entirely until it closes.
        if self.chrome.settings_page().is_some() {
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
        // Built before the renderer is borrowed too: it fills the palette's
        // icon cache, which needs the app mutably.
        let mut palette_view = self.palette_popup();
        let input_view = self.input_view().map(|view| InputView {
            caret_visible: self.caret_lit(),
            ..view
        });
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
        let areas = self.layout().rects(viewport);
        let focused = self.tabs.layout.focused();
        let strip_frames = self.strip_frames(viewport, (cw, ch));
        let mode = self.open_tabs.mode(&focused);
        let editor_paints = paint_editors(
            &mut self.open_tabs,
            renderer,
            &self.config,
            &rects,
        );
        // Painting loaded each pane's view in turn; what the status bar and
        // the caret read next is the focused tab's.
        self.open_tabs.load_view(focused);
        // A list of what can be done at the cursor floats from the asking
        // editor's cursor; one whose editor is not shown is centred instead.
        if let (Some(view), Some(pick)) = (palette_view.as_mut(), self.chrome.editor_pick()) {
            if pick.placement == PickPlacement::Cursor {
                view.anchor = editor_paints
                    .iter()
                    .find(|paint| paint.tab == pick.tab)
                    .and_then(|paint| paint.caret_cell);
            }
        }
        let overlays = self.build_pane_overlays(&rects, renderer.theme());
        let hovered_pane = self.hovered_pane(&rects);
        let search = self.status_search();
        let status = self.build_status_bar(mode, renderer.theme(), search, notice, &focused);
        let status = status_enabled.then_some(&status);
        let find_view = self.find_popup(&rects);
        let which_key_view = self.which_key_popup(focused);
        let token_inspect_view = token_inspect_popup(
            &mut self.open_tabs,
            self.editor_cursor.as_ref(),
            self.host.token_inspect,
            &rects,
            (cw, ch),
            focused,
            renderer.theme(),
        );
        // Gathered before the renderer is held mutably: the bands read app
        // state the views below borrow alongside it.
        let focused_bands = self.term.block_bands(focused);
        // Rasterized before the renderer is taken mutably below, since filling
        // the icon cache needs it mutably too.
        let icon_placements = self.editor_icon_placements(&editor_paints, &rects, (cw, ch));
        let shown_editors: Vec<TabId> = editor_paints.iter().map(|paint| paint.tab).collect();
        let picture_placements = self.editor_canvas_placements(&shown_editors, &rects, ch);
        let editor_cursors = self.editor_carets();

        let preedit = self.preedit_overlay(focused);
        let active = self.active_pane();

        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        refresh_block_textures(&mut self.term.image_blocks, &rects, ch, renderer);
        let mut placements = image_placements(
            &self.term.image_blocks,
            &self.open_tabs,
            &rects,
            ch,
            &editor_paints,
        );
        placements.extend(picture_placements);
        placements.extend(icon_placements);
        placements.extend(rasterize_strips(renderer, &strip_frames));
        let views = build_pane_views(PaneViewInput {
            active,
            blink_phase: self.blink.phase,
            text_blink_visible: self.blink.text.is_visible(),
            preedit: preedit.as_ref(),
            config: &self.config,
            flash: self
                .input.focus
                .flash
                .and_then(|flash| Some((flash.tab, flash.strength(Instant::now())?))),
            focused,
            focused_bands: &focused_bands,
            hovered_pane,
            hovered_url: self.pointer.hovered_url.as_deref(),
            overlays: &overlays,
            editor_cursors: &editor_cursors,
            editor_paints: &editor_paints,
            rects: &rects,
            areas: &areas,
            selection: self.selection.span.as_ref(),
            tabs: &self.open_tabs,
            window_focused: self.input.focus.window,
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
            token_inspect_view.as_ref(),
        );

        self.update_ime_anchor(focused);
    }

    // --------------------------------------------------------------------
    // Popups and carets
    // --------------------------------------------------------------------

    /// Whether a text caret is drawn this frame: the cursor's blink setting,
    /// the window's focus, and where the blink clock is.
    fn caret_lit(&self) -> bool {
        is_caret_lit(
            self.config.cursor.blink,
            self.input.focus.window,
            self.blink.phase,
        )
    }

    /// The palette as the renderer draws it, its icons resolved through the
    /// cache kept between frames, which is emptied once the palette closes.
    fn palette_popup(&mut self) -> Option<PickerView> {
        let mut palette_icons = std::mem::take(&mut self.textures.palette_icons);
        let view = self.chrome.picker().map(|p| {
            palette_view(
                p,
                self.config.palette_match_underline,
                self.chrome.editor_pick().map(|pick| pick.label.as_str()),
                self.caret_lit(),
                &mut palette_icons,
            )
        });
        if view.is_none() {
            palette_icons.clear();
        }
        self.textures.palette_icons = palette_icons;
        view
    }

    /// What the status bar shows of a search: a search question being typed,
    /// which it shows over any search already lit, else the one lit.
    fn status_search(&self) -> Option<StatusSearch> {
        let typed = self
            .chrome.editor_prompt()
            .and_then(|prompt| prompt.typed_search());
        typed
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
            })
    }

    /// The find bar over the pane its search runs in, when one is open.
    fn find_popup(&self, rects: &[(TabId, Rect)]) -> Option<FindView> {
        let target = self.find_target()?;
        let (_, rect) = rects.iter().find(|(id, _)| *id == target.tab)?;
        let pane = App::layout_rect_to_pane(*rect);
        Some(FindView {
            caret: target.caret,
            caret_visible: self.caret_lit(),
            label: target.label,
            pane: (pane.x, pane.y, pane.width),
            query: target.query,
        })
    }

    /// The which-key popup for the focused tab.
    ///
    /// An editor part-way through a multi-key command says so through the
    /// same card the terminal's own prefixes use: a key sequence once it has
    /// waited the same delay as the terminal's prefixes, else the editor's own
    /// hint, a menu at once (the menus are how the editors are learned) and
    /// a prefix's keys once it has waited that delay too, else a terminal
    /// prefix that has stalled.
    fn which_key_popup(&self, focused: TabId) -> Option<WhichKeyView> {
        let sequence_view = self
            .sequence_hint(focused)
            .filter(|_| {
                self.input.keys
                    .prefix_since
                    .is_some_and(|s| s.elapsed() >= WHICH_KEY_DELAY)
            })
            .map(|(keys, items)| WhichKeyView {
                items,
                title: key_hint_title(&keys),
            });
        sequence_view
            .or_else(|| {
                self.open_tabs
                    .editor(&focused)
                    .and_then(|slot| slot.editor.hint())
                    .filter(|hint| {
                        let stalled = self
                            .input.keys
                            .editor_hint_since
                            .is_some_and(|since| since.elapsed() >= WHICH_KEY_DELAY);
                        hint.menu || stalled
                    })
                    .map(|hint| WhichKeyView {
                        items: hint.items,
                        title: hint.title,
                    })
            })
            .or_else(|| which_key_view(&self.input.keys.prefix, self.input.keys.prefix_since))
    }
}

impl App {

    // --------------------------------------------------------------------
    // Frame data
    // --------------------------------------------------------------------

    /// Draw the settings page as a single full-window grid: no tabbar, status
    /// bar, or panes, just the modal overlay.
    fn render_settings_frame(&mut self) {
        // The field rather than the accessor, so the renderer beside it can
        // be held mutably while the page is read.
        let Some(Overlay::Settings(editor)) = &self.chrome.overlay else {
            return;
        };
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let (cols, rows) = renderer.grid_size();
        let (cw, ch) = renderer.cell_size();
        let grid = build_settings_grid(editor, renderer.theme(), cols, rows);
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
            // Alone on its surface: nothing to share a border with.
            area: PaneRect {
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
            outlines: &[],
            selection: None,
            selection_block: false,
            completion: None,
            drop_list: None,
            info: None,
            toolbar: None,
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
            None,
        );
    }
}

// ========================================================================
// Data Structures
// ========================================================================

/// Viewport cells, as `(row, col)`, that one overlay paints over a pane.
pub(super) type Cells = Vec<(usize, usize)>;

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

/// The match among `spans` (`(row, start, end)` in painted cells) an editor's
/// cursor is on, on cursor row `row`: the one under the caret at column
/// `caret`, else the one nearest it, so a caret a column off its match (a
/// row's padding the editor does not count) still marks one; with no caret,
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
#[path = "compose_tests.rs"]
mod tests;
