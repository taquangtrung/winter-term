//! The thread a PDF is opened and drawn on: the document is parsed there
//! once and shared with the editor, and each page asked for is drawn there, so
//! drawing never holds up the window.

use std::collections::HashSet;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::thread;

use hayro::hayro_syntax::{LoadPdfError, Pdf};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{render, PixmapSettings, RenderCache, RenderSettings};

use crate::model::picture::Pixels;

use super::document::page_text;
use super::outline::{self, Entry};
use super::text::PageText;

// ========================================================================
// Data Structures
// ========================================================================

/// The thread drawing one document, and the two ways to talk to it.
pub struct Worker {
    replies: mpsc::Receiver<Reply>,
    requests: mpsc::Sender<Request>,
}

/// A page to draw, at a scale, for the layout numbered `generation`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Request {
    pub generation: u64,
    /// Whether to invert the drawn page's colors.
    pub invert: bool,
    pub page: usize,
    /// Pixels a point.
    pub scale: f32,
}

/// What the thread says back.
pub enum Reply {
    /// A page that could not be drawn, so it is not asked for again.
    Failed(usize),
    /// The document, or why it could not be read.
    Opened(Result<Opened, String>),
    /// A page, drawn.
    Page(Drawn),
    /// A page's text, read while nothing was waiting to be drawn.
    Text(usize, PageText),
}

/// A document the thread opened: shared with the editor, which reads its text.
pub struct Opened {
    /// The bookmarks it lists its sections under.
    pub outline: Vec<Entry>,
    pub pdf: Arc<Pdf>,
    /// Each page's size in points.
    pub sizes: Vec<(f32, f32)>,
}

/// A page the thread drew, and the request it answers.
pub struct Drawn {
    pub pixels: Arc<Pixels>,
    pub request: Request,
}

// ========================================================================
// Worker
// ========================================================================

impl Worker {
    /// Open the PDF at `path` on a thread of its own. The thread lives as
    /// long as the worker: dropping it ends the thread after the page it is
    /// drawing.
    pub fn open(path: PathBuf) -> Self {
        let (requests, asked) = mpsc::channel();
        let (answer, replies) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("winter-pdf".to_string())
            .spawn(move || serve(&path, &asked, &answer));
        if spawned.is_err() {
            // No thread: say so through the channel the thread would have used.
            let (answer, failed) = mpsc::channel();
            let _ = answer.send(Reply::Opened(Err("no thread to read it on".to_string())));
            return Self {
                replies: failed,
                requests,
            };
        }
        Self { replies, requests }
    }

    /// Ask for a page.
    pub fn request(&self, request: Request) {
        let _ = self.requests.send(request);
    }

