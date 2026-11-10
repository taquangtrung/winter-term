//! A page of Winter's written as HTML: an extension's view (the fragment its
//! [`View::page`](winter_ext_api::View::page) returns), the Settings page, the
//! Keys page. The page's HTML is wrapped in a stylesheet of the theme's colors,
//! laid out and painted by the web engine, and drawn as a picture over the tab.
//! It is sealed: nothing it names is fetched, and no script runs. A click is
//! told to the page's editor only as the action the element under it names in
//! `data-action`. A page taller than the pane scrolls, its `data-current`
//! element kept in view. At a fractional scale the page is drawn at the next
//! whole one and shrunk to the pane, since laid out at 125% a 1px border can
//! round away to nothing.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use winter_model::editor::ScrollPosition;
use winter_model::paint::Point;
use winter_model::picture::{Content, Tile};
use winter_render::Theme;

use super::preview::WebPage;

// ========================================================================
// Constants
// ========================================================================

/// The palette slots the stylesheet takes its hues from.
const ANSI_RED: usize = 1;
const ANSI_YELLOW: usize = 3;
const ANSI_BLUE: usize = 4;

/// How strongly the foreground tints the background for a band, a
/// border, and dim text, out of 255.
const TINT_ALPHA: u8 = 22;
const BORDER_ALPHA: u8 = 60;
const MUTED_ALPHA: u8 = 150;

/// The attribute marking the element a page keeps in view.
const CURRENT_MARKER: &str = "data-current";

/// The next key a painted page is uploaded under, from high in the range,
/// so it never meets a key another editor's tiles use.
static NEXT_KEY: AtomicU64 = AtomicU64::new(1 << 62);

// ========================================================================
// Data Structures
// ========================================================================

/// A page, laid out for the pane it is drawn in.
#[derive(Default)]
pub(super) struct PageView {
    /// Whether the theme is dark, which the engine is told.
    dark: bool,
    /// The pane's rows and columns, which a click is counted in.
    grid: (usize, usize),
    /// What the page was laid out from, to lay it out again only when it
    /// changes.
    markup: Option<String>,
    page: Option<WebPage>,
    /// The pane's size, in pixels.
    pane: (f32, f32),
    /// Screen pixels to a logical pixel.
    scale: f32,
    /// How far down the page the pane's top is, in pixels.
    scroll: f32,
    /// The theme's selection fill.
    selection: (u8, u8, u8),
    /// The theme as CSS, wrapped around every page.
    stylesheet: String,
    /// The page painted, as the host draws it.
    tiles: Vec<Tile>,
}

// ========================================================================
// PageView
// ========================================================================

impl PageView {
    /// Take the theme and the screen's scale, laying the page out again
    /// when either changed.
    pub(super) fn set_appearance(&mut self, theme: &Theme, scale: f32) {
        let stylesheet = stylesheet(theme);
        if stylesheet == self.stylesheet && scale == self.scale {
            return;
        }
        self.stylesheet = stylesheet;
        self.scale = scale;
        self.dark = theme.background.luminance() < 0.5;
        let fill = theme.selection_bg;
        self.selection = (fill.r, fill.g, fill.b);
        self.page = None;
    }

    /// Take the pane's size in pixels, laying the page out to it.
    pub(super) fn set_pane_size(&mut self, pane: (f32, f32)) {
        if pane == self.pane {
            return;
        }
        self.pane = pane;
        let (size, scale) = (self.drawn_size(), self.drawn_scale());
        if let Some(page) = self.page.as_mut() {
            page.resize(size, scale, self.dark);
            self.reveal_current();
            self.paint();
        }
    }

