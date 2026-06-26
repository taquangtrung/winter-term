//! Blocks in the scrollback: fold, yank, focus, and navigate operations, and
//! the natively drawn ones: drawn on the canvas, placed in the band their
//! rows reserve, kept in step as panes resize and output scrolls.

use base64::Engine;

use crate::model::input;
use crate::model::layout::{Rect, TabId};
use crate::model::mode::Mode;
use crate::terminal::block_queue::{BlockEntry, BlockKind};
use crate::terminal::session::{Terminal, MAX_IMAGE_ROWS};
use std::sync::Arc;

use serde_json::Value;
use winter_core::winter_proto::{EmitBlock, TEXT_PLAIN};
use winter_render::renderer::GpuRenderer;
use winter_render::ImagePlacement;

use crate::chart::{self, ChartStyle};
use crate::model::picture::{Content, PictureClip, PictureFrame};

use super::block_text::{csv_to_table, json_to_text};
use super::canvas::{own_size, upload, Fit};
use super::open_tabs::OpenTabs;
use super::render::ToolPaint;
use super::{App, ImageBlock};

// ========================================================================
// Constants
// ========================================================================

/// Raster image MIME types, decoded and drawn at their own size.
const RASTER_MIMES: [&str; 4] = ["image/gif", "image/jpeg", "image/png", "image/webp"];

const CSV_MIME: &str = "text/csv";

const JSON_MIME: &str = "application/json";

const MARKDOWN_MIME: &str = "text/markdown";

const SVG_MIME: &str = "image/svg+xml";

const VEGA_LITE_MIME: &str = "application/vnd.vega-lite+json";

const HTML_MIME: &str = "text/html";

/// Every representation a block is drawn from, richest first: the first one
/// a block's bundle carries that the canvas can draw is the one it shows. The
/// plain text every bundle carries comes last, for what nothing else covers.
const RICHNESS: [&str; 11] = [
    VEGA_LITE_MIME,
    HTML_MIME,
    SVG_MIME,
    MARKDOWN_MIME,
    CSV_MIME,
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    JSON_MIME,
    TEXT_PLAIN,
];

/// HTML elements an HTML block's text leaves out: what would run, and how it
/// would be styled, neither of which a text rendering has any use for.
const HTML_SKIPPED: [&str; 3] = ["script", "style", "noscript"];

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

// ========================================================================
// App: block operations
// ========================================================================

