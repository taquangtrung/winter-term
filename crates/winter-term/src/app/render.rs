//! Frame composition and WebView tile management.

use std::collections::HashMap;

use base64::Engine;
use serde_json::Value;

use crate::config::IconStyle;
use crate::model::layout::{PaneId, Rect};
use crate::model::mode::Mode;
use crate::model::page::{wrap_start, wrapped_lines, PageContent, PageIcon, PageStyle, PromptMode};
use crate::model::palette::{Palette, PaletteMode};
use crate::model::settings_page::{Control, SettingsField, SettingsPage};
use crate::terminal::block_queue::{BlockEntry, BlockKind};
use crate::terminal::pane::{Pane, MAX_IMAGE_ROWS};
use crate::terminal::webview::{self, PaneViewport, SurfaceMessage, SurfaceParams, TilePlacement};
use winter_core::winter_proto::EmitBlock;
use winter_render::renderer::{PaneRect, PaneView};
use winter_render::{
    Color, CursorShape, Grid, ImagePlacement, PaletteItem, PaletteView, RgbColor, StatusNotice,
    StatusSearch, Style, Theme, ThemeRgb,
};

use super::{page, status_bar, App, ImageBlock, ReflowSource};

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
/// How far a page's dim style is blended toward the background.
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
/// washes those hues out — worst on the red a deletion carries. Still kept
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
    page_paints: &'a [PagePaint],
    /// Where each page's caret sits, and whether it is typing there.
    page_cursors: &'a std::collections::HashMap<PaneId, PaneCaret>,
    /// The open pages, for the grid each one last painted into.
    pages: &'a std::collections::HashMap<PaneId, super::page::PageSlot>,
    /// Whether an overlay has the keyboard: the palette, or a question being
    /// answered in the input dialog.
    overlay_open: bool,
    panes: &'a std::collections::HashMap<PaneId, crate::terminal::pane::Pane>,
    rects: &'a [(PaneId, Rect)],
    selection: Option<&'a super::Selection>,
    window_focused: bool,
}