    /// Show `markup` in a pane of `grid` rows and columns: laid out again
    /// only when it, the theme, or the scale changed.
    pub(super) fn show(&mut self, markup: String, grid: (usize, usize)) {
        self.grid = grid;
        if self.page.is_some() && self.markup.as_deref() == Some(markup.as_str()) {
            return;
        }
        let html = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><style>{}</style></head>\
             <body>{markup}</body></html>",
            self.stylesheet
        );
        self.page = Some(WebPage::sealed(
            &html,
            self.drawn_size(),
            self.drawn_scale(),
            self.dark,
        ));
        self.markup = Some(markup);
        self.reveal_current();
        self.paint();
    }

    /// Show the view's lines again, the page put away.
    pub(super) fn hide(&mut self) {
        self.page = None;
        self.markup = None;
        self.scroll = 0.0;
        self.tiles.clear();
    }

    /// Whether a page is shown in place of the lines.
    pub(super) fn is_shown(&self) -> bool {
        self.page.is_some()
    }

    /// The page painted, for the host to draw over the pane.
    pub(super) fn tiles(&self) -> Vec<Tile> {
        self.tiles.clone()
    }

    /// The action the element under pane cell `at` names, if any.
    pub(super) fn action_at(&self, at: Point) -> Option<String> {
        let (rows, cols) = self.grid;
        if rows == 0 || cols == 0 {
            return None;
        }
        let x = (at.col as f32 + 0.5) * self.pane.0 / cols as f32;
        let y = (at.row as f32 + 0.5) * self.pane.1 / rows as f32;
        let drawn = self.drawn_per_pane();
        self.page
            .as_ref()?
            .action_at(x * drawn, (y + self.scroll) * drawn)
    }

    /// Scroll the page `pixels` up, positive upwards as the wheel counts.
    pub(super) fn scroll_by(&mut self, pixels: f32) {
        self.scroll_to(self.scroll - pixels);
    }

    /// Scroll the page `rows` of the pane's rows up.
    pub(super) fn scroll_rows(&mut self, rows: isize) {
        self.scroll_by(rows as f32 * self.row_height());
    }

    /// Scroll the pane's top to `scroll`, kept on the page, and paint what
    /// shows there.
    pub(super) fn scroll_to(&mut self, scroll: f32) {
        let scroll = scroll.clamp(0.0, self.max_scroll()).round();
        if scroll != self.scroll {
            self.scroll = scroll;
            self.paint();
        }
    }

    /// Where the pane sits on the page, for its scrollbar; `None` while
    /// the page fits it.
    pub(super) fn scroll_position(&self) -> Option<ScrollPosition> {
        let max = self.max_scroll();
        let visible = self.pane.1;
        (max > 0.0 && visible > 0.0).then_some(ScrollPosition::new(
            self.scroll.round() as usize,
            visible.round() as usize,
            (max + visible).round() as usize,
        ))
    }

    /// The farthest the pane's top goes down the page, in whole pixels: a
    /// page the pane's height, give or take its rounding, does not scroll.
    fn max_scroll(&self) -> f32 {
        let height = self.page.as_ref().map_or(0.0, WebPage::height);
        (height / self.drawn_per_pane() - self.pane.1)
            .floor()
            .max(0.0)
    }

    /// The scale the page is drawn at: the screen's, or the whole one above
    /// it when it is fractional.
    fn drawn_scale(&self) -> f32 {
        if self.scale > 0.0 {
            self.scale.ceil()
        } else {
            1.0
        }
    }

    /// Pixels of the page as drawn to a pixel of the pane.
    fn drawn_per_pane(&self) -> f32 {
        let screen = if self.scale > 0.0 { self.scale } else { 1.0 };
        self.drawn_scale() / screen
    }

    /// The pane's size as the page is drawn, in its own pixels.
    fn drawn_size(&self) -> (u32, u32) {
        let drawn = self.drawn_per_pane();
        pixel_size((self.pane.0 * drawn, self.pane.1 * drawn))
    }

    /// The height of one of the pane's rows, in pixels.
    fn row_height(&self) -> f32 {
        match self.grid.0 {
            0 => 0.0,
            rows => self.pane.1 / rows as f32,
        }
    }

    /// Keep the scroll on the page, and the page's current element in the
    /// pane with a row to spare around it, scrolling no more than that
    /// takes.
    fn reveal_current(&mut self) {
        // Before the pane has a size, nothing is out of view: revealing
        // against a pane of no height would scroll past the page's top.
        if self.pane.1 <= 0.0 {
            return;
        }
        let mut scroll = self.scroll.clamp(0.0, self.max_scroll());
        let span = self
            .page
            .as_ref()
            .and_then(|page| page.span_of(CURRENT_MARKER));
        let drawn = self.drawn_per_pane();
        if let Some((top, bottom)) = span.map(|(top, bottom)| (top / drawn, bottom / drawn)) {
            let spare = self.row_height();
            if top - spare < scroll {
                scroll = top - spare;
            } else if bottom + spare > scroll + self.pane.1 {
                scroll = bottom + spare - self.pane.1;
            }
        }
        self.scroll = scroll.clamp(0.0, self.max_scroll()).round();
    }

    /// Paint the page the pane's size, under a key of its own, so the host
    /// uploads it once: drawn at its own scale, shown the pane's size.
    fn paint(&mut self) {
        let (drawn, top) = (self.drawn_size(), self.scroll * self.drawn_per_pane());
        let Some(page) = self.page.as_mut() else {
            return;
        };
        let scene = page.paint(top, drawn, self.selection);
        let size = pixel_size(self.pane);
        self.tiles = vec![Tile {
            content: Content::Scene(Arc::new(scene)),
            height: size.1 as f32,
            key: NEXT_KEY.fetch_add(1, Ordering::Relaxed),
            width: size.0 as f32,
            x: 0.0,
            y: 0.0,
        }];
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The theme as a stylesheet for a page: its colors as CSS variables a page
/// styles itself with, and a body in them.
fn stylesheet(theme: &Theme) -> String {
    let tint = |alpha: u8| {
        theme
            .foreground
            .with_alpha(alpha)
            .over(theme.background)
            .to_hex()
    };
    let (fg, bg) = (theme.foreground.to_hex(), theme.background.to_hex());
    let accent = theme.cursor_bg.to_hex();
    let info = theme.ansi[ANSI_BLUE].to_hex();
    let warning = theme.ansi[ANSI_YELLOW].to_hex();
    let error = theme.ansi[ANSI_RED].to_hex();
    let (band, border, muted) = (tint(TINT_ALPHA), tint(BORDER_ALPHA), tint(MUTED_ALPHA));
    format!(
        ":root {{ --fg: {fg}; --bg: {bg}; --muted: {muted}; --accent: {accent}; \
         --border: {border}; --tint: {band}; --info: {info}; --warning: {warning}; \
         --error: {error}; }}
html {{ background: var(--bg); }}
body {{ margin: 0; padding: 12px 16px; color: var(--fg); background: var(--bg);
  font-family: sans-serif; font-size: 14px; line-height: 1.4; }}"
    )
}

/// `text` as HTML shows it: the characters that would be read as markup
/// written as entities.
pub(super) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// A pane's size as whole pixels, at least one each way.
fn pixel_size(pane: (f32, f32)) -> (u32, u32) {
    (
        pane.0.round().max(1.0) as u32,
        pane.1.round().max(1.0) as u32,
    )
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A page of `rows` rows 40 pixels tall, the one at `current` marked.
    fn rows_page(rows: usize, current: usize) -> String {
        (0..rows)
            .map(|at| {
                let marked = if at == current { " data-current" } else { "" };
                format!("<div style=\"height: 40px\"{marked}>row {at}</div>")
            })
            .collect()
    }

    #[test]
    fn test_a_page_taller_than_the_pane_scrolls_to_keep_its_current_element_in_view() {
        let mut view = PageView::default();
        view.set_appearance(&Theme::dark(), 1.0);
        view.set_pane_size((400.0, 300.0));
        view.show(rows_page(3, 0), (10, 40));
        assert_eq!(
            view.scroll_position(),
            None,
            "a page that fits has no scrollbar"
        );

        view.show(rows_page(50, 40), (10, 40));
        let at = view.scroll_position().expect("a taller page scrolls");
        assert!(at.total > at.visible);
        let shown = at.offset as f32..at.offset as f32 + 300.0;
        let current = 12.0 + 40.0 * 40.0;
        assert!(
            shown.contains(&current) && shown.contains(&(current + 40.0)),
            "the current row is in view: {shown:?}"
        );

        view.show(rows_page(50, 0), (10, 40));
        assert_eq!(view.scroll_position().map(|at| at.offset), Some(0));
    }
    #[test]
    fn test_at_a_fractional_scale_the_page_is_drawn_at_the_whole_one_and_shown_the_pane_s_size() {
        let mut view = PageView::default();
        view.set_appearance(&Theme::dark(), 1.5);
        view.set_pane_size((400.0, 300.0));
        view.show(rows_page(50, 40), (10, 40));
        let tile = &view.tiles()[0];
        let Content::Scene(drawn) = &tile.content else {
            panic!("a page is drawn as a scene");
        };
        assert_eq!((drawn.width, drawn.height), (533, 400), "drawn at 2x");
        assert_eq!(
            (tile.width, tile.height),
            (400.0, 300.0),
            "shown at the pane's size"
        );
        let at = view.scroll_position().expect("a taller page scrolls");
        let shown = at.offset as f32..at.offset as f32 + 300.0;
        let current = (12.0 + 40.0 * 40.0) * 1.5;
        assert!(
            shown.contains(&current) && shown.contains(&(current + 60.0)),
            "the current row is in view, counted in the pane's pixels: {shown:?}"
        );
    }
}
