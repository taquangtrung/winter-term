//! Frame composition and WebView tile management.

use base64::Engine;
use serde_json::Value;

use crate::model::layout::{PaneId, Rect};
use crate::model::mode::Mode;
use crate::model::palette::{Palette, PaletteMode};
use crate::model::settings_page::{Control, SettingsField, SettingsPage};
use crate::terminal::block_queue::{BlockEntry, BlockKind};
use crate::terminal::pane::{Pane, MAX_IMAGE_ROWS};
use crate::terminal::webview;
use winter_core::winter_proto::EmitBlock;
use winter_render::renderer::{PaneRect, PaneView};
use winter_render::{
    Color, CursorShape, Grid, ImagePlacement, NoticeKind, PaletteItem, PaletteView, RgbColor,
    StatusNotice, StatusSearch, Style, Theme, ThemeRgb,
};

use super::{status_bar, App, ImageBlock, ReflowSource};

// ========================================================================
// Constants
// ========================================================================

/// Raster image MIME types rendered natively on the GPU. Other rich types
/// (HTML, markdown, ...) still go to the WebView.
const RASTER_MIMES: [&str; 4] = ["image/gif", "image/jpeg", "image/png", "image/webp"];
const CSV_MIME: &str = "text/csv";
const JSON_MIME: &str = "application/json";
const MARKDOWN_MIME: &str = "text/markdown";
const SVG_MIME: &str = "image/svg+xml";

/// Blank space left above a block's image inside its reserved band, as a
/// fraction of the cell height, so a block reads as its own object instead of
/// butting straight against the line of output above it.
///
/// Taken *out* of the band, not added to it. The reservation is what keeps the
/// following prompt flush under the block, so padding the band's height here
/// would instead open the same gap below every block.
const BLOCK_PAD_TOP_RATIO: f32 = 0.5;

/// Opacity a closed live block's image placement draws at, so a reader can
/// tell a finished block from one still accepting patches.
const CLOSED_BLOCK_ALPHA: f32 = 0.5;

/// How long a pending key prefix must sit unresolved before the which-key
/// popup explains it. Long enough that a fluent multi-key sequence never
/// raises it, short enough that a genuine pause gets an answer.
const WHICH_KEY_DELAY: std::time::Duration = std::time::Duration::from_millis(1000);

/// Settings-page layout, in cells: the label indent, the column dim notes start
/// at, the right margin values align to, and the body's first row (below the
/// header band and its divider).
const SETTINGS_LEFT_PAD: usize = 4;
const SETTINGS_NOTE_COL: usize = 28;
const SETTINGS_RIGHT_PAD: usize = 4;
const SETTINGS_FIRST_ROW: usize = 3;
/// Footer hint shown along the bottom of the settings page.
const SETTINGS_HINT: &str = "↑/↓ Move     ←/→ Change     Space Toggle     Enter/Esc Close";

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
    config: &'a crate::config::Config,
    focused: PaneId,
    /// The focused pane's block bands, `(abs_row, rows)` in anchor order: the
    /// reserved rows a nav cursor treats as one stop, and the span the
    /// block-as-cursor outline replaces the 1-cell cursor with.
    focused_bands: &'a [(usize, usize)],
    hovered_pane: Option<PaneId>,
    hovered_url: Option<&'a str>,
    modes: &'a std::collections::HashMap<PaneId, Mode>,
    nav_cursors: &'a std::collections::HashMap<PaneId, (usize, usize)>,
    overlays: &'a PaneOverlays,
    palette_open: bool,
    panes: &'a std::collections::HashMap<PaneId, crate::terminal::pane::Pane>,
    rects: &'a [(PaneId, Rect)],
    selection: Option<&'a super::Selection>,
    window_focused: bool,
}

