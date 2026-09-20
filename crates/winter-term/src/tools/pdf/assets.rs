//! What a PDF surface serves itself: the vendored pdf.js build, inflated out
//! of the payload `build.rs` packs into the binary, and the viewer document
//! Winter drives it with.
//!
//! Nothing here reaches the network or the filesystem. A PDF opens the same
//! way on a machine that has never been online.

use std::io::Read;

use flate2::read::DeflateDecoder;

use crate::model::page::SurfaceAsset;

include!(concat!(env!("OUT_DIR"), "/pdfjs_index.rs"));

// ========================================================================
// Constants
// ========================================================================

/// The deflated pdf.js payload, indexed by `PDFJS_INDEX`.
static PDFJS_BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/pdfjs.blob"));

/// Content type the viewer document is served under.
const HTML_MIME: &str = "text/html";

/// Content type every module is served under. A web engine refuses to run a
/// module script sent as anything else, which is the one way this can fail
/// silently.
const JS_MIME: &str = "text/javascript";

/// Content type the standard-font pack is served under. pdf.js fetches these
/// as bytes and parses them itself, so the type only has to not be a script:
/// a `.pfb` has no registered one of its own to give it.
const FONT_MIME: &str = "application/octet-stream";

/// Path prefix the vendored build is served under, keeping it apart from
/// Winter's own viewer files.
const PDFJS_PREFIX: &str = "pdfjs/";

/// What a vendored path ends in when it is a module rather than a font.
const MODULE_SUFFIX: &str = ".mjs";

/// The viewer document, the module that drives pdf.js inside it, and the
/// cursor-mode module that one imports.
const CARET_MJS: &str = include_str!("caret.mjs");
const VIEWER_HTML: &str = include_str!("viewer.html");
const VIEWER_MJS: &str = include_str!("viewer.mjs");

/// Path the cursor-mode module is served at. Imported by `viewer.mjs`.
const CARET_MJS_PATH: &str = "caret.mjs";

/// Path the viewer's own module is served at. Named by `viewer.html`.
const VIEWER_MJS_PATH: &str = "viewer.mjs";

/// Path the viewer document is served at, and so the surface's entry point.
pub const VIEWER_PATH: &str = "viewer.html";

// ========================================================================
// Free functions
// ========================================================================

/// Resolve one path under a PDF surface's asset root, or `None` for a path no
/// asset answers to.
pub fn asset(path: &str) -> Option<SurfaceAsset> {
    match path {
        VIEWER_PATH => Some(SurfaceAsset {
            bytes: VIEWER_HTML.as_bytes().to_vec(),
            mime: HTML_MIME,
        }),
        VIEWER_MJS_PATH => Some(SurfaceAsset {
            bytes: VIEWER_MJS.as_bytes().to_vec(),
            mime: JS_MIME,
        }),
        CARET_MJS_PATH => Some(SurfaceAsset {
            bytes: CARET_MJS.as_bytes().to_vec(),
            mime: JS_MIME,
        }),
        _ => path.strip_prefix(PDFJS_PREFIX).and_then(vendored),
    }
}

/// One file of the vendored pdf.js build, inflated on demand.
///
/// Each file is deflated on its own, so serving the library does not unpack
/// the worker beside it.
fn vendored(name: &str) -> Option<SurfaceAsset> {
    let position = PDFJS_INDEX
        .binary_search_by(|(packed, _, _, _)| (*packed).cmp(name))
        .ok()?;
    let (_, offset, packed_len, raw_len) = PDFJS_INDEX[position];
    let start = offset as usize;
    let packed = PDFJS_BLOB.get(start..start + packed_len as usize)?;
    let mut bytes = Vec::with_capacity(raw_len as usize);
    DeflateDecoder::new(packed).read_to_end(&mut bytes).ok()?;
    Some(SurfaceAsset {
        bytes,
        mime: vendored_mime(name),
    })
}

/// What a vendored file is served as: the library and its worker are module
/// scripts, and the font pack beside them is bytes.
fn vendored_mime(name: &str) -> &'static str {
    match name.ends_with(MODULE_SUFFIX) {
        true => JS_MIME,
        false => FONT_MIME,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_vendored_library_and_worker_both_inflate() {
        // The whole viewer hangs on these two being packed under the names
        // `viewer.mjs` imports them by, which is a build.rs/asset-path
        // agreement nothing else checks.
        for name in ["pdfjs/pdf.min.mjs", "pdfjs/pdf.worker.min.mjs"] {
            let served = asset(name).unwrap_or_else(|| panic!("{name} is packed"));
            assert_eq!(served.mime, JS_MIME);
            assert!(
                served.bytes.starts_with(b"/*"),
                "{name} must inflate to the pdf.js source, which opens with its \
                 license banner"
            );
        }
    }

    #[test]
    fn test_the_standard_font_pack_is_packed_as_bytes() {
        // A PDF that names a base font without embedding it draws nothing
        // legible unless pdf.js can fetch the font at the path it asks for,
        // which is this directory plus the file's own name.
        for name in [
            "pdfjs/standard_fonts/FoxitSerif.pfb",
            "pdfjs/standard_fonts/LiberationSans-Regular.ttf",
        ] {
            let served = asset(name).unwrap_or_else(|| panic!("{name} is packed"));
            assert_eq!(served.mime, FONT_MIME, "a font is not a script");
            assert!(!served.bytes.is_empty());
        }
        // The licences travel with the repository, not with the binary.
        assert!(asset("pdfjs/standard_fonts/LICENSE_FOXIT").is_none());
    }

    #[test]
    fn test_the_viewer_names_the_module_it_is_served_beside() {
        // `viewer.html` loading a path `asset` does not answer to leaves a
        // blank surface with nothing in the logs but a 404.
        assert!(VIEWER_HTML.contains(VIEWER_MJS_PATH));
        assert!(asset(VIEWER_MJS_PATH).is_some());
        assert!(
            VIEWER_MJS.contains(CARET_MJS_PATH),
            "viewer.mjs imports the cursor-mode module by this path"
        );
        assert!(asset(CARET_MJS_PATH).is_some());
    }

    #[test]
    fn test_an_unknown_path_resolves_to_nothing() {
        assert!(asset("../../etc/passwd").is_none());
        assert!(asset("pdfjs/").is_none());
        assert!(asset("").is_none());
    }
}
