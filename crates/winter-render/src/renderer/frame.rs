//! Drawing one frame: composing every pass into the surface texture.
//!
//! [`GpuRenderer::render`] is the orchestrator; each stage of the frame lives
//! in its own method below, grouped as per-pane composition, chrome, GPU
//! upload, and encoding.

use super::background::{
    build_bg_vertices_offset, cursor_outline_quads, cursor_quad, ensure_vertex_capacity,
    quad_vertices, separator_vertices, separators, BgParams, BgVertex, DotVertex, LineFlash,
    SeparatorStyle,
    BG_BUFFER_LABEL, CURSOR_HOLLOW_STROKE_WIDTH, DOT_BUFFER_LABEL,
};
use super::colors::{
    block_cursor_cell, cell_text_fg, clear_color, cursor_contrast_fg, mix_rgb,
    needs_dark_on_light_bold, FAINT_FACTOR,
};
use super::display::shown_cell;
use super::glyphs::{
    base_family, effective_bold_weight, is_braille, needs_complex_shaping, parse_weight,
    FallbackGlyphQuadFit,
};
use super::parts::status::status_metrics;
use super::raster::{view::TabbarText, DIM_FACTOR};
use super::widgets::{
    CompletionView, DropListView, FindView, InfoView, InputView, PickerView, TokenInspectView, ToolbarView, WhichKeyView,
};
use super::GpuRenderer;
use super::{PaneRect, PaneView, StatusBar, ToastCard};
use crate::glyph_quad::GlyphQuadPlacement;
use crate::image::ImagePlacement;
use crate::tabbar::TopTabbar;
use crate::theme::Rgb;
use glyphon::{Attrs, BufferLine, Color, Shaping, TextArea, TextBounds};
use wgpu::{
    Buffer, LoadOp, RenderPass, RenderPassColorAttachment, RenderPassDescriptor, RenderPipeline,
    StoreOp,
};
use winter_core::{Cell, CellWidth, Color as GridColor};

/// Glyph color over the current search match's tint.
const CURRENT_MATCH_FG: (u8, u8, u8) = (255, 255, 255);

/// Glyph color over every other search match's tint.
const SEARCH_MATCH_FG: (u8, u8, u8) = (0, 0, 0);

// ========================================================================
// Data Structures
// ========================================================================

/// Vertex and quad geometry accumulated across every pane while composing one
/// frame, before any of it is uploaded to the GPU.
#[derive(Default)]
struct FrameLayers {
    bg_verts: Vec<BgVertex>,
    dot_verts: Vec<DotVertex>,
    glyph_quads: Vec<GlyphQuadPlacement>,
    /// Quads drawn after the images in the content and before those that
    /// float over it: the separators between panes, which sit on the panes'
    /// strips of tabs, which are images, and under a tooltip or a menu.
    over_image_verts: Vec<BgVertex>,
    /// Quads drawn over the text and glyph quads but under the overlays:
    /// marks that must not be covered by a glyph's descender.
    over_text_verts: Vec<BgVertex>,
}

/// Font attributes shared by every row of every pane in one frame. Owned
/// rather than borrowed from the renderer because building a row's attrs
/// interleaves with `glyph_correction_font_size`, which needs `&mut self`.
struct FrameTextStyle {
    family: Option<String>,
    /// `None` when ligatures are enabled, so lines shape normally.
    ligatures_off: Option<glyphon::cosmic_text::FontFeatures>,
}

/// The whole cell columns and rows that fit inside one pane's rect.
#[derive(Clone, Copy)]
struct PaneExtent {
    cols: usize,
    rows: usize,
}

/// One grid row ready to hand to cosmic-text: its text and the attribute spans
/// over that text.
struct PaneRow {
    attrs: glyphon::AttrsList,
    text: String,
}

/// The per-cell overrides that sit on top of a cell's own SGR style.
#[derive(Clone, Copy)]
struct CellStyleContext {
    /// Rainbow-parens color for this cell's bracket glyph, if any.
    bracket_rgb: Option<(u8, u8, u8)>,
    /// Contrasting color forced onto the glyph when the block cursor would
    /// otherwise swallow it.
    cursor_text_fg: Option<(u8, u8, u8)>,
    /// Blend the resolved color toward the background (an inactive pane).
    dim: bool,
}

/// One cell's fallback glyph and everything needed to place its own quad.
struct FallbackGlyphQuad<'a> {
    cell: Option<&'a Cell>,
    col: usize,
    /// Contrasting color forced onto the glyph when the block cursor would
    /// otherwise swallow it.
    cursor_text_fg: Option<(u8, u8, u8)>,
    /// Blend the resolved color toward the background (an inactive pane).
    dim: bool,
    /// The fit and atlas slot of the glyph's raster.
    fit: FallbackGlyphQuadFit,
    /// Whether the glyph spans two columns, so its quad centers across both.
    is_wide: bool,
    /// Set when a quick-select or find label has replaced this cell's glyph.
    label_color: Option<Color>,
    rect: PaneRect,
    row: usize,
}

/// The physical pixel dimensions of the surface being drawn into.
#[derive(Clone, Copy)]
struct SurfaceSize {
    height: f32,
    width: f32,
}

/// Everything [`collect_text_areas`] needs to place this frame's text. Theme
/// colors arrive by value rather than behind a `&Theme` so the returned
/// `TextArea`s borrow only the buffers, leaving the renderer free to be
/// borrowed mutably for the `prepare` call that consumes them.
struct TextAreaInput<'a> {
    background: Rgb,
    cell_height: f32,
    foreground: Rgb,
    panes: &'a [PaneView<'a>],
    status_bar_fg: Rgb,
    /// The status bar's shaped line (left side), or `None` when the bar is hidden.
    status_buffer: Option<&'a glyphon::Buffer>,
    /// The status bar's shaped line (right side), or `None` when the bar is hidden.
    status_right_buffer: Option<&'a glyphon::Buffer>,
    status_right_x: f32,
    /// Where the left and the right side's lines are drawn from (see
    /// [`GpuRenderer::status_text_top`]).
    status_text_tops: (f32, f32),
    status_top: f32,
    surface: SurfaceSize,
    tabbar_texts: &'a [TabbarText],
    text_buffers: &'a [glyphon::Buffer],
}

/// Where each vertex range written into `bg_buffer` starts and how many
/// vertices it holds, so the render pass can draw the window border out of the
/// same buffer after everything else.
struct VertexRanges {
    bg_count: u32,
    border: BgBufferRange,
    dot_count: u32,
    over_image: BgBufferRange,
    over_text: BgBufferRange,
}

/// A run of vertices in the background vertex buffer, after the cell
/// backgrounds: where it starts, in bytes, and how many vertices it holds.
#[derive(Clone, Copy)]
struct BgBufferRange {
    count: u32,
    offset: wgpu::BufferAddress,
}

// ========================================================================
// GpuRenderer: frame composition
// ========================================================================

