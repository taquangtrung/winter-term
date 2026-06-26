//! The document's outline: the bookmarks a PDF lists its sections under,
//! read once when the document opens, each with the spot it points at.
//!
//! An entry's destination comes in four shapes: an explicit array naming a
//! page, a name looked up in the catalog's `/Dests`, a string looked up in
//! the `/Names` tree (what LaTeX's hyperref writes), or a `GoTo` action
//! carrying any of those.

use std::collections::{HashMap, HashSet};

use hayro::hayro_interpret::TransformExt;
use hayro::hayro_syntax::object::dict::keys::{
    A, D, DEST, DESTS, FIRST, KIDS, NAMES, NEXT, OUTLINES, S, TITLE,
};
use hayro::hayro_syntax::object::{Array, Dict, MaybeRef, Name, ObjRef, Object, String as PdfString};
use hayro::hayro_syntax::Pdf;
use hayro::kurbo::Point;

// ========================================================================
// Constants
// ========================================================================

/// The most entries read, and the deepest nesting followed: a malformed
/// outline that loops or nests without end stops here.
const MAX_ENTRIES: usize = 10_000;
const MAX_DEPTH: usize = 32;

/// The action an entry jumps with, when it carries one.
const GO_TO: &[u8] = b"GoTo";

// ========================================================================
// Data Structures
// ========================================================================

/// One bookmark, in the order the outline lists them, depth first.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// How deep it nests: zero for a top-level section.
    pub depth: usize,
    /// Where it points, when that could be worked out.
    pub target: Option<Target>,
    pub title: String,
}

/// A spot in the document: a page, counted from zero, and how far down it
/// in points, from the top as the page is shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub page: usize,
    pub y: f32,
}

/// What reading the outline needs at hand: which page each page object is,
/// and where named destinations are listed.
struct Reader<'a> {
    /// The catalog's `/Dests` dictionary, for destinations given by name.
    dests: Option<Dict<'a>>,
    /// The `/Names` tree's destinations, for those given by string.
    names: Option<Dict<'a>>,
    pages: HashMap<ObjRef, usize>,
    pdf: &'a Pdf,
}

// ========================================================================
// Reader
// ========================================================================

impl<'a> Reader<'a> {
    /// Each entry from `first` on and every entry under them, `depth` deep.
    fn walk(&self, first: Option<Dict<'a>>, depth: usize, seen: &mut HashSet<ObjRef>, out: &mut Vec<Entry>) {
        if depth >= MAX_DEPTH {
            return;
        }
        let mut item = first;
        while let Some(dict) = item {
            if out.len() >= MAX_ENTRIES {
                return;
            }
            // An entry met twice is a loop in the list, not a new one.
            if let Some(id) = dict.obj_id() {
                if !seen.insert(id.into()) {
                    return;
                }
            }
            out.push(Entry {
                depth,
                target: self.entry_target(&dict),
                title: dict
                    .get::<PdfString>(TITLE)
                    .map(|title| text_string(title.as_bytes()))
                    .unwrap_or_default(),
            });
            self.walk(dict.get::<Dict>(FIRST), depth + 1, seen, out);
            item = dict.get::<Dict>(NEXT);
        }
    }

    /// Where the entry `dict` points: its `/Dest`, or its `GoTo` action's.
    fn entry_target(&self, dict: &Dict<'a>) -> Option<Target> {
        if let Some(dest) = dict.get::<Object>(DEST) {
            return self.resolve(dest, 0);
        }
        let action = dict.get::<Dict>(A)?;
        let kind = action.get::<Name>(S)?;
        if &*kind != GO_TO {
            return None;
        }
        self.resolve(action.get::<Object>(D)?, 0)
    }

    /// The spot destination `dest` names, whatever shape it comes in.
    fn resolve(&self, dest: Object<'a>, hops: usize) -> Option<Target> {
        // A name that names a name that names a name is a loop.
        if hops > MAX_DEPTH {
            return None;
        }
        match dest {
            Object::Array(array) => self.explicit(&array),
            Object::Dict(dict) => self.resolve(dict.get::<Object>(D)?, hops + 1),
            Object::Name(name) => {
                let found = self.dests.as_ref()?.get::<Object>(&*name)?;
                self.resolve(found, hops + 1)
            }
            Object::String(key) => {
                let found = name_tree_find(self.names.clone()?, key.as_bytes(), 0)?;
                self.resolve(found, hops + 1)
            }
            _ => None,
        }
    }

