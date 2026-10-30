//! The engine a preview lays its page out and paints it with. Blitz, kept
//! behind this one type, so another engine can take its place without the
//! rest of the preview knowing.
//!
//! Everything here is measured in pixels of the screen, from the page's
//! top-left: the engine's own CSS pixels stay inside.

use std::any::Any;
use std::sync::Arc;

use anyrender::{
    Filter, Glyph, NormalizedCoord, Paint, PaintRef, PaintScene, RegisterResourceError, RenderContext,
    ResourceId,
};
use anyrender_vello::VelloScenePainter;
use blitz_dom::{local_name, BaseDocument, DocumentConfig, FontContext, BULLET_FONT};
use blitz_html::HtmlDocument;
use blitz_paint::paint_scene;
use blitz_traits::net::{Bytes, NetHandler, NetProvider, Request};
use blitz_traits::shell::{ColorScheme, Viewport};
use parley::fontique::{Blob, Collection, CollectionOptions, SourceCache};
use vello::kurbo::{Affine, Rect, Shape, Stroke, Vec2};
use vello::peniko::{BlendMode, Color, Fill, FontData, StyleRef};

use crate::model::picture::SizedScene;

use super::page::LINE_ATTRIBUTE;

// ========================================================================
// Constants
// ========================================================================

/// The fill Blitz paints a text selection in, whatever the page's colors:
/// a light blue light text cannot be read on.
const BLITZ_SELECTION: Color = Color::from_rgb8(180, 213, 255);

/// How many steps a point off the text is moved toward the middle of the
/// page, along its row, looking for text to select to.
const TEXT_PROBES: u16 = 8;

/// The largest local file a page may pull in, an image's say.
const MAX_LOCAL_FILE_BYTES: u64 = 32 * 1024 * 1024;

// ========================================================================
// Data Structures
// ========================================================================

/// A page laid out at a size: what it paints, where its links go, and where
/// each source line's block landed.
pub(super) struct WebPage {
    document: HtmlDocument,
    /// Each block's source line and the top of its box, by line.
    lines: Vec<(usize, f32)>,
    /// Screen pixels to a CSS pixel.
    scale: f32,
}

/// Hands the engine the files a page names on this machine: its images and
/// stylesheets. Nothing is fetched from the network, so a preview never
/// reaches out on its own; what is not a readable file is answered empty,
/// since a stylesheet left unanswered would hold back painting for good.
struct LocalFiles;

/// A painter passing everything through to `inner`, but for the fill of a
/// text selection, which it paints in `selection`.
struct Recolored<P> {
    inner: P,
    selection: Color,
}

// ========================================================================
// WebPage
// ========================================================================

impl WebPage {
    /// `html` laid out `size` screen pixels big at `scale`, relative links
    /// resolved against `base_url`, in a dark color scheme when `dark`.
    pub(super) fn new(html: &str, base_url: Option<String>, size: (u32, u32), scale: f32, dark: bool) -> Self {
        let config = DocumentConfig {
            base_url,
            font_ctx: Some(fonts()),
            net_provider: Some(Arc::new(LocalFiles)),
            viewport: Some(viewport(size, scale, dark)),
            ..Default::default()
        };
        let mut page = Self {
            document: HtmlDocument::from_html(html, config),
            lines: Vec::new(),
            scale,
        };
        page.lay_out();
        page
    }

    /// Lay the page out again `size` screen pixels big at `scale`.
    pub(super) fn resize(&mut self, size: (u32, u32), scale: f32, dark: bool) {
        self.scale = scale;
        self.document.set_viewport(viewport(size, scale, dark));
        self.lay_out();
    }

    /// How tall the laid-out page is.
    pub(super) fn height(&self) -> f32 {
        self.document
            .try_root_element()
            .map_or(0.0, |root| root.final_layout().size.height * self.scale)
    }

    /// The `size` stretch of the page starting `top` pixels down, painted
    /// from its top-left, a text selection filled in `selection`.
    pub(super) fn paint(&mut self, top: f32, size: (u32, u32), selection: (u8, u8, u8)) -> SizedScene {
        let mut painted = SizedScene {
            height: size.1,
            width: size.0,
            ..Default::default()
        };
        let mut painter = Recolored {
            inner: VelloScenePainter::new(&mut painted.scene),
            selection: Color::from_rgb8(selection.0, selection.1, selection.2),
        };
        self.paint_into(&mut painter, top, size);
        painted
    }