impl GpuRenderer {
    /// Render multiple panes to the surface. Each `PaneView` specifies a grid
    /// and its viewport rect. Pane dividers are drawn between adjacent panes.
    /// When `status` is set, a status bar is drawn across the bottom cell row;
    /// callers must leave that row free of panes (see [`Self::cell_size`]).
    /// When `tabbar` is set, the tabbar/menubar is drawn across the top cell
    /// row(s) (likewise reserved by the caller) and any open dropdown is
    /// composited over the content via the image pass.
    ///
    /// Composition order: per-pane geometry and text, then the chrome around
    /// them, then the GPU uploads, then one encoded frame.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        panes: &[PaneView],
        status: Option<&StatusBar>,
        tabbar: Option<&TopTabbar>,
        images: &[ImagePlacement],
        palette: Option<&PickerView>,
        input: Option<&InputView>,
        find: Option<&FindView>,
        toasts: &[ToastCard],
        which_key: Option<&WhichKeyView>,
        token_inspect: Option<&TokenInspectView>,
    ) {
        self.drew_blinking_text = false;
        self.chrome.tabbar_enabled = tabbar.is_some();
        self.chrome.status_bar_enabled = status.is_some();
        if let Some(t) = tabbar {
            self.chrome.modern = t.menu_style == crate::tabbar::MenuStyle::Modern;
        }
        let surface_texture = match self.acquire_surface_texture() {
            Some(texture) => texture,
            None => return,
        };

        let surface = SurfaceSize {
            height: self.config.height as f32,
            width: self.config.width as f32,
        };
        let resolution = glyphon::Resolution {
            width: self.config.width,
            height: self.config.height,
        };
        self.viewport.update(&self.queue, resolution);

        let mut layers = FrameLayers::default();
        let style = self.frame_text_style();
        // Reuse last frame's buffers so unchanged lines keep their cached shaping.
        let mut text_buffers = std::mem::take(&mut self.text_buffers);
        text_buffers.truncate(panes.len());

        for (pane_idx, pane) in panes.iter().enumerate() {
            self.push_pane_background(pane, surface, &mut layers);
            let rows = self.build_pane_rows(pane, &style, &mut layers);
            if pane_idx >= text_buffers.len() {
                text_buffers.push(glyphon::Buffer::new(
                    &mut self.font_system,
                    glyphon::Metrics::new(self.fonts.font_size, self.fonts.line_height),
                ));
            }
            self.shape_pane_rows(rows, &mut text_buffers[pane_idx]);
        }

        self.push_separator_verts(panes, surface, &mut layers);

        let status_metrics = status_metrics(self.fonts.font_size, self.fonts.line_height);
        let mut status_buffer = self.status_buffer.take().unwrap_or_else(|| {
            glyphon::Buffer::new(
                &mut self.font_system,
                status_metrics,
            )
        });
        status_buffer.set_metrics(status_metrics);
        let mut status_right_buffer = self.status_right_buffer.take().unwrap_or_else(|| {
            glyphon::Buffer::new(
                &mut self.font_system,
                status_metrics,
            )
        });
        status_right_buffer.set_metrics(status_metrics);
        // Exactly one cell row, flush with the window's bottom edge.
        let status_top = crate::status_bar_top_px(surface.height, self.fonts.cell_height);
        self.hits.status_hits.clear();
        self.status_icons.clear();
        let mut status_right_x = surface.width;
        if let Some(status) = status {
            self.push_status_bar_quads(
                status_top,
                (surface.width, surface.height),
                &mut layers.bg_verts,
            );
            let columns = self.shape_status_bar(
                status,
                style.family.as_deref(),
                surface.width,
                &mut status_buffer,
                &mut status_right_buffer,
            );
            self.push_status_cursor_quad(
                &columns,
                status_top,
                (surface.width, surface.height),
                &mut layers.bg_verts,
            );
            for (slot, (x_px, svg)) in columns.icons.iter().enumerate() {
                if let Some(placement) = self.status_icon_placement(slot, svg, *x_px, status_top) {
                    self.status_icons.push(placement);
                }
            }
            self.push_status_hover_quad(
                status,
                &columns,
                status_top,
                (surface.width, surface.height),
                &mut layers.bg_verts,
            );
            status_right_x = columns.right_x;
        }
        let status_text_tops = (
            self.status_text_top(&status_buffer, status_top),
            self.status_text_top(&status_right_buffer, status_top),
        );

        // Top tabbar (tabbar/menubar) bands and text. The dropdown overlay is
        // handled separately via the image pass so it sits above pane text.
        let tabbar_texts = match tabbar {
            Some(c) => self.draw_tabbar(c, surface.width),
            None => Vec::new(),
        };

        let ranges = self.upload_vertex_buffers(&mut layers, surface);
        self.prepare_tabbar_strip(tabbar, surface);
        let toolbars: Vec<&ToolbarView> = panes.iter().filter_map(|pane| pane.toolbar).collect();
        // One of each popup is drawn, so the focused pane's, the one the keys
        // reach: the first pane's in tree order took the slot from it.
        let focused_first = || panes.iter().filter(|pane| pane.focused).chain(panes.iter().filter(|pane| !pane.focused));
        let completion = focused_first().find_map(|pane| pane.completion);
        let info = focused_first().find_map(|pane| pane.info);
        let drop_list = focused_first().find_map(|pane| pane.drop_list);
        self.prepare_overlay_images(
            images,
            &toolbars,
            completion,
            info,
            drop_list,
            tabbar,
            palette,
            input,
            find,
            toasts,
            which_key,
            token_inspect,
            surface,
        );
        self.glyph_quad_pass.prepare(
            &self.device,
            &self.queue,
            &layers.glyph_quads,
            surface.width,
            surface.height,
        );

        let text_areas = collect_text_areas(TextAreaInput {
            background: self.theme.background,
            cell_height: self.fonts.cell_height,
            foreground: self.theme.foreground,
            panes,
            status_bar_fg: self.theme.status_bar_fg,
            status_buffer: status.map(|_| &status_buffer),
            status_right_buffer: status.map(|_| &status_right_buffer),
            status_right_x,
            status_text_tops,
            status_top,
            surface,
            tabbar_texts: &tabbar_texts,
            text_buffers: &text_buffers,
        });

        let prepared = self
            .text_renderer
            .prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.text_atlas,
                &self.viewport,
                text_areas,
                &mut self.swash_cache,
            )
            .is_ok();

        // Hand the buffers back for reuse next frame (preserves shape caches)
        // regardless of outcome, so a failed prepare doesn't also cost the
        // shape cache on top of the dropped frame.
        self.text_buffers = text_buffers;
        self.status_buffer = Some(status_buffer);
        self.status_right_buffer = Some(status_right_buffer);

        // glyphon returns `AtlasFull` when one frame's glyphs outgrow the
        // atlas (a big font size over many panes). That's reachable from
        // real PTY content, not just a bug, so skip this frame the same way
        // `acquire_surface_texture` already degrades on a transient surface
        // error instead of panicking.
        if prepared {
            self.encode_frame(surface_texture, &ranges);
        }
        // Release every glyph this frame did not draw for eviction. glyphon
        // pins each glyph it prepares until this runs, so without it nothing
        // is ever evicted: the atlas grows to the device's limit, `prepare`
        // fails on every frame after, and the window never draws again.
        self.text_atlas.trim();
    }

    // --------------------------------------------------------------------
    // Per-frame setup
    // --------------------------------------------------------------------

    /// The font attributes every row of this frame shapes with.
    fn frame_text_style(&self) -> FrameTextStyle {
        // When ligatures are disabled, suppress the OpenType features that build
        // them. Pure-ASCII lines already avoid ligatures via Basic shaping, but
        // any line with non-ASCII must use Advanced shaping (for font fallback),
        // which would otherwise re-enable ligatures; these features turn them off
        // there too.
        let ligatures_off = (!self.fonts.ligatures).then(|| {
            let mut ff = glyphon::cosmic_text::FontFeatures::new();
            ff.disable(glyphon::cosmic_text::FeatureTag::CONTEXTUAL_ALTERNATES);
            ff.disable(glyphon::cosmic_text::FeatureTag::STANDARD_LIGATURES);
            ff.disable(glyphon::cosmic_text::FeatureTag::CONTEXTUAL_LIGATURES);
            ff.disable(glyphon::cosmic_text::FeatureTag::DISCRETIONARY_LIGATURES);
            ff
        });
        FrameTextStyle {
            family: self.fonts.font_family.clone(),
            ligatures_off,
        }
    }

    // --------------------------------------------------------------------
    // Per-pane composition
    // --------------------------------------------------------------------

    /// Append one pane's cell backgrounds, cursors, and braille-dot quads.
    fn push_pane_background(
        &self,
        pane: &PaneView,
        surface: SurfaceSize,
        layers: &mut FrameLayers,
    ) {
        let grid = pane.grid;
        // Rows are placed in the grid area, centred in the pane.
        let rect = pane.rect.grid_area(self.fonts.cell_height);
        let extent = pane_cell_extent(rect, self.fonts.cell_width, self.fonts.cell_height);
        let pane_cols = extent.cols;
        let pane_rows = extent.rows;

        let bg_verts = build_bg_vertices_offset(
            grid,
            BgParams {
                ch: self.fonts.cell_height,
                content_right: rect.x + rect.width - crate::PANE_H_PAD,
                cursor_unfocused: pane.cursor_unfocused,
                cursor_shape: pane.cursor_shape,
                cw: self.fonts.cell_width,
                dim: pane.dim,
                draw_braille_dots: !self.fonts.font_has_braille,
                focused: pane.focused,
                hide_cursor: !pane.cursor_visible || pane.nav_cursor.is_some(),
                hovered_link: pane.hovered_link,
                labels: pane.labels,
                room: (pane_cols, pane_rows),
                overlay: pane.overlay,
                presentation: pane.presentation(),
                find_labels: pane.find_labels,
                offset_x: rect.x + crate::PANE_H_PAD,
                offset_y: rect.y,
                // The cursor's row gets a `cursorline` wash while the pane is
                // being navigated; it doesn't blink with the cursor itself.
                cursor_line: pane.cursor_line_row.map(|row| grid.wrapped_row_span(row)),
                line_flash: pane.line_flash.map(|(row, strength)| LineFlash {
                    rows: grid.wrapped_row_span(row),
                    strength,
                }),
                scroll_offset: pane.scroll_offset,
                scrollback_len: pane.scrollback_len,
                search_current: pane.search_current,
                search_matches: pane.search_matches,
                sentence_spans: pane.sentence_spans,
                outlines: pane.outlines,
                selection: pane.selection,
                selection_block: pane.selection_block,
                surface_h: surface.height,
                surface_w: surface.width,
                theme: &self.theme,
                url_underline: pane.url_underline,
            },
            &mut layers.dot_verts,
        );
        layers.bg_verts.extend_from_slice(&bg_verts);

        // The block-as-cursor: while the traversal cursor sits inside a rich
        // block's reserved band, the band itself is the cursor: an accent
        // outline around the whole band replaces the 1-cell block, which
        // would otherwise sit invisible in the blank space behind the image.
        // Steady, not blinked: it is a band-sized affordance, and a rectangle
        // that big flashing would be noise rather than feedback.
        if let Some((band_top, band_rows)) = pane.block_band {
            let px0 = rect.x + crate::PANE_H_PAD;
            let py0 = rect.y + band_top as f32 * self.fonts.cell_height;
            let px1 = px0 + pane_cols as f32 * self.fonts.cell_width;
            let py1 = py0 + band_rows as f32 * self.fonts.cell_height;
            for (qx0, qy0, qx1, qy1) in
                cursor_outline_quads(px0, py0, px1 - px0, py1 - py0, CURSOR_HOLLOW_STROKE_WIDTH)
            {
                layers.bg_verts.extend_from_slice(&quad_vertices(
                    qx0,
                    qy0,
                    qx1,
                    qy1,
                    self.theme.cursor_bg.as_linear(),
                    surface.width,
                    surface.height,
                ));
            }
        } else if let Some((nav_row, nav_col)) = pane.nav_cursor.filter(|_| pane.nav_cursor_visible)
        {
            if nav_row < pane_rows && nav_col < pane_cols {
                let px0 = rect.x + crate::PANE_H_PAD + nav_col as f32 * self.fonts.cell_width;
                let py0 = rect.y + nav_row as f32 * self.fonts.cell_height;
                // The traversal cursor takes the same unfocused form as the
                // shell's (see `build_bg_vertices_offset`): a hollow outline
                // of the cell, whatever its shape.
                if pane.cursor_unfocused {
                    for (qx0, qy0, qx1, qy1) in cursor_outline_quads(
                        px0,
                        py0,
                        self.fonts.cell_width,
                        self.fonts.cell_height,
                        CURSOR_HOLLOW_STROKE_WIDTH,
                    ) {
                        layers.bg_verts.extend_from_slice(&quad_vertices(
                            qx0,
                            qy0,
                            qx1,
                            qy1,
                            self.theme.cursor_bg.as_linear(),
                            surface.width,
                            surface.height,
                        ));
                    }
                } else {
                    let (qx0, qy0, qx1, qy1) = cursor_quad(
                        pane.cursor_shape,
                        px0,
                        py0,
                        self.fonts.cell_width,
                        self.fonts.cell_height,
                        false,
                    );
                    layers.bg_verts.extend_from_slice(&quad_vertices(
                        qx0,
                        qy0,
                        qx1,
                        qy1,
                        self.theme.cursor_bg.as_linear(),
                        surface.width,
                        surface.height,
                    ));
                }
            }
        }
    }

    /// Build one pane's rows of shaped-ready text, appending every glyph that
    /// needs its own independently scaled quad to `layers`.
    fn build_pane_rows(
        &mut self,
        pane: &PaneView,
        style: &FrameTextStyle,
        layers: &mut FrameLayers,
    ) -> Vec<PaneRow> {
        let grid = pane.grid;
        let presentation = pane.presentation();
        let rect = pane.rect.grid_area(self.fonts.cell_height);
        let extent = pane_cell_extent(rect, self.fonts.cell_width, self.fonts.cell_height);
        let pane_cols = extent.cols;
        let pane_rows = extent.rows;

        let mut default_attrs = Attrs::new()
            .family(base_family(style.family.as_deref()))
            .weight(parse_weight(
                self.fonts.normal_weight.as_deref(),
                glyphon::cosmic_text::Weight::NORMAL,
            ));
        if let Some(ff) = &style.ligatures_off {
            default_attrs = default_attrs.font_features(ff.clone());
        }
        let mut rows: Vec<PaneRow> = Vec::with_capacity(pane_rows);

        let pane_dim = pane.dim;

        let sel = pane.selection;
        let sel_norm = sel.map(|(r1, c1, r2, c2)| {
            if (r1, c1) > (r2, c2) {
                (r2, c2, r1, c1)
            } else {
                (r1, c1, r2, c2)
            }
        });

        let quick_label_color = Color::rgba(255, 200, 50, 255);
        let find_label_color = self.theme.find_label_fg.to_glyphon();
        // Both overlays replace the cell's glyph with their label; the quick
        // select labels are amber, the `f`/`t` jump labels take the theme's
        // find-label color over their light box.
        let label_map: std::collections::HashMap<(usize, usize), (char, Color)> = pane
            .labels
            .map(|l| {
                l.iter()
                    .map(|&(r, c, ch)| ((r, c), (ch, quick_label_color)))
                    .collect()
            })
            .unwrap_or_default();
        let bracket_map: std::collections::HashMap<(usize, usize), (u8, u8, u8)> = pane
            .bracket_colors
            .iter()
            .map(|&(row, col, rgb)| ((row, col), rgb))
            .collect();
        let label_map: std::collections::HashMap<(usize, usize), (char, Color)> = label_map
            .into_iter()
            .chain(
                pane.find_labels
                    .iter()
                    .map(|&(r, c, ch)| ((r, c), (ch, find_label_color))),
            )
            .collect();

        let search_match_set: std::collections::HashSet<(usize, usize)> =
            pane.search_matches.iter().copied().collect();
        let search_current_set: std::collections::HashSet<(usize, usize)> =
            pane.search_current.iter().copied().collect();

        // The viewport cell covered by an opaque block cursor, so the glyph
        // under it can be repainted when it would otherwise disappear into
        // the cursor's fill.
        let block_cursor = block_cursor_cell(
            pane.cursor_shape,
            pane.cursor_unfocused,
            pane.nav_cursor.filter(|_| pane.nav_cursor_visible),
            pane.cursor_visible,
            grid.cursor(),
            pane.scroll_offset,
        );

        for row in 0..grid.rows().min(pane_rows) {
            let mut text = String::with_capacity(grid.cols());
            let mut attrs_list = glyphon::AttrsList::new(&default_attrs);
            // `sel_norm` addresses rows absolutely (see
            // `Grid::to_absolute_row`), so this row's membership is tested
            // against its absolute index rather than the viewport row.
            let abs_row = grid.to_absolute_row(row);

            for col in 0..grid.cols().min(pane_cols) {
                let start = text.len();
                let shown = shown_cell(grid, pane.overlay, row, col, presentation);
                let cell = shown.as_deref();
                self.drew_blinking_text |= cell.is_some_and(|c| c.style.blink);

                // The right half of a double-width char carries no glyph; skip
                // it so the wide glyph from the left half spans both columns.
                if matches!(cell.map(|c| c.width), Some(CellWidth::Spacer)) {
                    continue;
                }

                let label = label_map.get(&(row, col)).copied();
                let label_char = label.map(|(ch, _)| ch);
                // Set when this cell is under the block cursor and its own
                // color would vanish into the cursor fill.
                let cursor_text_fg: Option<(u8, u8, u8)> = block_cursor
                    .filter(|&pos| pos == (row, col))
                    .and_then(|_| cursor_contrast_fg(cell_text_fg(cell, &self.theme), &self.theme));
                let raw = label_char.unwrap_or_else(|| cell.map(|c| c.ch).unwrap_or(' '));
                // When the font lacks braille, it is painted as quads in the
                // background pass instead; emit a space here so the glyph
                // layer doesn't draw a misaligned, proportionally-shaped
                // fallback on top of it. When the font has braille, keep it
                // so the font renders it directly (crisp and anti-aliased).
                let ch = if label_char.is_none() && !self.fonts.font_has_braille && is_braille(raw) {
                    ' '
                } else {
                    raw
                };

                // Trailing combining codepoints (ZWJ sequences, variation
                // selectors, skin-tone modifiers, a paired flag half); labels
                // never carry a tail (they're a single synthesized char).
                let tail = if label_char.is_none() {
                    cell.and_then(|c| c.tail.as_deref())
                } else {
                    None
                };

                // Fallback-font glyphs (Dingbats, symbols, ...) rarely advance by
                // exactly one cell (e.g. Claude Code's rotating star spinner).
                // Draw them as their own independently-scaled quad instead of
                // asking cosmic-text to reshape them to fit, which would
                // otherwise perturb this row's shared ascent/descent. Emit a
                // space here so the glyph layer leaves this cell's ink to the
                // quad pass below.
                let is_wide = matches!(cell.map(|c| c.width), Some(CellWidth::Wide));
                let quad_fit = self.ensure_fallback_glyph_quad(ch, tail, is_wide);
                if quad_fit.is_some() {
                    text.push(' ');
                } else {
                    text.push(ch);
                    // `tail` rides along in the same shaping run as `ch` so
                    // cosmic-text's GSUB rules can compose them into one glyph.
                    if let Some(tail) = tail {
                        text.push_str(tail);
                    }
                }

                if let Some(fit) = quad_fit {
                    self.push_fallback_glyph_quad(
                        FallbackGlyphQuad {
                            cell,
                            col,
                            cursor_text_fg,
                            dim: pane_dim,
                            fit,
                            is_wide,
                            label_color: label.map(|(_, color)| color),
                            rect,
                            row,
                        },
                        layers,
                    );
                } else if let Some((_, label_color)) = label {
                    let label_attrs = Attrs::new()
                        .family(base_family(style.family.as_deref()))
                        .weight(effective_bold_weight(
                            self.fonts.bold_weight.as_deref(),
                            self.fonts.font_has_bold,
                        ))
                        .color(label_color);
                    attrs_list.add_span(start..text.len(), &label_attrs);
                } else if sel_norm.is_some_and(|(sr1, sc1, sr2, sc2)| {
                    (abs_row, col) >= (sr1, sc1) && (abs_row, col) <= (sr2, sc2)
                }) {
                    // The selection highlights the background only; the
                    // glyph keeps the cell's own foreground (reverse video
                    // still swaps, as it would unselected) so selected
                    // text reads the same as the rest of the line, unless
                    // it is nearly the selection's own color, when it is
                    // inverted (toned down). Only a bold cell is bold.
                    let sel_bg = self.theme.selection_bg;
                    let (r, g, b) =
                        cursor_text_fg.unwrap_or_else(|| cell_text_fg(cell, &self.theme));
                    let readable = Rgb::new(r, g, b).readable_on(sel_bg);
                    let text_fg = (readable.r, readable.g, readable.b);
                    let bold = cell.is_some_and(|c| c.style.bold);
                    let attrs = self.highlight_attrs(style, ch, cell, text_fg, pane_dim, bold);
                    attrs_list.add_span(start..text.len(), &attrs);
                } else if search_current_set.contains(&(row, col)) {
                    let attrs =
                        self.highlight_attrs(style, ch, cell, CURRENT_MATCH_FG, pane_dim, true);
                    attrs_list.add_span(start..text.len(), &attrs);
                } else if search_match_set.contains(&(row, col)) {
                    let attrs =
                        self.highlight_attrs(style, ch, cell, SEARCH_MATCH_FG, pane_dim, true);
                    attrs_list.add_span(start..text.len(), &attrs);
                } else if let Some(cell) = cell {
                    let context = CellStyleContext {
                        bracket_rgb: bracket_map.get(&(row, col)).copied(),
                        cursor_text_fg,
                        dim: pane_dim,
                    };
                    if let Some(attrs) = self.cell_glyph_attrs(cell, ch, context, style) {
                        attrs_list.add_span(start..text.len(), &attrs);
                    }
                }
            }

            rows.push(PaneRow {
                attrs: attrs_list,
                text,
            });
        }

        rows
    }

    /// The attributes of a glyph drawn over a highlight (the selection or a
    /// search match): `fg` faded toward the background in a dimmed pane,
    /// bold when asked for, italic when the cell is, and with ligatures held
    /// off when they are configured off. Ligature features are left alone
    /// for characters needing complex shaping, since disabling them would
    /// also block the GSUB substitutions some fonts use to compose emoji and
    /// ZWJ sequences into a single glyph.
    fn highlight_attrs<'a>(
        &self,
        style: &'a FrameTextStyle,
        ch: char,
        cell: Option<&Cell>,
        fg: (u8, u8, u8),
        dim: bool,
        bold: bool,
    ) -> Attrs<'a> {
        let (r, g, b) = fg;
        let mut attrs = Attrs::new()
            .family(base_family(style.family.as_deref()))
            .color(dim_toward_bg(r, g, b, self.theme.background, dim));
        attrs = match bold {
            true => attrs.weight(effective_bold_weight(
                self.fonts.bold_weight.as_deref(),
                self.fonts.font_has_bold,
            )),
            false => attrs.weight(parse_weight(
                self.fonts.normal_weight.as_deref(),
                glyphon::cosmic_text::Weight::NORMAL,
            )),
        };
        if cell.is_some_and(|c| c.style.italic) {
            attrs = attrs.style(glyphon::cosmic_text::Style::Italic);
        }
        if !needs_complex_shaping(ch) {
            if let Some(ff) = &style.ligatures_off {
                attrs = attrs.font_features(ff.clone());
            }
        }
        attrs
    }

    /// Push one pane's rows into its reused text buffer and shape them.
    fn shape_pane_rows(&mut self, rows: Vec<PaneRow>, buffer: &mut glyphon::Buffer) {
        // Do NOT call `buffer.set_monospace_width` here; it is not a no-op.
        // At fractional physical font sizes (fractional DPI scale: logical
        // 15px at 125% is 18.75px) cosmic-text 0.18.2 quantizes the requested
        // advance to a coarser grid (measured: a Cascadia Code "M" asked for
        // 10.986px but then rendered at 11.133px), so glyph advances stop
        // equaling `self.fonts.cell_width`. The cursor and cell backgrounds are
        // drawn at `col * cell_width`, so that per-glyph mismatch accumulates
        // one column at a time until the cursor sits on top of already-typed
        // text, worse the further right you go. With the call gone,
        // primary-font glyphs render at their natural advance, which is
        // exactly what `measure_cell` sets `cell_width` to, so the glyph run
        // and the cursor stride stay locked at every size. Wide/fallback
        // glyphs never relied on this call: `ensure_fallback_glyph_quad`
        // pulls any glyph whose advance diverges from the cell out of this
        // buffer into the manually positioned quad pass, itself drawn at
        // `col * cell_width`.
        let ending = glyphon::cosmic_text::LineEnding::default();
        let row_count = rows.len();
        for (i, row) in rows.into_iter().enumerate() {
            let PaneRow {
                attrs: attrs_list,
                text,
            } = row;
            // Basic shaping keeps glyphs on the primary monospace font at
            // native cell-width advances, which is what the grid relies on
            // for column alignment. Only escalate to Advanced shaping (font
            // fallback + complex shaping) for characters that actually need
            // it (emoji, CJK, accents): forcing the box-drawing and braille
            // ranges through Advanced picks fallback glyphs with non-cell
            // advances, which drifts and breaks TUIs like btop.
            let advanced = self.fonts.ligatures || text.chars().any(needs_complex_shaping);
            let shaping = if advanced {
                Shaping::Advanced
            } else {
                Shaping::Basic
            };
            if i < buffer.lines.len() {
                // A line's shaping follows from its text under the (stable)
                // ligatures setting, so infer the existing line's mode from
                // its current text: keep the glyph cache with set_text when
                // the mode is unchanged, else reset to the new shaping.
                let cur_advanced =
                    self.fonts.ligatures || buffer.lines[i].text().chars().any(needs_complex_shaping);
                if advanced == cur_advanced {
                    buffer.lines[i].set_text(&text, ending, attrs_list);
                } else {
                    buffer.lines[i].reset_new(text, ending, attrs_list, shaping);
                }
            } else {
                buffer
                    .lines
                    .push(BufferLine::new(&text, ending, attrs_list, shaping));
            }
        }
        buffer.lines.truncate(row_count);
        buffer.shape_until_scroll(&mut self.font_system, false);
    }

    /// Place one fallback glyph as its own independently scaled quad, centered
    /// in the cell it occupies. Does nothing when the glyph never made it into
    /// the quad atlas.
    fn push_fallback_glyph_quad(&mut self, glyph: FallbackGlyphQuad, layers: &mut FrameLayers) {
        // Selection highlights only the background (see the span in
        // `build_pane_rows`); the glyph keeps its own foreground whether or not
        // it falls inside the selection.
        let color = if let Some(label_color) = glyph.label_color {
            label_color
        } else if let Some((r, g, b)) = glyph.cursor_text_fg {
            dim_toward_bg(r, g, b, self.theme.background, glyph.dim)
        } else {
            match glyph.cell.map(|c| c.style.foreground) {
                Some(GridColor::Rgb(rgb)) => {
                    dim_toward_bg(rgb.r, rgb.g, rgb.b, self.theme.background, glyph.dim)
                }
                Some(GridColor::Indexed(idx)) => {
                    let (r, g, b) = self.theme.palette_color(idx);
                    dim_toward_bg(r, g, b, self.theme.background, glyph.dim)
                }
                None | Some(GridColor::Default) => {
                    let fg = self.theme.foreground;
                    dim_toward_bg(fg.r, fg.g, fg.b, self.theme.background, glyph.dim)
                }
            }
        };
        let FallbackGlyphQuadFit { scale, slot } = glyph.fit;
        let quad_w = slot.width as f32 * scale;
        let quad_h = slot.height as f32 * scale;
        // A wide glyph's quad centers across the two columns it occupies (this
        // cell plus its Spacer), not just the first.
        let target_width = if glyph.is_wide {
            self.fonts.cell_width * 2.0
        } else {
            self.fonts.cell_width
        };
        let cell_x = glyph.rect.x + crate::PANE_H_PAD + glyph.col as f32 * self.fonts.cell_width;
        let cell_y = glyph.rect.y + glyph.row as f32 * self.fonts.cell_height;
        layers.glyph_quads.push(GlyphQuadPlacement {
            color: (
                color.r() as f32 / 255.0,
                color.g() as f32 / 255.0,
                color.b() as f32 / 255.0,
            ),
            height: quad_h,
            slot,
            width: quad_w,
            x: cell_x + (target_width - quad_w) / 2.0,
            y: cell_y + (self.fonts.cell_height - quad_h) / 2.0,
        });
    }

    /// The attribute span for one ordinary cell: neither label-replaced nor
    /// inside the selection. `None` when the cell needs no span of its own and
    /// can inherit the row's default attributes.
    fn cell_glyph_attrs<'a>(
        &self,
        cell: &Cell,
        ch: char,
        context: CellStyleContext,
        style: &'a FrameTextStyle,
    ) -> Option<Attrs<'a>> {
        let mut attrs = Attrs::new().family(base_family(style.family.as_deref()));

        let fg_rgb = match cell.style.foreground {
            GridColor::Rgb(rgb) => (rgb.r, rgb.g, rgb.b),
            GridColor::Indexed(idx) => self.theme.palette_color(idx),
            GridColor::Default => {
                let fg = self.theme.foreground;
                (fg.r, fg.g, fg.b)
            }
        };
        let bg_rgb = match cell.style.background {
            GridColor::Rgb(rgb) => (rgb.r, rgb.g, rgb.b),
            GridColor::Indexed(idx) => self.theme.palette_color(idx),
            GridColor::Default => {
                let bg = self.theme.background;
                (bg.r, bg.g, bg.b)
            }
        };
        // SGR 7 (reverse video): the glyph paints in what would otherwise be
        // the background, matching the swapped highlight quad drawn in the
        // background pass.
        let text_fg = if cell.style.reversed { bg_rgb } else { fg_rgb };
        // A glyph that would be lost inside the block cursor's fill is
        // repainted in a contrasting color instead.
        let text_fg = context.cursor_text_fg.unwrap_or(text_fg);
        // SGR 2 (faint): fade the glyph toward whatever sits behind it, which
        // is the foreground color when the cell is reversed.
        let text_fg = if cell.style.faint {
            let behind = if cell.style.reversed { fg_rgb } else { bg_rgb };
            let faded = mix_rgb(
                Rgb::new(text_fg.0, text_fg.1, text_fg.2),
                Rgb::new(behind.0, behind.1, behind.2),
                FAINT_FACTOR,
            );
            (faded.r, faded.g, faded.b)
        } else {
            text_fg
        };
        // An explicit cell background (an SGR color, not the pane's base
        // background) or a reversed cell counts as a highlight, so it gets the
        // same synthetic-bold compensation as a selection.
        let is_highlighted =
            cell.style.reversed || !matches!(cell.style.background, GridColor::Default);
        let highlight_bg = if cell.style.reversed { fg_rgb } else { bg_rgb };
        let bg_needs_bold = is_highlighted && needs_dark_on_light_bold(text_fg, highlight_bg);

        if cell.style.bold || bg_needs_bold {
            attrs = attrs.weight(effective_bold_weight(
                self.fonts.bold_weight.as_deref(),
                self.fonts.font_has_bold,
            ));
        } else {
            attrs = attrs.weight(parse_weight(
                self.fonts.normal_weight.as_deref(),
                glyphon::cosmic_text::Weight::NORMAL,
            ));
        }
        if cell.style.italic {
            attrs = attrs.style(glyphon::cosmic_text::Style::Italic);
        }

        let text_color_explicit = cell.style.foreground != GridColor::Default
            || cell.style.reversed
            || cell.style.faint
            || context.cursor_text_fg.is_some()
            || context.bracket_rgb.is_some();
        if let Some((r, g, b)) = context.bracket_rgb {
            // Rainbow parens recolor the bracket glyph but keep the cell's own
            // bold/italic, and yield to the cursor-contrast fix above, which
            // exists to keep the glyph visible at all.
            let fg = context.cursor_text_fg.unwrap_or((r, g, b));
            attrs = attrs.color(dim_toward_bg(
                fg.0,
                fg.1,
                fg.2,
                self.theme.background,
                context.dim,
            ));
        } else if text_color_explicit {
            attrs = attrs.color(dim_toward_bg(
                text_fg.0,
                text_fg.1,
                text_fg.2,
                self.theme.background,
                context.dim,
            ));
        }

        // Same as the selection span in `build_pane_rows`: complex-shaping
        // characters (emoji, CJK, accents) keep the font's default GSUB
        // features so composed glyphs still form when ligatures are otherwise
        // disabled. This needs its own span even in the common
        // default-color/no-bold/no-italic case, since that case otherwise
        // inherits `default_attrs`, which does carry the disabling features.
        let is_complex = needs_complex_shaping(ch);
        if !is_complex {
            if let Some(ff) = &style.ligatures_off {
                attrs = attrs.font_features(ff.clone());
            }
        }

        let needs_span = text_color_explicit
            || cell.style.bold
            || cell.style.italic
            || bg_needs_bold
            || (is_complex && style.ligatures_off.is_some());
        needs_span.then_some(attrs)
    }

    // --------------------------------------------------------------------
    // Chrome around the panes
    // --------------------------------------------------------------------

    /// The color split dividers are drawn in: the theme's divider at full
    /// strength, so the line between panes stands out from the fainter lines
    /// inside a pane's tab strip. The pane strips draw their share of a
    /// divider with this too, so the two always meet in one color.
    pub(super) fn split_divider_color(&self) -> Rgb {
        self.theme.divider
    }

    /// The separators around the panes, the status bar's top edge among them,
    /// each drawn once, over their strips: the focused pane's bottom border in
    /// the focus color (see `background::divider`).
    fn push_separator_verts(&self, panes: &[PaneView], surface: SurfaceSize, layers: &mut FrameLayers) {
        let areas: Vec<PaneRect> = panes.iter().map(|pane| pane.area).collect();
        let style = SeparatorStyle {
            focus: self.theme.active_pane_border.as_linear(),
            plain: self.split_divider_color().as_linear(),
            status: self.theme.status_bar_border.as_linear(),
            width: self.chrome.divider_width,
        };
        let focused = panes.iter().position(|pane| pane.focused);
        // The status bar's top edge is a separator too, a bottom border.
        let status_top = self
            .chrome
            .status_bar_enabled
            .then(|| crate::status_bar_top_px(surface.height, self.fonts.cell_height));
        let found = separators(&areas, style.width, status_top);
        layers
            .over_image_verts
            .extend(separator_vertices(&found, focused, style, (surface.width, surface.height)));
    }

    // --------------------------------------------------------------------
    // GPU upload
    // --------------------------------------------------------------------

    /// Write this frame's cell-background, window-border, and braille-dot
    /// geometry into the GPU vertex buffers, and report where each range
    /// landed so the render pass can draw them in the right order.
    fn upload_vertex_buffers(
        &mut self,
        layers: &mut FrameLayers,
        surface: SurfaceSize,
    ) -> VertexRanges {
        let bg_count = layers.bg_verts.len() as u32;
        let border_verts = window_border_verts(
            surface,
            self.theme.window_border.as_linear(),
            self.chrome.decorated,
        );
        // One buffer, four runs: the cell backgrounds, what draws over the
        // text, what draws over the strips, and the window border drawn last
        // of all.
        let mut bytes: Vec<u8> = Vec::new();
        let mut append = |verts: &[BgVertex]| {
            let range = BgBufferRange {
                count: verts.len() as u32,
                offset: bytes.len() as wgpu::BufferAddress,
            };
            bytes.extend(verts.iter().flat_map(|v| v.to_bytes()));
            range
        };
        append(&layers.bg_verts);
        let over_text = append(&layers.over_text_verts);
        let over_image = append(&layers.over_image_verts);
        let border = append(&border_verts);
        ensure_vertex_capacity(
            &self.device,
            &mut self.bg_buffer,
            BG_BUFFER_LABEL,
            bytes.len() as u64,
        );
        self.queue.write_buffer(&self.bg_buffer, 0, &bytes);

        let dot_count = layers.dot_verts.len() as u32;
        let dot_bytes: Vec<u8> = layers.dot_verts.iter().flat_map(|v| v.to_bytes()).collect();
        ensure_vertex_capacity(
            &self.device,
            &mut self.dot_buffer,
            DOT_BUFFER_LABEL,
            dot_bytes.len() as u64,
        );
        if dot_count > 0 {
            self.queue.write_buffer(&self.dot_buffer, 0, &dot_bytes);
        }

        VertexRanges {
            bg_count,
            border,
            dot_count,
            over_image,
            over_text,
        }
    }

    // --------------------------------------------------------------------
    // Overlay passes
    // --------------------------------------------------------------------

    /// Rasterize and prepare the top-tabbar strip.
    fn prepare_tabbar_strip(&mut self, tabbar: Option<&TopTabbar>, surface: SurfaceSize) {
        // The top-tabbar strip (band + rounded tab pills) is composited before the
        // text pass so the tab cards sit under the tab titles.
        let tabbar_strip = match tabbar {
            Some(c) => self.tabbar_strip_placement(c, surface.width, surface.height),
            None => Vec::new(),
        };
        // One layer: nothing of it floats over anything else.
        self.tabbar_strip_pass
            .prepare(&self.queue, &tabbar_strip, tabbar_strip.len(), surface.width, surface.height);
    }

    /// Prepare the image pass with the caller's block images plus every
    /// rasterized overlay that must sit above the pane text.
    #[allow(clippy::too_many_arguments)]
    fn prepare_overlay_images(
        &mut self,
        images: &[ImagePlacement],
        toolbars: &[&ToolbarView],
        completion: Option<&CompletionView>,
        info: Option<&InfoView>,
        drop_list: Option<&DropListView>,
        tabbar: Option<&TopTabbar>,
        palette: Option<&PickerView>,
        input: Option<&InputView>,
        find: Option<&FindView>,
        toasts: &[ToastCard],
        which_key: Option<&WhichKeyView>,
        token_inspect: Option<&TokenInspectView>,
        surface: SurfaceSize,
    ) {
        // The open dropdown and command palette are rasterized to textures and
        // drawn by the image pass (after the text pass) so they overlay content.
        let (w, h) = (surface.width, surface.height);
        let mut all_images: Vec<ImagePlacement> = images.to_vec();
        // A pane's toolbar lies over its own top rows, under every popup.
        all_images.extend(self.rasterize_toolbars(toolbars, w, h));
        // Everything from here floats over the content, over the lines drawn
        // on the strips too (see `FrameLayers::over_image_verts`).
        let base = all_images.len();
        if let Some(view) = drop_list {
            all_images.extend(self.rasterize_drop_list(view, w, h));
        }
        if let Some(view) = info {
            all_images.extend(self.rasterize_info(view, w, h));
        }
        if let Some(view) = completion {
            all_images.extend(self.rasterize_completion(view, w, h));
        }
        all_images.extend_from_slice(&self.status_icons);
        if let Some(c) = tabbar {
            all_images.extend(self.rasterize_dropdown(c, w, h));
            all_images.extend(self.rasterize_url_tooltip(c, w, h));
        }
        if let Some(p) = palette {
            all_images.extend(self.rasterize_palette(p, w, h));
        }
        match input {
            Some(i) => all_images.extend(self.rasterize_input(i, w, h)),
            None => {
                self.hits.input_rect = None;
                self.hits.input_button_rects.clear();
            }
        }
        match find {
            Some(f) => all_images.extend(self.rasterize_find(f, w, h)),
            None => self.hits.find_rect = None,
        }
        all_images.extend(self.rasterize_toasts(toasts, w, h));
        if let Some(wk) = which_key {
            all_images.extend(self.rasterize_which_key(wk, w, h));
        }
        if let Some(ti) = token_inspect {
            all_images.extend(self.rasterize_token_inspect(ti, w, h));
        }
        self.image_pass.prepare(&self.queue, &all_images, base, w, h);
    }

    // --------------------------------------------------------------------
    // Encoding
    // --------------------------------------------------------------------

    /// Encode and submit the one render pass this frame draws, then present.
    fn encode_frame(&mut self, surface_texture: wgpu::SurfaceTexture, ranges: &VertexRanges) {
        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("winter frame"),
            });

        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("winter clear + bg"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        // The sRGB surface expects a linear clear value, so decode
                        // the (sRGB) background to keep the displayed color exact.
                        load: LoadOp::Clear(clear_color(
                            self.theme.background,
                            self.background_opacity(),
                            self.config.alpha_mode,
                        )),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            if ranges.bg_count > 0 {
                pass.set_pipeline(&self.bg_pipeline);
                pass.set_vertex_buffer(0, self.bg_buffer.slice(..));
                pass.draw(0..ranges.bg_count, 0..1);
            }

            // Anti-aliased braille dots blend over the cell backgrounds.
            if ranges.dot_count > 0 {
                pass.set_pipeline(&self.dot_pipeline);
                pass.set_vertex_buffer(0, self.dot_buffer.slice(..));
                pass.draw(0..ranges.dot_count, 0..1);
            }

            // Tabbar strip sits above the bg quads but below the text so the
            // rounded tab cards back the tab titles.
            self.tabbar_strip_pass.render_base(&mut pass);

            // Reachable only if `prepare` and this call disagree about the
            // atlas/viewport passed in, not from ordinary content; skip the
            // frame the same way a failed `prepare` above does rather than
            // panicking on it.
            if self
                .text_renderer
                .render(&self.text_atlas, &self.viewport, &mut pass)
                .is_err()
            {
                return;
            }

            // Fallback glyphs that needed a cell-width fit (e.g. Claude Code's
            // spinner): drawn as their own quads, not through cosmic-text, so
            // their correction never touched this row's ascent/descent.
            self.glyph_quad_pass.render(&mut pass);

            // The marks that must read over glyph descenders (the active
            // pane's underline), still under the overlays drawn next.
            draw_bg_range(
                &mut pass,
                &self.bg_pipeline,
                &self.bg_buffer,
                ranges.over_text,
            );

            self.image_pass.render_base(&mut pass);

            // The lines on the panes' strips, which are images, under what
            // floats over the content: a tooltip or a menu covers them.
            draw_bg_range(&mut pass, &self.bg_pipeline, &self.bg_buffer, ranges.over_image);

            self.image_pass.render_overlays(&mut pass);

            // Drawn last so the window border sits on top of the tabbar strip,
            // glyphs, and image blocks instead of being painted over by them.
            draw_bg_range(&mut pass, &self.bg_pipeline, &self.bg_buffer, ranges.border);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        self.queue.present(surface_texture);
    }
}