/// One page pane's painted frame: the grid the page rendered itself into, and
/// the row its cursor line sits on.
struct PagePaint {
    cursor_line: Option<usize>,
    /// Icons to rasterize over the painted rows. Empty unless the icon style
    /// calls for artwork; a glyph style has already been drawn into the grid.
    icons: Vec<PageIcon>,
    pane: PaneId,
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
        page_cursors,
        page_paints,
        pages,
        overlay_open,
        panes,
        rects,
        selection,
        window_focused,
    } = input;
    let mut views: Vec<PaneView> = Vec::new();
    for (i, (id, rect)) in rects.iter().enumerate() {
        // A page covers the pane's terminal while it is open, so it is checked
        // first: the grid underneath keeps updating, unseen.
        if let Some(paint) = page_paints.iter().find(|paint| paint.pane == *id) {
            if let Some(grid) = pages.get(id).and_then(|slot| slot.painted.as_ref()) {
                views.push(page_pane_view(
                    paint,
                    grid,
                    *rect,
                    *id == focused,
                    config,
                    selection.filter(|s| s.pane == *id),
                    page_cursors.get(id).copied(),
                ));
            }
        } else if let Some(pane) = panes.get(id) {
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
                // Unreachable for a terminal pane, whose mode is never Page.
                Mode::Page => config.cursor.normal,
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
            let cursor_visible = if overlay_open || !pane.grid().cursor_visible() {
                // An overlay with the keyboard has the cursor too, and DECTCEM
                // (CSI ?25l) lets a full-screen app like btop hide it outright.
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

/// Build the view for a page pane.
///
/// A page has no terminal caret of its own, so the cursor is parked out of
/// bounds (suppressing it) and the cursor line marks where the page's own
/// selection sits. A text selection over the page is drawn here the same way a
/// terminal pane draws one: the rows it names are the page's, so it has to be
/// resolved against the grid the page painted rather than the pane's.
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

fn page_pane_view<'a>(
    paint: &'a PagePaint,
    grid: &'a Grid,
    rect: Rect,
    focused: bool,
    config: &crate::config::Config,
    selection: Option<&'a super::Selection>,
    text_cursor: Option<PaneCaret>,
) -> PaneView<'a> {
    let (sel_tuple, sel_block) = match selection {
        Some(s) => (
            Some((s.start_row, s.start_col, s.end_row, s.end_col)),
            s.block,
        ),
        None => (None, false),
    };
    PaneView {
        bracket_colors: &[],
        block_band: None,
        // A page's caret is a block over the cell it is on, as the traversal
        // cursor is everywhere else; a page taking typed text takes the shape
        // the terminal's own cursor has while typing, so the two read apart at
        // a glance.
        cursor_shape: match text_cursor.map(|caret| caret.insert) {
            Some(true) => config.cursor.insert,
            _ => CursorShape::Block,
        },
        cursor_unfocused: false,
        cursor_visible: false,
        dim: !focused && config.dim_inactive,
        focused,
        grid,
        hovered_link: 0,
        labels: None,
        find_labels: &[],
        // Out of bounds when there is no text cursor, which suppresses it: a
        // page has no caret of its own, so the cursor line is the only mark of
        // where the page's own selection sits.
        nav_cursor: Some(
            text_cursor
                .map(|caret| (caret.row, caret.col))
                .unwrap_or((grid.rows(), grid.cols())),
        ),
        cursor_line_row: paint.cursor_line,
        nav_cursor_visible: text_cursor.is_some(),
        rect: App::layout_rect_to_pane(rect),
        scroll_offset: 0,
        scrollback_len: 0,
        search_matches: &[],
        search_current: &[],
        sentence_spans: &[],
        selection: sel_tuple,
        selection_block: sel_block,
        url_underline: false,
    }
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

/// Hand a page surface the colors it has to match, as a global its document
/// reads on load. Only the host knows the theme, and a surface that guessed
/// would sit in the pane as an obviously foreign rectangle.
fn surface_theme_script(theme: &Theme) -> String {
    format!(
        "window.winterTheme={{\"background\":\"{}\",\"foreground\":\"{}\"}};",
        css_color(theme.background),
        css_color(theme.foreground)
    )
}

/// One theme color as CSS hex.
fn css_color(color: ThemeRgb) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
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
    covered: &[PagePaint],
) -> Vec<ImagePlacement> {
    let mut placements: Vec<ImagePlacement> = Vec::new();
    for img in blocks {
        let Some((_, rect)) = rects.iter().find(|(id, _)| *id == img.pane_id) else {
            continue;
        };
        // A tool page owns every row of the pane it covers, the same way an
        // alternate-screen app does below.
        if covered.iter().any(|paint| paint.pane == img.pane_id) {
            continue;
        }
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
fn palette_view(palette: &Palette, match_underline: bool, pick: Option<&str>) -> PaletteView {
    let empty_message = match palette.mode {
        PaletteMode::Files => "Nothing here by that name",
        PaletteMode::History => "No matching history",
        PaletteMode::PagePick => "Nothing matches",
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
        // A page's list is headed by what it is a list of, and a browser by
        // where it is; the command palette needs no naming and is drawn
        // without a heading.
        title: match &palette.dir {
            Some(dir) => dir.display().to_string(),
            None => pick.unwrap_or_default().to_string(),
        },
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
        // What is about to be drawn is the current state, so the pending-repaint
        // flag is spent here. Clearing it at the top rather than the bottom keeps
        // a change made while this frame is being built from being swallowed: it
        // sets the flag again and earns its own frame.
        self.dirty = false;
        // The settings page is a full-window modal; it replaces the panes, tabbar,
        // status bar, and block tiles entirely until it closes.
        if self.settings_page.is_some() {
            self.render_settings_frame();
            return;
        }

        // Questions are asked in the dialog over the middle of the window, so
        // the status bar is left to say what it has to say.
        let notice = self.active_notice().map(|(text, kind)| StatusNotice {
            kind,
            text: text.to_string(),
        });
        // A tool's question, or the one the app asks for a new theme's name:
        // both are a line typed in answer to a label, and both are read in
        // the same place.
        let input_view = self.input_view().or_else(|| {
            self.theme_name_input
                .as_ref()
                .map(|input| page::input_dialog("New theme name", input, PromptMode::Text))
        });
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
        // Pages paint themselves into a grid of their own, so the renderer
        // draws a tool pane through exactly the path a terminal pane takes. A
        // page is handed its row count first, so it can window a listing longer
        // than the pane around its own cursor.
        let icon_style = self.config.icons;
        let mut page_paints: Vec<PagePaint> = Vec::new();
        for (id, rect) in &rects {
            let (cols, rows) = renderer.grid_size_for(App::layout_rect_to_pane(*rect));
            let Some(slot) = self.pages.get_mut(id) else {
                continue;
            };
            let content = slot.page.content(rows, cols, self.page_wrap);
            // Kept on the slot rather than in the paint, so that selecting over
            // the pane after this frame reads the rows the user can see.
            slot.cursor_line = content.cursor_line;
            slot.painted = Some(build_page_grid(
                &content,
                renderer.theme(),
                cols,
                rows,
                icon_style,
                self.page_wrap,
            ));
            page_paints.push(PagePaint {
                cursor_line: content.cursor_line,
                icons: if icon_style == IconStyle::Svg {
                    content.icons.clone()
                } else {
                    Vec::new()
                },
                pane: *id,
            });
        }
        let hovered_pane = self.hovered_pane(&rects);
        let search = self.search.query.as_ref().map(|q| StatusSearch {
            query: q.clone(),
            match_index: self.search.match_index,
            match_total: self.search.match_total,
            reverse: self.search.reverse,
        });
        let page_name = self.pages.get(&focused).map(|slot| slot.status_label());
        let status = status_bar(
            mode,
            renderer.theme(),
            search,
            notice,
            &self.config.status_bar,
            page_name.as_deref(),
        );
        let status = status_enabled.then_some(&status);
        let palette_view = self.palette.as_ref().map(|p| {
            palette_view(
                p,
                self.config.palette_match_underline,
                self.page_pick.as_ref().map(|pick| pick.label.as_str()),
            )
        });
        // A page part-way through a multi-key command says so through the
        // same card the terminal's own prefixes use, and says it at once: the
        // menus are how the tools are learned, where the terminal's hints
        // wait to see whether the typing stalls.
        let which_key_view = self
            .pages
            .get(&focused)
            .and_then(|slot| slot.page.hint())
            .map(|hint| winter_render::WhichKeyView {
                items: hint.items,
                title: hint.title,
            })
            .or_else(|| which_key_view(&self.pending, self.pending_since));
        // Gathered before the renderer is held mutably: the bands read app
        // state the views below borrow alongside it.
        let focused_bands = self.block_bands(focused);
        // Rasterized before the renderer is taken mutably below, since filling
        // the icon cache needs it mutably too.
        let icon_placements = self.page_icon_placements(&page_paints, &rects, (cw, ch));
        // Every page pane shows a block cursor, not only one that `v` has
        // started a text cursor in. The row band says which entry is current;
        // the cursor says which cell a selection would start from, and without
        // it a page looks like it has no cursor at all.
        let page_cursors: std::collections::HashMap<PaneId, PaneCaret> = self
            .pages
            .iter()
            .filter_map(|(id, slot)| {
                if let Some(cursor) = self.page_cursor.as_ref().filter(|c| c.pane == *id) {
                    return Some((*id, PaneCaret::at(cursor.row, cursor.col)));
                }
                let row = slot.cursor_line?;
                // A page that edits text says which cell it is on; one whose
                // cursor is a whole row leaves it to the row's first painted
                // character, which is where the eye starts reading it.
                if let Some(caret) = slot.page.caret() {
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
                    PaneCaret::at(row, super::page_cursor::first_non_blank(grid, row)),
                ))
            })
            .collect();

        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        reflow_width_wrapped_blocks(&mut self.image_blocks, &rects, renderer);
        let mut placements =
            image_placements(&self.image_blocks, &self.panes, &rects, ch, &page_paints);
        placements.extend(icon_placements);
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
            page_cursors: &page_cursors,
            page_paints: &page_paints,
            pages: &self.pages,
            overlay_open: self.palette.is_some() || input_view.is_some(),
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
            input_view.as_ref(),
            toast.as_ref(),
            which_key_view.as_ref(),
        );

        self.reposition_block_tiles(&rects, ch);
        self.place_page_surfaces(&rects, ch);
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

    /// The GPU texture for one icon at one pixel box, rasterizing it the first
    /// time it is asked for.
    ///
    /// The whole cache is dropped when the box changes, which happens only on a
    /// font-size or DPI change: every texture in it was rasterized for the old
    /// box, and an icon scaled from the wrong size is exactly the blur that
    /// rasterizing per size exists to avoid.
    fn icon_texture(&mut self, name: &str, box_w: u32, box_h: u32) -> Option<u64> {
        if self.icon_texture_box != (box_w, box_h) {
            for id in self.icon_textures.values().flatten() {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.free_image(*id);
                }
            }
            self.icon_textures.clear();
            self.icon_texture_box = (box_w, box_h);
        }
        let key = (name.to_string(), box_w, box_h);
        if let Some(cached) = self.icon_textures.get(&key) {
            return *cached;
        }
        let id = self.next_image_id;
        let uploaded = crate::icons::svg(name).is_some_and(|svg| {
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

    /// Quads for every icon the pages want drawn this frame.
    ///
    /// Recomputed from scratch each frame rather than anchored, because a page
    /// repaints its rows from its own model every frame anyway: there is no
    /// scrollback row for an icon to drift away from.
    fn page_icon_placements(
        &mut self,
        paints: &[PagePaint],
        rects: &[(PaneId, Rect)],
        cell: (f32, f32),
    ) -> Vec<ImagePlacement> {
        let (cw, ch) = cell;
        let box_w = (cw * PageIcon::WIDTH as f32).round().max(1.0) as u32;
        let box_h = ch.round().max(1.0) as u32;
        let mut placements = Vec::new();
        for paint in paints {
            let Some((_, rect)) = rects.iter().find(|(id, _)| *id == paint.pane) else {
                continue;
            };
            let pane_rect = App::layout_rect_to_pane(*rect);
            for icon in &paint.icons {
                let Some(name) = crate::icons::name_for(&icon.kind) else {
                    continue;
                };
                let Some(id) = self.icon_texture(&name, box_w, box_h) else {
                    continue;
                };
                let x = pane_rect.x + icon.col as f32 * cw;
                let y = pane_rect.y + icon.row as f32 * ch;
                // Clipped out rather than drawn past the pane's last row.
                if y + ch > pane_rect.y + pane_rect.height || x + cw > pane_rect.x + pane_rect.width
                {
                    continue;
                }
                placements.push(ImagePlacement {
                    alpha: 1.0,
                    height: box_h as f32,
                    id,
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
    fn reposition_block_tiles(&mut self, rects: &[(PaneId, Rect)], ch: f32) {
        let viewports = self.tile_viewports(rects);
        // Placing every tile is a round-trip to the platform WebView each, so
        // only do it when a pane actually moved or scrolled; otherwise plain
        // typing, which never moves a tile, stalls on that IPC.
        if self.last_tile_layout.as_ref() == Some(&viewports) {
            return;
        }
        self.webview_mgr.reposition_tiles(&viewports, ch);
        self.last_tile_layout = Some(viewports);
    }

    /// Give every page that owns a surface a WebView over its pane, building
    /// it the first time the page is drawn, and take off screen the surfaces
    /// of panes that no longer show the page they belong to.
    ///
    /// A surface covers its pane from the first row its page leaves it down:
    /// the rows above stay the page's own, painted in the terminal's font, so
    /// a PDF keeps a native header over a document Winter cannot draw.
    fn place_page_surfaces(&mut self, rects: &[(PaneId, Rect)], ch: f32) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let theme_script = match &self.renderer {
            Some(renderer) => surface_theme_script(renderer.theme()),
            None => return,
        };

        let mut showing: Vec<PaneId> = Vec::new();
        for (pane_id, rect) in rects {
            let Some(slot) = self.pages.get(pane_id) else {
                continue;
            };
            let Some(surface) = slot.page.surface() else {
                continue;
            };
            let pane_rect = Self::layout_rect_to_pane(*rect);
            let top = pane_rect.y + surface.top_row as f32 * ch;
            let placement = TilePlacement {
                height: (pane_rect.y + pane_rect.height - top).max(0.0) as u32,
                // A surface fills the rect it is given, so nothing of it is
                // ever hidden by an edge the way a scrolled tile's band is.
                scroll_top: 0,
                width: pane_rect.width.max(0.0) as u32,
                x: pane_rect.x as i32,
                y: top as i32,
            };
            if !self.webview_mgr.has_surface(*pane_id) {
                let params = SurfaceParams {
                    init_script: theme_script.clone(),
                    rect: placement,
                    surface,
                };
                if let Err(e) = self.webview_mgr.create_surface(*pane_id, params, &window) {
                    eprintln!("winter: page surface error: {e}");
                    continue;
                }
            }
            self.webview_mgr.place_surface(*pane_id, &placement);
            showing.push(*pane_id);
        }
        self.webview_mgr.hide_surfaces_except(&showing);
    }

    /// Run whatever the open pages have queued for their surfaces, and hand
    /// each page back what its own surface posted out.
    pub(crate) fn pump_page_surfaces(&mut self) {
        for (pane_id, slot) in self.pages.iter_mut() {
            if let Some(js) = slot.page.take_surface_script() {
                self.webview_mgr.run_surface_script(*pane_id, &js);
            }
        }
        for message in self.webview_mgr.drain_surface_messages() {
            match message {
                SurfaceMessage::Key(relayed) => {
                    self.route_surface_key(relayed.pane_id, relayed.key)
                }
                SurfaceMessage::Text(said) => {
                    let Some(slot) = self.pages.get_mut(&said.pane_id) else {
                        continue;
                    };
                    let outcome = slot.page.on_surface_message(said.body);
                    self.act_on_page_outcome(said.pane_id, outcome);
                }
            }
            self.dirty = true;
        }
    }

    /// Where each pane's grid sits this frame, for the panes whose tiles
    /// should show.
    ///
    /// `rects` covers the active tab alone, so a background tab's panes are
    /// already out. Left out on top of those: a pane showing an
    /// alternate-screen app, and a pane a tool page covers. Those are the
    /// panes `image_placements` skips too, for the same reason: something
    /// else owns the viewport, and a primary-screen block painted over it
    /// would hide what does until it goes away.
    fn tile_viewports(&self, rects: &[(PaneId, Rect)]) -> HashMap<PaneId, PaneViewport> {
        let mut viewports = HashMap::new();
        for (pane_id, rect) in rects {
            let Some(pane) = self.panes.get(pane_id) else {
                continue;
            };
            if pane.grid().is_alt_screen() || self.pages.contains_key(pane_id) {
                continue;
            }
            let pane_rect = Self::layout_rect_to_pane(*rect);
            viewports.insert(
                *pane_id,
                PaneViewport {
                    height: pane_rect.height,
                    // Tiles are anchored absolutely, so what moves them is the
                    // absolute row at the top of this pane's viewport: it
                    // advances both when the user scrolls and when new output
                    // pushes lines into history.
                    top_row: pane.grid().to_absolute_row(0),
                    width: pane_rect.width,
                    x: pane_rect.x,
                    y: pane_rect.y,
                },
            );
        }
        viewports
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
                height: (entry.reserved_rows as f32 * ch) as u32,
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

/// Paint a page into a fresh `cols` x `rows` grid: its rows in order, each span
/// resolved from a semantic style to the theme's colors, clipped at the pane's
/// last row.
fn build_page_grid(
    content: &PageContent,
    theme: &Theme,
    cols: usize,
    rows: usize,
    icons: IconStyle,
    wrap: bool,
) -> Grid {
    let mut grid = Grid::new(cols, rows);
    if cols == 0 {
        return grid;
    }
    // The screen row each of the page's own rows starts on, so icons — which
    // name their row among the page's — land on the right screen row once
    // earlier rows wrap.
    let mut row_starts: Vec<usize> = Vec::with_capacity(content.rows.len());
    let mut screen = 0;
    'page: for (index, spans) in content.rows.iter().enumerate() {
        row_starts.push(screen);
        // The column this row's wrapped continuations start at, reserving the
        // room its leading span gives the diff marker.
        let indent = wrap_start(content.wrap_indents.get(index).copied().unwrap_or(0), cols);
        let lead = spans.first().map(|span| page_span_style(span.style, theme));
        // The row flattened across its spans, each char carrying its own
        // style: wrapping works on words, which straddle spans, so the fold
        // is chosen over the whole row and each painted piece keeps the
        // color of the span it came from.
        let mut chars = Vec::new();
        let mut styles = Vec::new();
        for span in spans {
            let style = page_span_style(span.style, theme);
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
                    break 'page;
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
                put(&mut grid, screen, col, &piece, style);
                col += run_end - at;
                at = run_end;
            }
        }
        screen += 1;
        if screen >= rows {
            break;
        }
    }
    // A glyph is a real cell, so it goes into the grid the page just painted
    // and is selected, copied and themed like any other character. It keeps the
    // style already on the cell rather than imposing one, so the band on a
    // marked row runs through it unbroken.
    if icons == IconStyle::Font {
        for icon in &content.icons {
            let Some(row) = row_starts.get(icon.row).copied() else {
                continue;
            };
            if row >= rows || icon.col >= cols {
                continue;
            }
            let style = grid
                .cell(row, icon.col)
                .map(|cell| cell.style)
                .unwrap_or_else(|| page_span_style(PageStyle::Normal, theme));
            put(&mut grid, row, icon.col, &icon.glyph.to_string(), style);
        }
    }
    grid
}

/// Resolve a page's semantic style against the active theme. The diff styles
/// tint the background with the theme's palette the way a diff editor does,
/// rather than carrying colors of their own, so every theme gets bands that
/// belong to it.
fn page_span_style(style: PageStyle, theme: &Theme) -> Style {
    match style {
        PageStyle::Accent => Style {
            foreground: theme_rgb(theme.cursor_bg),
            ..Style::default()
        },
        PageStyle::Added => Style {
            background: mix_rgb(theme.background, theme.ansi[ANSI_GREEN], PAGE_DIFF_LINE_MIX),
            foreground: theme_rgb(theme.foreground),
            ..Style::default()
        },
        PageStyle::AddedEdit => Style {
            background: mix_rgb(theme.background, theme.ansi[ANSI_GREEN], PAGE_DIFF_EDIT_MIX),
            bold: true,
            foreground: theme_rgb(theme.foreground),
            ..Style::default()
        },
        // What a change did to a file is told by hue and carried in bold, the
        // colors magic-vscode's change list uses resolved against the theme's
        // own palette rather than pinned to its hex: green for what arrived,
        // red for what left, blue for what was edited where it stands, yellow
        // for what moved, magenta for what is still contested.
        PageStyle::ChangeAdded => change_style(theme, ANSI_GREEN),
        PageStyle::ChangeConflict => change_style(theme, ANSI_MAGENTA),
        PageStyle::ChangeDeleted => change_style(theme, ANSI_RED),
        PageStyle::ChangeModified => change_style(theme, ANSI_BLUE),
        PageStyle::ChangeRenamed => change_style(theme, ANSI_YELLOW),
        PageStyle::Dim => Style {
            foreground: mix_rgb(theme.foreground, theme.background, PAGE_DIM_MIX),
            ..Style::default()
        },
        PageStyle::Header => Style {
            bold: true,
            foreground: theme_rgb(theme.foreground),
            ..Style::default()
        },
        // A section's heading wears the hue of what sits under it, the way
        // magic-vscode's own headings do: cyan for what is staged, yellow for
        // what is not, green for what is untracked, red for what a merge left
        // contested. The count beside it stays receded, so the color says
        // which section without the row shouting.
        PageStyle::HeadingConflict => heading_style(theme, ANSI_RED),
        PageStyle::HeadingStaged => heading_style(theme, ANSI_CYAN),
        PageStyle::HeadingUnstaged => heading_style(theme, ANSI_YELLOW),
        PageStyle::HeadingUntracked => heading_style(theme, ANSI_GREEN),
        // Everything else a git view heads a block with — the recent commits,
        // a log's title, and the labels the header block is read down — is
        // blue, which is where magic-vscode's grammar sends every heading it
        // has no change color for.
        PageStyle::HeadingPlain => heading_style(theme, ANSI_BLUE),
        PageStyle::Hunk => Style {
            background: mix_rgb(theme.background, theme.ansi[ANSI_BLUE], PAGE_HUNK_MIX),
            foreground: mix_rgb(theme.foreground, theme.background, PAGE_DIM_MIX),
            ..Style::default()
        },
        PageStyle::Marked => Style {
            background: theme_rgb(theme.selection_bg),
            bold: true,
            foreground: theme_rgb(theme.selection_fg),
            ..Style::default()
        },
        PageStyle::Normal => Style {
            foreground: theme_rgb(theme.foreground),
            ..Style::default()
        },
        PageStyle::Removed => Style {
            background: mix_rgb(theme.background, theme.ansi[ANSI_RED], PAGE_DIFF_LINE_MIX),
            foreground: theme_rgb(theme.foreground),
            ..Style::default()
        },
        PageStyle::RemovedEdit => Style {
            background: mix_rgb(theme.background, theme.ansi[ANSI_RED], PAGE_DIFF_EDIT_MIX),
            bold: true,
            foreground: theme_rgb(theme.foreground),
            ..Style::default()
        },
        // The four ref styles are told apart by hue rather than by weight,
        // with only the checked-out branch also carrying bold so the eye lands
        // on it first. The hues are magic-vscode's: a branch here is magenta,
        // one on a remote green, and a tag cyan, which keeps a local name and
        // the remote one beside it from reading as the same thing.
        PageStyle::RefHead => Style {
            bold: true,
            foreground: theme_rgb(theme.ansi[ANSI_MAGENTA]),
            ..Style::default()
        },
        PageStyle::RefLocal => Style {
            foreground: theme_rgb(theme.ansi[ANSI_MAGENTA]),
            ..Style::default()
        },
        PageStyle::RefRemote => Style {
            foreground: theme_rgb(theme.ansi[ANSI_GREEN]),
            ..Style::default()
        },
        PageStyle::RefTag => Style {
            foreground: theme_rgb(theme.ansi[ANSI_CYAN]),
            ..Style::default()
        },
        PageStyle::Section => Style {
            background: section_band(theme),
            foreground: theme_rgb(theme.foreground),
            ..Style::default()
        },
        // Source is colored out of the theme's own palette rather than out of
        // a scheme of its own, so a file reads as the terminal beside it does:
        // the comment hue the shells and the pagers already use, and one hue
        // per kind of thing after that.
        PageStyle::SyntaxComment => Style {
            foreground: theme_rgb(theme.ansi[ANSI_BRIGHT_BLACK]),
            ..Style::default()
        },
        PageStyle::SyntaxKeyword => Style {
            foreground: theme_rgb(theme.ansi[ANSI_MAGENTA]),
            ..Style::default()
        },
        PageStyle::SyntaxNumber => Style {
            foreground: theme_rgb(theme.ansi[ANSI_YELLOW]),
            ..Style::default()
        },
        PageStyle::SyntaxString => Style {
            foreground: theme_rgb(theme.ansi[ANSI_GREEN]),
            ..Style::default()
        },
        PageStyle::SyntaxType => Style {
            foreground: theme_rgb(theme.ansi[ANSI_CYAN]),
            ..Style::default()
        },
        PageStyle::Unpushed => Style {
            foreground: theme_rgb(theme.ansi[ANSI_RED]),
            ..Style::default()
        },
        PageStyle::SectionFolded => Style {
            background: section_band(theme),
            foreground: mix_rgb(theme.foreground, theme.background, PAGE_DIM_MIX),
            ..Style::default()
        },
    }
}

/// The band a file's row is painted on, the same whichever way the row is
/// folded: the fold triangle at its head says which, and a band that changed
/// color with it would break the row into two tones, since the change's name
/// beside the triangle carries a hue of its own.
fn section_band(theme: &Theme) -> Color {
    mix_rgb(theme.background, theme.ansi[ANSI_CYAN], PAGE_SECTION_MIX)
}

/// How a section's heading is painted: its own hue, in bold, since a heading
/// is the loudest row of the block it opens.
fn heading_style(theme: &Theme, ansi: usize) -> Style {
    Style {
        bold: true,
        foreground: theme_rgb(theme.ansi[ansi]),
        ..Style::default()
    }
}

/// How a change's name is painted where it heads a file's band: its own hue,
/// in bold, over the band the rest of the row sits on, so the row reads as one
/// bar with the change called out on it.
fn change_style(theme: &Theme, ansi: usize) -> Style {
    Style {
        background: section_band(theme),
        bold: true,
        foreground: theme_rgb(theme.ansi[ansi]),
        ..Style::default()
    }
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
    use crate::model::page::{PageRow, PageSpan};

    #[test]
    fn test_page_grid_lays_spans_out_left_to_right() {
        let content = PageContent::new(vec![vec![
            PageSpan::plain("Zoom Pane"),
            PageSpan::new(PageStyle::Accent, "Shift-Alt-="),
        ]]);
        let grid = build_page_grid(&content, &Theme::dark(), 40, 4, IconStyle::None, false);
        let first = grid
            .to_text()
            .lines()
            .next()
            .unwrap_or_default()
            .to_string();
        assert_eq!(first.trim_end(), "Zoom PaneShift-Alt-=");
    }

    #[test]
    fn test_page_grid_draws_a_font_icon_into_the_reserved_columns() {
        // The page leaves the icon's columns blank whatever the setting is, so
        // a glyph style has to paint the glyph in, and only then: painting it
        // under `Svg` would leave a glyph showing through the artwork, and
        // under `None` would put back the column the setting removes.
        let content =
            PageContent::new(vec![vec![PageSpan::plain("   name")]]).with_icons(vec![PageIcon {
                col: 0,
                glyph: 'X',
                kind: crate::model::page::PageIconKind::File {
                    name: "name".to_string(),
                },
                row: 0,
            }]);
        let painted = |style| {
            build_page_grid(&content, &Theme::dark(), 20, 2, style, false)
                .to_text()
                .lines()
                .next()
                .unwrap_or_default()
                .to_string()
        };
        assert!(painted(IconStyle::Font).starts_with('X'));
        assert!(painted(IconStyle::Svg).starts_with("   name"));
        assert!(painted(IconStyle::None).starts_with("   name"));
    }

    #[test]
    fn test_page_grid_clips_rows_past_the_pane_height() {
        // A page longer than its pane must lose the overflow, not write past
        // the grid's last row.
        let rows: Vec<PageRow> = (0..10)
            .map(|i| vec![PageSpan::plain(format!("row{i}"))])
            .collect();
        let grid = build_page_grid(
            &PageContent::new(rows),
            &Theme::dark(),
            20,
            3,
            IconStyle::None,
            false,
        );
        let text = grid.to_text();
        assert!(text.contains("row2"), "the last visible row is painted");
        assert!(
            !text.contains("row3"),
            "rows past the pane height are dropped"
        );
    }

    #[test]
    fn test_a_row_wider_than_the_pane_wraps_onto_the_next_screen_row() {
        let content = PageContent::new(vec![vec![PageSpan::plain("abcdefghij")]]);
        let wrapped = build_page_grid(&content, &Theme::dark(), 5, 2, IconStyle::None, true);
        let text = wrapped.to_text();
        // Five cells a line: the row's text spills onto the screen row under
        // it rather than being lost at the pane's edge.
        assert!(text.contains("abcde"), "the first line, got {text:?}");
        assert!(text.contains("fghij"), "the wrapped rest, got {text:?}");
        let clipped = build_page_grid(&content, &Theme::dark(), 5, 2, IconStyle::None, false);
        let text = clipped.to_text();
        assert!(text.contains("abcde"), "the first line, got {text:?}");
        assert!(!text.contains("fghij"), "the edge clips the rest");
    }

    #[test]
    fn test_a_wrapped_row_folds_at_its_last_word_boundary() {
        // "the quick brown fox" in ten cells folds after "quick", the last
        // word that fits, and the space the fold lands on vanishes with it.
        let content = PageContent::new(vec![vec![PageSpan::plain("the quick brown fox")]]);
        let grid = build_page_grid(&content, &Theme::dark(), 10, 2, IconStyle::None, true);
        let text = grid.to_text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0].trim_end(), "the quick", "folds at the space");
        assert_eq!(lines[1].trim_end(), "brown fox", "carries the word down");
    }

    #[test]
    fn test_a_fold_splits_spans_but_keeps_each_pieces_style() {
        // The fold lands inside the second span, and both of its pieces keep
        // the span's color: words wrap, colors follow the words. The break
        // lands before "bbb", not after it — the space past the margin belongs
        // to the fold, exactly where the grid's own wrap would take it.
        let content = PageContent::new(vec![vec![
            PageSpan::plain("aaa "),
            PageSpan::new(PageStyle::Accent, "bbb ccc"),
        ]]);
        let theme = Theme::dark();
        let grid = build_page_grid(&content, &theme, 7, 2, IconStyle::None, true);
        let text = grid.to_text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0].trim_end(), "aaa");
        assert_eq!(lines[1].trim_end(), "bbb ccc");
        let plain = page_span_style(PageStyle::Normal, &theme);
        let accent = page_span_style(PageStyle::Accent, &theme);
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
        let content = PageContent::new(vec![
            vec![PageSpan::plain("aaa"), PageSpan::plain("bbb")],
            vec![PageSpan::plain("zz")],
        ]);
        let grid = build_page_grid(&content, &Theme::dark(), 4, 3, IconStyle::None, true);
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
    fn test_a_clipped_row_still_takes_its_own_screen_row() {
        // With wrapping off, a row wider than the pane loses its tail — but it
        // still occupies one screen row, and the row under it must paint on
        // the next screen row rather than over the clipped one.
        let content = PageContent::new(vec![
            vec![PageSpan::plain("0123456789")],
            vec![PageSpan::plain("next")],
        ]);
        let grid = build_page_grid(&content, &Theme::dark(), 5, 2, IconStyle::None, false);
        let text = grid.to_text();
        assert!(text.contains("01234"), "the clipped row keeps its start");
        assert!(text.contains("next"), "the next row paints under it");
    }

    #[test]
    fn test_a_wrapped_diff_line_continues_past_its_marker() {
        // The continuation of a wrapped diff line starts past the marker's
        // column, so the spilled text lines up under the text it follows, not
        // under the marker — and the room it leaves keeps the line's own
        // tint, so a wrapped line reads as one tinted line rather than one
        // that loses its color at the fold.
        let content = PageContent::new(vec![vec![
            PageSpan::new(PageStyle::Removed, "    -"),
            PageSpan::new(PageStyle::RemovedEdit, "old".repeat(4)),
        ]])
        .with_wrap_indents(vec![5]);
        let theme = Theme::dark();
        let grid = build_page_grid(&content, &theme, 10, 3, IconStyle::None, true);
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
            Some(page_span_style(PageStyle::Removed, &theme)),
            "the gutter keeps the line's tint"
        );
        assert_eq!(
            grid.cell(1, 5).map(|cell| cell.style),
            Some(page_span_style(PageStyle::RemovedEdit, &theme)),
            "the spilled text keeps its own style"
        );
    }

    #[test]
    fn test_a_truncated_diff_line_keeps_its_row_under_the_next_line() {
        // The shape the bug came from: a removed line long enough to be
        // clipped, with the added line under it. Before the fix the added
        // line painted over the removed one's screen row, so the decorator
        // showed one line wearing the wrong side's tint.
        let content = PageContent::new(vec![
            vec![
                PageSpan::new(PageStyle::Removed, "-"),
                PageSpan::new(PageStyle::RemovedEdit, "old".repeat(10)),
            ],
            vec![
                PageSpan::new(PageStyle::Added, "+"),
                PageSpan::new(PageStyle::AddedEdit, "new"),
            ],
        ]);
        let theme = Theme::dark();
        let grid = build_page_grid(&content, &theme, 6, 2, IconStyle::None, false);
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
            Some(page_span_style(PageStyle::Removed, &theme)),
            "the removed row's cells keep the removed tint"
        );
        assert_eq!(
            grid.cell(1, 0).map(|cell| cell.style),
            Some(page_span_style(PageStyle::Added, &theme)),
            "the added row's cells carry the added tint"
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

        let placements = image_placements(&app.image_blocks, &app.panes, &rects, 20.0, &[]);
        assert_eq!(placements.len(), 1, "fixture: the block places normally");

        app.panes
            .get_mut(&pane_id)
            .unwrap()
            .grid_mut()
            .enter_alt_screen();
        let placements = image_placements(&app.image_blocks, &app.panes, &rects, 20.0, &[]);
        assert!(
            placements.is_empty(),
            "an alt-screen pane must not have primary-screen blocks painted over it"
        );

        app.panes
            .get_mut(&pane_id)
            .unwrap()
            .grid_mut()
            .leave_alt_screen();
        let placements = image_placements(&app.image_blocks, &app.panes, &rects, 20.0, &[]);
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