    /// Paint the `size` stretch of the page starting `top` pixels down with
    /// `painter`.
    fn paint_into(&mut self, painter: &mut impl PaintScene, top: f32, size: (u32, u32)) {
        let document: &mut BaseDocument = &mut self.document;
        // Blitz paints from where its viewport is scrolled to; the page is
        // left unscrolled otherwise, so its geometry reads from the top.
        document.set_viewport_scroll(blitz_dom::Point { x: 0.0, y: f64::from(top / self.scale) });
        paint_scene(painter, document, f64::from(self.scale), size.0, size.1, 0, 0);
        document.set_viewport_scroll(blitz_dom::Point { x: 0.0, y: 0.0 });
    }

    /// Where the link at `(x, y)` goes, as its `href` reads.
    pub(super) fn link_at(&self, x: f32, y: f32) -> Option<String> {
        let hit = self.document.hit(x / self.scale, y / self.scale)?;
        let mut at = Some(hit.node_id);
        while let Some(id) = at {
            let node = self.document.get_node(id)?;
            if node.data.is_element_with_tag_name(&local_name!("a")) {
                if let Some(href) = node.attr(local_name!("href")) {
                    return Some(href.to_string());
                }
            }
            at = node.parent;
        }
        None
    }

    /// Start a selection at `(x, y)`, letting go of any other. Says whether
    /// there is text there to start it on.
    pub(super) fn select_from(&mut self, x: f32, y: f32) -> bool {
        self.document.clear_text_selection();
        let Some(x) = self.text_along(x, y) else {
            return false;
        };
        let Some((node, offset)) = self.document.find_text_position(x, y / self.scale) else {
            return false;
        };
        self.document.set_text_selection(node, offset, node, offset);
        true
    }

    /// Stretch the selection to `(x, y)`. Says whether it moved.
    pub(super) fn select_to(&mut self, x: f32, y: f32) -> bool {
        match self.text_along(x, y) {
            Some(x) => self.document.extend_text_selection_to_point(x, y / self.scale),
            None => false,
        }
    }

    /// The nearest point to `(x, y)` along its row, toward the middle of the
    /// page, that has text under it, in the engine's own pixels: a drag out
    /// into a margin still selects to the end of the line, as a browser's
    /// does.
    fn text_along(&self, x: f32, y: f32) -> Option<f32> {
        let (x, y) = (x / self.scale, y / self.scale);
        let middle = self.document.viewport().window_size.0 as f32 / self.scale / 2.0;
        (0..=TEXT_PROBES)
            .map(|step| x + (middle - x) * f32::from(step) / f32::from(TEXT_PROBES))
            .find(|probe| self.document.find_text_position(*probe, y).is_some())
    }

    /// The text selected, where any is.
    pub(super) fn selected_text(&self) -> Option<String> {
        self.document.get_selected_text().filter(|text| !text.is_empty())
    }

    /// Let go of the selection. Says whether there was text selected.
    pub(super) fn clear_selection(&mut self) -> bool {
        let had = self.selected_text().is_some();
        self.document.clear_text_selection();
        had
    }

    /// The top of the block holding source line `line`: the last block
    /// starting at or above it.
    pub(super) fn line_top(&self, line: usize) -> Option<f32> {
        let after = self.lines.partition_point(|(start, _)| *start <= line);
        after.checked_sub(1).map(|at| self.lines[at].1)
    }

    /// The source line of the block at height `y`: the last one whose top is
    /// at or above it.
    pub(super) fn line_at(&self, y: f32) -> Option<usize> {
        self.lines
            .iter()
            .filter(|(_, top)| *top <= y)
            .max_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
            .map(|(line, _)| *line)
    }

    /// The top of the element a `#name` link points at.
    pub(super) fn fragment_top(&self, name: &str) -> Option<f32> {
        let target = self.document.get_fragment_target(name)?;
        self.top_of(target)
    }

    /// Style and lay the page out, and read where each block landed.
    fn lay_out(&mut self) {
        self.document.resolve(0.0);
        let selector = format!("[{LINE_ATTRIBUTE}]");
        let marked = self.document.query_selector_all(&selector).unwrap_or_default();
        let mut lines: Vec<(usize, f32)> = marked
            .iter()
            .filter_map(|id| {
                let node = self.document.get_node(*id)?;
                let line = node.attr(LINE_ATTRIBUTE.into())?.parse().ok()?;
                Some((line, self.top_of(*id)?))
            })
            .collect();
        lines.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        self.lines = lines;
    }