    /// The spot an explicit destination array points at: its page, and the
    /// top its fit names, if it names one.
    fn explicit(&self, array: &Array<'a>) -> Option<Target> {
        let mut items = array.raw_iter();
        let page = match items.next()? {
            MaybeRef::Ref(page) => *self.pages.get(&page)?,
            MaybeRef::NotRef(Object::Number(number)) => usize::try_from(number.as_i64()).ok()?,
            MaybeRef::NotRef(_) => return None,
        };
        let rest: Vec<Object> = array.iter::<Object>().skip(1).collect();
        let fit = match rest.first() {
            Some(Object::Name(name)) => name.as_str().to_string(),
            _ => String::new(),
        };
        // Which of the numbers after the fit is the top edge, in the page's
        // own upward points.
        let top_at = match fit.as_str() {
            "XYZ" => Some(2),
            "FitH" | "FitBH" => Some(1),
            "FitR" => Some(4),
            _ => None,
        };
        let top = top_at.and_then(|at| match rest.get(at) {
            Some(Object::Number(number)) => Some(number.as_f64()),
            _ => None,
        });
        let y = match top {
            Some(top) => self.shown_y(page, top)?,
            None => 0.0,
        };
        Some(Target { page, y })
    }

    /// How far down page `page`, as shown, its own upward `top` is.
    fn shown_y(&self, page: usize, top: f64) -> Option<f32> {
        let page = self.pdf.pages().get(page)?;
        let shown = page.initial_transform(true).to_kurbo() * Point::new(0.0, top);
        let (_, height) = page.render_dimensions();
        Some((shown.y as f32).clamp(0.0, height))
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// `pdf`'s outline, flattened depth first: empty when it has none.
pub fn read(pdf: &Pdf) -> Vec<Entry> {
    let xref = pdf.xref();
    let Some(catalog) = xref.get::<Dict>(xref.root_id()) else {
        return Vec::new();
    };
    let Some(outlines) = catalog.get::<Dict>(OUTLINES) else {
        return Vec::new();
    };
    let pages = pdf
        .pages()
        .iter()
        .enumerate()
        .filter_map(|(index, page)| Some((page.raw().obj_id()?.into(), index)))
        .collect();
    let reader = Reader {
        dests: catalog.get::<Dict>(DESTS),
        names: catalog
            .get::<Dict>(NAMES)
            .and_then(|names| names.get::<Dict>(DESTS)),
        pages,
        pdf,
    };
    let mut entries = Vec::new();
    reader.walk(outlines.get::<Dict>(FIRST), 0, &mut HashSet::new(), &mut entries);
    entries
}

/// The value filed under `key` in the name tree under `node`.
fn name_tree_find<'a>(node: Dict<'a>, key: &[u8], depth: usize) -> Option<Object<'a>> {
    if depth >= MAX_DEPTH {
        return None;
    }
    if let Some(names) = node.get::<Array>(NAMES) {
        let mut items = names.iter::<Object>();
        while let (Some(name), Some(value)) = (items.next(), items.next()) {
            if matches!(&name, Object::String(name) if name.as_bytes() == key) {
                return Some(value);
            }
        }
    }
    let kids = node.get::<Array>(KIDS)?;
    kids.iter::<Dict>()
        .find_map(|kid| name_tree_find(kid, key, depth + 1))
}

/// A PDF text string as text: UTF-16 behind its byte-order mark, UTF-8
/// behind its own, and otherwise one character a byte, which reads the
/// PDFDocEncoding a title is usually in right for all but a few symbols.
/// Line breaks and tabs become spaces, since a title is shown on one line.
fn text_string(bytes: &[u8]) -> String {
    let text = match bytes {
        [0xFE, 0xFF, rest @ ..] => {
            let units: Vec<u16> = rest
                .chunks_exact(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        }
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => bytes.iter().map(|&byte| char::from(byte)).collect(),
    };
    text.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>()
        .trim()
        .to_string()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_utf16_title_is_decoded_behind_its_mark() {
        let bytes = [0xFE, 0xFF, 0x00, 0x49, 0x00, 0x6E, 0x00, 0x74, 0x00, 0x72, 0x00, 0x6F];
        assert_eq!(text_string(&bytes), "Intro");
    }

    #[test]
    fn test_a_title_s_line_break_reads_as_a_space() {
        assert_eq!(text_string(b"Two\rlines\n"), "Two lines");
    }

    #[test]
    fn test_a_latin1_byte_is_its_own_character() {
        assert_eq!(text_string(b"Caf\xe9"), "Café");
    }
}