// ========================================================================
// Geometry and color, free of the GPU
// ========================================================================

/// The whole cell columns and rows that fit inside `rect`.
fn pane_cell_extent(rect: PaneRect, cell_w: f32, cell_h: f32) -> PaneExtent {
    PaneExtent {
        cols: ((rect.width - crate::PANE_H_PAD * 2.0) / cell_w).floor() as usize,
        rows: (rect.height / cell_h).floor() as usize,
    }
}

/// Blend one RGB triple toward `bg`, which is how an inactive pane's text is
/// de-emphasized. Returns the color unchanged when `dim` is false.
fn dim_toward_bg(r: u8, g: u8, b: u8, bg: Rgb, dim: bool) -> Color {
    if !dim {
        return Color::rgba(r, g, b, 255);
    }
    let lerp =
        |v: u8, bv: u8| -> u8 { (v as f32 + (bv as f32 - v as f32) * DIM_FACTOR).round() as u8 };
    Color::rgba(lerp(r, bg.r), lerp(g, bg.g), lerp(b, bg.b), 255)
}

/// The window's own 1px outline, or empty when the OS draws a frame.
///
/// An undecorated window has no OS-drawn frame, so the renderer draws this
/// itself. Kept in a vertex range of its own, appended after the cell
/// backgrounds and drawn last, so it sits on top of the tabbar strip,
/// glyphs, and image blocks instead of being painted over by them.
fn window_border_verts(
    surface: SurfaceSize,
    border: (f32, f32, f32),
    decorated: bool,
) -> Vec<BgVertex> {
    let mut border_verts = Vec::new();
    if !decorated {
        border_verts.extend_from_slice(&quad_vertices(
            0.0,
            0.0,
            surface.width,
            1.0,
            border,
            surface.width,
            surface.height,
        ));
        border_verts.extend_from_slice(&quad_vertices(
            0.0,
            surface.height - 1.0,
            surface.width,
            surface.height,
            border,
            surface.width,
            surface.height,
        ));
        border_verts.extend_from_slice(&quad_vertices(
            0.0,
            0.0,
            1.0,
            surface.height,
            border,
            surface.width,
            surface.height,
        ));
        border_verts.extend_from_slice(&quad_vertices(
            surface.width - 1.0,
            0.0,
            surface.width,
            surface.height,
            border,
            surface.width,
            surface.height,
        ));
    }
    border_verts
}

