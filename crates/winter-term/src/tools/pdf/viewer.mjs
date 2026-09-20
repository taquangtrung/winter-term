// Winter's PDF surface: pdf.js drawing a continuous column of pages, driven
// by the keys the host's page binds rather than by any chrome of its own.
//
// Everything here loads over Winter's surface protocol. `./pdfjs/` is the
// vendored pdf.js build and `/doc` is the one file the page asked to open;
// nothing else resolves, so there is no origin to reach out to.

import * as pdfjs from "./pdfjs/pdf.min.mjs";
import * as caret from "./caret.mjs";

// ========================================================================
// Constants
// ========================================================================

/// Where the surface serves the document the page opened.
const DOC_URL = "/doc";

/// The pdf.js worker, off the same surface protocol as the library.
const WORKER_URL = "./pdfjs/pdf.worker.min.mjs";

/// Where the vendored standard-font pack is served. A PDF is allowed to name
/// one of the fourteen base fonts without carrying it, and pdf.js asks for
/// the matching file under this directory when it meets one.
const STANDARD_FONTS_URL = "./pdfjs/standard_fonts/";

/// Pixels one `j`/`k` scrolls. A line of the document rather than a line of
/// the grid: the terminal's rows mean nothing to a rendered page.
const LINE_PX = 48;

/// Pixels one `h`/`l` scrolls sideways, which only moves anything on a page
/// zoomed past the pane's width.
const COLUMN_PX = 64;

/// How much one zoom step multiplies or divides the scale by.
const ZOOM_STEP = 1.25;

/// Scale bounds, so a zoom held down cannot allocate a canvas the engine
/// refuses to draw or shrink a page to nothing.
const MIN_SCALE = 0.1;
const MAX_SCALE = 10;

/// Viewport margin, in screen heights, over which a page is rendered before
/// it scrolls into view and dropped after it leaves. One screen either way
/// keeps scrolling ahead of the renderer without holding the whole document
/// as canvases.
const RENDER_MARGIN_SCREENS = 1;

/// Device pixels per CSS pixel to rasterize at, capped: a HiDPI screen wants
/// more than 1 for sharp glyphs, and a canvas past 2 costs memory for detail
/// no display resolves.
const MAX_PIXEL_RATIO = 2;

// ========================================================================
// State
// ========================================================================

/// Everything the surface knows about what it is showing. Held in one object
/// so the command API and the render loop cannot drift apart.
const state = {
  /// Placeholder elements, one per page, in document order.
  frames: [],
  /// Whether the frames have been given their sizes at least once. Before
  /// that they are all zero-height and stacked at the top, so asking which
  /// page is on screen would answer the last one.
  laidOut: false,
  /// The `PDFDocumentProxy`, once it loads.
  doc: null,
  /// Whether the scale follows the pane's width, until a zoom pins it.
  fitWidth: true,
  /// The page last reported to the host, so an unchanged scroll stays quiet.
  reported: 0,
  /// CSS pixels per PDF unit.
  scale: 1,
  /// Intrinsic size of each page at scale 1, in PDF units.
  sizes: [],
};

// ========================================================================
// The caret's view of the document
// ========================================================================

// Cursor mode needs the text's geometry, which only this module knows: which
// page is drawn in which frame, and what the scale between a page's own units
// and the pixels it is drawn at currently is.
caret.attach({
  frameFor: (page) => state.frames[page - 1],
  getPage: (page) => state.doc.getPage(page),
  pageCount: () => state.frames.length,
  post,
  scale: () => state.scale,
  scrollTo,
  scrollTop,
  viewportHeight,
  visiblePages,
});

/// The pages with any part of them on screen, for the motions that work off
/// what the reader can actually see.
function visiblePages() {
  const top = scrollTop();
  const bottom = top + viewportHeight();
  const pages = [];
  for (let index = 0; index < state.frames.length; index += 1) {
    const frame = state.frames[index];
    if (frame.offsetTop + frame.offsetHeight >= top && frame.offsetTop <= bottom) {
      pages.push(index + 1);
    }
  }
  return pages;
}

// ========================================================================
// Host channel
// ========================================================================

/// Tell the host what the surface is showing. The page paints this in its own
/// header, in the terminal's font, rather than the surface drawing chrome the
/// rest of Winter would not match.
function postState() {
  post({
    page: currentPage(),
    pages: state.frames.length,
    percent: scrolledPercent(),
    scale: state.scale,
  });
}

/// Tell the host something went wrong, for it to show where it shows its own
/// errors.
function postError(error) {
  post({ error: String(error && error.message ? error.message : error) });
}

function post(payload) {
  try {
    window.ipc.postMessage(JSON.stringify(payload));
  } catch (e) {
    // No channel means no host listening, which is only ever the case in a
    // browser opened by hand. Rendering must carry on regardless.
  }
}

// ========================================================================
// Commands the host's page binds keys to
// ========================================================================