    /// The top of node `id`'s box.
    fn top_of(&self, id: blitz_dom::NodeId) -> Option<f32> {
        let rect = self.document.get_client_bounding_rect(id)?;
        Some(rect.y as f32 * self.scale)
    }
}

// ========================================================================
// LocalFiles
// ========================================================================

impl NetProvider for LocalFiles {
    fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let bytes = match request.url.scheme() {
            "file" => request.url.to_file_path().ok().and_then(|path| read_local(&path)),
            _ => None,
        };
        handler.bytes(request.url.to_string(), Bytes::from(bytes.unwrap_or_default()));
    }
}

/// The bytes of the local file at `path`, for a page that links it: only a
/// regular file, and only one small enough to hold. The page is laid out on
/// the window's thread, and `![](/dev/zero)` or a named pipe would otherwise
/// grow memory without end or never answer.
fn read_local(path: &std::path::Path) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_LOCAL_FILE_BYTES {
        return None;
    }
    std::fs::read(path).ok()
}

// ========================================================================
// Recolored
// ========================================================================

impl<P: PaintScene> RenderContext for Recolored<P> {
    fn try_register_custom_resource(&mut self, resource: Box<dyn Any>) -> Result<ResourceId, RegisterResourceError> {
        self.inner.try_register_custom_resource(resource)
    }

    fn unregister_resource(&mut self, resource_id: ResourceId) {
        self.inner.unregister_resource(resource_id);
    }

    fn renderer_specific_context(&self) -> Option<Box<dyn Any>> {
        self.inner.renderer_specific_context()
    }
}

impl<P: PaintScene> PaintScene for Recolored<P> {
    fn reset(&mut self) {
        self.inner.reset();
    }

    fn push_layer(
        &mut self,
        fill: Fill,
        blend: impl Into<BlendMode>,
        alpha: f32,
        transform: Affine,
        clip: &impl Shape,
        filter: Option<Arc<Filter>>,
        backdrop_filter: Option<Arc<Filter>>,
    ) {
        self.inner.push_layer(fill, blend, alpha, transform, clip, filter, backdrop_filter);
    }

    fn push_clip_layer(&mut self, fill: Fill, transform: Affine, clip: &impl Shape) {
        self.inner.push_clip_layer(fill, transform, clip);
    }

    fn pop_layer(&mut self) {
        self.inner.pop_layer();
    }

