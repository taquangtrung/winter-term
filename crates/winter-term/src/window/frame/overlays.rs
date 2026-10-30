//! What the frame draws over the panes: the editors' carets, a content band,
//! the overlays a pane wears, and the icons it places.

use std::collections::HashMap;

use crate::model::layout::{Rect, TabId};
use crate::model::paint::Icon;
use winter_render::{ImagePlacement, Theme};

use crate::window::terminal::navigation::reading::{bracket_marks, resolve_bracket_colors, sentence_spans};
use crate::window::App;

use super::compose::{EditorPaint, PaneCaret, PaneOverlays};

// ========================================================================
// App: what is drawn over the panes
// ========================================================================

impl App {
    /// Where each editor tab's block cursor sits.
    ///
    /// Every editor tab shows one, not only one that `v` has started a text
    /// cursor in. The row band says which entry is current; the cursor says
    /// which cell a selection would start from, and without it an editor looks
    /// like it has no cursor at all.
    pub(super) fn editor_carets(&self) -> HashMap<TabId, PaneCaret> {
        self.open_tabs
            .editors()
            .filter_map(|(id, slot)| {
                if let Some(cursor) = self.editor_cursor.as_ref().filter(|c| c.tab == *id) {
                    return Some((*id, PaneCaret::at(cursor.row, cursor.col)));
                }
                let row = slot.view.cursor_line?;
                // An editor that edits text says which cell it is on; one whose
                // cursor is a whole row leaves it to the row's first painted
                // character, which is where the eye starts reading it.
                if let Some(caret) = slot.editor.caret() {
                    return Some((
                        *id,
                        PaneCaret {
                            col: caret.col,
                            insert: caret.insert,
                            row,
                        },
                    ));
                }
                let col = self.editor_column(*id, row)?;
                Some((*id, PaneCaret::at(row, col)))
            })
            .collect()
    }

    /// The band of the window the panes are laid out in.
    pub(super) fn content_band(
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
            winter_render::status_bar_height_px(ch)
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
    pub(super) fn build_pane_overlays(&self, rects: &[(TabId, Rect)], theme: &Theme) -> PaneOverlays {
        let qs_labels: Vec<(usize, usize, char)> = self
            .chrome.labels
            .quick
            .as_ref()
            .map(|labels| labels.iter().map(|ql| (ql.row, ql.col, ql.label)).collect())
            .unwrap_or_default();
        // The `f`/`t` jump overlay's labels, for the focused pane only (that's
        // where the cursor being moved lives).
        let find_label_data: Vec<(usize, usize, char)> = self
            .chrome.labels
            .find
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
                        sentence_spans(terminal.grid())
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
                        let marks = bracket_marks(terminal.grid());
                        resolve_bracket_colors(&marks, theme)
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
    pub(super) fn icon_texture(
        &mut self,
        name: &str,
        full: (u32, u32),
        box_w: u32,
        box_h: u32,
    ) -> Option<u64> {
        if self.textures.icon_box != full {
            for id in self.textures.icons.values().flatten() {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.free_image(*id);
                }
            }
            self.textures.icons.clear();
            self.textures.icon_box = full;
        }
        let key = (name.to_string(), box_w, box_h);
        if let Some(cached) = self.textures.icons.get(&key) {
            return *cached;
        }
        let id = self.textures.next_image_id;
        let uploaded = crate::icons::named_svg(name).is_some_and(|svg| {
            self.renderer
                .as_mut()
                .is_some_and(|renderer| renderer.upload_svg_in_box(id, &svg, box_w, box_h))
        });
        let slot = uploaded.then(|| {
            self.textures.next_image_id += 1;
            id
        });
        self.textures.icons.insert(key, slot);
        slot
    }

    /// Quads for every icon the editors want drawn this frame.
    ///
    /// Recomputed from scratch each frame rather than anchored, because an editor
    /// repaints its rows from its own model every frame anyway: there is no
    /// scrollback row for an icon to drift away from.
    pub(super) fn editor_icon_placements(
        &mut self,
        paints: &[EditorPaint],
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
    pub(super) fn hovered_pane(&self, rects: &[(TabId, Rect)]) -> Option<TabId> {
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
}
