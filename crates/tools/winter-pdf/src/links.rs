//! The links a page carries, read from its link annotations: hyperref's
//! `\ref`, `\cite`, and contents lines, which point at a spot in the
//! document, and the URLs a document points out at, each with the box on the
//! page it is followed from.

use hayro::hayro_interpret::TransformExt;
use hayro::hayro_syntax::object::dict::keys::{A, ANNOTS, RECT, S, SUBTYPE, URI};
use hayro::hayro_syntax::object::{Array, Dict, Name, Rect, String as PdfString};
use hayro::hayro_syntax::Pdf;
use hayro::kurbo::Point;

use super::outline::{Reader, Target};

// ========================================================================
// Constants
// ========================================================================

/// The annotation subtype a link is.
const LINK: &[u8] = b"Link";

/// The action a link to a URL carries.
const URI_ACTION: &[u8] = b"URI";

/// The most links read from one page: a malformed page listing annotations
/// without end stops here.
const MAX_LINKS: usize = 2_000;

/// How hyperref starts the name of a citation's destination.
const CITATION_PREFIX: &[u8] = b"cite.";

// ========================================================================
// Data Structures
// ========================================================================

/// One link on a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    /// The box it is followed from, as `(left, top, width, height)` in
    /// points, down from the page's top-left as it is shown.
    pub bounds: (f32, f32, f32, f32),
    /// Where it leads.
    pub to: LinkTo,
}

/// A link as a page lists it: one that leads somewhere, or a citation
/// whose destination the PDF no longer names, by the box it is followed
/// from, for the bibliography to be looked up from its text.
#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    Citation((f32, f32, f32, f32)),
    Link(Link),
}

/// Where a link leads.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkTo {
    /// A spot in the document.
    Spot(Target),
    /// A URL, for the system to open.
    Uri(String),
}

// ========================================================================
// Link
// ========================================================================

impl Link {
    /// Whether the point `(x, y)`, in the page's shown points, is in it.
    pub fn holds(&self, x: f32, y: f32) -> bool {
        let (left, top, width, height) = self.bounds;
        (left..=left + width).contains(&x) && (top..=top + height).contains(&y)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The links on page `page` of `pdf`, in the order it lists them: those
/// leading nowhere this can work out are left out, but for a citation.
pub fn page_links(pdf: &Pdf, page: usize) -> Vec<Found> {
    let (Some(reader), Some(shown)) = (Reader::new(pdf), pdf.pages().get(page)) else {
        return Vec::new();
    };
    let Some(annots) = shown.raw().get::<Array>(ANNOTS) else {
        return Vec::new();
    };
    let transform = shown.initial_transform(true).to_kurbo();
    annots
        .iter::<Dict>()
        .take(MAX_LINKS)
        .filter(|annot| {
            annot
                .get::<Name>(SUBTYPE)
                .is_some_and(|kind| &*kind == LINK)
        })
        .filter_map(|annot| {
            let rect = annot.get::<Rect>(RECT)?;
            let a = transform * Point::new(rect.x0, rect.y0);
            let b = transform * Point::new(rect.x1, rect.y1);
            let (left, top) = (a.x.min(b.x) as f32, a.y.min(b.y) as f32);
            let bounds = (
                left,
                top,
                (a.x - b.x).abs() as f32,
                (a.y - b.y).abs() as f32,
            );
            match link_to(&reader, &annot) {
                Some(to) => Some(Found::Link(Link { bounds, to })),
                None => reader
                    .dest_name(&annot)
                    .filter(|name| name.starts_with(CITATION_PREFIX))
                    .map(|_| Found::Citation(bounds)),
            }
        })
        .collect()
}

/// Where the link annotation `annot` leads: the URL its action names, or
/// the spot its destination does.
fn link_to(reader: &Reader, annot: &Dict) -> Option<LinkTo> {
    let uri = annot
        .get::<Dict>(A)
        .filter(|action| {
            action
                .get::<Name>(S)
                .is_some_and(|kind| &*kind == URI_ACTION)
        })
        .and_then(|action| action.get::<PdfString>(URI));
    match uri {
        Some(uri) => Some(LinkTo::Uri(
            String::from_utf8_lossy(uri.as_bytes()).into_owned(),
        )),
        None => reader.target(annot).map(LinkTo::Spot),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-page PDF, 200 by 100 points: page one links to the top of page
    /// two, and out to a URL. Written with no cross-reference table, which
    /// a reader rebuilds.
    const LINKED: &str = "%PDF-1.4
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Annots [5 0 R 6 0 R 7 0 R 8 0 R 9 0 R] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] >> endobj
5 0 obj << /Type /Annot /Subtype /Link /Rect [10 70 60 80] /Dest [4 0 R /XYZ 0 100 0] >> endobj
6 0 obj << /Type /Annot /Subtype /Link /Rect [10 20 90 30] /A << /S /URI /URI (https://example.org) >> >> endobj
7 0 obj << /Type /Annot /Subtype /Text /Rect [0 0 10 10] >> endobj
8 0 obj << /Type /Annot /Subtype /Link /Rect [100 70 120 80] /Dest (cite.Smith20) >> endobj
9 0 obj << /Type /Annot /Subtype /Link /Rect [100 20 120 30] /Dest (section.1) >> endobj
trailer << /Root 1 0 R >>
%%EOF
";

    #[test]
    fn test_a_page_s_links_lead_to_spots_and_urls_from_boxes_measured_down() {
        let pdf = Pdf::new(LINKED.as_bytes().to_vec()).expect("a PDF");
        let links: Vec<Link> = page_links(&pdf, 0)
            .into_iter()
            .filter_map(|found| match found {
                Found::Link(link) => Some(link),
                Found::Citation(_) => None,
            })
            .collect();
        assert_eq!(links.len(), 2, "the note is no link: {links:?}");
        // PDF measures up from the bottom; the box is measured down.
        assert_eq!(links[0].bounds, (10.0, 20.0, 50.0, 10.0));
        assert_eq!(links[0].to, LinkTo::Spot(Target { page: 1, y: 0.0 }));
        assert_eq!(links[1].to, LinkTo::Uri("https://example.org".to_string()));
        assert!(links[0].holds(30.0, 25.0) && !links[0].holds(30.0, 75.0));
        assert!(page_links(&pdf, 1).is_empty());
    }

    #[test]
    fn test_a_citation_named_nowhere_is_kept_for_its_text_and_any_other_is_not() {
        let pdf = Pdf::new(LINKED.as_bytes().to_vec()).expect("a PDF");
        let citations: Vec<Found> = page_links(&pdf, 0)
            .into_iter()
            .filter(|found| matches!(found, Found::Citation(_)))
            .collect();
        assert_eq!(citations, [Found::Citation((100.0, 20.0, 20.0, 10.0))]);
    }
}