    fn stroke<'a>(
        &mut self,
        style: &Stroke,
        transform: Affine,
        brush: impl Into<PaintRef<'a>>,
        brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        self.inner.stroke(style, transform, brush, brush_transform, shape);
    }

    fn fill<'a>(
        &mut self,
        style: Fill,
        transform: Affine,
        brush: impl Into<PaintRef<'a>>,
        brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        let brush = match brush.into() {
            Paint::Solid(color) if color == BLITZ_SELECTION => Paint::Solid(self.selection),
            other => other,
        };
        self.inner.fill(style, transform, brush, brush_transform, shape);
    }

    fn draw_glyphs<'a, 's: 'a>(
        &'s mut self,
        font: &'a FontData,
        font_size: f32,
        hint: bool,
        normalized_coords: &'a [NormalizedCoord],
        embolden: Vec2,
        style: impl Into<StyleRef<'a>>,
        brush: impl Into<PaintRef<'a>>,
        brush_alpha: f32,
        transform: Affine,
        glyph_transform: Option<Affine>,
        glyphs: impl Iterator<Item = Glyph> + Clone,
    ) {
        self.inner.draw_glyphs(
            font,
            font_size,
            hint,
            normalized_coords,
            embolden,
            style,
            brush,
            brush_alpha,
            transform,
            glyph_transform,
            glyphs,
        );
    }

    fn draw_box_shadow(&mut self, transform: Affine, rect: Rect, brush: Color, radius: f64, std_dev: f64) {
        self.inner.draw_box_shadow(transform, rect, brush, radius, std_dev);
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// The fonts a page is set in: the system's, found once a thread and shared
/// by every page after, since a page is laid out afresh on every keystroke
/// and finding them takes longer than the rest of its layout.
fn fonts() -> FontContext {
    thread_local! {
        static FONTS: FontContext = {
            let mut fonts = FontContext {
                collection: Collection::new(CollectionOptions {
                    shared: true,
                    system_fonts: true,
                }),
                source_cache: SourceCache::new_shared(),
            };
            fonts.collection.register_fonts(Blob::new(Arc::new(BULLET_FONT) as _), None);
            fonts
        };
    }
    FONTS.with(Clone::clone)
}

/// The viewport a page is laid out in: `size` screen pixels at `scale`.
fn viewport(size: (u32, u32), scale: f32, dark: bool) -> Viewport {
    let scheme = match dark {
        true => ColorScheme::Dark,
        false => ColorScheme::Light,
    };
    Viewport::new(size.0.max(1), size.1.max(1), scale, scheme)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn test_a_page_cannot_pull_in_a_device_or_a_pipe() {
        // `![](/dev/zero)` read without end on the window's thread.
        assert_eq!(read_local(std::path::Path::new("/dev/zero")), None);
        let file = std::env::temp_dir().join(format!("winter-preview-local-{}.txt", std::process::id()));
        std::fs::write(&file, b"ok").expect("file");
        assert_eq!(read_local(&file), Some(b"ok".to_vec()));
        let _ = std::fs::remove_file(&file);
    }

    const PAGE: &str = "<!doctype html><html><body style=\"margin:0\">\
        <div data-line=\"0\"><h1 id=\"top\">Title</h1></div>\
        <div data-line=\"2\" style=\"height:500px\">filler</div>\
        <div data-line=\"7\"><a href=\"https://example.com/\">a link</a></div>\
        </body></html>";

    #[test]
    fn test_blocks_and_links_are_found_where_the_page_laid_them_out() {
        let page = WebPage::new(PAGE, None, (400, 300), 2.0, false);
        let link_top = page.line_top(7).expect("line 7 laid out");
        assert!(link_top >= 1000.0, "500 CSS pixels of filler at scale 2: {link_top}");
        assert_eq!(page.line_top(5), page.line_top(2), "a line between blocks belongs to the one above");
        assert_eq!(page.line_at(link_top + 1.0), Some(7));
        assert_eq!(
            page.link_at(10.0, link_top + 4.0).as_deref(),
            Some("https://example.com/"),
            "a link far below the first screen is still hit"
        );
        assert_eq!(page.link_at(10.0, 4.0), None);
        assert_eq!(page.fragment_top("top"), page.line_top(0));
        assert!(page.height() > link_top);
    }

    #[test]
    fn test_a_selection_dragged_over_text_is_read_back_and_painted_in_the_theme_s_fill() {
        let mut page = WebPage::new(PAGE, None, (400, 300), 1.0, false);
        let title = page.line_top(0).expect("title laid out") + 10.0;
        assert!(page.select_from(1.0, title), "text where the drag starts");
        assert!(page.select_to(300.0, title));
        assert_eq!(page.selected_text().as_deref(), Some("Title"));

        let theme_fill = Color::from_rgb8(34, 87, 160);
        let mut painter = Recolored {
            inner: anyrender::Scene::new(),
            selection: theme_fill,
        };
        page.paint_into(&mut painter, 0.0, (400, 300));
        let fills: Vec<Color> = painter
            .inner
            .commands
            .iter()
            .filter_map(|command| match command {
                anyrender::recording::RenderCommand::Fill(fill) => match fill.brush {
                    Paint::Solid(color) => Some(color),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert!(fills.contains(&theme_fill), "the selection takes the theme's fill");
        assert!(!fills.contains(&BLITZ_SELECTION), "never Blitz's own");

        assert!(page.clear_selection());
        assert_eq!(page.selected_text(), None);
    }

    #[test]
    fn test_an_image_beside_the_page_is_loaded_and_one_on_the_network_is_not_waited_for() {
        let dir = std::env::temp_dir().join(format!("winter-preview-image-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        image::RgbaImage::new(40, 30).save(dir.join("shot.png")).expect("png");
        let html = "<!doctype html><html><head><link rel=\"stylesheet\" href=\"https://example.com/a.css\"></head>\
            <body style=\"margin:0\"><div data-line=\"0\"><img src=\"shot.png\"></div><div data-line=\"1\">after</div></body></html>";
        let base = url::Url::from_file_path(dir.join("notes.md")).ok().map(String::from);
        let page = WebPage::new(html, base, (400, 300), 1.0, false);
        let after = page.line_top(1).expect("laid out, not held back by the remote stylesheet");
        assert!(after >= 30.0, "the image takes its 30 pixels: {after}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
