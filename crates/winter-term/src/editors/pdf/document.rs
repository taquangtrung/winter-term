//! An open document: its pages' sizes, and each page's text, read from the
//! PDF the first time something needs it and kept.

use std::collections::HashMap;
use std::sync::Arc;

use hayro::hayro_interpret::font::{self, GlyphRun};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_interpret::{
    interpret_page, BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image,
    ImageDrawProps, InterpreterCache, SoftMask,
};
use hayro::hayro_syntax::Pdf;
use hayro::kurbo::{BezPath, Point, Rect};

use super::links::{page_links, Found, Link, LinkTo};
use super::references;
use super::outline::{Entry, Target};
use super::text::{union, Glyph, PageText};

// ========================================================================
// Constants
// ========================================================================

/// Glyph units to an em: hayro places every glyph in a space this many to
/// its font size, the convention of Type 1 and of most fonts besides.
const UNITS_PER_EM: f64 = 1000.0;

/// How many pages, from the first, the text's box is measured over: enough
/// to see past a title page, few enough to read at once.
const TEXT_BOX_PAGES: usize = 10;

/// How far a glyph with no advance of its own is taken to reach, in glyph
/// units: half an em, a typical letter.
const FALLBACK_ADVANCE: f64 = 500.0;

// ========================================================================
// Data Structures
// ========================================================================

/// One open document.
pub struct Document {
    /// Each page's links, once read.
    links: HashMap<usize, Vec<Link>>,
    /// The bookmarks it lists its sections under.
    outline: Vec<Entry>,
    /// The parsed PDF, read for text; `None` for a document made of text
    /// alone, as a test's is.
    pdf: Option<Arc<Pdf>>,
    /// Where each entry of the numbered bibliography starts, once a
    /// citation has needed it looked for.
    references: Option<HashMap<usize, Target>>,
    /// Each page's size in points.
    sizes: Vec<(f32, f32)>,
    /// Each page's text, once read.
    texts: HashMap<usize, PageText>,
}

/// Collects the glyphs a page draws, and draws nothing.
struct GlyphCollector {
    glyphs: Vec<Glyph>,
}

// ========================================================================
// Document
// ========================================================================

impl Document {
    /// The document `pdf`, its pages `sizes` points, its bookmarks
    /// `outline`.
    pub fn new(pdf: Arc<Pdf>, sizes: Vec<(f32, f32)>, outline: Vec<Entry>) -> Self {
        Self {
            links: HashMap::new(),
            outline,
            pdf: Some(pdf),
            references: None,
            sizes,
            texts: HashMap::new(),
        }
    }

    /// A document with no PDF behind it, of pages `sizes` reading `texts`.
    #[cfg(test)]
    pub fn of_text(sizes: Vec<(f32, f32)>, texts: Vec<PageText>) -> Self {
        Self {
            links: HashMap::new(),
            outline: Vec::new(),
            pdf: None,
            references: None,
            sizes,
            texts: texts.into_iter().enumerate().collect(),
        }
    }

    /// A text-only document with bookmarks `outline`.
    #[cfg(test)]
    pub fn with_outline(self, outline: Vec<Entry>) -> Self {
        Self { outline, ..self }
    }

    /// A text-only document whose page `page` carries `links`.
    #[cfg(test)]
    pub fn with_links(mut self, page: usize, links: Vec<Link>) -> Self {
        self.links.insert(page, links);
        self
    }

    /// A text-only document whose page `page` carries citation links with
    /// no destination of their own, from boxes `citations`.
    #[cfg(test)]
    pub fn with_citations(mut self, page: usize, citations: Vec<(f32, f32, f32, f32)>) -> Self {
        let found = citations.into_iter().map(Found::Citation).collect();
        let links = self.resolve(page, found);
        self.links.insert(page, links);
        self
    }

    /// Page `page`'s links, read the first time they are asked for: none
    /// past the last page.
    pub fn links(&mut self, page: usize) -> &[Link] {
        if !self.links.contains_key(&page) {
            let found = match &self.pdf {
                Some(pdf) => page_links(pdf, page),
                None => Vec::new(),
            };
            let links = self.resolve(page, found);
            self.links.insert(page, links);
        }
        &self.links[&page]
    }

    /// The links `found` on page `page`, a citation sent to the entry of the
    /// bibliography its number names, or left out when none can be found.
    fn resolve(&mut self, page: usize, found: Vec<Found>) -> Vec<Link> {
        found
            .into_iter()
            .filter_map(|found| match found {
                Found::Link(link) => Some(link),
                Found::Citation(bounds) => {
                    let text = references::text_in(self.text(page)?, bounds);
                    let target = self.reference(references::cited_number(&text)?)?;
                    Some(Link {
                        bounds,
                        to: LinkTo::Spot(target),
                    })
                }
            })
            .collect()
    }

    /// Where entry `number` of the numbered bibliography starts, the
    /// bibliography looked for the first time one is asked for.
    fn reference(&mut self, number: usize) -> Option<Target> {
        if self.references.is_none() {
            self.references = Some(self.find_references());
        }
        self.references.as_ref()?.get(&number).copied()
    }