// ========================================================================
// Text placement
// ========================================================================

/// Lay out every text area this frame draws: one per pane, then the status
/// bar, then the tabbar labels.
fn collect_text_areas<'a>(input: TextAreaInput<'a>) -> Vec<TextArea<'a>> {
    let mut text_areas: Vec<TextArea> = input
        .text_buffers
        .iter()
        .zip(input.panes.iter())
        .map(|(buffer, pane)| {
            let rect = pane.rect.grid_area(input.cell_height);
            let fg = input.foreground;
            let default_color = dim_toward_bg(fg.r, fg.g, fg.b, input.background, pane.dim);
            TextArea {
                buffer,
                left: (rect.x + crate::PANE_H_PAD).round(),
                top: rect.y.round(),
                bounds: TextBounds {
                    left: (rect.x + crate::PANE_H_PAD).round() as i32,
                    top: rect.y.round() as i32,
                    right: (rect.x + rect.width - crate::PANE_H_PAD).round() as i32,
                    bottom: (rect.y + rect.height).round() as i32,
                },
                default_color,
                scale: 1.0,
                custom_glyphs: &[],
            }
        })
        .collect();

    if let Some(status_buffer) = input.status_buffer {
        // Its letters centered in the bar; clipped only by the bar itself, so
        // a tall glyph's ink is never cut by the padding.
        text_areas.push(TextArea {
            buffer: status_buffer,
            left: 0.0,
            top: input.status_text_tops.0,
            bounds: TextBounds {
                left: 0,
                top: input.status_top.round() as i32,
                right: input.surface.width as i32,
                bottom: (input.status_top + crate::status_bar_height_px(input.cell_height)).round()
                    as i32,
            },
            default_color: input.status_bar_fg.to_glyphon(),
            scale: 1.0,
            custom_glyphs: &[],
        });
    }

    if let Some(right_buffer) = input.status_right_buffer {
        text_areas.push(TextArea {
            buffer: right_buffer,
            left: input.status_right_x.round(),
            top: input.status_text_tops.1,
            bounds: TextBounds {
                left: 0,
                top: input.status_top.round() as i32,
                right: input.surface.width as i32,
                bottom: (input.status_top + crate::status_bar_height_px(input.cell_height)).round()
                    as i32,
            },
            default_color: input.status_bar_fg.to_glyphon(),
            scale: 1.0,
            custom_glyphs: &[],
        });
    }

    for text in input.tabbar_texts {
        text_areas.push(TextArea {
            buffer: &text.buffer,
            left: text.left.round(),
            top: text.top.round(),
            bounds: TextBounds {
                left: text.bounds.left,
                top: text.bounds.top,
                right: text.bounds.right,
                bottom: text.bounds.bottom,
            },
            default_color: text.color,
            scale: 1.0,
            custom_glyphs: &[],
        });
    }

    text_areas
}