impl App {
    pub(crate) fn yank_block_source(&mut self, focused: TabId) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let text = self
            .block_in_view(focused)
            .and_then(|idx| terminal.scrollback().blocks().get(idx))
            .map(|block| block.plain_text())
            .filter(|text| !text.is_empty());
        // Borrows of `pane` end here, freeing `self` for the clipboard handle.
        let Some(text) = text else {
            return;
        };
        if let Some(clipboard) = self.clipboard() {
            let copied = clipboard.set_text(&text).is_ok();
            if copied {
                self.set_notice("Copied to clipboard");
            }
        }
    }

    /// Export the plain-text content of the focused block to the system clipboard.
    pub(crate) fn export_focused_block_text(&mut self, focused: TabId) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let text = self
            .block_in_view(focused)
            .and_then(|idx| terminal.scrollback().blocks().get(idx))
            .map(|block| {
                let mut content = format!("$ {}\n", block.command);
                content.push_str(&block.plain_text());
                content
            });
        let Some(text) = text else {
            self.set_error("No block content to export");
            return;
        };
        if let Some(clipboard) = self.clipboard() {
            if clipboard.set_text(&text).is_ok() {
                self.set_notice("Copied block text to clipboard");
            } else {
                self.set_error("Could not copy to clipboard");
            }
        }
    }

    /// Export the SVG graphic in the focused block (if any) to a temporary file and open it.
    pub(crate) fn export_focused_block_svg(&mut self, focused: TabId) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let svg = self
            .block_in_view(focused)
            .and_then(|idx| terminal.scrollback().blocks().get(idx))
            .and_then(|block| block.svg_content());
        let Some(svg) = svg else {
            self.set_error("Focused block has no SVG graphic");
            return;
        };
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("winter-block-{timestamp}.svg"));
        if let Err(e) = std::fs::write(&path, svg) {
            self.set_error(format!("Could not write SVG file: {e}"));
            return;
        }
        match ::open::that_detached(&path) {
            Ok(()) => self.set_notice(format!("Opened {}", path.display())),
            Err(e) => self.set_error(format!("Could not open SVG file: {e}")),
        }
    }

    pub(crate) fn toggle_fold(&mut self, focused: TabId) {
        let Some(idx) = self.block_in_view(focused) else {
            return;
        };

        let Some(tab) = self.open_tabs.terminal_tab_mut(&focused) else {
            return;
        };
        if !tab.folded_blocks.remove(&idx) {
            tab.folded_blocks.insert(idx);
        }
        self.dirty = true;
    }

    pub(crate) fn is_block_folded(&self, tab_id: TabId, block_index: usize) -> bool {
        self.open_tabs
            .terminal_tab(&tab_id)
            .is_some_and(|tab| tab.folded_blocks.contains(&block_index))
    }

    /// Scroll the focused pane so the next or previous block's first row is
    /// at the top of the view. Past the newest block the view returns to the
    /// live bottom; before the oldest it stays on the oldest.
    pub(crate) fn focus_block(&mut self, nav: input::BlockNav, focused: TabId) {
        let Some(terminal) = self.open_tabs.terminal_mut(&focused) else {
            return;
        };
        let top = terminal.grid().to_absolute_row(0);
        let starts = terminal.block_rows();
        let target = match nav {
            input::BlockNav::Next => starts.iter().copied().find(|&start| start > top),
            input::BlockNav::Previous => starts
                .iter()
                .copied()
                .rev()
                .find(|&start| start < top)
                .or_else(|| starts.first().copied()),
        };
        let grid = terminal.grid_mut();
        match target {
            Some(start) => grid.scroll_to_absolute_row(start),
            None => grid.set_scroll_offset(0),
        }
        self.dirty = true;
    }

    /// The block the focused pane's view is on: the one its Normal-mode
    /// cursor sits in; else, when the view is scrolled back, the one at its
    /// top row; else, at the live bottom, the newest block with output (the
    /// command that just finished, not the prompt waiting below it).
    pub(crate) fn block_in_view(&self, focused: TabId) -> Option<usize> {
        let terminal = self.open_tabs.terminal(&focused)?;
        let grid = terminal.grid();
        let in_normal = self
            .open_tabs
            .mode(&focused)
            .is_some_and(|mode| matches!(mode, Mode::Normal | Mode::Visual));
        let view_row = match self.nav_cursor(focused).filter(|_| in_normal) {
            Some((row, _)) => Some(row),
            None => (grid.scroll_offset() > 0).then_some(0),
        };
        match view_row {
            Some(row) => block_at_row(terminal.block_rows(), grid.to_absolute_row(row)),
            None => terminal
                .scrollback()
                .blocks()
                .iter()
                .rposition(|block| !block.output.is_empty()),
        }
    }
}

// ========================================================================
// App: native blocks
// ========================================================================

impl App {
    pub(crate) fn create_block_tiles(&mut self, entries: &[(TabId, BlockEntry)]) {
        let ch = match &self.renderer {
            Some(r) => r.cell_size().1,
            None => return,
        };
        let rects = self.pane_rects();
        let debug = std::env::var_os("WINTER_BLOCK_DEBUG").is_some();
        let Some(style) = self.chart_style() else {
            return;
        };

        for (tab_id, entry) in entries {
            if self.is_block_folded(*tab_id, entry.block_index) {
                continue;
            }
            let pane_rect = match rects.iter().find(|(id, _)| *id == *tab_id) {
                Some((_, r)) => Self::layout_rect_to_pane(*r).grid_area(ch),
                None => continue,
            };

            let Some(content) = native_content(&entry.emit, &style) else {
                continue;
            };
            // The band is always the rows the grid actually reserved,
            // never a constant re-picked here: any disagreement shows up
            // as dead space between the block and the next prompt, or as
            // a block overdrawing it.
            let mut block = ImageBlock {
                abs_row: entry.abs_row,
                block_index: entry.block_index,
                closed: entry.closed,
                content,
                drawn: None,
                id: self.next_image_id,
                max_rows: entry.reserved_rows,
                natural: None,
                segment_index: entry.segment_index,
                tab_id: *tab_id,
            };
            let Some(renderer) = self.renderer.as_mut() else {
                continue;
            };
            if !draw_block(renderer, &mut block, pane_rect.width, ch) {
                if debug {
                    eprintln!("winter: a block could not be drawn");
                }
                continue;
            }
            self.next_image_id += 1;
            let wraps = block.content.wraps();
            let natural = block.natural;
            if debug {
                eprintln!("winter: native block id={} {natural:?}", block.id);
            }
            self.image_blocks.push(block);
            // Markdown and text reserve the default band, because their
            // height depends on font metrics and a wrap width the PTY
            // thread cannot know. Now that the content is laid out its
            // real height is known, so give back what it did not need (or
            // take the rows it turned out to be short). Images and SVG
            // were reserved from their own size and scale to fit.
            //
            // Only for one-shot blocks: a live block is re-measured on
            // every patch, and fitting it here would make the rows below
            // it jitter as its content streams in.
            if let (true, true, Some((nat_w, nat_h))) =
                (entry.kind == BlockKind::Content, wraps, natural)
            {
                let pos = self.image_blocks.len() - 1;
                let content_h = native_content_height(nat_w, nat_h, pane_rect.width);
                self.set_band_rows(*tab_id, pos, block_rows(content_h, ch));
            }
        }
    }

