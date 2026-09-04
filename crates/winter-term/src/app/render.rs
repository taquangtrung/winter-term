//! Frame composition and WebView tile management.

use base64::Engine;
use serde_json::Value;

use crate::model::layout::{PaneId, Rect};
use crate::model::mode::Mode;
use crate::model::settings_page::{Control, SettingsField, SettingsPage};
use crate::terminal::block_queue::BlockEntry;
use crate::terminal::pane::{BLOCK_RESERVE_ROWS, MAX_IMAGE_ROWS};
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

/// Opacity a closed live block's image placement draws at, so a reader can
/// tell a finished block from one still accepting patches.
const CLOSED_BLOCK_ALPHA: f32 = 0.5;

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
        // it's configured hidden — it's the only place search feedback (query
        // text, match position) is shown, so there'd otherwise be nowhere to
        // put it. Reverts to the configured visibility as soon as the search
        // ends (`search_query` back to `None`). Shared with `viewport_rect`/
        // `resize_all_panes` via `status_bar_visible` so pane geometry and the
        // PTY's row count always agree with what's drawn here.
        let status_enabled = self.status_bar_visible();
        // When the status bar is (really) hidden it can't surface the notice,
        // so float it as a bottom-center toast instead (avoids showing it in
        // both places).
        let toast = if status_enabled { None } else { notice.clone() };
        // Built before the renderer is borrowed, since it reads tab/menu state.
        let tabbar = self.build_top_tabbar();
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let (full_cols, full_rows) = renderer.grid_size();
        let (cw, ch) = renderer.cell_size();

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
        let layout_vp = Rect::new(
            0.0,
            top_h_on_screen + top_pad,
            w,
            (rows as f32 * ch).max(1.0),
        );
        let content_rows = if ch > 0.0 {
            (layout_vp.height / ch).floor() as usize
        } else {
            1
        };

        let rects = self.tabs[self.active_tab].rects(layout_vp);
        let sel = self.selection.as_ref().map(|s| {
            (
                s.pane,
                s.start_row,
                s.start_col,
                s.end_row,
                s.end_col,
                s.block,
            )
        });

        let focused = self.tabs[self.active_tab].focused();
        let mode = self.modes.get(&focused).copied().unwrap_or_default();
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
        let query_str = self.search_query.as_deref().filter(|q| !q.is_empty());
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
                |(id, _)| match (query_str, self.search_current, self.panes.get(id)) {
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
            let theme = renderer.theme();
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

        let (cx, cy) = self.cursor_pos;
        let hovered_pane: Option<PaneId> = rects
            .iter()
            .find(|(_, r)| {
                let pr = Self::layout_rect_to_pane(*r);
                cx >= pr.x && cx < pr.x + pr.width && cy >= pr.y && cy < pr.y + pr.height
            })
            .map(|(id, _)| *id);

        let mut views: Vec<PaneView> = Vec::new();
        for (i, (id, rect)) in rects.iter().enumerate() {
            if let Some(pane) = self.panes.get(id) {
                let (sel_tuple, sel_block) = match sel {
                    Some((pid, sr, sc, er, ec, block)) if pid == *id => {
                        (Some((sr, sc, er, ec)), block)
                    }
                    _ => (None, false),
                };
                let labels = if *id == focused && !qs_labels.is_empty() {
                    Some(qs_labels.as_slice())
                } else {
                    None
                };
                // Each pane's cursor follows its own mode (stored per pane), so a
                // non-focused pane shows the configured shape for its mode rather
                // than always reverting to a stale Block the shell reported via
                // DECSCUSR.
                let pane_mode = self.modes.get(id).copied().unwrap_or_default();
                let nav_cursor =
                    if *id == focused && matches!(pane_mode, Mode::Normal | Mode::Visual) {
                        // Direct field access (not the method) so only `nav_cursors`
                        // is borrowed, avoiding a conflict with the live `pane` borrow.
                        self.nav_cursors.get(id).copied()
                    } else {
                        None
                    };
                // The cursor-line band is not focus-gated: a pane left in Normal
                // mode keeps showing where its cursor is, so switching panes (and
                // back) doesn't lose your place. Only the cursor block itself is
                // drawn for the focused pane alone.
                let cursor_line_row = cursor_line_row(pane_mode, self.nav_cursors.get(id).copied());
                let config_shape = match pane_mode {
                    Mode::Insert => self.config.cursor.insert,
                    Mode::Normal => self.config.cursor.normal,
                    Mode::Visual => self.config.cursor.visual,
                    Mode::BlockFocus => self.config.cursor.block_focus,
                };
                let cursor_shape = effective_cursor_shape(
                    !pane.is_at_prompt(),
                    pane.grid().reported_cursor_shape(),
                    config_shape,
                );
                let hovered_link = if hovered_pane == Some(*id) {
                    self.hovered_url
                        .as_deref()
                        .map(|url| pane.grid().find_link_id(url))
                        .unwrap_or(0)
                } else {
                    0
                };
                let is_focused = *id == focused;
                let cursor_unfocused = is_focused && !self.window_focused;
                let cursor_visible = if self.palette.is_some() || !pane.grid().cursor_visible() {
                    // The palette steals focus, and DECTCEM (CSI ?25l) lets a
                    // full-screen app like btop hide the cursor outright.
                    false
                } else if cursor_unfocused {
                    // An unfocused cursor marks "not receiving keystrokes"; blinking
                    // it would fight that signal by making it disappear half the
                    // time.
                    true
                } else if is_focused {
                    !self.config.cursor.blink || self.blink_phase
                } else {
                    !self.config.cursor.hide_in_inactive
                };
                views.push(PaneView {
                    bracket_colors: &bracket_color_data[i],
                    cursor_shape,
                    cursor_unfocused,
                    cursor_visible,
                    dim: !is_focused && self.config.dim_inactive,
                    focused: is_focused,
                    grid: pane.grid(),
                    hovered_link,
                    labels,
                    find_labels: if *id == focused {
                        &find_label_data
                    } else {
                        &[]
                    },
                    nav_cursor,
                    cursor_line_row,
                    // The Normal-mode cursor blinks on the same timer as the
                    // shell's, so it's as easy to spot while navigating — but
                    // holds steady, in its unfocused form, while the window
                    // lacks focus.
                    nav_cursor_visible: cursor_unfocused
                        || !self.config.cursor.blink
                        || self.blink_phase,
                    rect: Self::layout_rect_to_pane(*rect),
                    scroll_offset: pane.grid().scroll_offset(),
                    scrollback_len: pane.grid().scrollback_len(),
                    search_matches: &search_match_data[i],
                    search_current: &search_current_data[i],
                    sentence_spans: &sentence_span_data[i],
                    selection: sel_tuple,
                    selection_block: sel_block,
                    url_underline: self.config.url_underline,
                });
            }
        }

        // Re-rasterize width-wrapped blocks (markdown/CSV/JSON) whose pane width
        // changed since they were last rendered, so wrapping stays correct on
        // resize. Intrinsic-size blocks (raster/SVG) have `reflow == None`.
        for i in 0..self.image_blocks.len() {
            let Some((_, rect)) = rects
                .iter()
                .find(|(id, _)| *id == self.image_blocks[i].pane_id)
            else {
                continue;
            };
            let target_w = Self::layout_rect_to_pane(*rect).width;
            let target = target_w.floor() as u32;
            if self.image_blocks[i].rastered_width == target
                || self.image_blocks[i].reflow.is_none()
            {
                continue;
            }
            let id = self.image_blocks[i].id;
            let dims = match self.image_blocks[i].reflow.as_ref() {
                Some(ReflowSource::Markdown(md)) => {
                    renderer.upload_markdown(id, &md.clone(), target_w)
                }
                Some(ReflowSource::Text(text)) => renderer.upload_text(id, &text.clone(), target_w),
                None => None,
            };
            if let Some((nat_w, nat_h)) = dims {
                let block = &mut self.image_blocks[i];
                block.nat_w = nat_w;
                block.nat_h = nat_h;
                block.rastered_width = target;
            }
        }

        // Place native image blocks at their grid row (scaled to fit the pane
        // width, preserving aspect), skipping any scrolled off the content area.
        let mut placements: Vec<ImagePlacement> = Vec::new();
        for img in &self.image_blocks {
            let Some((_, rect)) = rects.iter().find(|(id, _)| *id == img.pane_id) else {
                continue;
            };
            let pane_rect = Self::layout_rect_to_pane(*rect);
            let scroll_offset = self
                .panes
                .get(&img.pane_id)
                .map(|p| p.grid().scroll_offset())
                .unwrap_or(0);
            let visible_row = img.grid_row as isize - scroll_offset as isize;
            if visible_row < 0 || visible_row as usize >= content_rows {
                continue;
            }
            let nat_w = img.nat_w as f32;
            let nat_h = img.nat_h as f32;
            let band_h = img.max_rows as f32 * ch;
            let (display_w, display_h) = if nat_w <= 0.0 || nat_h <= 0.0 {
                (0.0, 0.0)
            } else if img.fit_to_band {
                // Images/SVG: scale down to fit the reserved band.
                let scale = (pane_rect.width / nat_w).min(band_h / nat_h).min(1.0);
                (nat_w * scale, nat_h * scale)
            } else {
                // Text/markdown: native size (wrapped to pane width).
                let w = nat_w.min(pane_rect.width);
                (w, nat_h * w / nat_w)
            };
            // Clip the bottom to the band and the content area (above the status
            // bar) so a tall block never overruns either.
            let y = pane_rect.y + visible_row as f32 * ch;
            let available = (pane_rect.y + pane_rect.height - y).max(0.0);
            let limit = band_h.min(available);
            let (height, v_max) = if display_h > limit && display_h > 0.0 {
                (limit, limit / display_h)
            } else {
                (display_h, 1.0)
            };
            placements.push(ImagePlacement {
                alpha: if img.closed { CLOSED_BLOCK_ALPHA } else { 1.0 },
                height,
                id: img.id,
                v_max,
                width: display_w,
                x: pane_rect.x,
                y,
            });
        }

        let search = self.search_query.as_ref().map(|q| StatusSearch {
            query: q.clone(),
            match_index: self.search_match_index,
            match_total: self.search_match_total,
            reverse: self.search_reverse,
        });
        let status = status_bar(
            mode,
            renderer.theme(),
            search,
            notice,
            &self.config.status_bar,
        );
        let status = status_enabled.then_some(&status);
        let palette_view = self.palette.as_ref().map(|p| PaletteView {
            empty_message: if p.mode == crate::model::palette::PaletteMode::History {
                "No matching history".to_string()
            } else if p.mode == crate::model::palette::PaletteMode::RecentDirs {
                "No recent directories".to_string()
            } else if p.mode == crate::model::palette::PaletteMode::Panes {
                "No matching panes".to_string()
            } else if p.mode == crate::model::palette::PaletteMode::Swoop {
                "No matching lines".to_string()
            } else {
                "No matching commands".to_string()
            },
            items: p
                .filtered
                .iter()
                .map(|&i| PaletteItem {
                    action: p.entries[i].action.clone(),
                    label: p.entries[i].label.clone(),
                    match_positions: p.entries[i].match_positions.clone(),
                    shortcut: p.entries[i].shortcut.clone(),
                })
                .collect(),
            match_underline: self.config.palette_match_underline,
            query: p.query.clone(),
            selected: p.selected,
        });
        let which_key_view = self
            .pending_since
            .filter(|since| since.elapsed() >= std::time::Duration::from_millis(1000))
            .and_then(|_| {
                self.pending
                    .hint()
                    .map(|(title, items)| winter_render::WhichKeyView {
                        items: items
                            .iter()
                            .map(|(k, v)| (k.to_string(), v.to_string()))
                            .collect(),
                        title: title.to_string(),
                    })
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

        let focused = self.tabs[self.active_tab].focused();
        // Tiles for panes outside the active tab are hidden so background tabs
        // don't show through; the active tab's tiles are positioned by scroll.
        let active_panes: std::collections::HashSet<PaneId> =
            self.tabs[self.active_tab].panes().into_iter().collect();
        if let Some(pane) = self.panes.get(&focused) {
            let scroll_offset = pane.grid().scroll_offset();
            let (_, ch) = renderer.cell_size();
            let focused_rect = rects.iter().find(|(id, _)| *id == focused);
            let pane_y = focused_rect.map(|(_, r)| r.y).unwrap_or(0.0);

            // Repositioning every tile does a GTK round-trip per WebView; only do
            // it when the scroll position or layout actually changed, otherwise
            // plain typing (which never moves tiles) stalls on GTK IPC.
            let layout = (scroll_offset, full_rows, ch.to_bits(), pane_y.to_bits());
            if self.last_tile_layout != Some(layout) {
                self.last_tile_layout = Some(layout);
                self.webview_mgr.reposition_tiles(
                    scroll_offset,
                    full_rows,
                    ch,
                    pane_y,
                    &active_panes,
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
        let rects = self.tabs[self.active_tab].rects(layout_vp);
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
                    let (dims, reflow, fit_to_band, max_rows) = match &source {
                        NativeImage::Markdown(md) => (
                            renderer.upload_markdown(id, md, pane_rect.width),
                            Some(ReflowSource::Markdown(md.clone())),
                            false,
                            BLOCK_RESERVE_ROWS,
                        ),
                        NativeImage::Raster(bytes) => {
                            (renderer.upload_image(id, bytes), None, true, MAX_IMAGE_ROWS)
                        }
                        NativeImage::Svg(markup) => (
                            renderer.upload_svg(id, markup.as_bytes()),
                            None,
                            true,
                            BLOCK_RESERVE_ROWS,
                        ),
                        NativeImage::Text(text) => (
                            renderer.upload_text(id, text, pane_rect.width),
                            Some(ReflowSource::Text(text.clone())),
                            false,
                            BLOCK_RESERVE_ROWS,
                        ),
                    };
                    if let Some((nat_w, nat_h)) = dims {
                        self.next_image_id += 1;
                        self.image_blocks.push(ImageBlock {
                            block_index: entry.block_index,
                            closed: entry.closed,
                            fit_to_band,
                            grid_row: entry.grid_row,
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
                grid_row: entry.grid_row,
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
                let rects = self.tabs[self.active_tab].rects(layout_vp);
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
                    let (add, insert_at) = {
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
                        let growth = band_growth_rows(nat_h as f32, ch, block.max_rows);
                        (growth, block.grid_row + block.max_rows)
                    };
                    // A patch can grow the content past its reserved band:
                    // insert the missing rows — shifting the grid rows, later
                    // anchors, and tiles below — instead of clipping. Only
                    // while the grown band still fits on screen; larger bands
                    // keep the clip-to-available behavior.
                    let grid_rows = self
                        .panes
                        .get(pane_id)
                        .map(|p| p.grid().rows())
                        .unwrap_or(0);
                    if add > 0 && insert_at + add <= grid_rows {
                        self.image_blocks[pos].max_rows += add;
                        if let Some(pane) = self.panes.get_mut(pane_id) {
                            pane.insert_band_rows(insert_at, add);
                        }
                        for other in &mut self.image_blocks {
                            if other.pane_id == *pane_id && other.grid_row >= insert_at {
                                other.grid_row += add;
                            }
                        }
                        self.webview_mgr
                            .shift_tiles_at_or_below(*pane_id, insert_at, add);
                        self.last_tile_layout = None;
                    }
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
            let Some((grid_row, reserved_rows)) = self.webview_mgr.tile_band(
                report.pane_id,
                report.block_index,
                report.segment_index,
            ) else {
                continue;
            };
            let add = band_growth_rows(report.height_px, ch, reserved_rows);
            if add == 0 {
                continue;
            }
            let insert_at = grid_row + reserved_rows;
            let grid_rows = self
                .panes
                .get(&report.pane_id)
                .map(|p| p.grid().rows())
                .unwrap_or(0);
            if insert_at + add > grid_rows {
                continue;
            }
            if let Some(pane) = self.panes.get_mut(&report.pane_id) {
                pane.insert_band_rows(insert_at, add);
            }
            for block in &mut self.image_blocks {
                if block.pane_id == report.pane_id && block.grid_row >= insert_at {
                    block.grid_row += add;
                }
            }
            self.webview_mgr
                .shift_tiles_at_or_below(report.pane_id, insert_at, add);
            self.webview_mgr.resize_tile(
                report.pane_id,
                report.block_index,
                report.segment_index,
                reserved_rows + add,
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
/// (`\e[2 q`) when they redraw on a resize — e.g. after a pane is split or
/// closed — which would otherwise leak a stale Block into Insert mode and
/// clobber the user's configured Bar.
///
/// Trusting the report as soon as the alt screen is entered (rather than
/// waiting for a Bar to prove the program signals modality) matters because a
/// full-screen app's very first frame is already meaningful: vim/nvim opens
/// straight into Normal mode and reports Block immediately, before the user
/// ever presses `i` — waiting for a Bar sighting would show the host's Insert
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
/// Deliberately takes no notion of focus — a pane left in Normal or Visual mode
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

/// The GPU-renderable source for a block's richest representation, or `None`
/// when it should render in the WebView (HTML, ...).
/// Extra rows a patched block needs beyond its reserved band: the content's
/// rastered height in whole cell rows minus what is already reserved, capped
/// so a growing block can never reserve more rows than the image cap — a
/// runaway patch stream must not eat the whole screen.
fn band_growth_rows(nat_h: f32, cell_height: f32, reserved: usize) -> usize {
    if cell_height <= 0.0 || nat_h <= 0.0 {
        return 0;
    }
    let needed = (nat_h / cell_height).ceil() as usize;
    needed
        .saturating_sub(reserved)
        .min(MAX_IMAGE_ROWS.saturating_sub(reserved))
}

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
    fn test_band_growth_rows_returns_only_the_overflow_capped_at_the_image_rows() {
        // With a 10px cell: 130px content over a 12-row band needs exactly 1
        // more row; a runaway stream is capped at the image row limit instead
        // of eating the screen; content within the band asks for nothing.
        assert_eq!(band_growth_rows(130.0, 10.0, 12), 1);
        assert_eq!(band_growth_rows(100.0, 10.0, 12), 0);
        assert_eq!(band_growth_rows(500.0, 10.0, 12), MAX_IMAGE_ROWS - 12);
        assert_eq!(band_growth_rows(300.0, 10.0, MAX_IMAGE_ROWS), 0);
        assert_eq!(band_growth_rows(0.0, 10.0, 12), 0);
    }

    #[test]
    fn test_cursor_line_row_is_kept_for_unfocused_panes() {
        // The band follows the pane's mode and cursor only — no focus term — so a
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
}