/// Draw `range` of the background vertex buffer with the opaque quad
/// pipeline; nothing when it is empty.
fn draw_bg_range<'pass>(
    pass: &mut RenderPass<'pass>,
    pipeline: &'pass RenderPipeline,
    buffer: &'pass Buffer,
    range: BgBufferRange,
) {
    if range.count == 0 {
        return;
    }
    pass.set_pipeline(pipeline);
    pass.set_vertex_buffer(0, buffer.slice(range.offset..));
    pass.draw(0..range.count, 0..1);
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(width: f32, height: f32) -> PaneRect {
        PaneRect {
            height,
            width,
            x: 0.0,
            y: 0.0,
        }
    }

    #[test]
    fn test_pane_narrower_than_its_padding_yields_no_columns() {
        // `PANE_H_PAD` is subtracted from both edges before the division, so a
        // pane thinner than that goes negative. It has to land on zero columns
        // rather than wrapping into a huge count that the row loop would then
        // try to walk.
        let extent = pane_cell_extent(rect(crate::PANE_H_PAD, 100.0), 9.0, 18.0);
        assert_eq!(extent.cols, 0);
    }

    #[test]
    fn test_pane_extent_floors_to_whole_cells() {
        // Partial cells are not drawable, so both axes floor rather than round:
        // 95px of 9px cells is ten whole columns once the padding is removed.
        let extent = pane_cell_extent(rect(95.0 + crate::PANE_H_PAD * 2.0, 98.0), 9.0, 18.0);
        assert_eq!((extent.cols, extent.rows), (10, 5));
    }

    #[test]
    fn test_undimmed_color_passes_through_untouched() {
        let bg = Rgb::new(0, 0, 0);
        assert_eq!(
            dim_toward_bg(200, 100, 50, bg, false),
            Color::rgba(200, 100, 50, 255)
        );
    }

    #[test]
    fn test_dimming_moves_each_channel_toward_the_background() {
        // An inactive pane fades toward its own background, so the result sits
        // between the two ends and never past either.
        let fg = (200u8, 100u8, 50u8);
        let bg = Rgb::new(0, 0, 0);
        let dimmed = dim_toward_bg(fg.0, fg.1, fg.2, bg, true);
        assert!(dimmed.r() < fg.0, "red should move toward the background");
        assert!(dimmed.r() > bg.r, "red should not reach the background");
        assert_eq!(dimmed.a(), 255, "dimming must not touch alpha");
    }

    #[test]
    fn test_dimming_toward_a_lighter_background_raises_channels() {
        // The blend is a lerp, not a darkening: on a light theme the same call
        // has to move the channel up.
        let dimmed = dim_toward_bg(0, 0, 0, Rgb::new(255, 255, 255), true);
        assert!(dimmed.r() > 0, "should move up toward a light background");
    }

    #[test]
    fn test_a_decorated_window_draws_no_border_of_its_own() {
        // The OS frame is the border; drawing another would double it.
        let surface = SurfaceSize {
            height: 600.0,
            width: 800.0,
        };
        assert!(window_border_verts(surface, (1.0, 1.0, 1.0), true).is_empty());
    }

    #[test]
    fn test_an_undecorated_window_outlines_all_four_edges_inside_the_surface() {
        // Four quads, six vertices each, and every one within the surface: a
        // vertex outside normalized device coordinates is clipped away, which
        // would silently drop an edge.
        let surface = SurfaceSize {
            height: 600.0,
            width: 800.0,
        };
        let verts = window_border_verts(surface, (1.0, 1.0, 1.0), false);
        assert_eq!(verts.len(), 4 * 6, "one quad per edge");
        for v in &verts {
            assert!((-1.0..=1.0).contains(&v.x), "x {} outside NDC", v.x);
            assert!((-1.0..=1.0).contains(&v.y), "y {} outside NDC", v.y);
        }
    }
}
