//! Drawing one frame: composing every pass into the surface texture.

use super::*;

// ========================================================================
// Items
// ========================================================================

impl GpuRenderer {
    /// Render multiple panes to the surface. Each `PaneView` specifies a grid
    /// and its viewport rect. Pane dividers are drawn between adjacent panes.
    /// When `status` is set, a status bar is drawn across the bottom cell row;
    /// callers must leave that row free of panes (see [`Self::cell_size`]).
    /// When `tabbar` is set, the tabbar/menubar is drawn across the top cell
    /// row(s) (likewise reserved by the caller) and any open dropdown is
    /// composited over the content via the image pass.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        panes: &[PaneView],
        status: Option<&StatusBar>,
        tabbar: Option<&TopTabbar>,
        images: &[ImagePlacement],
        palette: Option<&PaletteView>,
        toast: Option<&StatusNotice>,
        which_key: Option<&WhichKeyView>,
    ) {
        self.tabbar_enabled = tabbar.is_some();
        self.status_bar_enabled = status.is_some();
        if let Some(t) = tabbar {
            self.modern = t.menu_style == crate::tabbar::MenuStyle::Modern;
        }
        let surface_texture = match self.acquire_surface_texture() {
            Some(texture) => texture,
            None => return,
        };

        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let surface_w = self.config.width as f32;
        let surface_h = self.config.height as f32;

        let resolution = glyphon::Resolution {
            width: self.config.width,
            height: self.config.height,
        };
        self.viewport.update(&self.queue, resolution);

        let mut all_bg_verts = Vec::new();
        let mut all_dot_verts: Vec<DotVertex> = Vec::new();
        let mut all_glyph_quads: Vec<GlyphQuadPlacement> = Vec::new();
        // Reuse last frame's buffers so unchanged lines keep their cached shaping.
        let mut text_buffers = std::mem::take(&mut self.text_buffers);
        text_buffers.truncate(panes.len());

        // Owned (not borrowed from `self`) because building each row's attrs
        // interleaves with `glyph_correction_font_size`, which needs `&mut self`.
        let fam = self.font_family.clone();

        // When ligatures are disabled, suppress the OpenType features that build
        // them. Pure-ASCII lines already avoid ligatures via Basic shaping, but
        // any line with non-ASCII must use Advanced shaping (for font fallback),
        // which would otherwise re-enable ligatures; these features turn them off
        // there too. `None` when ligatures are on, so they shape normally.
        let lig_off_features = (!self.ligatures).then(|| {
            let mut ff = glyphon::cosmic_text::FontFeatures::new();
            ff.disable(glyphon::cosmic_text::FeatureTag::CONTEXTUAL_ALTERNATES);
            ff.disable(glyphon::cosmic_text::FeatureTag::STANDARD_LIGATURES);
            ff.disable(glyphon::cosmic_text::FeatureTag::CONTEXTUAL_LIGATURES);
            ff.disable(glyphon::cosmic_text::FeatureTag::DISCRETIONARY_LIGATURES);
            ff
        });

        for (pane_idx, pane) in panes.iter().enumerate() {
            let grid = pane.grid;
            let rect = pane.rect;
            let pane_cols =
                ((rect.width - crate::PANE_H_PAD * 2.0) / self.cell_width).floor() as usize;
            let pane_rows = (rect.height / self.cell_height).floor() as usize;

            let bg_verts = build_bg_vertices_offset(
                grid,
                BgParams {
                    ch: self.cell_height,
                    content_right: rect.x + rect.width - crate::PANE_H_PAD,
                    cursor_unfocused: pane.cursor_unfocused,
                    cursor_shape: pane.cursor_shape,
                    cw: self.cell_width,
                    dim: pane.dim,
                    draw_braille_dots: !self.font_has_braille,
                    focused: pane.focused,
                    hide_cursor: !pane.cursor_visible || pane.nav_cursor.is_some(),
                    hovered_link: pane.hovered_link,
                    labels: pane.labels,
                    find_labels: pane.find_labels,
                    offset_x: rect.x + crate::PANE_H_PAD,
                    offset_y: rect.y,
                    // The cursor's row gets a `cursorline` wash while the pane is
                    // being navigated; it doesn't blink with the cursor itself.
                    cursor_line: pane.cursor_line_row.map(|row| grid.wrapped_row_span(row)),
                    scroll_offset: pane.scroll_offset,
                    scrollback_len: pane.scrollback_len,
                    search_current: pane.search_current,
                    search_matches: pane.search_matches,
                    sentence_spans: pane.sentence_spans,
                    selection: pane.selection,
                    selection_block: pane.selection_block,
                    surface_h,
                    surface_w,
                    theme: &self.theme,
                    url_underline: pane.url_underline,
                },
                &mut all_dot_verts,
            );
            all_bg_verts.extend_from_slice(&bg_verts);

            if let Some((nav_row, nav_col)) = pane.nav_cursor.filter(|_| pane.nav_cursor_visible) {
                if nav_row < pane_rows && nav_col < pane_cols {
                    let px0 = rect.x + crate::PANE_H_PAD + nav_col as f32 * self.cell_width;
                    let py0 = rect.y + nav_row as f32 * self.cell_height;
                    // The traversal cursor takes the same unfocused form as the
                    // shell's (see `build_bg_vertices_offset`): block-shaped, a
                    // hollow outline; bar- or underline-shaped, a faded fill.
                    if pane.cursor_unfocused && pane.cursor_shape == CursorShape::Block {
                        for (qx0, qy0, qx1, qy1) in cursor_outline_quads(
                            px0,
                            py0,
                            self.cell_width,
                            self.cell_height,
                            CURSOR_HOLLOW_STROKE_WIDTH,
                        ) {
                            all_bg_verts.extend_from_slice(&quad_vertices(
                                qx0,
                                qy0,
                                qx1,
                                qy1,
                                self.theme.cursor_bg.as_linear(),
                                surface_w,
                                surface_h,
                            ));
                        }
                    } else {
                        let (qx0, qy0, qx1, qy1) = cursor_quad(
                            pane.cursor_shape,
                            px0,
                            py0,
                            self.cell_width,
                            self.cell_height,
                            false,
                        );
                        let color = if pane.cursor_unfocused {
                            lerp_to_bg(
                                self.theme.cursor_bg.as_linear(),
                                self.theme.background.as_linear(),
                            )
                        } else {
                            self.theme.cursor_bg.as_linear()
                        };
                        all_bg_verts.extend_from_slice(&quad_vertices(
                            qx0, qy0, qx1, qy1, color, surface_w, surface_h,
                        ));
                    }
                }
            }

            let mut default_attrs =
                Attrs::new()
                    .family(base_family(fam.as_deref()))
                    .weight(parse_weight(
                        self.normal_weight.as_deref(),
                        glyphon::cosmic_text::Weight::NORMAL,
                    ));
            if let Some(ff) = &lig_off_features {
                default_attrs = default_attrs.font_features(ff.clone());
            }
            let mut rows_data: Vec<(String, glyphon::AttrsList)> = Vec::with_capacity(pane_rows);

            let theme_bg = self.theme.background;
            let pane_dim = pane.dim;
            let dim_text = |r: u8, g: u8, b: u8| -> Color {
                if pane_dim {
                    let lerp = |v: u8, bv: u8| -> u8 {
                        (v as f32 + (bv as f32 - v as f32) * DIM_FACTOR).round() as u8
                    };
                    Color::rgba(
                        lerp(r, theme_bg.r),
                        lerp(g, theme_bg.g),
                        lerp(b, theme_bg.b),
                        255,
                    )
                } else {
                    Color::rgba(r, g, b, 255)
                }
            };

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
                    let cell = grid.visible_cell(row, col);

                    // The right half of a double-width char carries no glyph; skip
                    // it so the wide glyph from the left half spans both columns.
                    if matches!(cell.map(|c| c.width), Some(CellWidth::Spacer)) {
                        continue;
                    }

                    let label = label_map.get(&(row, col)).copied();
                    let label_char = label.map(|(ch, _)| ch);
                    // Set when this cell is under the block cursor and its own
                    // color would vanish into the cursor fill.
                    let cursor_text_fg: Option<(u8, u8, u8)> =
                        block_cursor.filter(|&pos| pos == (row, col)).and_then(|_| {
                            cursor_contrast_fg(cell_text_fg(cell, &self.theme), &self.theme)
                        });
                    let raw = label_char.unwrap_or_else(|| cell.map(|c| c.ch).unwrap_or(' '));
                    // When the font lacks braille, it is painted as quads in the
                    // background pass instead; emit a space here so the glyph
                    // layer doesn't draw a misaligned, proportionally-shaped
                    // fallback on top of it. When the font has braille, keep it
                    // so the font renders it directly (crisp and anti-aliased).
                    let ch = if label_char.is_none() && !self.font_has_braille && is_braille(raw) {
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
                    let quad_scale = self.ensure_fallback_glyph_quad(ch, tail, is_wide);
                    if quad_scale.is_some() {
                        text.push(' ');
                    } else {
                        text.push(ch);
                        // `tail` rides along in the same shaping run as `ch` so
                        // cosmic-text's GSUB rules can compose them into one glyph.
                        if let Some(tail) = tail {
                            text.push_str(tail);
                        }
                    }

                    if let Some(scale) = quad_scale {
                        // Selection highlights only the background (see the
                        // span below); the glyph keeps its own foreground
                        // whether or not it falls inside the selection.
                        let color = if let Some((_, label_color)) = label {
                            label_color
                        } else if let Some((r, g, b)) = cursor_text_fg {
                            dim_text(r, g, b)
                        } else {
                            match cell.map(|c| c.style.foreground) {
                                Some(GridColor::Rgb(rgb)) => dim_text(rgb.r, rgb.g, rgb.b),
                                Some(GridColor::Indexed(idx)) => {
                                    let (r, g, b) = theme_indexed_color(&self.theme, idx);
                                    dim_text(r, g, b)
                                }
                                _ => {
                                    let fg = self.theme.foreground;
                                    dim_text(fg.r, fg.g, fg.b)
                                }
                            }
                        };
                        let key = glyph_key(ch, tail);
                        if let Some((w, h)) = self.glyph_quad_pass.dims(&key) {
                            let quad_w = w as f32 * scale;
                            let quad_h = h as f32 * scale;
                            // A wide glyph's quad centers across the two columns it
                            // occupies (this cell plus its Spacer), not just the first.
                            let target_width = if is_wide {
                                self.cell_width * 2.0
                            } else {
                                self.cell_width
                            };
                            let cell_x = rect.x + crate::PANE_H_PAD + col as f32 * self.cell_width;
                            let cell_y = rect.y + row as f32 * self.cell_height;
                            all_glyph_quads.push(GlyphQuadPlacement {
                                color: (
                                    color.r() as f32 / 255.0,
                                    color.g() as f32 / 255.0,
                                    color.b() as f32 / 255.0,
                                ),
                                height: quad_h,
                                key,
                                width: quad_w,
                                x: cell_x + (target_width - quad_w) / 2.0,
                                y: cell_y + (self.cell_height - quad_h) / 2.0,
                            });
                        }
                    } else if let Some((_, label_color)) = label {
                        let label_attrs = Attrs::new()
                            .family(base_family(fam.as_deref()))
                            .weight(effective_bold_weight(
                                self.bold_weight.as_deref(),
                                self.font_has_bold,
                            ))
                            .color(label_color);
                        attrs_list.add_span(start..text.len(), &label_attrs);
                    } else if sel_norm.is_some_and(|(sr1, sc1, sr2, sc2)| {
                        (abs_row, col) >= (sr1, sc1) && (abs_row, col) <= (sr2, sc2)
                    }) {
                        // The selection highlights the background only; the
                        // glyph keeps the cell's own foreground (reverse video
                        // still swaps, as it would unselected) so selected
                        // text reads the same as the rest of the line.
                        let sel_bg = self.theme.selection_bg;
                        let text_fg =
                            cursor_text_fg.unwrap_or_else(|| cell_text_fg(cell, &self.theme));
                        let mut span_attrs = Attrs::new()
                            .family(base_family(fam.as_deref()))
                            .color(dim_text(text_fg.0, text_fg.1, text_fg.2));
                        let cell_bold = cell.is_some_and(|c| c.style.bold);
                        if cell_bold
                            || needs_dark_on_light_bold(text_fg, (sel_bg.r, sel_bg.g, sel_bg.b))
                        {
                            span_attrs = span_attrs.weight(effective_bold_weight(
                                self.bold_weight.as_deref(),
                                self.font_has_bold,
                            ));
                        }
                        if cell.is_some_and(|c| c.style.italic) {
                            span_attrs = span_attrs.style(glyphon::cosmic_text::Style::Italic);
                        }
                        // Disabling ligature features would also block the GSUB
                        // substitutions some fonts use to compose emoji/ZWJ
                        // sequences into a single glyph, so only strip them for
                        // characters that don't need complex shaping.
                        if !needs_complex_shaping(ch) {
                            if let Some(ff) = &lig_off_features {
                                span_attrs = span_attrs.font_features(ff.clone());
                            }
                        }
                        attrs_list.add_span(start..text.len(), &span_attrs);
                    } else if let Some(cell) = cell {
                        let mut attrs = Attrs::new().family(base_family(fam.as_deref()));

                        let fg_rgb = match cell.style.foreground {
                            GridColor::Rgb(rgb) => (rgb.r, rgb.g, rgb.b),
                            GridColor::Indexed(idx) => theme_indexed_color(&self.theme, idx),
                            GridColor::Default => {
                                let fg = self.theme.foreground;
                                (fg.r, fg.g, fg.b)
                            }
                        };
                        let bg_rgb = match cell.style.background {
                            GridColor::Rgb(rgb) => (rgb.r, rgb.g, rgb.b),
                            GridColor::Indexed(idx) => theme_indexed_color(&self.theme, idx),
                            GridColor::Default => {
                                let bg = self.theme.background;
                                (bg.r, bg.g, bg.b)
                            }
                        };
                        // SGR 7 (reverse video): the glyph paints in what would
                        // otherwise be the background, matching the swapped
                        // highlight quad drawn in the background pass.
                        let text_fg = if cell.style.reversed { bg_rgb } else { fg_rgb };
                        // A glyph that would be lost inside the block cursor's
                        // fill is repainted in a contrasting color instead.
                        let text_fg = cursor_text_fg.unwrap_or(text_fg);
                        // An explicit cell background (an SGR color, not the pane's
                        // base background) or a reversed cell counts as a highlight,
                        // so it gets the same synthetic-bold compensation as a
                        // selection.
                        let is_highlighted = cell.style.reversed
                            || !matches!(cell.style.background, GridColor::Default);
                        let highlight_bg = if cell.style.reversed { fg_rgb } else { bg_rgb };
                        let bg_needs_bold =
                            is_highlighted && needs_dark_on_light_bold(text_fg, highlight_bg);

                        if cell.style.bold || bg_needs_bold {
                            attrs = attrs.weight(effective_bold_weight(
                                self.bold_weight.as_deref(),
                                self.font_has_bold,
                            ));
                        } else {
                            attrs = attrs.weight(parse_weight(
                                self.normal_weight.as_deref(),
                                glyphon::cosmic_text::Weight::NORMAL,
                            ));
                        }
                        if cell.style.italic {
                            attrs = attrs.style(glyphon::cosmic_text::Style::Italic);
                        }

                        let text_color_explicit = cell.style.foreground != GridColor::Default
                            || cell.style.reversed
                            || cursor_text_fg.is_some()
                            || bracket_map.contains_key(&(row, col));
                        if let Some((r, g, b)) = bracket_map.get(&(row, col)) {
                            // Rainbow parens recolor the bracket glyph but keep
                            // the cell's own bold/italic — and yield to the
                            // cursor-contrast fix above, which exists to keep
                            // the glyph visible at all.
                            let fg = cursor_text_fg.unwrap_or((*r, *g, *b));
                            attrs = attrs.color(dim_text(fg.0, fg.1, fg.2));
                        } else if text_color_explicit {
                            attrs = attrs.color(dim_text(text_fg.0, text_fg.1, text_fg.2));
                        }

                        // Same as the selection span above: complex-shaping
                        // characters (emoji, CJK, accents) keep the font's
                        // default GSUB features so composed glyphs still form
                        // when ligatures are otherwise disabled. This needs its
                        // own span even in the common default-color/no-bold/
                        // no-italic case, since that case otherwise inherits
                        // `default_attrs`, which does carry the disabling
                        // features.
                        let is_complex = needs_complex_shaping(ch);
                        if !is_complex {
                            if let Some(ff) = &lig_off_features {
                                attrs = attrs.font_features(ff.clone());
                            }
                        }

                        if text_color_explicit
                            || cell.style.bold
                            || cell.style.italic
                            || bg_needs_bold
                            || (is_complex && lig_off_features.is_some())
                        {
                            attrs_list.add_span(start..text.len(), &attrs);
                        }
                    }
                }

                rows_data.push((text, attrs_list));
            }

            if pane_idx >= text_buffers.len() {
                text_buffers.push(glyphon::Buffer::new(
                    &mut self.font_system,
                    glyphon::Metrics::new(self.font_size, self.line_height),
                ));
            }
            let buffer = &mut text_buffers[pane_idx];
            // Do NOT call `buffer.set_monospace_width` here; it is not a no-op.
            // At fractional physical font sizes (fractional DPI scale: logical
            // 15px at 125% is 18.75px) cosmic-text 0.18.2 quantizes the requested
            // advance to a coarser grid (measured: a Cascadia Code "M" asked for
            // 10.986px but then rendered at 11.133px), so glyph advances stop
            // equaling `self.cell_width`. The cursor and cell backgrounds are
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
            let row_count = rows_data.len();
            for (i, (text, attrs_list)) in rows_data.into_iter().enumerate() {
                // Basic shaping keeps glyphs on the primary monospace font at
                // native cell-width advances, which is what the grid relies on
                // for column alignment. Only escalate to Advanced shaping (font
                // fallback + complex shaping) for characters that actually need
                // it (emoji, CJK, accents): forcing the box-drawing and braille
                // ranges through Advanced picks fallback glyphs with non-cell
                // advances, which drifts and breaks TUIs like btop.
                let advanced = self.ligatures || text.chars().any(needs_complex_shaping);
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
                        self.ligatures || buffer.lines[i].text().chars().any(needs_complex_shaping);
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

        if panes.len() > 1 {
            for i in 0..panes.len() {
                for j in (i + 1)..panes.len() {
                    let a = panes[i].rect;
                    let b = panes[j].rect;
                    let divider = compute_divider(
                        a,
                        b,
                        surface_w,
                        surface_h,
                        self.theme.divider.as_linear(),
                        self.divider_width,
                    );
                    if let Some(dv) = divider {
                        all_bg_verts.extend_from_slice(&dv);
                    }
                }
            }
        }

        let mut status_buffer = self.status_buffer.take().unwrap_or_else(|| {
            glyphon::Buffer::new(
                &mut self.font_system,
                glyphon::Metrics::new(self.font_size, self.line_height),
            )
        });
        // Exactly one cell row, flush with the window's bottom edge.
        let status_top = crate::status_bar_top_px(surface_h, self.cell_height);

        if let Some(status) = status {
            all_bg_verts.extend_from_slice(&quad_vertices(
                0.0,
                status_top,
                surface_w,
                status_top + 1.0,
                self.theme.status_bar_border.as_linear(),
                surface_w,
                surface_h,
            ));

            all_bg_verts.extend_from_slice(&quad_vertices(
                0.0,
                status_top + 1.0,
                surface_w,
                status_top + crate::STATUS_BAR_HEIGHT * self.cell_height,
                self.theme.background.as_linear(),
                surface_w,
                surface_h,
            ));

            // Build status bar text segments dynamically
            let mut status_text = String::new();
            let mut spans = Vec::new();

            // Font attributes
            let accent_attrs = Attrs::new()
                .family(base_family(fam.as_deref()))
                .weight(effective_bold_weight(
                    self.bold_weight.as_deref(),
                    self.font_has_bold,
                ))
                .color(status.accent.to_glyphon());
            let muted_attrs = Attrs::new()
                .family(base_family(fam.as_deref()))
                .weight(parse_weight(
                    self.normal_weight.as_deref(),
                    glyphon::cosmic_text::Weight::NORMAL,
                ))
                .color(self.theme.ansi[8].to_glyphon());
            let error_attrs = Attrs::new()
                .family(base_family(fam.as_deref()))
                .weight(effective_bold_weight(
                    self.bold_weight.as_deref(),
                    self.font_has_bold,
                ))
                .color(self.theme.ansi[1].to_glyphon());
            let info_attrs = Attrs::new()
                .family(base_family(fam.as_deref()))
                .weight(effective_bold_weight(
                    self.bold_weight.as_deref(),
                    self.font_has_bold,
                ))
                .color(self.theme.ansi[4].to_glyphon());

            // Mode label (e.g. Normal, Insert, Block)
            let mode_start = status_text.len();
            status_text.push_str(&status.mode);
            let mode_end = status_text.len();
            spans.push((mode_start..mode_end, accent_attrs));

            // The live `/` search query follows the mode label while a search
            // is active, showing the query text and its match position; hidden
            // once the search is cancelled (`status.search` is `None`).
            if let Some(ref search) = status.search {
                let sep_start = status_text.len();
                status_text.push_str("  •  ");
                let sep_end = status_text.len();
                spans.push((sep_start..sep_end, muted_attrs.clone()));

                let query_start = status_text.len();
                status_text.push(if search.reverse { '?' } else { '/' });
                status_text.push_str(&search.query);
                let query_end = status_text.len();
                spans.push((query_start..query_end, info_attrs.clone()));

                if search.match_total > 0 {
                    let count_start = status_text.len();
                    status_text
                        .push_str(&format!("  {}/{}", search.match_index, search.match_total));
                    let count_end = status_text.len();
                    spans.push((count_start..count_end, muted_attrs.clone()));
                } else if !search.query.is_empty() {
                    let none_start = status_text.len();
                    status_text.push_str("  no matches");
                    let none_end = status_text.len();
                    spans.push((none_start..none_end, muted_attrs.clone()));
                }
            }

            // A transient notice follows the mode label: red for errors, green
            // for info confirmations (e.g. "Copied to clipboard").
            if let Some(ref notice) = status.notice {
                let sep_start = status_text.len();
                status_text.push_str("  •  ");
                let sep_end = status_text.len();
                spans.push((sep_start..sep_end, muted_attrs.clone()));

                let notice_attrs = match notice.kind {
                    NoticeKind::Error => error_attrs,
                    NoticeKind::Info => info_attrs,
                };
                let notice_start = status_text.len();
                status_text.push_str(&notice.text);
                let notice_end = status_text.len();
                spans.push((notice_start..notice_end, notice_attrs));
            }

            // Apply attributes to the text buffer line
            let default_attrs = Attrs::new()
                .family(base_family(fam.as_deref()))
                .color(self.theme.ansi[8].to_glyphon());
            let mut attrs_list = glyphon::AttrsList::new(&default_attrs);
            for (range, attrs) in spans {
                attrs_list.add_span(range, &attrs);
            }

            let ending = glyphon::cosmic_text::LineEnding::default();
            if status_buffer.lines.is_empty() {
                status_buffer.lines.push(BufferLine::new(
                    &status_text,
                    ending,
                    attrs_list,
                    Shaping::Advanced,
                ));
            } else {
                status_buffer.lines[0].set_text(&status_text, ending, attrs_list);
            }
            status_buffer.lines.truncate(1);
            status_buffer.shape_until_scroll(&mut self.font_system, false);
        }

        // Top tabbar (tabbar/menubar) bands and text. The dropdown overlay is
        // handled separately via the image pass so it sits above pane text.
        let tabbar_texts = match tabbar {
            Some(c) => self.draw_tabbar(c, surface_w),
            None => Vec::new(),
        };

        let bg_count = all_bg_verts.len() as u32;
        let bg_bytes: Vec<u8> = all_bg_verts.iter().flat_map(|v| v.to_bytes()).collect();
        self.queue.write_buffer(&self.bg_buffer, 0, &bg_bytes);

        // An undecorated window has no OS-drawn frame, so the renderer draws its
        // own 1px outline around the outer edge. Kept in a separate vertex range
        // appended after `bg_bytes` and drawn last (see below), so it sits on top
        // of the tabbar strip, glyphs, and image blocks instead of being painted
        // over by them.
        let mut border_verts = Vec::new();
        if !self.decorated {
            let border = self.theme.window_border.as_linear();
            border_verts.extend_from_slice(&quad_vertices(
                0.0, 0.0, surface_w, 1.0, border, surface_w, surface_h,
            ));
            border_verts.extend_from_slice(&quad_vertices(
                0.0,
                surface_h - 1.0,
                surface_w,
                surface_h,
                border,
                surface_w,
                surface_h,
            ));
            border_verts.extend_from_slice(&quad_vertices(
                0.0, 0.0, 1.0, surface_h, border, surface_w, surface_h,
            ));
            border_verts.extend_from_slice(&quad_vertices(
                surface_w - 1.0,
                0.0,
                surface_w,
                surface_h,
                border,
                surface_w,
                surface_h,
            ));
        }
        let border_count = border_verts.len() as u32;
        let border_offset = bg_bytes.len() as wgpu::BufferAddress;
        if border_count > 0 {
            let border_bytes: Vec<u8> = border_verts.iter().flat_map(|v| v.to_bytes()).collect();
            self.queue
                .write_buffer(&self.bg_buffer, border_offset, &border_bytes);
        }

        // Cap braille dots to the buffer capacity (whole quads of 6 vertices) so
        // an unusually dense frame can't overrun it; dropped dots just don't draw.
        let max_dot_verts = (DOT_BUFFER_SIZE as usize / std::mem::size_of::<DotVertex>()) / 6 * 6;
        if all_dot_verts.len() > max_dot_verts {
            all_dot_verts.truncate(max_dot_verts);
        }
        let dot_count = all_dot_verts.len() as u32;
        let dot_bytes: Vec<u8> = all_dot_verts.iter().flat_map(|v| v.to_bytes()).collect();
        if dot_count > 0 {
            self.queue.write_buffer(&self.dot_buffer, 0, &dot_bytes);
        }

        // The top-tabbar strip (band + rounded tab pills) is composited before the
        // text pass so the tab cards sit under the tab titles.
        let tabbar_strip: Vec<ImagePlacement> = tabbar
            .and_then(|c| self.rasterize_tabbar_strip(c, surface_w))
            .into_iter()
            .collect();
        self.tabbar_strip_pass
            .prepare(&self.queue, &tabbar_strip, surface_w, surface_h);

        // The open dropdown and command palette are rasterized to textures and
        // drawn by the image pass (after the text pass) so they overlay content.
        let mut all_images: Vec<ImagePlacement> = images.to_vec();
        if let Some(c) = tabbar {
            all_images.extend(self.rasterize_dropdown(c, surface_w));
            if let Some(placement) = self.rasterize_url_tooltip(c, surface_w, surface_h) {
                all_images.push(placement);
            }
        }
        if let Some(p) = palette {
            all_images.extend(self.rasterize_palette(p, surface_w, surface_h));
        }
        if let Some(t) = toast {
            if let Some(placement) = self.rasterize_toast(t, surface_w) {
                all_images.push(placement);
            }
        }
        if let Some(wk) = which_key {
            all_images.extend(self.rasterize_which_key(wk, surface_w, surface_h));
        }
        self.image_pass
            .prepare(&self.queue, &all_images, surface_w, surface_h);

        self.glyph_quad_pass
            .prepare(&self.queue, &all_glyph_quads, surface_w, surface_h);

        let mut text_areas: Vec<TextArea> = text_buffers
            .iter()
            .zip(panes.iter())
            .map(|(buffer, pane)| {
                let fg = self.theme.foreground;
                let bg = self.theme.background;
                let default_color = if pane.dim {
                    let lerp = |v: u8, bv: u8| -> u8 {
                        (v as f32 + (bv as f32 - v as f32) * DIM_FACTOR).round() as u8
                    };
                    Color::rgba(lerp(fg.r, bg.r), lerp(fg.g, bg.g), lerp(fg.b, bg.b), 255)
                } else {
                    fg.to_glyphon()
                };
                TextArea {
                    buffer,
                    left: (pane.rect.x + crate::PANE_H_PAD).round(),
                    top: pane.rect.y.round(),
                    bounds: TextBounds {
                        left: (pane.rect.x + crate::PANE_H_PAD).round() as i32,
                        top: pane.rect.y.round() as i32,
                        right: (pane.rect.x + pane.rect.width - crate::PANE_H_PAD).round() as i32,
                        bottom: (pane.rect.y + pane.rect.height).round() as i32,
                    },
                    default_color,
                    scale: 1.0,
                    custom_glyphs: &[],
                }
            })
            .collect();

        if status.is_some() {
            text_areas.push(TextArea {
                buffer: &status_buffer,
                left: 0.0,
                top: (status_top
                    + 1.0
                    + (crate::STATUS_BAR_HEIGHT * self.cell_height - 1.0 - self.line_height) / 2.0)
                    .round(),
                bounds: TextBounds {
                    left: 0,
                    top: status_top.round() as i32,
                    right: surface_w as i32,
                    bottom: (status_top + crate::STATUS_BAR_HEIGHT * self.cell_height).round()
                        as i32,
                },
                default_color: self.theme.status_bar_fg.to_glyphon(),
                scale: 1.0,
                custom_glyphs: &[],
            });
        }

        for text in &tabbar_texts {
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

        // glyphon returns `AtlasFull` when ordinary heavy content (large
        // CJK/emoji dumps, a big font size, many panes) outgrows the glyph
        // atlas. That's reachable from real PTY content, not just a bug, so
        // skip this frame the same way `acquire_surface_texture` already
        // degrades on a transient surface error instead of panicking.
        if !prepared {
            return;
        }

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
                        load: LoadOp::Clear(wgpu::Color {
                            r: srgb_to_linear_f64(self.theme.background.r),
                            g: srgb_to_linear_f64(self.theme.background.g),
                            b: srgb_to_linear_f64(self.theme.background.b),
                            a: 1.0,
                        }),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            if bg_count > 0 {
                pass.set_pipeline(&self.bg_pipeline);
                pass.set_vertex_buffer(0, self.bg_buffer.slice(..));
                pass.draw(0..bg_count, 0..1);
            }

            // Anti-aliased braille dots blend over the cell backgrounds.
            if dot_count > 0 {
                pass.set_pipeline(&self.dot_pipeline);
                pass.set_vertex_buffer(0, self.dot_buffer.slice(..));
                pass.draw(0..dot_count, 0..1);
            }

            // Tabbar strip sits above the bg quads but below the text so the
            // rounded tab cards back the tab titles.
            self.tabbar_strip_pass.render(&mut pass);

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

            self.image_pass.render(&mut pass);

            // Drawn last so the window border sits on top of the tabbar strip,
            // glyphs, and image blocks instead of being painted over by them.
            if border_count > 0 {
                pass.set_pipeline(&self.bg_pipeline);
                pass.set_vertex_buffer(0, self.bg_buffer.slice(border_offset..));
                pass.draw(0..border_count, 0..1);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        surface_texture.present();
    }
}