/// Build one `PaneView` per laid-out pane: what the renderer should draw for
/// that pane this frame.
fn build_pane_views<'a>(input: PaneViewInput<'a>) -> Vec<PaneView<'a>> {
    let PaneViewInput {
        blink_phase,
        config,
        focused,
        focused_bands,
        hovered_pane,
        hovered_url,
        modes,
        nav_cursors,
        overlays,
        palette_open,
        panes,
        rects,
        selection,
        window_focused,
    } = input;
    let mut views: Vec<PaneView> = Vec::new();
    for (i, (id, rect)) in rects.iter().enumerate() {
        if let Some(pane) = panes.get(id) {
            let (sel_tuple, sel_block) = match selection {
                Some(s) if s.pane == *id => (
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
            // Each pane's cursor follows its own mode (stored per pane), so a
            // non-focused pane shows the configured shape for its mode rather
            // than always reverting to a stale Block the shell reported via
            // DECSCUSR.
            let pane_mode = modes.get(id).copied().unwrap_or_default();
            let nav_cursor = if *id == focused && matches!(pane_mode, Mode::Normal | Mode::Visual) {
                // Direct field access (not the method) so only `nav_cursors`
                // is borrowed, avoiding a conflict with the live `pane` borrow.
                nav_cursors.get(id).copied()
            } else {
                None
            };
            // The block-as-cursor: while the traversal cursor sits inside a
            // rich block's reserved band, the band itself is the cursor. The
            // span is clipped to the viewport so an outline never draws past
            // an edge the image itself is cropped at.
            let block_band = nav_cursor.and_then(|(nav_row, _)| {
                focused_bands.iter().find_map(|&(abs_row, band_rows)| {
                    let top = pane.grid().to_viewport_row(abs_row);
                    let bottom = top + band_rows as isize;
                    let rows = pane.grid().rows() as isize;
                    let (start, end) = (top.max(0), bottom.min(rows));
                    let nav = nav_row as isize;
                    (nav >= start && nav < end)
                        .then(|| (start.max(0) as usize, (end - start).max(1) as usize))
                })
            });
            // The cursor-line band is not focus-gated: a pane left in Normal
            // mode keeps showing where its cursor is, so switching panes (and
            // back) doesn't lose your place. Only the cursor block itself is
            // drawn for the focused pane alone.
            let cursor_line_row = cursor_line_row(pane_mode, nav_cursors.get(id).copied());
            let config_shape = match pane_mode {
                Mode::Insert => config.cursor.insert,
                Mode::Normal => config.cursor.normal,
                Mode::Visual => config.cursor.visual,
                Mode::BlockFocus => config.cursor.block_focus,
            };
            let cursor_shape = effective_cursor_shape(
                !pane.is_at_prompt(),
                pane.grid().reported_cursor_shape(),
                config_shape,
            );
            let hovered_link = if hovered_pane == Some(*id) {
                hovered_url
                    .map(|url| pane.grid().find_link_id(url))
                    .unwrap_or(0)
            } else {
                0
            };
            let is_focused = *id == focused;
            let cursor_unfocused = is_focused && !window_focused;
            let cursor_visible = if palette_open || !pane.grid().cursor_visible() {
                // The palette steals focus, and DECTCEM (CSI ?25l) lets a
                // full-screen app like btop hide the cursor outright.
                false
            } else if cursor_unfocused {
                // An unfocused cursor marks "not receiving keystrokes"; blinking
                // it would fight that signal by making it disappear half the
                // time.
                true
            } else if is_focused {
                !config.cursor.blink || blink_phase
            } else {
                !config.cursor.hide_in_inactive
            };
            views.push(PaneView {
                bracket_colors: &overlays.bracket_colors[i],
                block_band,
                cursor_shape,
                cursor_unfocused,
                cursor_visible,
                dim: !is_focused && config.dim_inactive,
                focused: is_focused,
                grid: pane.grid(),
                hovered_link,
                labels,
                find_labels: if *id == focused {
                    &overlays.find_labels
                } else {
                    &[]
                },
                nav_cursor,
                cursor_line_row,
                // The Normal-mode cursor blinks on the same timer as the
                // shell's, so it's as easy to spot while navigating, but
                // holds steady, in its unfocused form, while the window
                // lacks focus.
                nav_cursor_visible: cursor_unfocused || !config.cursor.blink || blink_phase,
                rect: App::layout_rect_to_pane(*rect),
                scroll_offset: pane.grid().scroll_offset(),
                scrollback_len: pane.grid().scrollback_len(),
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

/// Re-rasterize width-wrapped blocks (markdown/CSV/JSON) whose pane width
/// changed since they were last rendered, so wrapping stays correct on resize.
/// Intrinsic-size blocks (raster/SVG) have `reflow == None` and are skipped.
fn reflow_width_wrapped_blocks(
    blocks: &mut [ImageBlock],
    rects: &[(PaneId, Rect)],
    renderer: &mut winter_render::renderer::GpuRenderer,
) {
    for block in blocks.iter_mut() {
        let Some((_, rect)) = rects.iter().find(|(id, _)| *id == block.pane_id) else {
            continue;
        };
        let target_w = App::layout_rect_to_pane(*rect).width;
        let target = target_w.floor() as u32;
        if block.rastered_width == target || block.reflow.is_none() {
            continue;
        }
        let id = block.id;
        let dims = match block.reflow.as_ref() {
            Some(ReflowSource::Markdown(md)) => renderer.upload_markdown(id, &md.clone(), target_w),
            Some(ReflowSource::Text(text)) => renderer.upload_text(id, &text.clone(), target_w),
            None => None,
        };
        if let Some((nat_w, nat_h)) = dims {
            block.nat_w = nat_w;
            block.nat_h = nat_h;
            block.rastered_width = target;
        }
    }
}

/// The on-screen slice of a block whose band starts at viewport row `top_row`
/// of a pane `pane_h` pixels tall starting at `pane_top`: where to draw it and
/// which rows of its texture survive clipping.
struct BlockClip {
    /// Drawn height in pixels, always the part inside the pane.
    height: f32,
    /// Bottom of the sampled texture, `1.0` when nothing is cropped.
    v_max: f32,
    /// Top of the sampled texture, `0.0` when nothing is cropped.
    v_min: f32,
    /// Top edge in pixels from the window's top.
    y: f32,
}

/// Clip a block's `display_h`-tall image to the `band_h` pixels of its reserved
/// band still available to it, and to the pane, given the image's top edge
/// `band_top` pixels below the pane's own top.
///
/// `band_top` is signed on purpose. A band scrolls above the pane's first row
/// well before it leaves the viewport, and dropping it at that point makes a
/// tall block vanish the moment its first line does; cropping the texture
/// instead keeps the rows still on screen aligned with the grid rows they
/// belong to. Returns `None` only once no part of the block is visible.
fn clip_block_band(
    band_top: f32,
    display_h: f32,
    band_h: f32,
    pane_top: f32,
    pane_h: f32,
) -> Option<BlockClip> {
    if display_h <= 0.0 {
        return None;
    }
    // Pane-relative extent of the drawn image: the band's top, running for as
    // much of the image as the band holds.
    let drawn_top = band_top.max(0.0);
    let drawn_bottom = (band_top + display_h.min(band_h)).min(pane_h);
    if drawn_bottom <= drawn_top {
        return None;
    }
    Some(BlockClip {
        height: drawn_bottom - drawn_top,
        v_max: (drawn_bottom - band_top) / display_h,
        v_min: (drawn_top - band_top) / display_h,
        y: pane_top + drawn_top,
    })
}

/// Whether `pane` can still absorb `extra` blank rows inserted at absolute row
/// `abs_row`, the first row past a band that has outgrown its reservation.
///
/// False once the row has scrolled into history (the grid only inserts into
/// its live rows) or once the insert would push the band past the bottom of
/// the screen. Both cases fall back to clipping the block to its band.
fn band_has_room(pane: &Pane, abs_row: usize, extra: usize) -> bool {
    let grid = pane.grid();
    let Some(live_row) = abs_row.checked_sub(grid.absolute_live_top()) else {
        return false;
    };
    live_row + extra <= grid.rows()
}

/// Place native image blocks at their anchor row, scaled to fit the pane width
/// and preserving aspect, cropping any part outside the pane's content area.
fn image_placements(
    blocks: &[ImageBlock],
    panes: &std::collections::HashMap<PaneId, crate::terminal::pane::Pane>,
    rects: &[(PaneId, Rect)],
    cell_height: f32,
) -> Vec<ImagePlacement> {
    let mut placements: Vec<ImagePlacement> = Vec::new();
    for img in blocks {
        let Some((_, rect)) = rects.iter().find(|(id, _)| *id == img.pane_id) else {
            continue;
        };
        let Some(pane) = panes.get(&img.pane_id) else {
            continue;
        };
        // While the pane shows the alternate screen, a full-screen app owns
        // every row of the viewport. A primary-screen block painted on top
        // would hide the app's own rows (four lines of vim, a htop gauge)
        // until it exits, so it is not drawn at all until the pane returns.
        if pane.grid().is_alt_screen() {
            continue;
        }
        let pane_rect = App::layout_rect_to_pane(*rect);
        let top_row = pane.grid().to_viewport_row(img.abs_row);
        let nat_w = img.nat_w as f32;
        let nat_h = img.nat_h as f32;
        if nat_w <= 0.0 || nat_h <= 0.0 {
            continue;
        }
        // The image is inset below the band's first row by the top padding,
        // and gets whatever height the band has left after it.
        let pad_top = cell_height * BLOCK_PAD_TOP_RATIO;
        let band_top = top_row as f32 * cell_height + pad_top;
        let band_h = (img.max_rows as f32 * cell_height - pad_top).max(0.0);
        let (display_w, display_h) = if img.fit_to_band {
            // Images/SVG: scale down to fit the reserved band.
            let scale = (pane_rect.width / nat_w).min(band_h / nat_h).min(1.0);
            (nat_w * scale, nat_h * scale)
        } else {
            // Text/markdown: native size (wrapped to pane width).
            let w = nat_w.min(pane_rect.width);
            (w, nat_h * w / nat_w)
        };
        let Some(clip) =
            clip_block_band(band_top, display_h, band_h, pane_rect.y, pane_rect.height)
        else {
            continue;
        };
        placements.push(ImagePlacement {
            alpha: if img.closed { CLOSED_BLOCK_ALPHA } else { 1.0 },
            height: clip.height,
            id: img.id,
            v_max: clip.v_max,
            v_min: clip.v_min,
            width: display_w,
            x: pane_rect.x,
            y: clip.y,
        });
    }
    placements
}

/// The renderer's view of the command palette: its filtered items, the query
/// behind them, and the empty-state message matching what it searches over.
fn palette_view(palette: &Palette, match_underline: bool) -> PaletteView {
    let empty_message = match palette.mode {
        PaletteMode::History => "No matching history",
        PaletteMode::Panes => "No matching panes",
        PaletteMode::RecentDirs => "No recent directories",
        PaletteMode::Swoop => "No matching lines",
        PaletteMode::Commands
        | PaletteMode::MuxAttachRemote
        | PaletteMode::MuxKill
        | PaletteMode::MuxNew
        | PaletteMode::MuxSessions => "No matching commands",
    };
    PaletteView {
        empty_message: empty_message.to_string(),
        items: palette
            .filtered
            .iter()
            .map(|&i| PaletteItem {
                action: palette.entries[i].action.clone(),
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
            title: title.to_string(),
        })
}

// ========================================================================
// App: rendering
// ========================================================================

impl App {
    pub(crate) fn render_frame(&mut self) {
        // The settings page is a full-window modal; it replaces the panes, tabbar,
        // status bar, and block tiles entirely until it closes.
        if self.settings_page.is_some() {
            self.render_settings_frame();
            return;
        }

        // While a new-theme name is being entered, show the live input in place
        // of any transient notice (reuses the same status-bar/toast display).
        let notice = if let Some(input) = &self.theme_name_input {
            Some(StatusNotice {
                kind: NoticeKind::Info,
                text: format!("New theme name: {input}\u{2502}"),
            })
        } else {
            self.active_notice().map(|(text, kind)| StatusNotice {
                kind,
                text: text.to_string(),
            })
        };
        // A live `/` search forces the status bar on for its duration even if
        // it's configured hidden: it's the only place search feedback (query
        // text, match position) is shown, so there'd otherwise be nowhere to
        // put it. Reverts to the configured visibility as soon as the search
        // ends (`search.query` back to `None`). Shared with `viewport_rect`/
        // `resize_all_panes` via `status_bar_visible` so pane geometry and the
        // PTY's row count always agree with what's drawn here.
        let status_enabled = self.status_bar_visible();
        // When the status bar is (really) hidden it can't surface the notice,
        // so float it as a bottom-center toast instead (avoids showing it in
        // both places).
        let toast = if status_enabled { None } else { notice.clone() };
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
        let rects = self.tabs.all[self.tabs.active].rects(viewport);
        let focused = self.tabs.all[self.tabs.active].focused();
        let mode = self.modes.get(&focused).copied().unwrap_or_default();
        let overlays = self.build_pane_overlays(&rects, renderer.theme());
        let hovered_pane = self.hovered_pane(&rects);
        let search = self.search.query.as_ref().map(|q| StatusSearch {
            query: q.clone(),
            match_index: self.search.match_index,
            match_total: self.search.match_total,
            reverse: self.search.reverse,
        });
        let status = status_bar(
            mode,
            renderer.theme(),
            search,
            notice,
            &self.config.status_bar,
        );
        let status = status_enabled.then_some(&status);
        let palette_view = self
            .palette
            .as_ref()
            .map(|p| palette_view(p, self.config.palette_match_underline));
        let which_key_view = which_key_view(&self.pending, self.pending_since);
        // Gathered before the renderer is held mutably: the bands read app
        // state the views below borrow alongside it.
        let focused_bands = self.block_bands(focused);

        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        reflow_width_wrapped_blocks(&mut self.image_blocks, &rects, renderer);
        let placements = image_placements(&self.image_blocks, &self.panes, &rects, ch);
        let views = build_pane_views(PaneViewInput {
            blink_phase: self.blink_phase,
            config: &self.config,
            focused,
            focused_bands: &focused_bands,
            hovered_pane,
            hovered_url: self.pointer.hovered_url.as_deref(),
            modes: &self.modes,
            nav_cursors: &self.nav_cursors,
            overlays: &overlays,
            palette_open: self.palette.is_some(),
            panes: &self.panes,
            rects: &rects,
            selection: self.selection.span.as_ref(),
            window_focused: self.window_focused,
        });
        renderer.render(
            &views,
            status,
            Some(&tabbar),
            &placements,
            palette_view.as_ref(),
            toast.as_ref(),
            which_key_view.as_ref(),
        );

        self.reposition_block_tiles(&rects, full_rows, ch);
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
            winter_render::modern_tabbar_height_px(ch)
        } else {
            top_rows as f32 * ch
        };
        let status_h = if status_enabled {
            winter_render::STATUS_BAR_HEIGHT * ch
        } else {
            0.0
        };

        // Floor to whole cell rows and center the leftover sub-row slack above
        // and below the pane band, whether or not the status bar eats into it,
        // so a window height that isn't an exact multiple of the cell height
        // never leaves a dead, un-drawable strip pinned to one edge.
        let (rows, top_pad) = super::content_band(h - top_h_on_screen - status_h, ch);
        Rect::new(
            0.0,
            top_h_on_screen + top_pad,
            w,
            (rows as f32 * ch).max(1.0),
        )
    }

    /// Precompute the per-pane overlay data the pane views borrow as slices:
    /// labels, search highlights, sentence tints, and bracket colors. Built up
    /// front, in pane order, so the view loop can hand out slices of it.
    fn build_pane_overlays(&self, rects: &[(PaneId, Rect)], theme: &Theme) -> PaneOverlays {
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
        let query_str = self.search.query.as_deref().filter(|q| !q.is_empty());
        let search_match_data: Vec<Vec<(usize, usize)>> = rects
            .iter()
            .map(|(id, _)| match (query_str, self.panes.get(id)) {
                (Some(qs), Some(pane)) => {
                    crate::app::navigation::search::visible_match_cells(pane.grid(), qs)
                }
                _ => vec![],
            })
            .collect();
        // The focused match's cells, drawn in a different color than the rest.
        // Its position is absolute, so it only contributes cells while the row it
        // sits on is actually on screen.
        let search_current_data: Vec<Vec<(usize, usize)>> = rects
            .iter()
            .map(
                |(id, _)| match (query_str, self.search.current, self.panes.get(id)) {
                    (Some(qs), Some((pane_id, (abs_row, col))), Some(pane)) if pane_id == *id => {
                        let grid = pane.grid();
                        let top = grid.to_absolute_row(0);
                        if abs_row >= top && abs_row < top + grid.rows() {
                            let len = qs.chars().count().max(1);
                            (0..len).map(|k| (abs_row - top, col + k)).collect()
                        } else {
                            vec![]
                        }
                    }
                    _ => vec![],
                },
            )
            .collect();

        let sentence_span_data = if self.config.sentence_highlight {
            rects
                .iter()
                .map(|(id, _)| {
                    if let Some(pane) = self.panes.get(id) {
                        super::navigation::reading::sentence_spans(pane.grid())
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

        let bracket_color_data = if self.config.rainbow_parens {
            rects
                .iter()
                .map(|(id, _)| {
                    if let Some(pane) = self.panes.get(id) {
                        let marks = super::navigation::reading::bracket_marks(pane.grid());
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

    /// Which pane the mouse pointer is over, if any.
    fn hovered_pane(&self, rects: &[(PaneId, Rect)]) -> Option<PaneId> {
        let (cx, cy) = self.pointer.cursor_pos;
        let hovered_pane: Option<PaneId> = rects
            .iter()
            .find(|(_, r)| {
                let pr = Self::layout_rect_to_pane(*r);
                cx >= pr.x && cx < pr.x + pr.width && cy >= pr.y && cy < pr.y + pr.height
            })
            .map(|(id, _)| *id);

        hovered_pane
    }

    /// Move the WebView block tiles to follow this frame's scroll and layout.
    fn reposition_block_tiles(&mut self, rects: &[(PaneId, Rect)], full_rows: usize, ch: f32) {
        let focused = self.tabs.all[self.tabs.active].focused();
        // Tiles for panes outside the active tab are hidden so background tabs
        // don't show through; the active tab's tiles are positioned by scroll.
        let active_panes: std::collections::HashSet<PaneId> = self.tabs.all[self.tabs.active]
            .panes()
            .into_iter()
            .collect();
        // Panes currently showing the alternate screen: their WebView tiles
        // are hidden for the same reason `image_placements` skips them (a
        // full-screen app owns the viewport), and they reappear once the app
        // exits. Tracked in the layout key so the flip itself repositions.
        let alt_panes: std::collections::HashSet<PaneId> = self
            .panes
            .iter()
            .filter(|(_, pane)| pane.grid().is_alt_screen())
            .map(|(id, _)| *id)
            .collect();
        if let Some(pane) = self.panes.get(&focused) {
            // Tiles are anchored absolutely, so what moves them is the absolute
            // row currently at the top of the viewport: it advances both when
            // the user scrolls and when new output pushes lines into history.
            let viewport_top = pane.grid().to_absolute_row(0);

            let focused_rect = rects.iter().find(|(id, _)| *id == focused);
            let pane_y = focused_rect.map(|(_, r)| r.y).unwrap_or(0.0);

            // Repositioning every tile does a GTK round-trip per WebView; only do
            // it when the scroll position or layout actually changed, otherwise
            // plain typing (which never moves tiles) stalls on GTK IPC.
            let mut alt_key: Vec<PaneId> = alt_panes.iter().copied().collect();
            alt_key.sort_by_key(|id| id.0);
            let layout = (
                viewport_top,
                full_rows,
                ch.to_bits(),
                pane_y.to_bits(),
                alt_key,
            );
            if self.last_tile_layout.as_ref() != Some(&layout) {
                self.last_tile_layout = Some(layout);
                self.webview_mgr.reposition_tiles(
                    viewport_top,
                    full_rows,
                    ch,
                    pane_y,
                    &active_panes,
                    &alt_panes,
                );
            }
        }
    }

    /// Draw the settings page as a single full-window grid: no tabbar, status
    /// bar, panes, or block tiles, just the modal overlay.
    fn render_settings_frame(&mut self) {
        let Some(page) = &self.settings_page else {
            return;
        };
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let (cols, rows) = renderer.grid_size();
        let (cw, ch) = renderer.cell_size();
        let grid = build_settings_grid(page, renderer.theme(), cols, rows);
        let view = PaneView {
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
        );
    }

    pub(crate) fn create_block_tiles(&mut self, entries: &[(PaneId, BlockEntry)]) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let ch = match &self.renderer {
            Some(r) => r.cell_size().1,
            None => return,
        };
        let vp = self.viewport_rect();
        let layout_vp = Rect::new(vp.x, vp.y, vp.width, vp.height);
        let rects = self.tabs.all[self.tabs.active].rects(layout_vp);
        let font_family = self.config.font_family.clone();
        let font_size = self.config.font_size;
        let remote_assets = self.config.security.block_remote_assets;
        let debug = std::env::var_os("WINTER_BLOCK_DEBUG").is_some();

        for (pane_id, entry) in entries {
            if self.is_block_folded(*pane_id, entry.block_index) {
                continue;
            }
            let pane_rect = match rects.iter().find(|(id, _)| *id == *pane_id) {
                Some((_, r)) => Self::layout_rect_to_pane(*r),
                None => continue,
            };

            // Route images (raster + SVG) to the native GPU pass; everything
            // else (HTML, markdown, ...) renders in a WebView.
            if let Some(source) = native_image_source(&entry.emit) {
                let id = self.next_image_id;
                if let Some(renderer) = self.renderer.as_mut() {
                    // Width-wrapped blocks keep their source so they can be
                    // re-rasterized on resize; intrinsic-size ones (raster/SVG)
                    // do not need it.
                    // Images/SVG scale to fit the band; text shows at native
                    // size and clips. Width-wrapped kinds keep their source for
                    // re-rasterization on resize.
                    // The band is always the rows the grid actually reserved
                    // (`entry.reserved_rows`), never a constant re-picked here:
                    // any disagreement shows up as dead space between the block
                    // and the next prompt, or as a block overdrawing it.
                    let max_rows = entry.reserved_rows;
                    let (dims, reflow, fit_to_band) = match &source {
                        NativeImage::Markdown(md) => (
                            renderer.upload_markdown(id, md, pane_rect.width),
                            Some(ReflowSource::Markdown(md.clone())),
                            false,
                        ),
                        NativeImage::Raster(bytes) => {
                            (renderer.upload_image(id, bytes), None, true)
                        }
                        NativeImage::Svg(markup) => {
                            (renderer.upload_svg(id, markup.as_bytes()), None, true)
                        }
                        NativeImage::Text(text) => (
                            renderer.upload_text(id, text, pane_rect.width),
                            Some(ReflowSource::Text(text.clone())),
                            false,
                        ),
                    };
                    if let Some((nat_w, nat_h)) = dims {
                        self.next_image_id += 1;
                        self.image_blocks.push(ImageBlock {
                            block_index: entry.block_index,
                            closed: entry.closed,
                            fit_to_band,
                            abs_row: entry.abs_row,
                            id,
                            max_rows,
                            nat_h,
                            nat_w,
                            pane_id: *pane_id,
                            rastered_width: pane_rect.width.floor() as u32,
                            reflow,
                            segment_index: entry.segment_index,
                        });
                        if debug {
                            eprintln!("winter: image block id={id} {nat_w}x{nat_h}");
                        }
                        // Markdown and text reserve the default band, because
                        // their height depends on font metrics and wrap width
                        // that the PTY thread has no way to know. Now that the
                        // content is laid out the real height is known, so give
                        // back what it did not need (or take the rows it turned
                        // out to be short). Images and SVG skip this: their band
                        // was already reserved from their intrinsic size, and
                        // they scale to fit whatever it is.
                        //
                        // Only for one-shot blocks. A live block is re-measured
                        // on every patch, so fitting it here would make the rows
                        // below it jitter as its content streams in.
                        let one_shot = entry.kind == BlockKind::Content;
                        if one_shot && !fit_to_band {
                            let pos = self.image_blocks.len() - 1;
                            let content_h =
                                native_content_height(nat_w as f32, nat_h as f32, pane_rect.width);
                            self.set_band_rows(*pane_id, pos, band_fit_rows(content_h, ch));
                        }
                    } else if debug {
                        eprintln!("winter: image decode failed for block");
                    }
                }
                continue;
            }

            let html = {
                let theme = self.renderer.as_ref().expect("renderer present").theme();
                webview::render_block_html(
                    &entry.emit,
                    theme,
                    font_family.as_deref(),
                    font_size,
                    remote_assets,
                )
            };
            let params = webview::TileParams {
                abs_row: entry.abs_row,
                html,
                x: pane_rect.x as i32,
                y: pane_rect.y as i32,
                width: pane_rect.width as u32,
                height: webview::WebViewManager::block_pixel_height(ch),
            };
            match self
                .webview_mgr
                .create_block_tile(*pane_id, entry, params, &window)
            {
                Ok(()) if debug => eprintln!("winter: tile built ok"),
                Ok(()) => {}
                Err(e) => eprintln!("winter: block WebView error: {e}"),
            }
        }
        self.last_tile_layout = None;
    }

    /// Resize the band of `self.image_blocks[pos]` to `rows`, inserting or
    /// removing grid rows so the blank band behind a block matches the height
    /// its content actually needs.
    ///
    /// Everything anchored below the band follows: later blocks, WebView tiles,
    /// the pane's pending anchors and queued entries, and the shell's cursor.
    /// Growing is refused when the taller band would not fit on screen (the
    /// block is clipped instead, as before); shrinking always fits by
    /// construction.
    fn set_band_rows(&mut self, pane_id: PaneId, pos: usize, rows: usize) {
        let Some(block) = self.image_blocks.get(pos) else {
            return;
        };
        let (have, abs_row) = (block.max_rows, block.abs_row);
        if rows == have {
            return;
        }
        if rows > have {
            let add = rows - have;
            let at = abs_row + have;
            if !self
                .panes
                .get(&pane_id)
                .is_some_and(|p| band_has_room(p, at, add))
            {
                return;
            }
            if let Some(pane) = self.panes.get_mut(&pane_id) {
                pane.insert_band_rows(at, add);
            }
            self.shift_blocks_at_or_below(pane_id, at, add as isize);
        } else {
            let spare = have - rows;
            // The first row the block does not need; everything from there
            // down closes up against it.
            let at = abs_row + rows;
            if let Some(pane) = self.panes.get_mut(&pane_id) {
                pane.remove_band_rows(at, spare);
            }
            self.shift_blocks_at_or_below(pane_id, at, -(spare as isize));
        }
        self.image_blocks[pos].max_rows = rows;
        self.last_tile_layout = None;
    }

    /// Drop every block of `pane_id` whose reserved band overlaps the erased
    /// absolute row span `[start, end)`.
    ///
    /// A block draws an image over grid rows it does not own; erasing those
    /// rows (a `clear`, or any full-screen erase) blanks the grid but cannot
    /// reach the image, so without this the block stays on screen painted over
    /// whatever the shell writes next.
    pub(crate) fn drop_blocks_in(&mut self, pane_id: PaneId, (start, end): (usize, usize)) {
        self.retain_image_blocks(|block| {
            block.pane_id != pane_id
                || block.abs_row >= end
                || block.abs_row + block.max_rows <= start
        });
        self.webview_mgr.remove_tiles_in(pane_id, (start, end));
    }

    /// Drop the rendered blocks whose source segments the scrollback's
    /// retention budget elided: the content is gone, so the texture and the
    /// WebView tile can never show anything again, and an image block's
    /// decoded texture is megabytes that would otherwise never come back.
    pub(crate) fn drop_elided_blocks(&mut self, elided: &[(PaneId, usize, usize)]) {
        for &(pane_id, block_index, segment_index) in elided {
            self.retain_image_blocks(|block| {
                block.pane_id != pane_id
                    || block.block_index != block_index
                    || block.segment_index != segment_index
            });
            self.webview_mgr
                .remove_tile(pane_id, block_index, segment_index);
        }
    }

    /// Push every absolute-row anchor the app holds for `pane_id` through a
    /// resize reflow's row remap (see [`winter_render::RowRemap`]): image
    /// blocks, WebView tiles, the selection and its Visual anchor, the search
    /// cursor, vim marks and the jump/changelists. A resize rebuilds the live
    /// screen by replaying logical lines, so a line that re-wraps differently
    /// moves every row below it — an anchor left at its old row draws its
    /// block over the wrong content (the "block detached after resize"
    /// overlap).
    pub(crate) fn remap_pane_anchors(&mut self, pane_id: PaneId, remap: &winter_render::RowRemap) {
        let map = |row: usize| remap.map(row);
        for block in &mut self.image_blocks {
            if block.pane_id == pane_id {
                block.abs_row = map(block.abs_row);
            }
        }
        self.webview_mgr.remap_tiles(pane_id, remap);
        if let Some(span) = &mut self.selection.span {
            if span.pane == pane_id {
                span.start_row = map(span.start_row);
                span.end_row = map(span.end_row);
            }
        }
        // The Visual anchor belongs to the focused pane alone (it is held
        // only while that pane is in Visual mode), so it follows this remap
        // only when that is the pane being remapped.
        let focused = self.tabs.all[self.tabs.active].focused();
        if pane_id == focused {
            if let Some((row, _)) = &mut self.selection.visual_anchor {
                *row = map(*row);
            }
        }
        if let Some(last) = &mut self.selection.last_visual {
            if last.pane == pane_id {
                last.anchor.0 = map(last.anchor.0);
                last.cursor.0 = map(last.cursor.0);
            }
        }
        if let Some((search_pane, (row, _))) = &mut self.search.current {
            if *search_pane == pane_id {
                *row = map(*row);
            }
        }
        for ((mark_pane, _), (row, _)) in self.vim.marks.iter_mut() {
            if *mark_pane == pane_id {
                *row = map(*row);
            }
        }
        if let Some(list) = self.vim.jump_lists.get_mut(&pane_id) {
            list.remap_rows(&map);
        }
        if let Some(list) = self.vim.change_lists.get_mut(&pane_id) {
            list.remap_rows(&map);
        }
        self.last_tile_layout = None;
        self.dirty = true;
    }

    /// Keep only the image blocks matching `keep`, freeing the GPU texture of
    /// every dropped one.
    ///
    /// An `ImageBlock` entry and its texture have separate lifetimes: the
    /// entry names the block, the texture caches its decoded pixels under the
    /// renderer's id. Every removal path must go through here, or the texture
    /// outlives its block for the life of the process — a few megabytes each
    /// for real screenshots, with nothing left on screen to show for them.
    pub(crate) fn retain_image_blocks(&mut self, keep: impl Fn(&ImageBlock) -> bool) {
        let removed: Vec<ImageBlock> = self
            .image_blocks
            .extract_if(.., |block| !keep(block))
            .collect();
        if removed.is_empty() {
            return;
        }
        if let Some(renderer) = self.renderer.as_mut() {
            for block in &removed {
                renderer.free_image(block.id);
            }
        }
        self.last_tile_layout = None;
        self.dirty = true;
    }

    /// Move every block and WebView tile of `pane_id` anchored at or below
    /// absolute `row` by `delta` rows, after a band above them changed size.
    fn shift_blocks_at_or_below(&mut self, pane_id: PaneId, row: usize, delta: isize) {
        for block in &mut self.image_blocks {
            if block.pane_id == pane_id && block.abs_row >= row {
                block.abs_row = block.abs_row.saturating_add_signed(delta);
            }
        }
        self.webview_mgr
            .shift_tiles_at_or_below(pane_id, row, delta);
    }

    pub(crate) fn update_live_tiles(&mut self, patched: &[(PaneId, usize)]) {
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        let theme = renderer.theme().clone();
        let ch = renderer.cell_size().1;
        let font_family = self.config.font_family.clone();
        let font_size = self.config.font_size;
        let remote_assets = self.config.security.block_remote_assets;

        for (pane_id, entry_idx) in patched {
            let entry = match self.panes.get(pane_id) {
                Some(p) => p.block_queue().entries().get(*entry_idx).cloned(),
                None => None,
            };
            let Some(entry) = entry else {
                continue;
            };

            // Native-rendered live blocks (markdown/CSV/JSON/text, and raster
            // or SVG mimes): re-upload the patched content into the block's
            // existing texture and refresh its layout dims. Height changes
            // stay clipped to the reserved band, like resize reflows.
            if let Some(source) = native_image_source(&entry.emit) {
                let vp = self.viewport_rect();
                let layout_vp = Rect::new(vp.x, vp.y, vp.width, vp.height);
                let rects = self.tabs.all[self.tabs.active].rects(layout_vp);
                let Some((_, rect)) = rects.iter().find(|(id, _)| *id == *pane_id) else {
                    continue;
                };
                let pane_rect = Self::layout_rect_to_pane(*rect);
                let Some(pos) = self.image_blocks.iter().position(|b| {
                    b.pane_id == *pane_id
                        && b.block_index == entry.block_index
                        && b.segment_index == entry.segment_index
                }) else {
                    continue;
                };
                let Some(renderer) = self.renderer.as_mut() else {
                    continue;
                };
                let id = self.image_blocks[pos].id;
                let dims = match &source {
                    NativeImage::Markdown(md) => renderer.upload_markdown(id, md, pane_rect.width),
                    NativeImage::Raster(bytes) => renderer.upload_image(id, bytes),
                    NativeImage::Svg(markup) => renderer.upload_svg(id, markup.as_bytes()),
                    NativeImage::Text(text) => renderer.upload_text(id, text, pane_rect.width),
                };
                if let Some((nat_w, nat_h)) = dims {
                    let block = &mut self.image_blocks[pos];
                    block.closed = entry.closed;
                    block.nat_w = nat_w;
                    block.nat_h = nat_h;
                    block.rastered_width = pane_rect.width.floor() as u32;
                    block.reflow = match &source {
                        NativeImage::Markdown(md) => Some(ReflowSource::Markdown(md.clone())),
                        NativeImage::Text(text) => Some(ReflowSource::Text(text.clone())),
                        NativeImage::Raster(_) | NativeImage::Svg(_) => None,
                    };
                    // A patch can push the content past its reserved band, so
                    // grow to fit instead of clipping. Grow-only: a live
                    // block's height moves with every patch, and shrinking it
                    // back each time would make the rows below it jitter.
                    let want = band_fit_rows(nat_h as f32, ch).max(block.max_rows);
                    self.set_band_rows(*pane_id, pos, want);
                }
                continue;
            }

            let html = webview::render_block_html(
                &entry.emit,
                &theme,
                font_family.as_deref(),
                font_size,
                remote_assets,
            );
            if let Err(e) = self.webview_mgr.update_tile_html(*pane_id, &entry, &html) {
                eprintln!("winter: live-block update error: {e}");
            }
        }
    }

    /// Grow WebView tiles past their fixed default height to fit queued
    /// content-height reports. A tile that would need to exceed the pane's
    /// viewport is left as-is, still clipping its content internally.
    pub(crate) fn process_webview_height_reports(&mut self) {
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        let ch = renderer.cell_size().1;

        for report in self.webview_mgr.drain_height_reports() {
            let Some((abs_row, reserved_rows)) = self.webview_mgr.tile_band(
                report.pane_id,
                report.block_index,
                report.segment_index,
            ) else {
                continue;
            };
            // Grow-only, like a live block: a WebView re-reports its height on
            // every patch, and shrinking on each one would jitter the rows
            // below it.
            let want = band_fit_rows(report.height_px, ch).max(reserved_rows);
            let add = want - reserved_rows;
            if add == 0 {
                continue;
            }
            let insert_at = abs_row + reserved_rows;
            let has_room = self
                .panes
                .get(&report.pane_id)
                .is_some_and(|p| band_has_room(p, insert_at, add));
            if !has_room {
                continue;
            }
            if let Some(pane) = self.panes.get_mut(&report.pane_id) {
                pane.insert_band_rows(insert_at, add);
            }
            self.shift_blocks_at_or_below(report.pane_id, insert_at, add as isize);
            self.webview_mgr.resize_tile(
                report.pane_id,
                report.block_index,
                report.segment_index,
                want,
                ch,
            );
            self.last_tile_layout = None;
        }
    }
}

// ========================================================================
// Data Structures
// ========================================================================

/// The resolved theme colors the settings page paints with, bundled so the
/// row/value helpers can share one palette.
struct SettingsPalette {
    accent: Color,
    accent_fg: Color,
    fg: Color,
    muted: Color,
    selected_bg: Color,
}

/// A block representation the GPU can render directly, bypassing the WebView.
enum NativeImage {
    /// Markdown source, laid out and rasterized by the renderer.
    Markdown(String),
    /// Encoded raster bytes (PNG/JPEG/GIF/WebP) for the `image` decoder.
    Raster(Vec<u8>),
    /// SVG markup for the `resvg` rasterizer.
    Svg(String),
    /// Preformatted monospace text (a CSV table or pretty-printed JSON).
    Text(String),
}

// ========================================================================
// Functions
// ========================================================================

/// Paint the settings page into a fresh `cols` x `rows` grid: an elevated header
/// band, then titled sections of rows. Each row shows a label, a dim note, and a
/// right-aligned control; the selected row gets an accent bar and a highlight. A
/// key-hint footer sits along the bottom.
fn build_settings_grid(page: &SettingsPage, theme: &Theme, cols: usize, rows: usize) -> Grid {
    let mut grid = Grid::new(cols, rows);
    let pal = SettingsPalette {
        accent: theme_rgb(theme.cursor_bg),
        accent_fg: theme_rgb(theme.cursor_fg),
        fg: theme_rgb(theme.foreground),
        muted: mix_rgb(theme.foreground, theme.background, 0.45),
        selected_bg: theme_rgb(theme.menu_hover_bg),
    };
    let header_bg = theme_rgb(theme.menu_bg);
    let divider = theme_rgb(theme.divider);

    // Header band: a "Settings" title on an elevated strip, underlined by a rule.
    let band = Style {
        background: header_bg,
        ..Style::default()
    };
    put(&mut grid, 0, 0, &" ".repeat(cols), band);
    put(
        &mut grid,
        0,
        SETTINGS_LEFT_PAD,
        "Settings",
        Style {
            background: header_bg,
            bold: true,
            foreground: pal.fg,
            ..Style::default()
        },
    );
    put(
        &mut grid,
        1,
        0,
        &"─".repeat(cols),
        Style {
            foreground: divider,
            ..Style::default()
        },
    );

    // Body: sections of field rows. Stop before the footer's divider and hint.
    let body_end = rows.saturating_sub(2);
    let mut row = SETTINGS_FIRST_ROW;
    let mut section: Option<&str> = None;
    for (i, field) in page.fields.iter().enumerate() {
        if let Some(name) = field.section.as_deref() {
            if section != Some(name) {
                section = Some(name);
                row += 1; // spacer above the section header
                if row >= body_end {
                    break;
                }
                put(
                    &mut grid,
                    row,
                    SETTINGS_LEFT_PAD,
                    &name.to_uppercase(),
                    Style {
                        bold: true,
                        foreground: pal.accent,
                        ..Style::default()
                    },
                );
                row += 1;
            }
        }
        if row >= body_end {
            break;
        }
        draw_field_row(&mut grid, row, cols, field, i == page.selected, &pal);
        row += 1;
    }

    put(
        &mut grid,
        rows.saturating_sub(2),
        0,
        &"─".repeat(cols),
        Style {
            foreground: divider,
            ..Style::default()
        },
    );
    put(
        &mut grid,
        rows.saturating_sub(1),
        center_col(cols, SETTINGS_HINT.chars().count()),
        SETTINGS_HINT,
        Style {
            foreground: pal.muted,
            ..Style::default()
        },
    );
    grid
}

/// Paint one field row: an optional accent bar and highlight when selected, the
/// label, the right-aligned control, and the dim note between them.
fn draw_field_row(
    grid: &mut Grid,
    row: usize,
    cols: usize,
    field: &SettingsField,
    selected: bool,
    pal: &SettingsPalette,
) {
    let row_bg = if selected {
        pal.selected_bg
    } else {
        Color::Default
    };
    if selected {
        put(
            grid,
            row,
            0,
            &" ".repeat(cols),
            Style {
                background: pal.selected_bg,
                ..Style::default()
            },
        );
        // A left accent bar marks the focused row, like VSCode's focused setting.
        put(
            grid,
            row,
            0,
            "▌",
            Style {
                background: pal.selected_bg,
                foreground: pal.accent,
                ..Style::default()
            },
        );
    }
    put(
        grid,
        row,
        SETTINGS_LEFT_PAD,
        &field.label,
        Style {
            background: row_bg,
            foreground: pal.fg,
            ..Style::default()
        },
    );

    let value_col = draw_value(grid, row, cols, &field.control, selected, row_bg, pal);
    if let Some(note) = field.note.as_deref() {
        if SETTINGS_NOTE_COL + 1 < value_col {
            let budget = value_col - SETTINGS_NOTE_COL - 1;
            let text: String = note.chars().take(budget).collect();
            put(
                grid,
                row,
                SETTINGS_NOTE_COL,
                &text,
                Style {
                    background: row_bg,
                    foreground: pal.muted,
                    ..Style::default()
                },
            );
        }
    }
}

/// Draw a field's control, right-aligned to the margin, and return the column it
/// starts at so the caller can keep the note clear of it. Toggles render as an
/// `ON` pill or dim `OFF`; choices and numbers as `‹ value ›`; text inline with a
/// caret when focused.
fn draw_value(
    grid: &mut Grid,
    row: usize,
    cols: usize,
    control: &Control,
    selected: bool,
    row_bg: Color,
    pal: &SettingsPalette,
) -> usize {
    let on_value = Style {
        background: pal.accent,
        bold: true,
        foreground: pal.accent_fg,
        ..Style::default()
    };
    let muted = Style {
        background: row_bg,
        foreground: pal.muted,
        ..Style::default()
    };
    let accent = Style {
        background: row_bg,
        foreground: pal.accent,
        ..Style::default()
    };
    let segments: Vec<(String, Style)> = match control {
        Control::Toggle(t) if t.on => vec![(" ON ".to_string(), on_value)],
        Control::Toggle(_) => vec![(" OFF ".to_string(), muted)],
        Control::Choice(c) => {
            let label = c
                .options
                .get(c.index)
                .map(|o| o.label.as_str())
                .unwrap_or("");
            bracketed(label, muted, accent)
        }
        Control::Number(n) => {
            let value = format!("{:.*}", n.decimals, n.value);
            bracketed(&value, muted, accent)
        }
        Control::Text(t) => {
            let (text, style) = if t.value.is_empty() {
                ("default".to_string(), muted)
            } else {
                (
                    t.value.clone(),
                    Style {
                        background: row_bg,
                        foreground: pal.fg,
                        ..Style::default()
                    },
                )
            };
            let mut segments = vec![(text, style)];
            if selected {
                segments.push(("▏".to_string(), accent));
            }
            segments
        }
    };

    let width: usize = segments.iter().map(|(s, _)| s.chars().count()).sum();
    let start = cols.saturating_sub(SETTINGS_RIGHT_PAD + width);
    let mut col = start;
    for (text, style) in &segments {
        put(grid, row, col, text, *style);
        col += text.chars().count();
    }
    start
}

/// The `‹ value ›` segments for a choice or number, value in `accent` and the
/// guillemets in `muted`.
fn bracketed(value: &str, muted: Style, accent: Style) -> Vec<(String, Style)> {
    vec![
        ("‹ ".to_string(), muted),
        (value.to_string(), accent),
        (" ›".to_string(), muted),
    ]
}

/// Write `text` into `grid` starting at `(row, col)`, in `style`, truncated to
/// the grid width so it never wraps onto the next row.
fn put(grid: &mut Grid, row: usize, col: usize, text: &str, style: Style) {
    if col >= grid.cols() {
        return;
    }
    let budget = grid.cols() - col;
    grid.move_to(row, col);
    grid.set_style(style);
    for ch in text.chars().take(budget) {
        grid.print(ch);
    }
}

/// The starting column that centers `len` cells within `cols`.
fn center_col(cols: usize, len: usize) -> usize {
    cols.saturating_sub(len) / 2
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

/// Convert a theme color into an explicit grid cell color.
fn theme_rgb(c: ThemeRgb) -> Color {
    Color::Rgb(RgbColor {
        r: c.r,
        g: c.g,
        b: c.b,
    })
}

/// Blend `a` toward `b` by `t` in `[0, 1]`, e.g. to derive a muted text color
/// partway between the foreground and the background.
fn mix_rgb(a: ThemeRgb, b: ThemeRgb, t: f32) -> Color {
    let blend = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t).round() as u8;
    Color::Rgb(RgbColor {
        r: blend(a.r, b.r),
        g: blend(a.g, b.g),
        b: blend(a.b, b.b),
    })
}

/// The on-screen height of a natively-drawn text block (markdown, CSV, JSON)
/// rasterized at `nat_w` x `nat_h` into a pane `pane_w` wide.
///
/// Mirrors the non-`fit_to_band` branch of [`image_placements`]: the block is
/// drawn at its natural size, scaled down only if it is wider than the pane.
/// Both must agree, or the band reserved for a block and the pixels it draws
/// disagree by exactly that scale factor.
fn native_content_height(nat_w: f32, nat_h: f32, pane_w: f32) -> f32 {
    if nat_w <= 0.0 {
        return 0.0;
    }
    let w = nat_w.min(pane_w);
    nat_h * w / nat_w
}

/// Whole cell rows a block whose content rasterized to `content_h` pixels
/// should reserve: enough to show all of it, never less than one row, and
/// never more than [`MAX_IMAGE_ROWS`] so a runaway patch stream cannot eat the
/// whole screen.
fn band_fit_rows(content_h: f32, cell_height: f32) -> usize {
    if cell_height <= 0.0 || content_h <= 0.0 {
        return 1;
    }
    ((content_h / cell_height).ceil() as usize).clamp(1, MAX_IMAGE_ROWS)
}

/// The GPU-renderable source for a block's richest representation, or `None`
/// when it should render in the WebView (HTML, ...).
fn native_image_source(emit: &EmitBlock) -> Option<NativeImage> {
    let mime = webview::richest_mime(emit)?;
    let value = emit.bundle.get(mime)?;
    if RASTER_MIMES.contains(&mime) {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(value.as_str()?)
            .ok()?;
        Some(NativeImage::Raster(bytes))
    } else if mime == SVG_MIME {
        Some(NativeImage::Svg(value.as_str()?.to_string()))
    } else if mime == MARKDOWN_MIME {
        Some(NativeImage::Markdown(value.as_str()?.to_string()))
    } else if mime == CSV_MIME {
        Some(NativeImage::Text(csv_to_table(value.as_str()?)))
    } else if mime == JSON_MIME {
        Some(NativeImage::Text(json_to_text(value)))
    } else {
        None
    }
}

/// Format CSV rows into a column-aligned monospace table. Simple split on `,`;
/// quoted commas are not handled (acceptable for a preview).
fn csv_to_table(csv: &str) -> String {
    let rows: Vec<Vec<&str>> = csv
        .lines()
        .map(|line| line.split(',').map(str::trim).collect())
        .collect();
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0usize; columns];
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                out.push_str("  ");
            }
            out.push_str(cell);
            for _ in cell.chars().count()..widths[i] {
                out.push(' ');
            }
        }
        out.push('\n');
    }
    out
}

/// Pretty-print a JSON value. The bundle may carry it as a JSON string (from a
/// shell client) or as a structured value; both are normalized to pretty text.
fn json_to_text(value: &Value) -> String {
    let parsed = value
        .as_str()
        .and_then(|s| serde_json::from_str::<Value>(s).ok());
    let target = parsed.as_ref().unwrap_or(value);
    serde_json::to_string_pretty(target).unwrap_or_else(|_| value.to_string())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_band_fit_rows_covers_the_content_within_the_image_row_cap() {
        // With a 10px cell: 130px of content needs 13 whole rows, a partial row
        // still costs a whole one, and a runaway stream is capped at the image
        // row limit rather than eating the screen.
        assert_eq!(band_fit_rows(130.0, 10.0), 13);
        assert_eq!(band_fit_rows(131.0, 10.0), 14);
        assert_eq!(band_fit_rows(100.0, 10.0), 10);
        assert_eq!(band_fit_rows(5000.0, 10.0), MAX_IMAGE_ROWS);
        // A block always occupies at least one row, however small or unmeasured
        // its content: a zero-row band would put the next prompt on top of it.
        assert_eq!(band_fit_rows(1.0, 10.0), 1);
        assert_eq!(band_fit_rows(0.0, 10.0), 1);
        assert_eq!(band_fit_rows(100.0, 0.0), 1);
    }

    #[test]
    fn test_cursor_line_row_is_kept_for_unfocused_panes() {
        // The band follows the pane's mode and cursor only, no focus term, so a
        // pane you navigated stays banded after you switch to another pane.
        assert_eq!(cursor_line_row(Mode::Normal, Some((4, 2))), Some(4));
        assert_eq!(cursor_line_row(Mode::Visual, Some((0, 0))), Some(0));
        // Insert (or a pane that was never navigated) shows no band.
        assert_eq!(cursor_line_row(Mode::Insert, Some((4, 2))), None);
        assert_eq!(cursor_line_row(Mode::BlockFocus, Some((4, 2))), None);
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
    fn test_clip_block_band_crops_the_top_instead_of_dropping_a_half_scrolled_block() {
        // Regression: a band whose first row scrolls above the pane used to be
        // skipped outright, so a tall block popped out of existence the moment
        // its top line left the viewport instead of sliding off it. Starting
        // 20px above a 100px-tall image means the top fifth is cropped and the
        // rest is drawn flush against the pane's top edge.
        let clip = clip_block_band(-20.0, 100.0, 100.0, 50.0, 400.0).expect("still visible");
        assert_eq!(clip.y, 50.0, "drawn flush with the pane's top edge");
        assert_eq!(clip.height, 80.0);
        assert_eq!(clip.v_min, 0.2);
        assert_eq!(clip.v_max, 1.0);
    }

    #[test]
    fn test_clip_block_band_crops_the_bottom_at_the_pane_edge() {
        // A band starting 30px from the bottom of a 100px-tall pane shows only
        // its first 30px, cropped rather than squashed.
        let clip = clip_block_band(70.0, 100.0, 100.0, 0.0, 100.0).expect("still visible");
        assert_eq!(clip.y, 70.0);
        assert_eq!(clip.height, 30.0);
        assert_eq!(clip.v_min, 0.0);
        assert_eq!(clip.v_max, 0.3);
    }

    #[test]
    fn test_clip_block_band_crops_content_taller_than_its_reserved_band() {
        // Text/markdown draws at native size: the part past the reserved band
        // is cropped so the prompt below it is never overdrawn.
        let clip = clip_block_band(0.0, 200.0, 50.0, 0.0, 400.0).expect("visible");
        assert_eq!(clip.height, 50.0);
        assert_eq!(clip.v_max, 0.25);
    }

    #[test]
    fn test_clip_block_band_drops_a_block_fully_outside_the_pane() {
        // Scrolled entirely above, and entirely below, the pane.
        assert!(clip_block_band(-100.0, 100.0, 100.0, 0.0, 400.0).is_none());
        assert!(clip_block_band(400.0, 100.0, 100.0, 0.0, 400.0).is_none());
        // A zero-height image has nothing to draw and no valid `v` range.
        assert!(clip_block_band(0.0, 0.0, 100.0, 0.0, 400.0).is_none());
    }
    #[test]
    fn test_block_top_padding_comes_out_of_the_band_not_out_of_the_prompt_below() {
        // The image is inset below its band's first row, and gives up that
        // much of the band's height rather than growing past it: the bottom
        // edge must still land inside the reservation, or the padding would
        // reappear as a gap under every block instead of above it.
        let cell_height = 20.0;
        let band_rows = 12.0;
        let pad = cell_height * BLOCK_PAD_TOP_RATIO;
        let band_top = pad;
        let band_h = band_rows * cell_height - pad;
        assert!(pad > 0.0, "there is real padding to check");

        let clip = clip_block_band(band_top, band_h, band_h, 0.0, 1000.0).expect("visible");
        assert_eq!(clip.y, pad, "the image starts below the band's first row");
        assert_eq!(
            clip.y + clip.height,
            band_rows * cell_height,
            "and still ends inside the reserved band"
        );
        assert_eq!(clip.v_min, 0.0, "nothing is cropped by the padding itself");
        assert_eq!(clip.v_max, 1.0);
    }

    /// An `ImageBlock` for `pane` anchored at `abs_row`, spanning `max_rows`
    /// rows of source segment `block_index`/`segment_index`, with a rasterized
    /// size that makes it placeable.
    fn image_block(
        pane: PaneId,
        abs_row: usize,
        block_index: usize,
        segment_index: usize,
    ) -> ImageBlock {
        ImageBlock {
            abs_row,
            block_index,
            closed: false,
            fit_to_band: true,
            id: block_index as u64,
            max_rows: 4,
            nat_h: 40,
            nat_w: 80,
            pane_id: pane,
            rastered_width: 400,
            reflow: None,
            segment_index,
        }
    }

    /// An app with one live `cat` pane at `focused`, plus one image block per
    /// `(abs_row, block_index, segment_index)` anchor, laid out over a
    /// 400x300 rect. Building the blocks inside sidesteps the chicken-and-egg
    /// of needing the pane id before the app exists.
    fn app_with_image_blocks(
        anchors: &[(usize, usize, usize)],
    ) -> (App, PaneId, Vec<(PaneId, Rect)>) {
        let mut app = App::new();
        let id = app.tab().panes()[0];
        let pane = crate::terminal::pane::Pane::with_command(
            40,
            8,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        app.panes.insert(id, pane);
        app.image_blocks = anchors
            .iter()
            .map(|&(abs_row, block_index, segment_index)| {
                image_block(id, abs_row, block_index, segment_index)
            })
            .collect();
        let rects = vec![(id, Rect::new(0.0, 0.0, 400.0, 300.0))];
        (app, id, rects)
    }

    #[test]
    fn test_image_placements_skip_a_pane_on_the_alternate_screen() {
        // Regression: a block anchored in the primary screen kept drawing on
        // top of vim/less/htop once the pane entered the alternate screen,
        // hiding the rows the full-screen app had drawn there.
        let (mut app, pane_id, rects) = app_with_image_blocks(&[(1, 0, 1)]);

        let placements = image_placements(&app.image_blocks, &app.panes, &rects, 20.0);
        assert_eq!(placements.len(), 1, "fixture: the block places normally");

        app.panes
            .get_mut(&pane_id)
            .unwrap()
            .grid_mut()
            .enter_alt_screen();
        let placements = image_placements(&app.image_blocks, &app.panes, &rects, 20.0);
        assert!(
            placements.is_empty(),
            "an alt-screen pane must not have primary-screen blocks painted over it"
        );

        app.panes
            .get_mut(&pane_id)
            .unwrap()
            .grid_mut()
            .leave_alt_screen();
        let placements = image_placements(&app.image_blocks, &app.panes, &rects, 20.0);
        assert_eq!(
            placements.len(),
            1,
            "leaving the alt screen restores the block"
        );
    }

    #[test]
    fn test_drop_elided_blocks_drops_only_the_elided_segment() {
        // Regression: an elided block's ImageBlock entry (and its GPU texture)
        // stayed forever, because nothing connected the scrollback's retention
        // budget to the rendered block.
        let (mut app, pane_id, _rects) = app_with_image_blocks(&[(2, 0, 1), (9, 1, 1)]);

        app.drop_elided_blocks(&[(pane_id, 0, 1)]);

        assert_eq!(app.image_blocks.len(), 1);
        assert_eq!(app.image_blocks[0].block_index, 1, "the live block stays");
        assert!(app.dirty, "dropping a rendered block must request a redraw");
    }

    #[test]
    fn test_drop_blocks_in_keeps_blocks_outside_the_erased_span() {
        let (mut app, pane_id, _rects) = app_with_image_blocks(&[(2, 0, 1), (10, 1, 1)]);

        // Erase rows 0..5: only the band at row 2 overlaps.
        app.drop_blocks_in(pane_id, (0, 5));

        assert_eq!(app.image_blocks.len(), 1);
        assert_eq!(app.image_blocks[0].abs_row, 10);
    }

    #[test]
    fn test_pane_resize_remaps_image_block_anchors() {
        // Regression (#4, "blocks detach on a resize that re-wraps"): a
        // 30-char line above the block re-wrapped from 2 rows to 3 when the
        // grid narrowed, but the anchor kept row 2 and the image drew over
        // the re-wrapped line's last row. The reflow's row remap must carry
        // the anchor down to the band's own first blank row.
        let mut app = App::new();
        let id = app.tab().panes()[0];
        let mut pane = crate::terminal::pane::Pane::with_command(
            20,
            8,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        {
            let grid = pane.grid_mut();
            let long = "abcdefghijklmnopqrstuvwxyz0123"; // 30 chars: rows 0-1
            assert_eq!(long.chars().count(), 30);
            for ch in long.chars() {
                grid.print(ch);
            }
            grid.carriage_return();
            grid.line_feed(); // the band's first row: 2
            for _ in 0..3 {
                grid.line_feed(); // blank band rows 2-4
            }
            for ch in "tail".chars() {
                grid.print(ch); // row 5
            }
        }
        app.panes.insert(id, pane);
        app.image_blocks.push(image_block(id, 2, 0, 1));

        let pane = app.panes.get_mut(&id).unwrap();
        pane.resize(10, 8); // the 30 chars now wrap rows 0-2
        for remap in pane.take_row_remaps() {
            app.remap_pane_anchors(id, &remap);
        }

        let grid = app.panes[&id].grid();
        assert_eq!(
            app.image_blocks[0].abs_row,
            grid.to_absolute_row(3),
            "the anchor must follow the band below the re-wrapped line"
        );
        assert!(app.dirty, "a remapped anchor must request a redraw");
    }
}