    /// The numbered bibliography, read back from the last page to the one
    /// where its first entry is: a bibliography follows the text, so the
    /// pages before it are never read for it.
    fn find_references(&mut self) -> HashMap<usize, Target> {
        let mut pages = Vec::new();
        for page in (0..self.page_count()).rev() {
            let markers = self.text(page).map(references::markers).unwrap_or_default();
            let first = markers.iter().any(|marker| marker.number == 1);
            pages.push((page, markers));
            if first {
                break;
            }
        }
        let found = pages
            .into_iter()
            .rev()
            .flat_map(|(page, markers)| markers.into_iter().map(move |marker| (page, marker)));
        references::bibliography(found)
    }

    /// Page `page`'s links if they have been read already, without reading
    /// them: none otherwise.
    pub fn read_links(&self, page: usize) -> &[Link] {
        self.links.get(&page).map_or(&[], Vec::as_slice)
    }

    /// The bookmarks it lists its sections under, depth first.
    pub fn outline(&self) -> &[Entry] {
        &self.outline
    }

    /// How many pages it has.
    pub fn page_count(&self) -> usize {
        self.sizes.len()
    }

    /// Each page's size in points.
    pub fn sizes(&self) -> &[(f32, f32)] {
        &self.sizes
    }

    /// Keep `text` as page `page`'s, read elsewhere, unless it has been read
    /// already.
    pub fn insert_text(&mut self, page: usize, text: PageText) {
        self.texts.entry(page).or_insert(text);
    }

    /// The box the text of the first [`TEXT_BOX_PAGES`] pages falls in, as
    /// `(left, top, width, height)` in points, or `None` for pages with no
    /// text, as a scan's are.
    pub fn text_box(&mut self) -> Option<(f32, f32, f32, f32)> {
        let mut found: Option<(f32, f32, f32, f32)> = None;
        for page in 0..self.page_count().min(TEXT_BOX_PAGES) {
            let Some(text) = self.text(page) else {
                continue;
            };
            for bounds in text.lines.iter().filter_map(|line| line.bounds()) {
                found = Some(found.map_or(bounds, |found| union(found, bounds)));
            }
        }
        found
    }

    /// Page `page`'s text if it has been read already, without reading it.
    pub fn read(&self, page: usize) -> Option<&PageText> {
        self.texts.get(&page)
    }

    /// Page `page`'s text, read the first time it is asked for. `None` past
    /// the last page.
    pub fn text(&mut self, page: usize) -> Option<&PageText> {
        if page >= self.sizes.len() {
            return None;
        }
        if !self.texts.contains_key(&page) {
            let text = self
                .pdf
                .as_ref()
                .map(|pdf| page_text(pdf, page))
                .unwrap_or_default();
            self.texts.insert(page, text);
        }
        self.texts.get(&page)
    }
}

// ========================================================================
// GlyphCollector
// ========================================================================

impl<'a> Device<'a> for GlyphCollector {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}

    fn push_clip_path(&mut self, _: &ClipPath) {}

    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}

    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
        for glyph in run.glyphs() {
            let Some(text) = glyph.as_unicode().map(unicode_text) else {
                continue;
            };
            let place = props.transform * glyph.transform();
            let origin = place * Point::ORIGIN;
            let advance = match &**glyph {
                font::Glyph::Outline(outline) => {
                    outline.advance_width().map_or(FALLBACK_ADVANCE, f64::from)
                }
                font::Glyph::Type3(_) => FALLBACK_ADVANCE,
            };
            let end = place * Point::new(advance, 0.0);
            let up = place * Point::new(0.0, UNITS_PER_EM);
            self.glyphs.push(Glyph {
                height: origin.distance(up) as f32,
                text,
                width: (end.x - origin.x).abs() as f32,
                x: origin.x as f32,
                y: origin.y as f32,
            });
        }
    }

    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}

    fn pop_clip(&mut self) {}

    fn pop_transparency_group(&mut self) {}
}

// ========================================================================
// Free functions
// ========================================================================

/// The text of page `page` of `pdf`, rebuilt from the glyphs it draws.
pub fn page_text(pdf: &Pdf, page: usize) -> PageText {
    PageText::new(read_glyphs(pdf, page))
}

/// What a glyph's Unicode value reads as.
fn unicode_text(unicode: BfString) -> String {
    match unicode {
        BfString::Char(ch) => ch.to_string(),
        BfString::String(text) => text,
    }
}

/// The glyphs page `page` of `pdf` draws, placed as the page is shown:
/// turned and cropped, at scale one, down from its top-left.
fn read_glyphs(pdf: &Pdf, page: usize) -> Vec<Glyph> {
    let Some(page) = pdf.pages().get(page) else {
        return Vec::new();
    };
    let (width, height) = page.render_dimensions();
    let cache = InterpreterCache::new();
    let mut context = Context::new(
        page.initial_transform(true).to_kurbo(),
        Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
        &cache,
        page.xref(),
        Default::default(),
    );
    let mut collector = GlyphCollector { glyphs: Vec::new() };
    // A page hayro cannot read gives no text, rather than taking the tab down.
    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        interpret_page(page, &mut context, &mut collector);
    }));
    match read {
        Ok(()) => collector.glyphs,
        Err(_) => Vec::new(),
    }
}