// How far a motion moves is decided by the host's page, which owns the vim
// grammar: a count, a prefix, and what they resolve to. Everything here is a
// primitive that takes the already-resolved amount, so the two cannot end up
// disagreeing about what `3j` means.
window.winterPdf = {
  scrollLines(lines) {
    scrollBy(lines * LINE_PX);
  },

  scrollColumns(steps) {
    document.documentElement.scrollLeft += steps * COLUMN_PX;
  },

  scrollScreens(fraction) {
    scrollBy(fraction * viewportHeight());
  },

  goToPage(number) {
    const index = clamp(number, 1, state.frames.length) - 1;
    const frame = state.frames[index];
    if (!frame) return;
    scrollTo(frame.offsetTop - gutter());
  },

  goToLastPage() {
    window.winterPdf.goToPage(state.frames.length);
  },

  goToPageBy(delta) {
    window.winterPdf.goToPage(currentPage() + delta);
  },

  /// Put the page being read at an edge of the viewport without moving which
  /// page that is, the way vim's own `zt`/`zz`/`zb` move the window rather
  /// than the cursor.
  placePage(where) {
    const frame = state.frames[currentPage() - 1];
    if (!frame) return;
    const offsets = { bottom: viewportHeight() - frame.offsetHeight, center: (viewportHeight() - frame.offsetHeight) / 2, top: 0 };
    const offset = offsets[where] ?? 0;
    scrollTo(frame.offsetTop - gutter() - offset);
  },

  zoom(direction) {
    const factor = direction > 0 ? ZOOM_STEP : 1 / ZOOM_STEP;
    setScale(state.scale * factor, false);
  },

  fitWidth() {
    setScale(widthFittingScale(), true);
  },

  /// Say where in the document the reader is, for the host to show. Vim's own
  /// `Ctrl-G` answers with the file, the line, and a percentage; the host
  /// knows the file name, so only the position comes from here.
  reportPosition() {
    post({
      announce: true,
      page: currentPage(),
      pages: state.frames.length,
      percent: scrolledPercent(),
    });
  },

  // Cursor mode. The host's page owns the grammar and hands these already
  // resolved motions, so nothing here parses a key.
  caretEnter: caret.enter,
  caretFind: caret.find,
  caretLeave: caret.leave,
  caretMotion: caret.motion,
  caretPlaceLine: caret.placeLine,
  caretRepeatFind: caret.repeatFind,
  caretSearch: caret.search,
  caretSearchStep: caret.searchStep,
  caretSwapEnds: caret.swapEnds,
  caretVisual: caret.visual,
  caretYankLines: caret.yankLines,
  caretYankSelection: caret.yankSelection,
};

// ========================================================================
// Layout and rendering
// ========================================================================

/// Lay every page out at the current scale, keeping the reader where they
/// were: a zoom or a pane resize must not jump the document back to page one.
function relayout() {
  const anchor = state.laidOut ? currentPage() : 1;
  for (let index = 0; index < state.frames.length; index += 1) {
    const frame = state.frames[index];
    const size = state.sizes[index];
    frame.style.width = `${Math.round(size.width * state.scale)}px`;
    frame.style.height = `${Math.round(size.height * state.scale)}px`;
    discard(frame);
  }
  state.laidOut = true;
  window.winterPdf.goToPage(anchor);
  renderVisible();
  // The text's own model survives a zoom, since it is in the page's units,
  // but what was drawn over the old layout has to be drawn again.
  caret.invalidate();
}

/// Draw the pages within the render margin and drop the ones outside it, so
/// a long document costs a few canvases rather than all of them.
function renderVisible() {
  const margin = viewportHeight() * RENDER_MARGIN_SCREENS;
  const top = scrollTop() - margin;
  const bottom = scrollTop() + viewportHeight() + margin;

  for (const frame of state.frames) {
    const frameTop = frame.offsetTop;
    const frameBottom = frameTop + frame.offsetHeight;
    if (frameBottom < top || frameTop > bottom) {
      discard(frame);
      continue;
    }
    void render(frame);
  }
}

/// Rasterize one page into its frame, unless that is already in hand at the
/// current scale.
async function render(frame) {
  const index = Number(frame.dataset.index);
  const token = `${state.scale}`;
  if (frame.dataset.rendered === token || frame.dataset.rendering === token) {
    return;
  }
  frame.dataset.rendering = token;

  try {
    const page = await state.doc.getPage(index + 1);
    // A zoom or a resize landed while the worker was busy; that relayout has
    // already asked for the size that is now wanted.
    if (frame.dataset.rendering !== token) return;

    const ratio = Math.min(window.devicePixelRatio || 1, MAX_PIXEL_RATIO);
    const viewport = page.getViewport({ scale: state.scale * ratio });
    const canvas = document.createElement("canvas");
    canvas.width = Math.round(viewport.width);
    canvas.height = Math.round(viewport.height);
    await page.render({ canvasContext: canvas.getContext("2d"), viewport }).promise;
    if (frame.dataset.rendering !== token) return;

    frame.replaceChildren(canvas);
    frame.dataset.rendered = token;
  } catch (error) {
    postError(error);
  } finally {
    delete frame.dataset.rendering;
  }
}