    /// Everything the thread has said since the last call.
    pub fn poll(&self) -> Vec<Reply> {
        self.replies.try_iter().collect()
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// The thread's whole life: open the document, then draw what is asked
/// until the worker is dropped.
fn serve(path: &PathBuf, asked: &mpsc::Receiver<Request>, answer: &mpsc::Sender<Reply>) {
    let pdf = match std::fs::read(path)
        .map_err(|e| e.to_string())
        .and_then(|bytes| Pdf::new(bytes).map_err(load_error))
    {
        Ok(pdf) => Arc::new(pdf),
        Err(why) => {
            let _ = answer.send(Reply::Opened(Err(why)));
            return;
        }
    };
    let sizes = pdf
        .pages()
        .iter()
        .map(|page| page.render_dimensions())
        .collect();
    // A malformed outline costs the bookmarks, not the document.
    let outline = panic::catch_unwind(AssertUnwindSafe(|| outline::read(&pdf))).unwrap_or_default();
    let opened = Opened {
        outline,
        pdf: Arc::clone(&pdf),
        sizes,
    };
    if answer.send(Reply::Opened(Ok(opened))).is_err() {
        return;
    }
    let cache = RenderCache::new();
    let settings = Default::default();
    let pages = pdf.pages().len();
    let mut unread = 0..pages;
    loop {
        // Between drawings, each page's text is read in turn, so a search
        // or a motion seldom has to read one itself: drawing what is on
        // screen comes first, and the next request is looked for after
        // every page read.
        let first = match unread.clone().next() {
            Some(page) => match asked.try_recv() {
                Ok(request) => request,
                Err(mpsc::TryRecvError::Empty) => {
                    unread.next();
                    if answer
                        .send(Reply::Text(page, page_text(&pdf, page)))
                        .is_err()
                    {
                        return;
                    }
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => return,
            },
            None => match asked.recv() {
                Ok(request) => request,
                Err(_) => return,
            },
        };
        // Everything asked for meanwhile, the newest first, and only for the
        // newest layout: pages scrolled past while one was drawing, or asked
        // for at a zoom since left, are not worth drawing.
        let mut batch: Vec<Request> = std::iter::once(first).chain(asked.try_iter()).collect();
        let newest = batch.iter().map(|request| request.generation).max();
        batch.retain(|request| Some(request.generation) == newest);
        let mut seen = HashSet::new();
        for request in batch.into_iter().rev() {
            if !seen.insert(request.page) {
                continue;
            }
            let Some(page) = pdf.pages().get(request.page) else {
                continue;
            };
            let pixmap_settings = PixmapSettings {
                bg_color: WHITE,
                x_scale: request.scale,
                y_scale: request.scale,
            };
            // A malformed page takes down nothing but itself.
            let drawn = panic::catch_unwind(AssertUnwindSafe(|| {
                render(
                    page,
                    &cache,
                    &settings,
                    &RenderSettings::default(),
                    &pixmap_settings,
                )
            }));
            let reply = match drawn {
                Ok(pixmap) => {
                    // Drawn over white, so every pixel is opaque and its
                    // premultiplied bytes are its straight ones.
                    let mut rgba = pixmap.data_as_u8_slice().to_vec();
                    if request.invert {
                        invert(&mut rgba);
                    }
                    Reply::Page(Drawn {
                        pixels: Arc::new(Pixels {
                            height: u32::from(pixmap.height()),
                            rgba,
                            width: u32::from(pixmap.width()),
                        }),
                        request,
                    })
                }
                Err(_) => Reply::Failed(request.page),
            };
            if answer.send(reply).is_err() {
                return;
            }
        }
    }
}

/// Invert the colors of opaque pixels `rgba`, leaving their alpha.
fn invert(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        for channel in &mut pixel[..3] {
            *channel = u8::MAX - *channel;
        }
    }
}

/// What a PDF that would not open says.
fn load_error(error: LoadPdfError) -> String {
    match error {
        LoadPdfError::Decryption(_) => "it is encrypted, and needs a password".to_string(),
        LoadPdfError::Invalid => "it is not a PDF that can be read".to_string(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    /// A one-page PDF, 200 by 100 points, black on its left half. Written
    /// with no cross-reference table: a reader rebuilds one, as hayro does.
    const ONE_PAGE: &str = "%PDF-1.4
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R >> endobj
4 0 obj << /Length 21 >> stream
0 0 100 100 re f
endstream endobj
trailer << /Root 1 0 R >>
%%EOF
";

    fn wait_for(worker: &Worker, mut done: impl FnMut(Reply) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if worker.poll().into_iter().any(&mut done) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("the worker never answered");
    }

    #[test]
    fn test_a_page_is_drawn_at_the_scale_asked_for() {
        let path = std::env::temp_dir().join(format!("winter-pdf-{}.pdf", std::process::id()));
        std::fs::write(&path, ONE_PAGE).expect("a scratch PDF");
        let worker = Worker::open(path.clone());
        wait_for(&worker, |reply| match reply {
            Reply::Opened(opened) => {
                let sizes = opened.map(|opened| opened.sizes);
                assert_eq!(sizes, Ok(vec![(200.0, 100.0)]));
                true
            }
            Reply::Failed(_) | Reply::Page(_) | Reply::Text(..) => false,
        });
        worker.request(Request {
            generation: 1,
            invert: false,
            page: 0,
            scale: 2.0,
        });
        wait_for(&worker, |reply| match reply {
            Reply::Page(drawn) => {
                let pixels = &drawn.pixels;
                assert_eq!((pixels.width, pixels.height), (400, 200));
                let at = |x: usize, y: usize| pixels.rgba[(y * 400 + x) * 4];
                assert_eq!(at(10, 100), 0, "the filled left half is black");
                assert_eq!(at(390, 100), 255, "the rest is the white page");
                true
            }
            Reply::Failed(_) | Reply::Opened(_) | Reply::Text(..) => false,
        });
        worker.request(Request {
            generation: 2,
            invert: true,
            page: 0,
            scale: 2.0,
        });
        wait_for(&worker, |reply| match reply {
            Reply::Page(drawn) => {
                let pixels = &drawn.pixels;
                let at = |x: usize, y: usize| pixels.rgba[(y * 400 + x) * 4..][..4].to_vec();
                assert_eq!(
                    at(10, 100),
                    [255, 255, 255, 255],
                    "inverted, black is white"
                );
                assert_eq!(
                    at(390, 100),
                    [0, 0, 0, 255],
                    "and the page black, still opaque"
                );
                true
            }
            Reply::Failed(_) | Reply::Opened(_) | Reply::Text(..) => false,
        });
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_each_page_s_text_is_read_while_nothing_is_drawing() {
        let path = std::env::temp_dir().join(format!("winter-pdftext-{}.pdf", std::process::id()));
        std::fs::write(&path, ONE_PAGE).expect("a scratch PDF");
        let worker = Worker::open(path.clone());
        // The page draws a box and no text, so what comes back is a page
        // with no lines: read, rather than never sent.
        wait_for(&worker, |reply| match reply {
            Reply::Text(page, text) => {
                assert_eq!(page, 0);
                assert!(text.lines.is_empty());
                true
            }
            Reply::Failed(_) | Reply::Opened(_) | Reply::Page(_) => false,
        });
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_a_file_that_is_no_pdf_says_why() {
        let path = std::env::temp_dir().join(format!("winter-nopdf-{}.pdf", std::process::id()));
        std::fs::write(&path, "plain text").expect("a scratch file");
        let worker = Worker::open(path.clone());
        wait_for(&worker, |reply| match reply {
            Reply::Opened(opened) => {
                assert!(opened.is_err());
                true
            }
            Reply::Failed(_) | Reply::Page(_) | Reply::Text(..) => false,
        });
        std::fs::remove_file(&path).ok();
    }
}