    /// The colors a chart block is drawn in: the terminal's own.
    fn chart_style(&self) -> Option<ChartStyle> {
        let ink = self.renderer.as_ref()?.theme().foreground;
        Some(ChartStyle {
            foreground: (ink.r, ink.g, ink.b),
        })
    }

    /// Resize the band of `self.image_blocks[pos]` to `rows`, inserting or
    /// removing grid rows so the blank band behind a block matches the height
    /// its content actually needs.
    ///
    /// Everything anchored below the band follows: later blocks, the pane's
    /// pending anchors and queued entries, and the shell's cursor.
    /// Growing is refused when the taller band would not fit on screen (the
    /// block is clipped instead, as before); shrinking always fits by
    /// construction.
    fn set_band_rows(&mut self, tab_id: TabId, pos: usize, rows: usize) {
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
                .open_tabs
                .terminal(&tab_id)
                .is_some_and(|p| band_has_room(p, at, add))
            {
                return;
            }
            if let Some(terminal) = self.open_tabs.terminal_mut(&tab_id) {
                terminal.insert_band_rows(at, add);
            }
            self.shift_blocks_at_or_below(tab_id, at, add as isize);
        } else {
            let spare = have - rows;
            // The first row the block does not need; everything from there
            // down closes up against it.
            let at = abs_row + rows;
            if let Some(terminal) = self.open_tabs.terminal_mut(&tab_id) {
                terminal.remove_band_rows(at, spare);
            }
            self.shift_blocks_at_or_below(tab_id, at, -(spare as isize));
        }
        self.image_blocks[pos].max_rows = rows;
    }

    /// Drop every block of `tab_id` whose reserved band overlaps the erased
    /// absolute row span `[start, end)`.
    ///
    /// A block draws an image over grid rows it does not own; erasing those
    /// rows (a `clear`, or any full-screen erase) blanks the grid but cannot
    /// reach the image, so without this the block stays on screen painted over
    /// whatever the shell writes next.
    pub(crate) fn drop_blocks_in(&mut self, tab_id: TabId, (start, end): (usize, usize)) {
        self.retain_image_blocks(|block| {
            block.tab_id != tab_id
                || block.abs_row >= end
                || block.abs_row + block.max_rows <= start
        });
    }

    /// Drop the rendered blocks whose source segments the scrollback's
    /// retention budget elided: the content is gone, so the texture can never
    /// show anything again, and an image block's decoded texture is megabytes
    /// that would otherwise never come back.
    pub(crate) fn drop_elided_blocks(&mut self, elided: &[(TabId, usize, usize)]) {
        for &(tab_id, block_index, segment_index) in elided {
            self.retain_image_blocks(|block| {
                block.tab_id != tab_id
                    || block.block_index != block_index
                    || block.segment_index != segment_index
            });
        }
    }

    /// Push every absolute-row anchor the app holds for `tab_id` through a
    /// resize reflow's row remap (see [`winter_render::RowRemap`]): image
    /// blocks, the selection and its Visual anchor, the search
    /// cursor, vim marks and the jump/changelists. A resize rebuilds the live
    /// screen by replaying logical lines, so a line that re-wraps differently
    /// moves every row below it: an anchor left at its old row draws its
    /// block over the wrong content (the "block detached after resize"
    /// overlap).
    pub(crate) fn remap_pane_anchors(&mut self, tab_id: TabId, remap: &winter_render::RowRemap) {
        let map = |row: usize| remap.map(row);
        for block in &mut self.image_blocks {
            if block.tab_id == tab_id {
                block.abs_row = map(block.abs_row);
            }
        }
        if let Some(span) = &mut self.selection.span {
            if span.tab == tab_id {
                span.start_row = map(span.start_row);
                span.end_row = map(span.end_row);
            }
        }
        // The Visual anchor belongs to the focused pane alone (it is held
        // only while that pane is in Visual mode), so it follows this remap
        // only when that is the pane being remapped.
        let focused = self.tabs.layout.focused();
        if tab_id == focused {
            if let Some((row, _)) = &mut self.selection.visual_anchor {
                *row = map(*row);
            }
        }
        if let Some(last) = &mut self.selection.last_visual {
            if last.tab == tab_id {
                last.anchor.0 = map(last.anchor.0);
                last.cursor.0 = map(last.cursor.0);
            }
        }
        if let Some((search_pane, (row, _))) = &mut self.search.current {
            if *search_pane == tab_id {
                *row = map(*row);
            }
        }
        if let Some(vim) = self.vim.tabs.get_mut(&tab_id) {
            for (row, _) in vim.marks.values_mut() {
                *row = map(*row);
            }
            vim.jump_list.remap_rows(&map);
            vim.change_list.remap_rows(&map);
        }
        self.dirty = true;
    }

    /// Keep only the image blocks matching `keep`, freeing the GPU texture of
    /// every dropped one.
    ///
    /// An `ImageBlock` entry and its texture have separate lifetimes: the
    /// entry names the block, the texture caches its decoded pixels under the
    /// renderer's id. Every removal path must go through here, or the texture
    /// outlives its block for the life of the process, a few megabytes each
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
        self.dirty = true;
    }

    /// Move every block of `tab_id` anchored at or below
    /// absolute `row` by `delta` rows, after a band above them changed size.
    fn shift_blocks_at_or_below(&mut self, tab_id: TabId, row: usize, delta: isize) {
        for block in &mut self.image_blocks {
            if block.tab_id == tab_id && block.abs_row >= row {
                block.abs_row = block.abs_row.saturating_add_signed(delta);
            }
        }
    }

    pub(crate) fn update_live_tiles(&mut self, patched: &[(TabId, usize)]) {
        let Some(ch) = self
            .renderer
            .as_ref()
            .map(|renderer| renderer.cell_size().1)
        else {
            return;
        };
        let Some(style) = self.chart_style() else {
            return;
        };

        for (tab_id, entry_idx) in patched {
            let entry = match self.open_tabs.terminal(tab_id) {
                Some(p) => p.block_queue().entries().get(*entry_idx).cloned(),
                None => None,
            };
            let Some(entry) = entry else {
                continue;
            };

            // A live block takes the patched content and is drawn again in
            // its own texture. Its band grows to fit, but never shrinks.
            let Some(content) = native_content(&entry.emit, &style) else {
                continue;
            };
            let rects = self.pane_rects();
            let Some((_, rect)) = rects.iter().find(|(id, _)| *id == *tab_id) else {
                continue;
            };
            let pane_rect = Self::layout_rect_to_pane(*rect);
            let Some(pos) = self.image_blocks.iter().position(|b| {
                b.tab_id == *tab_id
                    && b.block_index == entry.block_index
                    && b.segment_index == entry.segment_index
            }) else {
                continue;
            };
            let Some(renderer) = self.renderer.as_mut() else {
                continue;
            };
            let block = &mut self.image_blocks[pos];
            block.closed = entry.closed;
            block.content = content;
            block.drawn = None;
            block.natural = None;
            if draw_block(renderer, block, pane_rect.width, ch) {
                // A patch can push the content past its band, so grow to
                // fit instead of clipping. Grow-only: shrinking back on
                // every patch would make the rows below it jitter.
                let nat_h = block.natural.map_or(0.0, |(_, height)| height);
                let want = block_rows(nat_h, ch).max(block.max_rows);
                self.set_band_rows(*tab_id, pos, want);
            }
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The index of the block absolute row `abs_row` belongs to: the newest one
/// starting at or above it, given each block's start row (see
/// [`Terminal::block_rows`]). `None` above the first block.
pub(crate) fn block_at_row(starts: &[usize], abs_row: usize) -> Option<usize> {
    starts.iter().rposition(|&start| start <= abs_row)
}

/// Draw every block again where the size it is shown at has changed: text
/// laid out to a pane whose width changed, and an SVG shown at a new size.
pub(super) fn refresh_block_textures(
    blocks: &mut [ImageBlock],
    rects: &[(TabId, Rect)],
    cell_height: f32,
    renderer: &mut GpuRenderer,
) {
    for block in blocks.iter_mut() {
        let Some((_, rect)) = rects.iter().find(|(id, _)| *id == block.tab_id) else {
            continue;
        };
        let pane_width = App::layout_rect_to_pane(*rect).width;
        draw_block(renderer, block, pane_width, cell_height);
    }
}

/// Draw `block`'s content into its texture as it is shown in a pane
/// `pane_width` wide, unless it is drawn that way already, and keep its own
/// size. `false` for content that cannot be drawn.
fn draw_block(
    renderer: &mut GpuRenderer,
    block: &mut ImageBlock,
    pane_width: f32,
    cell_height: f32,
) -> bool {
    if block.natural.is_none() {
        block.natural = own_size(renderer, &block.content);
    }
    let band = band_height(block.max_rows, cell_height);
    let shown = match block.natural {
        Some(natural) => block_display(&block.content, natural, pane_width, band),
        None => (pane_width, band),
    };
    let fit = Fit::shown(&block.content, shown);
    if block.drawn == Some(fit) {
        return true;
    }
    let Some(size) = upload(renderer, block.id, &block.content, fit) else {
        return false;
    };
    // An SVG keeps the size it declares; everything else's is what drawing
    // it came to.
    if !block.content.is_vector() {
        block.natural = Some(size);
    }
    block.drawn = Some(fit);
    true
}

/// The rows a block drawn on the canvas needs for content `content_h` pixels
/// tall: enough for the content under the band's top padding, which comes
/// out of the band, so the last line is not cropped.
fn block_rows(content_h: f32, cell_height: f32) -> usize {
    band_fit_rows(content_h + cell_height * BLOCK_PAD_TOP_RATIO, cell_height)
}

/// The pixels a band of `rows` rows leaves for its block, under the padding
/// at its top.
fn band_height(rows: usize, cell_height: f32) -> f32 {
    (rows as f32 * cell_height - cell_height * BLOCK_PAD_TOP_RATIO).max(0.0)
}

/// How big a block of `content`, `natural` big, is shown in a pane
/// `pane_width` wide with `band` pixels of its band to fill: text at its own
/// size, wrapped to the pane; images and SVG scaled down to fit the band.
fn block_display(content: &Content, natural: (f32, f32), pane_width: f32, band: f32) -> (f32, f32) {
    let (nat_w, nat_h) = natural;
    if nat_w <= 0.0 || nat_h <= 0.0 {
        return (0.0, 0.0);
    }
    match content.wraps() {
        true => (
            nat_w.min(pane_width),
            native_content_height(nat_w, nat_h, pane_width),
        ),
        false => {
            let scale = (pane_width / nat_w).min(band / nat_h).min(1.0);
            (nat_w * scale, nat_h * scale)
        }
    }
}

/// The part of a block's `display`-sized image still on screen, its band
/// starting `band_top` pixels below the top of a pane `pane_h` tall: cropped
/// to the `band_h` pixels the band holds, and to the pane.
///
/// `band_top` is signed on purpose. A band scrolls above the pane's first row
/// well before it leaves the viewport, and dropping it at that point makes a
/// tall block vanish the moment its first line does; cropping instead keeps
/// the rows still on screen aligned with the grid rows they belong to.
/// Returns `None` only once no part of the block is visible.
fn block_clip(band_top: f32, display: (f32, f32), band_h: f32, pane_h: f32) -> Option<PictureClip> {
    let frame = PictureFrame {
        height: display.1,
        scale: 1.0,
        width: display.0,
        x: 0.0,
        y: band_top,
    };
    let top = band_top.max(0.0);
    let bottom = (band_top + band_h).min(pane_h);
    frame.clip((0.0, top, display.0, bottom - top))
}

/// Whether `pane` can still absorb `extra` blank rows inserted at absolute row
/// `abs_row`, the first row past a band that has outgrown its reservation.
///
/// False once the row has scrolled into history (the grid only inserts into
/// its live rows) or once the insert would push the band past the bottom of
/// the screen. Both cases fall back to clipping the block to its band.
fn band_has_room(terminal: &Terminal, abs_row: usize, extra: usize) -> bool {
    let grid = terminal.grid();
    let Some(live_row) = abs_row.checked_sub(grid.absolute_live_top()) else {
        return false;
    };
    live_row + extra <= grid.rows()
}

/// Place native image blocks at their anchor row, scaled to fit the pane width
/// and preserving aspect, cropping any part outside the pane's content area.
pub(super) fn image_placements(
    blocks: &[ImageBlock],
    tabs: &OpenTabs,
    rects: &[(TabId, Rect)],
    cell_height: f32,
    covered: &[ToolPaint],
) -> Vec<ImagePlacement> {
    let mut placements: Vec<ImagePlacement> = Vec::new();
    for img in blocks {
        let Some((_, rect)) = rects.iter().find(|(id, _)| *id == img.tab_id) else {
            continue;
        };
        // A tool owns every row of its tab, the same way an
        // alternate-screen app does below.
        if covered.iter().any(|paint| paint.tab == img.tab_id) {
            continue;
        }
        let Some(terminal) = tabs.terminal(&img.tab_id) else {
            continue;
        };
        // While the pane shows the alternate screen, a full-screen app owns
        // every row of the viewport. A primary-screen block painted on top
        // would hide the app's own rows (four lines of vim, a htop gauge)
        // until it exits, so it is not drawn at all until the pane returns.
        if terminal.grid().is_alt_screen() {
            continue;
        }
        let pane_rect = App::layout_rect_to_pane(*rect).grid_area(cell_height);
        let top_row = terminal.grid().to_viewport_row(img.abs_row);
        let Some(natural) = img.natural else {
            continue;
        };
        // The image is inset below the band's first row by the top padding,
        // and gets whatever height the band has left after it.
        let pad_top = cell_height * BLOCK_PAD_TOP_RATIO;
        let band_top = top_row as f32 * cell_height + pad_top;
        let band_h = band_height(img.max_rows, cell_height);
        let display = block_display(&img.content, natural, pane_rect.width, band_h);
        let Some(clip) = block_clip(band_top, display, band_h, pane_rect.height) else {
            continue;
        };
        placements.push(ImagePlacement {
            alpha: if img.closed { CLOSED_BLOCK_ALPHA } else { 1.0 },
            height: clip.height,
            id: img.id,
            u_max: clip.u_max,
            u_min: clip.u_min,
            v_max: clip.v_max,
            v_min: clip.v_min,
            width: clip.width,
            x: pane_rect.x + clip.x,
            y: pane_rect.y + clip.y,
        });
    }
    placements
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
pub(super) fn band_fit_rows(content_h: f32, cell_height: f32) -> usize {
    if cell_height <= 0.0 || content_h <= 0.0 {
        return 1;
    }
    ((content_h / cell_height).ceil() as usize).clamp(1, MAX_IMAGE_ROWS)
}

/// An HTML fragment as Markdown: its headings, lists, tables, links, and
/// emphasis, with scripts and styles left out. Fragments that do not parse
/// as anything come back as their text.
fn html_text(html: &str) -> String {
    let converter = htmd::HtmlToMarkdown::builder()
        .skip_tags(HTML_SKIPPED.to_vec())
        .build();
    converter.convert(html).unwrap_or_else(|_| html.to_string())
}

/// What the canvas draws for a block: its richest representation that can
/// be drawn, a chart in `style`, falling back past one that cannot (a chart
/// Winter does not draw) to the next. `None` for a bundle carrying nothing to
/// draw at all.
fn native_content(emit: &EmitBlock, style: &ChartStyle) -> Option<Content> {
    RICHNESS
        .iter()
        .filter_map(|mime| Some((*mime, emit.bundle.get(mime)?)))
        .find_map(|(mime, value)| content_of(mime, value, style))
}

/// What the canvas draws for `value`, a representation of type `mime`, or
/// `None` when it cannot draw it.
fn content_of(mime: &str, value: &Value, style: &ChartStyle) -> Option<Content> {
    match mime {
        VEGA_LITE_MIME => {
            let svg = chart::render(value, style)?;
            Some(Content::Vector(Arc::from(svg.as_bytes())))
        }
        HTML_MIME => Some(Content::Markdown(html_text(value.as_str()?).into())),
        SVG_MIME => Some(Content::Vector(Arc::from(value.as_str()?.as_bytes()))),
        MARKDOWN_MIME => Some(Content::Markdown(value.as_str()?.into())),
        CSV_MIME => Some(Content::Text(csv_to_table(value.as_str()?).into())),
        JSON_MIME => Some(Content::Text(json_to_text(value).into())),
        TEXT_PLAIN => Some(Content::Text(value.as_str()?.into())),
        mime if RASTER_MIMES.contains(&mime) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(value.as_str()?)
                .ok()?;
            Some(Content::Raster(bytes.into()))
        }
        _ => None,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_block_at_row_finds_the_newest_block_at_or_above_the_row() {
        // Regression: `.find()` over an ascending list whose first element
        // is always 0 matched at once, so every lookup landed on the oldest
        // block whatever row was asked about.
        let starts = [0, 10, 25, 40];
        assert_eq!(block_at_row(&starts, 5), Some(0));
        assert_eq!(block_at_row(&starts, 10), Some(1));
        assert_eq!(block_at_row(&starts, 39), Some(2));
        assert_eq!(block_at_row(&starts, 100), Some(3));
        assert_eq!(block_at_row(&[3], 1), None, "above the first block");
        assert_eq!(
            block_at_row(&[0, 7, 7], 8),
            Some(2),
            "a shared row is the newest's"
        );
    }

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
    fn test_an_html_block_keeps_its_text_and_structure_and_drops_its_scripts() {
        let markdown = html_text(
            "<h2>Report</h2><script>steal()</script><style>p{}</style>\
             <table><tr><th>Name</th></tr><tr><td>Alice</td></tr></table>",
        );
        assert!(markdown.contains("## Report"));
        assert!(markdown.contains("| Alice |"), "{markdown}");
        assert!(!markdown.contains("steal"), "nothing that would run");
        assert!(!markdown.contains("p{}"));
    }

    #[test]
    fn test_a_block_s_band_holds_all_of_its_content_under_the_padding() {
        // Regression: the band was sized to the content alone, and the top
        // padding coming out of it cropped the last line of every Markdown
        // block.
        let cell_height = 20.0;
        for content_h in [10.0, 95.0, 100.0, 333.0] {
            let rows = block_rows(content_h, cell_height);
            assert!(
                band_height(rows, cell_height) >= content_h,
                "{content_h}px of content in {rows} rows"
            );
        }
    }

    #[test]
    fn test_block_clip_crops_the_top_instead_of_dropping_a_half_scrolled_block() {
        // Regression: a band whose first row scrolls above the pane used to be
        // skipped outright, so a tall block popped out of existence the moment
        // its top line left the viewport instead of sliding off it. Starting
        // 20px above a 100px-tall image means the top fifth is cropped and the
        // rest is drawn flush against the pane's top edge.
        let clip = block_clip(-20.0, (100.0, 100.0), 100.0, 400.0).expect("still visible");
        assert_eq!(clip.y, 0.0, "drawn flush with the pane's top edge");
        assert_eq!(clip.height, 80.0);
        assert_eq!(clip.v_min, 0.2);
        assert_eq!(clip.v_max, 1.0);
    }

    #[test]
    fn test_block_clip_crops_the_bottom_at_the_pane_edge() {
        // A band starting 30px from the bottom of a 100px-tall pane shows only
        // its first 30px, cropped rather than squashed.
        let clip = block_clip(70.0, (100.0, 100.0), 100.0, 100.0).expect("still visible");
        assert_eq!(clip.y, 70.0);
        assert_eq!(clip.height, 30.0);
        assert_eq!(clip.v_min, 0.0);
        assert_eq!(clip.v_max, 0.3);
    }

    #[test]
    fn test_block_clip_crops_content_taller_than_its_reserved_band() {
        // Text/markdown draws at native size: the part past the reserved band
        // is cropped so the prompt below it is never overdrawn.
        let clip = block_clip(0.0, (100.0, 200.0), 50.0, 400.0).expect("visible");
        assert_eq!(clip.height, 50.0);
        assert_eq!(clip.v_max, 0.25);
    }

    #[test]
    fn test_block_clip_drops_a_block_fully_outside_the_pane() {
        // Scrolled entirely above, and entirely below, the pane.
        assert!(block_clip(-100.0, (100.0, 100.0), 100.0, 400.0).is_none());
        assert!(block_clip(400.0, (100.0, 100.0), 100.0, 400.0).is_none());
        // A zero-height image has nothing to draw and no valid `v` range.
        assert!(block_clip(0.0, (100.0, 0.0), 100.0, 400.0).is_none());
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

        let clip = block_clip(band_top, (100.0, band_h), band_h, 1000.0).expect("visible");
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
        tab: TabId,
        abs_row: usize,
        block_index: usize,
        segment_index: usize,
    ) -> ImageBlock {
        ImageBlock {
            abs_row,
            block_index,
            closed: false,
            content: Content::Raster(Arc::from(&[][..])),
            drawn: Some(Fit::Natural),
            id: block_index as u64,
            max_rows: 4,
            natural: Some((80.0, 40.0)),
            segment_index,
            tab_id: tab,
        }
    }

    /// An app with one live `cat` pane at `focused`, plus one image block per
    /// `(abs_row, block_index, segment_index)` anchor, laid out over a
    /// 400x300 rect. Building the blocks inside sidesteps the chicken-and-egg
    /// of needing the pane id before the app exists.
    fn app_with_image_blocks(
        anchors: &[(usize, usize, usize)],
    ) -> (App, TabId, Vec<(TabId, Rect)>) {
        let mut app = App::new();
        let id = app.layout().shown_tabs()[0];
        let terminal = crate::terminal::session::Terminal::with_command(
            40,
            8,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        app.open_tabs.insert_terminal(id, terminal);
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
        let (mut app, tab_id, rects) = app_with_image_blocks(&[(1, 0, 1)]);

        let placements = image_placements(&app.image_blocks, &app.open_tabs, &rects, 20.0, &[]);
        assert_eq!(placements.len(), 1, "fixture: the block places normally");

        app.open_tabs
            .terminal_mut(&tab_id)
            .unwrap()
            .grid_mut()
            .enter_alt_screen();
        let placements = image_placements(&app.image_blocks, &app.open_tabs, &rects, 20.0, &[]);
        assert!(
            placements.is_empty(),
            "an alt-screen pane must not have primary-screen blocks painted over it"
        );

        app.open_tabs
            .terminal_mut(&tab_id)
            .unwrap()
            .grid_mut()
            .leave_alt_screen();
        let placements = image_placements(&app.image_blocks, &app.open_tabs, &rects, 20.0, &[]);
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
        let (mut app, tab_id, _rects) = app_with_image_blocks(&[(2, 0, 1), (9, 1, 1)]);

        app.drop_elided_blocks(&[(tab_id, 0, 1)]);

        assert_eq!(app.image_blocks.len(), 1);
        assert_eq!(app.image_blocks[0].block_index, 1, "the live block stays");
        assert!(app.dirty, "dropping a rendered block must request a redraw");
    }

    #[test]
    fn test_drop_blocks_in_keeps_blocks_outside_the_erased_span() {
        let (mut app, tab_id, _rects) = app_with_image_blocks(&[(2, 0, 1), (10, 1, 1)]);

        // Erase rows 0..5: only the band at row 2 overlaps.
        app.drop_blocks_in(tab_id, (0, 5));

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
        let id = app.layout().shown_tabs()[0];
        let mut terminal = crate::terminal::session::Terminal::with_command(
            20,
            8,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        {
            let grid = terminal.grid_mut();
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
        app.open_tabs.insert_terminal(id, terminal);
        app.image_blocks.push(image_block(id, 2, 0, 1));

        let terminal = app.open_tabs.terminal_mut(&id).unwrap();
        terminal.resize(10, 8); // the 30 chars now wrap rows 0-2
        for remap in terminal.take_row_remaps() {
            app.remap_pane_anchors(id, &remap);
        }

        let grid = app.open_tabs.terminal(&id).unwrap().grid();
        assert_eq!(
            app.image_blocks[0].abs_row,
            grid.to_absolute_row(3),
            "the anchor must follow the band below the re-wrapped line"
        );
        assert!(app.dirty, "a remapped anchor must request a redraw");
    }

    #[test]
    fn test_export_focused_block_text_with_no_pane() {
        let mut app = App::new();
        app.export_focused_block_text(TabId(999));
        assert!(app.notice_text().is_none());
    }

    #[test]
    fn test_is_block_folded_defaults_to_false() {
        let app = App::new();
        assert!(!app.is_block_folded(TabId(1), 0));
    }
}