/// Give a page's frame back its blank placeholder, releasing the canvas.
function discard(frame) {
  if (!frame.dataset.rendered && !frame.dataset.rendering) return;
  frame.replaceChildren();
  delete frame.dataset.rendered;
  delete frame.dataset.rendering;
}

// ========================================================================
// Geometry helpers
// ========================================================================

/// The page filling most of the viewport, which is the one a reader would say
/// they are on.
function currentPage() {
  const middle = scrollTop() + viewportHeight() / 2;
  for (let index = 0; index < state.frames.length; index += 1) {
    const frame = state.frames[index];
    if (middle < frame.offsetTop + frame.offsetHeight) return index + 1;
  }
  return Math.max(state.frames.length, 1);
}

/// The scale at which the widest page just fits the pane, so no page needs
/// sideways scrolling that the surface deliberately does not offer.
///
/// Measured off the container rather than computed from the viewport and the
/// padding: the container's own width is what a page actually has to fit in,
/// whatever the box model did to get there.
function widthFittingScale() {
  const available = document.getElementById("pages").clientWidth;
  const widest = state.sizes.reduce((max, size) => Math.max(max, size.width), 0);
  if (widest <= 0 || available <= 0) return 1;
  return available / widest;
}

function setScale(scale, fitWidth) {
  state.fitWidth = fitWidth;
  state.scale = clamp(scale, MIN_SCALE, MAX_SCALE);
  relayout();
  postState();
}

function gutter() {
  const value = getComputedStyle(document.documentElement).getPropertyValue("--winter-gutter");
  return Number.parseFloat(value) || 0;
}

function viewportHeight() {
  return document.documentElement.clientHeight;
}

function scrollTop() {
  return document.documentElement.scrollTop;
}

function scrollBy(delta) {
  scrollTo(scrollTop() + delta);
}

function scrollTo(top) {
  document.documentElement.scrollTop = Math.max(top, 0);
}

/// How far through the document the reader is, as whole percent. A document
/// that fits the pane whole is at the end of itself, so it reads 100.
function scrolledPercent() {
  const scroller = document.documentElement;
  const travel = scroller.scrollHeight - scroller.clientHeight;
  if (travel <= 0) return 100;
  return Math.round((scroller.scrollTop / travel) * 100);
}

function clamp(value, low, high) {
  return Math.min(Math.max(value, low), high);
}

// ========================================================================
// Startup
// ========================================================================

/// Paint the surface in the terminal's own colors, which the host hands over
/// before this document loads because only the host knows the theme.
function applyTheme() {
  const theme = window.winterTheme;
  if (!theme) return;
  const root = document.documentElement;
  if (theme.background) root.style.setProperty("--winter-bg", theme.background);
  if (theme.foreground) root.style.setProperty("--winter-fg", theme.foreground);
}

function showMessage(text) {
  document.getElementById("message").textContent = text;
}

async function open() {
  applyTheme();
  pdfjs.GlobalWorkerOptions.workerSrc = new URL(WORKER_URL, import.meta.url).href;

  const doc = await pdfjs.getDocument({
    standardFontDataUrl: new URL(STANDARD_FONTS_URL, import.meta.url).href,
    url: DOC_URL,
    // The vendored pack rather than whatever the machine happens to have
    // installed, so a document draws the same shapes on every machine, and
    // the same ones on a machine with no fonts of its own at all.
    useSystemFonts: false,
  }).promise;
  state.doc = doc;

  // Every page's intrinsic size up front: a placeholder has to be the right
  // height before it is rendered, or scrolling would reflow under the reader
  // as pages arrive.
  const container = document.getElementById("pages");
  const frames = [];
  const sizes = [];
  for (let number = 1; number <= doc.numPages; number += 1) {
    const page = await doc.getPage(number);
    const viewport = page.getViewport({ scale: 1 });
    sizes.push({ height: viewport.height, width: viewport.width });

    const frame = document.createElement("div");
    frame.className = "page";
    frame.dataset.index = String(number - 1);
    frames.push(frame);
    container.append(frame);
  }
  state.frames = frames;
  state.sizes = sizes;

  setScale(widthFittingScale(), true);
  showMessage("");
}

document.addEventListener("scroll", () => {
  renderVisible();
  caret.reposition();
  const page = currentPage();
  if (page === state.reported) return;
  state.reported = page;
  postState();
});

window.addEventListener("resize", () => {
  if (state.fitWidth) {
    setScale(widthFittingScale(), true);
    return;
  }
  relayout();
});

open().catch((error) => {
  postError(error);
  showMessage(`Cannot open this PDF: ${error && error.message ? error.message : error}`);
});
