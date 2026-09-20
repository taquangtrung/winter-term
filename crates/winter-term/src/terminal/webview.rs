//! WebView tile manager: creates and positions child WebViews for rich blocks.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use winit::window::Window;
use winter_core::winter_proto::{EmitBlock, TrustTier};
use wry::dpi::{PhysicalPosition, PhysicalSize};
use wry::http::header::CONTENT_TYPE;
use wry::http::{Response, StatusCode};
use wry::{Rect, WebView, WebViewBuilder};

use crate::model::input::{Key, KeyCode};
use crate::model::layout::PaneId;
use crate::model::page::{PageSurface, SurfaceAsset};

use super::block_queue::BlockEntry;

// ========================================================================
// Constants
// ========================================================================

const BLOCK_HTML_SHELL: &str = include_str!("block_shell.html");

/// Marks the tile as closed so the shell's CSS dims it and shows a badge
/// (`terminal/block_shell.html`'s `.winter-closed` rule).
const CLOSED_TOGGLE_JS: &str = "document.body.classList.add('winter-closed');";

const CSP_ISOLATED: &str = "default-src 'none'; style-src 'unsafe-inline'; img-src data:;";
const CSP_RESTRICTED: &str =
    "default-src 'none'; style-src 'unsafe-inline'; img-src data:; script-src 'none';";

/// CDN bundles the Vega/Vega-Lite renderer needs. Only ever injected when the
/// user has opted in via `security.block-remote-assets`: rendering a block must
/// not, by default, make a network request the user did not ask for.
const VEGA_CDN_SCRIPTS: &[&str] = &[
    "https://cdn.jsdelivr.net/npm/vega@5",
    "https://cdn.jsdelivr.net/npm/vega-lite@5",
    "https://cdn.jsdelivr.net/npm/vega-embed@6",
];

/// Scheme the page-surface protocol answers on. Everything a surface loads
/// comes through it and nothing else resolves, so a document opened in one
/// reaches neither the network nor the rest of the filesystem.
const SURFACE_SCHEME: &str = "winter";

/// Host segment every surface URL carries. Fixed so the path means the same
/// thing on the platforms that map a custom scheme onto `http://`, where the
/// scheme and the first segment both land in the host.
const SURFACE_HOST: &str = "surface";

/// Path a surface's own document is served at: the one file outside the
/// surface's asset root it is allowed to read.
const SURFACE_DOC_PATH: &str = "doc";

/// Content type a surface's document is served under.
const SURFACE_DOC_MIME: &str = "application/pdf";

/// Hands every key the surface sees back to Winter, and stops the engine
/// acting on it.
///
/// A child WebView owns the keyboard for the whole window while it is up:
/// with one on screen the windowing layer never sees a key press at all, so
/// this is not a convenience but the only route a key has home. Winter then
/// resolves it exactly as a key typed anywhere else, which is what keeps `q`,
/// the pane chords, and the tab chords working from inside a document.
///
/// A bare modifier is left alone: it fires a keydown of its own, and relaying
/// it would send a stray press on every chord typed.
const SURFACE_KEY_RELAY_JS: &str = "\
window.addEventListener('keydown', function (event) {\
  if (['Shift','Control','Alt','Meta'].indexOf(event.key) !== -1) { return; }\
  event.preventDefault();\
  try {\
    window.ipc.postMessage(JSON.stringify({winterKey: {\
      alt: event.altKey, ctrl: event.ctrlKey, name: event.key, shift: event.shiftKey\
    }}));\
  } catch (e) {}\
}, true);";

/// Id of the element `terminal/block_shell.html` wraps a block's content in,
/// and the single owner of that name on this side. The host slides this
/// element to clip the tile, and measures it rather than the document: a
/// transform does not disturb the height its own box reports.
const CONTENT_ID: &str = "winter-content";

const MIME_RICHNESS: &[&str] = &[
    "application/vnd.vega-lite+json",
    "application/vnd.vega+json",
    "text/html",
    "image/svg+xml",
    "text/markdown",
    "text/csv",
    "image/png",
    "image/jpeg",
    "image/gif",
    "application/json",
    "text/plain",
];

/// Minimum time between two applied WebView content updates for the same
/// tile: a patch arriving sooner is held and applied once this elapses,
/// capping the update rate a fast-streaming tool can force (~10/s).
const PATCH_MIN_INTERVAL: Duration = Duration::from_millis(100);

// ========================================================================
// Data Structures
// ========================================================================

/// Where one pane's grid sits this frame: the absolute row at the top of its
/// viewport, and the pixel rect its rows are painted into.
///
/// A tile is placed against its own pane's entry, never the focused pane's.
/// Panes in a split scroll independently and start at different pixel origins,
/// so a tile positioned from the focused pane's numbers floats over unrelated
/// output in every pane but that one.
#[derive(PartialEq)]
pub struct PaneViewport {
    /// Pixel height of the pane's grid area.
    pub height: f32,
    /// Absolute row currently at the top of the pane's viewport (see
    /// [`winter_render::Grid::to_absolute_row`]).
    pub top_row: usize,
    /// Pixel width of the pane's grid area.
    pub width: f32,
    /// Pixel offset of the pane's left edge from the window's left edge.
    pub x: f32,
    /// Pixel offset of the pane's top edge from the window's top edge.
    pub y: f32,
}

/// Everything a page surface needs in order to be created.
pub struct SurfaceParams {
    /// Script run in the surface before its document loads, which is how the
    /// host hands it what only the host knows: the theme it has to match.
    pub init_script: String,
    /// Where the surface sits in the window, in physical pixels.
    pub rect: TilePlacement,
    /// What the surface serves and loads.
    pub surface: PageSurface,
}

/// Something a page's surface posted back out of its WebView.
pub enum SurfaceMessage {
    /// A key the surface saw. The WebView holds the window's keyboard while
    /// it is up, so this is how a key typed over a document reaches Winter.
    Key(SurfaceKey),
    /// What the surface said, for its own page to read.
    Text(SurfaceText),
}

/// The shape [`SURFACE_KEY_RELAY_JS`] posts a key in.
#[derive(Deserialize)]
struct RelayedKey {
    #[serde(rename = "winterKey")]
    winter_key: RelayedKeyBody,
}

/// The key itself, as the web engine named it.
#[derive(Deserialize)]
struct RelayedKeyBody {
    alt: bool,
    ctrl: bool,
    name: String,
    shift: bool,
}

/// A key a surface relayed, already in Winter's own terms.
pub struct SurfaceKey {
    /// The key, in the same terms the windowing layer would have given it.
    pub key: Key,
    /// The pane whose page owns the surface.
    pub pane_id: PaneId,
}

/// What a surface said, for the page that owns it.
pub struct SurfaceText {
    /// What the surface said, verbatim.
    pub body: String,
    /// The pane whose page owns the surface.
    pub pane_id: PaneId,
}

/// Everything a tile needs in order to be created and positioned.
pub struct TileParams {
    /// Grid row the tile is anchored to.
    pub abs_row: usize,
    /// Tile height in physical pixels.
    pub height: u32,
    /// The document to load into the tile.
    pub html: String,
    /// Tile width in physical pixels.
    pub width: u32,
    /// Tile position from the window's left edge, in physical pixels.
    pub x: i32,
    /// Tile position from the window's top edge, in physical pixels.
    pub y: i32,
}

/// A tile's rendered content height, reported by its own JS.
pub struct HeightReport {
    /// Index of the command block this tile renders.
    pub block_index: usize,
    /// The height the content reported after layout.
    pub height_px: f32,
    /// The pane the tile belongs to.
    pub pane_id: PaneId,
    /// Index of the segment within the command block.
    pub segment_index: usize,
}

/// Manages child WebViews that render rich content blocks inline in the
/// terminal. Each content block gets its own WebView positioned at the
/// block's pixel coordinates within the parent window.
pub struct WebViewManager {
    report_rx: mpsc::Receiver<HeightReport>,
    report_tx: mpsc::Sender<HeightReport>,
    surface_rx: mpsc::Receiver<SurfaceMessage>,
    surface_tx: mpsc::Sender<SurfaceMessage>,
    /// One surface per pane: a pane shows at most one page at a time.
    surfaces: HashMap<PaneId, SurfaceSlot>,
    tiles: HashMap<TileKey, TileSlot>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct TileKey {
    pane_id: PaneId,
    block_index: usize,
    segment_index: usize,
}

/// A patch received too soon after the last applied one, held for the next
/// due tick instead of being dropped.
struct PendingUpdate {
    closed: bool,
    html: String,
}

/// A page's own WebView, covering the pane its page is open in.
///
/// Unlike a block tile it is anchored to nothing in the scrollback: it fills
/// the rect its page leaves for it and lives exactly as long as the page
/// does.
struct SurfaceSlot {
    view: PlacedWebView,
}

struct TileSlot {
    abs_row: usize,
    /// Whether the block this tile renders is folded, and so hidden however
    /// its band lies against the pane.
    folded: bool,
    /// When the WebView's content was last actually updated, for the patch
    /// rate limit; `None` before the first update.
    last_applied: Option<Instant>,
    pane_id: PaneId,
    /// A patch rate-limited past the last update, applied on the next due
    /// frame instead of being lost.
    pending: Option<PendingUpdate>,
    /// Grid rows currently reserved for this tile's band; grows as its
    /// content grows.
    reserved_rows: usize,
    trust: TrustTier,
    view: PlacedWebView,
}

/// A tile's placement this frame: where its surface sits in the window, how
/// much of it the pane can show, and how far its content is scrolled to keep
/// the visible part aligned with the grid rows it belongs to.
struct TileClip {
    /// Drawn height in pixels, always the part inside the pane.
    height: f32,
    /// Pixels of content the pane's top edge hides, which the tile scrolls
    /// past so what remains lines up with its band.
    scroll_top: f32,
    /// Top edge in pixels from the window's top.
    y: f32,
}

/// A WebView the host positions, together with the geometry it was last
/// given. Shared by block tiles and page surfaces: both are child views the
/// host moves every frame, and both want the same round-trips skipped.
struct PlacedWebView {
    /// The geometry the view was last given, or `None` while it is hidden.
    placement: Option<TilePlacement>,
    webview: WebView,
}

/// The geometry a child WebView was last given. Held so an unchanged frame
/// skips setting it again: every placement call is a round-trip to the
/// platform WebView, and scrolling the grid moves every tile on screen.
#[derive(Clone, Copy, PartialEq)]
pub struct TilePlacement {
    /// Height in physical pixels.
    pub height: u32,
    /// Pixels of content hidden above the view's top edge, which it slides
    /// past. Always zero for a page surface, which is never clipped.
    pub scroll_top: i32,
    /// Width in physical pixels.
    pub width: u32,
    /// Offset of the left edge from the window's left edge.
    pub x: i32,
    /// Offset of the top edge from the window's top edge.
    pub y: i32,
}

// ========================================================================
// WebViewManager
// ========================================================================

impl WebViewManager {
    /// A manager owning no tiles yet.
    pub fn new() -> Self {
        let (report_tx, report_rx) = mpsc::channel();
        let (surface_tx, surface_rx) = mpsc::channel();
        Self {
            report_rx,
            report_tx,
            surface_rx,
            surface_tx,
            surfaces: HashMap::new(),
            tiles: HashMap::new(),
        }
    }

    /// Create a tile for a block and place it over the grid.
    pub fn create_block_tile(
        &mut self,
        pane_id: PaneId,
        entry: &BlockEntry,
        params: TileParams,
        window: &Window,
    ) -> Result<(), wry::Error> {
        let key = TileKey {
            pane_id,
            block_index: entry.block_index,
            segment_index: entry.segment_index,
        };

        if self.tiles.contains_key(&key) {
            return Ok(());
        }

        let mut html = sandboxed_html(&params.html, entry.trust);
        if entry.closed {
            html.push_str(&format!("<script>{CLOSED_TOGGLE_JS}</script>"));
        }
        let report_tx = self.report_tx.clone();
        let block_index = entry.block_index;
        let segment_index = entry.segment_index;

        let mut builder = WebViewBuilder::new()
            .with_html(&html)
            .with_bounds(placement_rect(&TilePlacement {
                height: params.height,
                scroll_top: 0,
                width: params.width,
                x: params.x,
                y: params.y,
            }))
            // Placed by the first `reposition_tiles`, which is the only
            // thing that knows where the pane is and how its grid is
            // scrolled. Showing it here would flash it at the wrong offset.
            .with_visible(false)
            .with_transparent(true)
            .with_navigation_handler(|_url| false)
            .with_ipc_handler(move |req| {
                if let Ok(height_px) = req.body().trim().parse::<f32>() {
                    let _ = report_tx.send(HeightReport {
                        block_index,
                        height_px,
                        pane_id,
                        segment_index,
                    });
                }
            });

        match entry.trust {
            TrustTier::Trusted => {}
            TrustTier::Restricted | TrustTier::Isolated => {
                builder = builder.with_javascript_disabled();
            }
        }

        let webview = builder.build_as_child(window)?;

        self.tiles.insert(
            key,
            TileSlot {
                abs_row: params.abs_row,
                folded: false,
                last_applied: Some(Instant::now()),
                pane_id,
                pending: None,
                // The band the grid actually reserved for this block, which a
                // tool can ask to be larger than the default. Sizing the tile
                // from the default instead leaves a block that asked for room
                // drawing into part of it.
                reserved_rows: entry.reserved_rows,
                trust: entry.trust,
                view: PlacedWebView::new(webview),
            },
        );
        Ok(())
    }

    /// Place every tile against its own pane's viewport, given where each
    /// pane's grid sits this frame.
    ///
    /// A tile whose pane is absent from `viewports` is hidden. That covers a
    /// pane in a background tab, a pane an alternate-screen app owns, and a
    /// pane a tool page covers: none of them paints the grid the tile is
    /// anchored to, and a block left over it hides what does.
    ///
    /// A tile straddling a pane edge is clipped to the pane rather than
    /// hidden, so a block slides under the boundary the way the rows around it
    /// do instead of vanishing the moment its first line does.
    pub fn reposition_tiles(
        &mut self,
        viewports: &HashMap<PaneId, PaneViewport>,
        cell_height: f32,
    ) {
        for (key, slot) in self.tiles.iter_mut() {
            let Some(viewport) = viewports.get(&key.pane_id) else {
                slot.hide();
                continue;
            };
            if slot.folded {
                slot.hide();
                continue;
            }
            let band_top = (slot.abs_row as isize - viewport.top_row as isize) as f32 * cell_height;
            let band_height = slot.reserved_rows as f32 * cell_height;
            match clip_tile_band(band_top, band_height, viewport.y, viewport.height) {
                Some(clip) => slot.place(viewport, &clip),
                None => slot.hide(),
            }
        }
    }

    /// Move tiles of `pane_id` anchored at or below `row` by `delta` rows: a
    /// band above them grew or shrank, so their anchors must follow the content
    /// they point at. The next `reposition_tiles` moves the views.
    pub fn shift_tiles_at_or_below(&mut self, pane_id: PaneId, row: usize, delta: isize) {
        for slot in self.tiles.values_mut() {
            if slot.pane_id == pane_id && slot.abs_row >= row {
                slot.abs_row = slot.abs_row.saturating_add_signed(delta);
            }
        }
    }

    /// Push every tile of `pane_id` through a resize reflow's row remap (see
    /// [`winter_render::RowRemap`]), the same as the app's image blocks: the
    /// reflow moved the rows a tile is anchored to, and a tile left at the
    /// old row floats over unrelated output.
    pub fn remap_tiles(&mut self, pane_id: PaneId, remap: &winter_render::RowRemap) {
        for slot in self.tiles.values_mut() {
            if slot.pane_id == pane_id {
                slot.abs_row = remap.map(slot.abs_row);
            }
        }
    }

    /// Update the HTML content of an existing tile (for live-block patches).
    /// A patch arriving too soon after the last applied one is held rather
    /// than firing immediately; a close always fires right away.
    pub fn update_tile_html(
        &mut self,
        pane_id: PaneId,
        entry: &BlockEntry,
        html: &str,
    ) -> Result<(), wry::Error> {
        let key = TileKey {
            pane_id,
            block_index: entry.block_index,
            segment_index: entry.segment_index,
        };
        let Some(slot) = self.tiles.get_mut(&key) else {
            return Ok(());
        };
        if is_update_due(slot.last_applied, entry.closed) {
            slot.pending = None;
            self.apply_tile_update(key, html, entry.closed);
        } else {
            slot.pending = Some(PendingUpdate {
                closed: entry.closed,
                html: html.to_string(),
            });
        }
        Ok(())
    }

    /// Apply every tile's pending update, if any, whose rate-limit window
    /// has elapsed since it was held back. Called once per frame.
    pub fn flush_due_tile_updates(&mut self) {
        let due: Vec<TileKey> = self
            .tiles
            .iter()
            // A pending update is never a close (`update_tile_html` applies
            // those immediately instead of queuing them), so `false` here
            // always matches what's actually held.
            .filter(|(_, slot)| slot.pending.is_some() && is_update_due(slot.last_applied, false))
            .map(|(key, _)| *key)
            .collect();
        for key in due {
            let Some(slot) = self.tiles.get_mut(&key) else {
                continue;
            };
            let Some(pending) = slot.pending.take() else {
                continue;
            };
            self.apply_tile_update(key, &pending.html, pending.closed);
        }
    }

    /// Sandbox `html`, replace the tile's DOM, and mark it applied now.
    fn apply_tile_update(&mut self, key: TileKey, html: &str, closed: bool) {
        let Some(slot) = self.tiles.get_mut(&key) else {
            return;
        };
        let sandboxed = sandboxed_html(html, slot.trust);
        let closed_js = if closed { CLOSED_TOGGLE_JS } else { "" };
        // A patch re-runs the shell, which drops the inline offset the last
        // placement set, so the clip is re-applied with the content.
        let clip = clip_js(slot.view.placement.map_or(0, |placed| placed.scroll_top));
        let height_js = height_report_js();
        let js = format!(
            "document.documentElement.innerHTML = {};{clip}{height_js}{closed_js}",
            serde_json::to_string(&sandboxed).unwrap_or_default()
        );
        let _ = slot.view.webview.evaluate_script(&js);
        slot.last_applied = Some(Instant::now());
    }

    /// Every tile content-height report queued since the last drain.
    pub fn drain_height_reports(&mut self) -> Vec<HeightReport> {
        self.report_rx.try_iter().collect()
    }

    /// A tile's current `(abs_row, reserved_rows)`, for computing how much
    /// further a height report can grow it before touching the grid.
    /// `None` when no matching tile exists.
    pub fn tile_band(
        &self,
        pane_id: PaneId,
        block_index: usize,
        segment_index: usize,
    ) -> Option<(usize, usize)> {
        let key = TileKey {
            pane_id,
            block_index,
            segment_index,
        };
        self.tiles
            .get(&key)
            .map(|slot| (slot.abs_row, slot.reserved_rows))
    }

    /// Every tile band of `pane_id`, as `(abs_row, reserved_rows)` pairs in
    /// anchor order: the same viewport rows a nav cursor treats as one stop,
    /// and the span the block-as-cursor outline is drawn around.
    pub fn tile_bands(&self, pane_id: PaneId) -> Vec<(usize, usize)> {
        let mut bands: Vec<(usize, usize)> = self
            .tiles
            .values()
            .filter(|slot| slot.pane_id == pane_id)
            .map(|slot| (slot.abs_row, slot.reserved_rows))
            .collect();
        bands.sort_unstable_by_key(|&(abs_row, _)| abs_row);
        bands
    }

    /// Record that a tile's band is now `reserved_rows` grid rows tall.
    /// Returns `false` when no matching tile exists (e.g. it closed after the
    /// report was queued).
    ///
    /// The surface itself is resized by the next [`Self::reposition_tiles`],
    /// which is the only place that knows how much of the band the pane can
    /// actually show.
    pub fn resize_tile(
        &mut self,
        pane_id: PaneId,
        block_index: usize,
        segment_index: usize,
        reserved_rows: usize,
    ) -> bool {
        let key = TileKey {
            pane_id,
            block_index,
            segment_index,
        };
        let Some(slot) = self.tiles.get_mut(&key) else {
            return false;
        };
        slot.reserved_rows = reserved_rows;
        true
    }

    /// Remove all WebView tiles belonging to a closed pane.
    pub fn remove_tiles_for_pane(&mut self, pane_id: PaneId) {
        self.tiles.retain(|key, _| key.pane_id != pane_id);
    }

    /// Remove the one tile rendering the named block segment, e.g. because
    /// the scrollback's retention budget elided the block's content. A no-op
    /// when the segment rendered natively (no tile exists for it).
    pub fn remove_tile(&mut self, pane_id: PaneId, block_index: usize, segment_index: usize) {
        self.tiles.remove(&TileKey {
            pane_id,
            block_index,
            segment_index,
        });
    }

    /// Remove tiles of `pane_id` whose band overlaps the erased absolute row
    /// span `[start, end)`: the grid rows they were placed over have been
    /// blanked, so the tile is left floating over unrelated output.
    pub fn remove_tiles_in(&mut self, pane_id: PaneId, (start, end): (usize, usize)) {
        self.tiles.retain(|key, slot| {
            key.pane_id != pane_id
                || slot.abs_row >= end
                || slot.abs_row + slot.reserved_rows <= start
        });
    }

    /// Hide every tile, e.g. while a full-window overlay (the settings page) is
    /// up. Tiles are re-shown by the next `reposition_tiles` after it closes.
    pub fn hide_all(&mut self) {
        for slot in self.tiles.values_mut() {
            slot.hide();
        }
    }

    /// Hide all WebView tiles for a folded block. The next
    /// [`Self::reposition_tiles`] keeps them hidden until the block unfolds.
    pub fn fold_block(&mut self, pane_id: PaneId, block_index: usize) {
        self.set_block_folded(pane_id, block_index, true);
    }

    /// Show all WebView tiles for an unfolded block, wherever the next
    /// [`Self::reposition_tiles`] finds their bands lying against the pane.
    pub fn unfold_block(&mut self, pane_id: PaneId, block_index: usize) {
        self.set_block_folded(pane_id, block_index, false);
    }

    /// Mark every tile of one block folded or unfolded.
    fn set_block_folded(&mut self, pane_id: PaneId, block_index: usize, folded: bool) {
        for (key, slot) in self.tiles.iter_mut() {
            if key.pane_id == pane_id && key.block_index == block_index {
                slot.folded = folded;
                if folded {
                    slot.hide();
                }
            }
        }
    }

    /// Forward a key event to the focused block's WebView by dispatching a
    /// synthetic KeyboardEvent via JavaScript. Returns true if a tile existed
    /// for the focused pane.
    pub fn forward_key_event(&mut self, pane_id: PaneId, bytes: &[u8]) -> bool {
        let key = String::from_utf8_lossy(bytes);
        let js = format!(
            "if(document.activeElement)document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{{key:{},bubbles:true}}));",
            serde_json::to_string(&key).unwrap_or_default()
        );
        let mut dispatched = false;
        for slot in self.tiles.values_mut() {
            if slot.pane_id == pane_id {
                let _ = slot.view.webview.evaluate_script(&js);
                dispatched = true;
            }
        }
        dispatched
    }

    /// How many tiles are currently alive.
    #[cfg(test)]
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }
}

// ========================================================================
// WebViewManager: page surfaces
// ========================================================================

impl WebViewManager {
    /// Give `pane_id`'s page a WebView of its own at `params.rect`, unless it
    /// already has one.
    ///
    /// The view resolves nothing but the page's own assets and the one
    /// document it named, both over [`SURFACE_SCHEME`], and it refuses every
    /// navigation: a document opened in it can reach neither the network nor
    /// anything else on disk.
    pub fn create_surface(
        &mut self,
        pane_id: PaneId,
        params: SurfaceParams,
        window: &Window,
    ) -> Result<(), wry::Error> {
        if self.surfaces.contains_key(&pane_id) {
            return Ok(());
        }
        let SurfaceParams {
            init_script,
            rect,
            surface,
        } = params;
        let PageSurface {
            assets,
            document,
            entry,
            top_row: _,
        } = surface;

        let surface_tx = self.surface_tx.clone();
        let webview = WebViewBuilder::new()
            .with_url(surface_url(&entry))
            .with_initialization_script(SURFACE_KEY_RELAY_JS)
            .with_initialization_script(&init_script)
            .with_bounds(placement_rect(&rect))
            // Placed and shown by the first `place_surface`, for the same
            // reason a tile is: the pane's geometry is known there, not here.
            .with_visible(false)
            // The surface's own origin and nothing else. Refusing every
            // navigation outright would refuse the first one, which is the
            // load of the viewer itself.
            .with_navigation_handler(|url| url.starts_with(&surface_origin()))
            .with_custom_protocol(SURFACE_SCHEME.to_string(), move |_id, request| {
                serve_surface(request.uri().path(), assets, &document)
            })
            .with_ipc_handler(move |req| {
                let _ = surface_tx.send(read_surface_message(req.body(), pane_id));
            })
            .build_as_child(window)?;

        self.surfaces.insert(
            pane_id,
            SurfaceSlot {
                view: PlacedWebView::new(webview),
            },
        );
        Ok(())
    }

    /// Move `pane_id`'s surface to `rect`, showing it if it was hidden. A
    /// no-op for a pane with no surface.
    pub fn place_surface(&mut self, pane_id: PaneId, rect: &TilePlacement) {
        if let Some(slot) = self.surfaces.get_mut(&pane_id) {
            slot.view.place(rect);
        }
    }

    /// Hide every surface but the ones belonging to `showing`, e.g. because
    /// their tab went to the background or their page closed.
    pub fn hide_surfaces_except(&mut self, showing: &[PaneId]) {
        for (pane_id, slot) in self.surfaces.iter_mut() {
            if !showing.contains(pane_id) {
                slot.view.hide();
            }
        }
    }

    /// Run `js` inside `pane_id`'s surface, which is how a key its page bound
    /// reaches the document the engine is drawing.
    pub fn run_surface_script(&self, pane_id: PaneId, js: &str) {
        if let Some(slot) = self.surfaces.get(&pane_id) {
            slot.view.run(js);
        }
    }

    /// Take everything the surfaces have posted since the last call.
    pub fn drain_surface_messages(&mut self) -> Vec<SurfaceMessage> {
        self.surface_rx.try_iter().collect()
    }

    /// Drop `pane_id`'s surface, because its page closed or its pane did.
    pub fn remove_surface(&mut self, pane_id: PaneId) {
        self.surfaces.remove(&pane_id);
    }

    /// Whether `pane_id` already has a surface.
    pub fn has_surface(&self, pane_id: PaneId) -> bool {
        self.surfaces.contains_key(&pane_id)
    }
}

// ========================================================================
// TileSlot
// ========================================================================

impl TileSlot {
    /// Move the tile to `clip` within `viewport`, showing it if it was hidden.
    fn place(&mut self, viewport: &PaneViewport, clip: &TileClip) {
        let want = TilePlacement {
            height: clip.height.max(0.0) as u32,
            scroll_top: clip.scroll_top as i32,
            width: viewport.width.max(0.0) as u32,
            x: viewport.x as i32,
            y: clip.y as i32,
        };
        self.view.place(&want);
    }

    /// Take the tile off screen, if it is on it.
    fn hide(&mut self) {
        self.view.hide();
    }
}

// ========================================================================
// PlacedWebView
// ========================================================================

impl PlacedWebView {
    /// A view the host has not placed yet, and so has not shown.
    fn new(webview: WebView) -> Self {
        Self {
            placement: None,
            webview,
        }
    }

    /// Move the view to `want`, showing it if it was hidden.
    ///
    /// Each platform call is skipped when its value has not changed since the
    /// last placement: scrolling the grid replaces every tile on screen, and
    /// on Linux each call is a GTK round-trip.
    fn place(&mut self, want: &TilePlacement) {
        let had = self.placement;
        if had == Some(*want) {
            return;
        }
        if had.is_none_or(|had| {
            (had.height, had.width, had.x, had.y) != (want.height, want.width, want.x, want.y)
        }) {
            let _ = self.webview.set_bounds(placement_rect(want));
        }
        if had.is_none_or(|had| had.scroll_top != want.scroll_top) {
            let _ = self.webview.evaluate_script(&clip_js(want.scroll_top));
        }
        if had.is_none() {
            let _ = self.webview.set_visible(true);
        }
        self.placement = Some(*want);
    }

    /// Take the view off screen, if it is on it.
    fn hide(&mut self) {
        if self.placement.is_some() {
            let _ = self.webview.set_visible(false);
            self.placement = None;
        }
    }

    /// Run `js` inside the view, ignoring whether the engine took it: nothing
    /// the host does depends on the result.
    fn run(&self, js: &str) {
        let _ = self.webview.evaluate_script(js);
    }
}

// ========================================================================
// Tile geometry
// ========================================================================

/// Read what a surface posted: a key it relayed, or something for its page.
///
/// Anything that does not parse as a relayed key is the page's, malformed
/// JSON included: a page reads its own surface's output and is the only thing
/// that can say whether it makes sense.
fn read_surface_message(body: &str, pane_id: PaneId) -> SurfaceMessage {
    let Ok(relayed) = serde_json::from_str::<RelayedKey>(body) else {
        return SurfaceMessage::Text(SurfaceText {
            body: body.to_string(),
            pane_id,
        });
    };
    // A key Winter has no code for is dropped rather than handed on as text,
    // which the page would try to read as its own output.
    let code = KeyCode::from_web_name(&relayed.winter_key.name);
    match code {
        Some(code) => SurfaceMessage::Key(SurfaceKey {
            key: Key {
                alt: relayed.winter_key.alt,
                code,
                ctrl: relayed.winter_key.ctrl,
                shift: relayed.winter_key.shift,
            },
            pane_id,
        }),
        None => SurfaceMessage::Text(SurfaceText {
            body: String::new(),
            pane_id,
        }),
    }
}

/// Serve one request out of a page surface, which can only ever be for the
/// page's own assets or for the single document it named.
///
/// `path` is the request's path with its leading slash, the same shape on
/// every platform because every surface URL carries [`SURFACE_HOST`].
/// Anything else 404s rather than falling through to the filesystem.
fn serve_surface(
    path: &str,
    assets: fn(&str) -> Option<SurfaceAsset>,
    document: &Path,
) -> Response<Cow<'static, [u8]>> {
    let path = path.trim_start_matches('/');
    if path == SURFACE_DOC_PATH {
        return match fs::read(document) {
            Ok(bytes) => asset_response(SURFACE_DOC_MIME, bytes),
            Err(_) => not_found(),
        };
    }
    match assets(path) {
        Some(asset) => asset_response(asset.mime, asset.bytes),
        None => not_found(),
    }
}

/// A `200` carrying `bytes` under `mime`.
fn asset_response(mime: &str, bytes: Vec<u8>) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .header(CONTENT_TYPE, mime)
        .body(Cow::Owned(bytes))
        .expect("a content-type header and an owned body always build")
}

/// An empty `404`, for a path no surface asset answers to.
fn not_found() -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Cow::Borrowed(&[][..]))
        .expect("a status and an empty body always build")
}

/// The origin every surface URL sits under, and so the only prefix a
/// navigation inside a surface is allowed to have.
fn surface_origin() -> String {
    #[cfg(any(windows, target_os = "android"))]
    {
        format!("http://{SURFACE_SCHEME}.{SURFACE_HOST}/")
    }
    #[cfg(not(any(windows, target_os = "android")))]
    {
        format!("{SURFACE_SCHEME}://{SURFACE_HOST}/")
    }
}

/// The URL a surface asset at `path` is reachable at.
///
/// Windows and Android map a custom scheme onto `http://<scheme>.<host>`,
/// while everywhere else it stays `<scheme>://<host>`; see
/// `WebViewBuilder::with_custom_protocol`. The host segment is the same either
/// way, so the path a request arrives with does not vary by platform.
fn surface_url(path: &str) -> String {
    format!("{}{path}", surface_origin())
}

/// The wry rect a placement names.
///
/// Physical, not logical. Everything upstream of here, from the grid's cell
/// size to a pane's rect, is in the window's physical pixels, and handing
/// those to wry as logical ones scales every tile and surface by the
/// display's scale factor: correct at 1.0, and wrong by exactly that factor
/// on any display that is not.
fn placement_rect(placement: &TilePlacement) -> Rect {
    Rect {
        position: PhysicalPosition::new(placement.x, placement.y).into(),
        size: PhysicalSize::new(placement.width, placement.height).into(),
    }
}

/// Slide a clipped tile's content up past the `offset` pixels its pane's top
/// edge hides, so the part still on screen lines up with the grid rows it
/// belongs to.
///
/// A transform rather than a scroll: the shell's body does not scroll, and a
/// transform changes nothing about the layout the content reports back.
fn clip_js(offset: i32) -> String {
    format!("try{{document.getElementById('{CONTENT_ID}').style.transform='translateY(-{offset}px)';}}catch(e){{}}")
}

/// Re-measure content height after a patch: a `<script>` tag assigned via
/// `innerHTML` never runs, so a patch must post its own measurement rather
/// than relying on the shell's initial-load one.
fn height_report_js() -> String {
    format!("try{{window.ipc.postMessage(String(document.getElementById('{CONTENT_ID}').scrollHeight));}}catch(e){{}}")
}

/// Clip a tile's `band_height`-tall band to a pane `pane_height` pixels tall
/// whose own top edge is `pane_top` pixels below the window's, given the
/// band's top edge `band_top` pixels below that pane top.
///
/// `band_top` is signed on purpose, and the rule is the one `clip_block_band`
/// applies to natively drawn blocks: a band scrolls above the pane's first row
/// well before it leaves the viewport, and hiding the tile at that point makes
/// a tall block vanish the moment its first line does. Scrolling the content
/// instead keeps the rows still on screen aligned with the grid rows they
/// belong to. Returns `None` only once no part of the band is visible.
fn clip_tile_band(
    band_top: f32,
    band_height: f32,
    pane_top: f32,
    pane_height: f32,
) -> Option<TileClip> {
    if band_height <= 0.0 {
        return None;
    }
    let drawn_top = band_top.max(0.0);
    let drawn_bottom = (band_top + band_height).min(pane_height);
    if drawn_bottom <= drawn_top {
        return None;
    }
    Some(TileClip {
        height: drawn_bottom - drawn_top,
        scroll_top: drawn_top - band_top,
        y: pane_top + drawn_top,
    })
}

// ========================================================================
// Patch-rate gate
// ========================================================================

/// Whether a tile update should apply now rather than wait: a close always
/// does (a one-shot transition, not a rapid stream); otherwise only once
/// [`PATCH_MIN_INTERVAL`] has passed since the last applied update.
fn is_update_due(last_applied: Option<Instant>, closed: bool) -> bool {
    closed || last_applied.is_none_or(|t| t.elapsed() >= PATCH_MIN_INTERVAL)
}

// ========================================================================
// Block HTML generation
// ========================================================================

fn sandboxed_html(content_html: &str, trust: TrustTier) -> String {
    let csp_meta = match trust {
        TrustTier::Isolated => Some(CSP_ISOLATED),
        TrustTier::Restricted => Some(CSP_RESTRICTED),
        TrustTier::Trusted => None,
    };
    match csp_meta {
        Some(policy) => {
            if content_html.contains("<head>") {
                content_html.replace(
                    "<head>",
                    &format!(
                        "<head><meta http-equiv=\"Content-Security-Policy\" content=\"{policy}\">"
                    ),
                )
            } else {
                format!(
                    "<html><head><meta http-equiv=\"Content-Security-Policy\" content=\"{policy}\"></head><body>{content_html}</body></html>"
                )
            }
        }
        None => content_html.to_string(),
    }
}

/// Wrap a block's chosen representation in the sandboxed HTML shell.
pub fn render_block_html(
    emit: &EmitBlock,
    theme: &winter_render::Theme,
    font_family: Option<&str>,
    font_size: f32,
    remote_assets: bool,
) -> String {
    let content = richest_content(emit, remote_assets);
    let bg_color = format!(
        "#{:02x}{:02x}{:02x}",
        theme.background.r, theme.background.g, theme.background.b
    );
    let fg_color = format!(
        "#{:02x}{:02x}{:02x}",
        theme.foreground.r, theme.foreground.g, theme.foreground.b
    );
    let font_family_str = match font_family {
        Some(f) if !f.trim().is_empty() => format!("'{}', monospace", f),
        _ => "monospace".to_string(),
    };

    BLOCK_HTML_SHELL
        .replace("{{BG_COLOR}}", &bg_color)
        .replace("{{FG_COLOR}}", &fg_color)
        .replace("{{FONT_FAMILY}}", &font_family_str)
        .replace("{{FONT_SIZE}}", &font_size.to_string())
        .replace("{{CONTENT}}", &content)
}

/// The richest MIME present in the bundle, per the render priority order. Used
/// by the app to route a block to the right backend (native GPU vs WebView).
pub fn richest_mime(emit: &EmitBlock) -> Option<&'static str> {
    MIME_RICHNESS
        .iter()
        .copied()
        .find(|mime| emit.bundle.get(mime).is_some())
}

fn richest_content(emit: &EmitBlock, remote_assets: bool) -> String {
    for mime in MIME_RICHNESS {
        if let Some(value) = emit.bundle.get(mime) {
            return render_mime(mime, value, remote_assets);
        }
    }
    escape_html(emit.bundle.text_plain().unwrap_or("[block]"))
}

fn render_mime(mime: &str, value: &serde_json::Value, remote_assets: bool) -> String {
    match mime {
        "application/vnd.vega-lite+json" | "application/vnd.vega+json" => {
            render_vega(value, remote_assets)
        }
        "text/html" => {
            let html = value.as_str().unwrap_or("");
            format!("<div style=\"padding:8px;\">{html}</div>")
        }
        "image/svg+xml" => {
            let svg = value.as_str().unwrap_or("");
            format!("<div style=\"padding:8px;\">{svg}</div>")
        }
        "text/markdown" => {
            let md = value.as_str().unwrap_or("");
            let html = markdown_to_html(md);
            format!("<div style=\"padding:8px;\">{html}</div>")
        }
        "text/csv" => {
            let csv = value.as_str().unwrap_or("");
            let html = csv_to_table(csv);
            format!("<div style=\"padding:8px;\">{html}</div>")
        }
        "application/json" => {
            let formatted = serde_json::to_string_pretty(value).unwrap_or_default();
            format!(
                "<pre style=\"padding:8px;white-space:pre-wrap;font-size:13px;\">{}</pre>",
                escape_html(&formatted)
            )
        }
        "text/plain" => {
            let text = value.as_str().unwrap_or("");
            format!(
                "<pre style=\"padding:8px;white-space:pre-wrap;\">{}</pre>",
                escape_html(text)
            )
        }
        other if other.starts_with("image/") => {
            let data = value.as_str().unwrap_or("");
            format!("<div style=\"padding:8px;\"><img src=\"data:{mime};base64,{data}\" style=\"max-width:100%;\" /></div>")
        }
        _ => {
            let text = value.as_str().unwrap_or("?");
            format!("<pre style=\"padding:8px;\">{}</pre>", escape_html(text))
        }
    }
}

/// A Vega/Vega-Lite spec, rendered as a live chart when the user has opted
/// into remote assets and as the pretty-printed spec otherwise.
///
/// Live rendering needs the Vega runtime, which is a multi-megabyte bundle this
/// crate does not vendor, so it can only come off a CDN. That is a network
/// request triggered by whatever wrote to the PTY, so it stays opt-in; the
/// fallback keeps the block readable rather than blank.
fn render_vega(value: &serde_json::Value, remote_assets: bool) -> String {
    let pretty = serde_json::to_string_pretty(value).unwrap_or_default();
    if !remote_assets {
        return format!(
            "<pre style=\"padding:8px;white-space:pre-wrap;font-size:13px;\">{}</pre>",
            escape_html(&pretty)
        );
    }
    let spec_json = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    let scripts: String = VEGA_CDN_SCRIPTS
        .iter()
        .map(|src| format!("<script src=\"{src}\"></script>"))
        .collect();
    format!(
        "<div id=\"vis\" style=\"width:100%;min-height:240px;padding:8px;\"></div>\
         <noscript><pre style=\"padding:8px;\">{}</pre></noscript>\
         {scripts}\
         <script>\
           var spec = {spec_json};\
           if (window.vegaEmbed) {{\
             vegaEmbed('#vis', spec, {{actions: false, theme: 'dark'}}).catch(console.error);\
           }}\
         </script>",
        escape_html(&pretty)
    )
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn markdown_to_html(md: &str) -> String {
    let mut html = String::new();
    let mut in_list = false;
    for line in md.lines() {
        if let Some(rest) = line.strip_prefix("# ") {
            if in_list {
                html.push_str("</ul>");
                in_list = false;
            }
            html.push_str(&format!("<h2>{}</h2>", escape_html(rest)));
        } else if let Some(rest) = line.strip_prefix("## ") {
            if in_list {
                html.push_str("</ul>");
                in_list = false;
            }
            html.push_str(&format!("<h3>{}</h3>", escape_html(rest)));
        } else if line.starts_with("- ") || line.starts_with("* ") {
            if !in_list {
                html.push_str("<ul>");
                in_list = true;
            }
            html.push_str(&format!("<li>{}</li>", escape_html(&line[2..])));
        } else if line.starts_with("```") {
            if in_list {
                html.push_str("</ul>");
                in_list = false;
            }
            html.push_str("<pre><code>");
        } else if !line.is_empty() {
            if in_list {
                html.push_str("</ul>");
                in_list = false;
            }
            html.push_str(&format!("<p>{}</p>", escape_html(line)));
        }
    }
    if in_list {
        html.push_str("</ul>");
    }
    html
}

fn csv_to_table(csv: &str) -> String {
    let mut rows = Vec::new();
    for line in csv.lines() {
        let cells: Vec<String> = line
            .split(',')
            .map(|cell| escape_html(cell.trim()))
            .collect();
        if !cells.is_empty() {
            rows.push(cells);
        }
    }
    if rows.is_empty() {
        return String::new();
    }
    let mut html = String::from("<table style=\"border-collapse:collapse;\">");
    for (i, row) in rows.iter().enumerate() {
        let tag = if i == 0 { "th" } else { "td" };
        html.push_str("<tr>");
        for cell in row {
            html.push_str(&format!(
                "<{tag} style=\"border:1px solid #ccc;padding:4px 8px;\">{cell}</{tag}>"
            ));
        }
        html.push_str("</tr>");
    }
    html.push_str("</table>");
    html
}

impl Default for WebViewManager {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {

    #[test]
    fn test_a_relayed_key_arrives_as_the_key_winter_binds() {
        // A surface is the only thing that sees a key while it is up, so a
        // relay that does not parse is a document that cannot be scrolled,
        // closed, or escaped from.
        let body = r#"{"winterKey":{"alt":false,"ctrl":true,"name":"c","shift":true}}"#;
        let SurfaceMessage::Key(relayed) = read_surface_message(body, PaneId(3)) else {
            panic!("a relayed key must not be read as the page's own output");
        };
        assert_eq!(relayed.key, Key::with_ctrl_shift(KeyCode::Char('c')));
        assert_eq!(relayed.pane_id, PaneId(3));
    }

    #[test]
    fn test_what_a_page_says_is_not_read_as_a_key() {
        // A page's own output shares the channel with the relay, and reading
        // one as the other would both lose the report and inject a keystroke.
        let body = r#"{"page":3,"pages":18}"#;
        let SurfaceMessage::Text(said) = read_surface_message(body, PaneId(0)) else {
            panic!("a page's output must reach its page");
        };
        assert_eq!(said.body, body);
    }

    #[test]
    fn test_a_key_winter_has_no_code_for_is_dropped_rather_than_passed_on() {
        // Handing it on as text would have the page try to read a keystroke
        // as its own output.
        let body =
            r#"{"winterKey":{"alt":false,"ctrl":false,"name":"BrightnessUp","shift":false}}"#;
        let SurfaceMessage::Text(said) = read_surface_message(body, PaneId(0)) else {
            panic!("an unmappable key is not a key");
        };
        assert!(said.body.is_empty());
    }
    use serde_json::Value;
    use winter_core::winter_proto::{BlockId, EmitBlock, MimeBundle, TrustTier};

    use super::*;

    fn svg_emit() -> EmitBlock {
        let mut bundle = MimeBundle::new();
        bundle.insert("image/svg+xml", Value::from("<svg width='10'/>"));
        bundle.insert("text/plain", Value::from("[svg]"));
        EmitBlock {
            bundle,
            id: BlockId(1),
            trust: TrustTier::Restricted,
        }
    }

    #[test]
    fn test_vega_block_makes_no_network_request_by_default() {
        // Security regression: rendering a Vega block unconditionally injected
        // three CDN <script> tags, so any block arriving over a PTY could make
        // the terminal fetch and run remote code.
        let mut bundle = MimeBundle::new();
        bundle.insert(
            "application/vnd.vega-lite+json",
            serde_json::json!({"mark": "bar"}),
        );
        let emit = EmitBlock {
            bundle,
            id: BlockId(1),
            trust: TrustTier::Restricted,
        };

        let html = richest_content(&emit, false);
        assert!(
            !html.contains("cdn.jsdelivr.net"),
            "no remote asset may be referenced without opt-in"
        );
        assert!(
            html.contains("mark"),
            "the spec stays readable as a fallback"
        );

        let opted_in = richest_content(&emit, true);
        assert!(opted_in.contains("cdn.jsdelivr.net"));
    }

    #[test]
    fn test_new_manager_has_no_tiles() {
        let mgr = WebViewManager::new();
        assert_eq!(mgr.tile_count(), 0);
    }

    #[test]
    fn test_drain_height_reports_returns_queued_reports() {
        // A tile's IPC handler only has a `Sender` to push through; this
        // pins the receiving half without needing a real WebView/window.
        let mut mgr = WebViewManager::new();
        let pid = PaneId(0);
        mgr.report_tx
            .send(HeightReport {
                block_index: 1,
                height_px: 42.0,
                pane_id: pid,
                segment_index: 0,
            })
            .unwrap();

        let reports = mgr.drain_height_reports();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].height_px, 42.0);
        assert!(
            mgr.drain_height_reports().is_empty(),
            "must drain, not peek"
        );
    }

    #[test]
    fn test_tile_band_and_resize_tile_are_none_for_a_missing_tile() {
        // A height report can arrive after its tile closed (pane/block
        // gone); both lookups must report absence, never panic.
        let mut mgr = WebViewManager::new();
        let pid = PaneId(0);
        assert_eq!(mgr.tile_band(pid, 0, 0), None);
        assert!(!mgr.resize_tile(pid, 0, 0, 12));
    }

    #[test]
    fn test_clip_tile_band_crops_a_band_hanging_over_the_pane_top() {
        // A band whose first rows have scrolled above the pane must stay on
        // screen, shortened by what is hidden and slid up by the same amount.
        // Dropping it here is what made a tall block vanish the moment its
        // first line did.
        let clip = clip_tile_band(-30.0, 100.0, 8.0, 200.0).expect("still partly visible");
        assert_eq!(clip.height, 70.0);
        assert_eq!(clip.scroll_top, 30.0);
        assert_eq!(clip.y, 8.0, "the visible part starts at the pane's top");
    }

    #[test]
    fn test_clip_tile_band_crops_a_band_running_past_the_pane_bottom() {
        // The tile must stop at the pane's bottom edge rather than painting
        // over the status bar and whatever else is below it.
        let clip = clip_tile_band(160.0, 100.0, 0.0, 200.0).expect("still partly visible");
        assert_eq!(clip.height, 40.0);
        assert_eq!(clip.scroll_top, 0.0, "nothing is hidden above it");
        assert_eq!(clip.y, 160.0);
    }

    #[test]
    fn test_clip_tile_band_is_none_once_the_band_has_left_the_pane() {
        assert!(
            clip_tile_band(-100.0, 100.0, 0.0, 200.0).is_none(),
            "a band exactly level with the pane top shows nothing"
        );
        assert!(
            clip_tile_band(200.0, 100.0, 0.0, 200.0).is_none(),
            "a band starting at the pane's bottom edge shows nothing"
        );
    }

    #[test]
    fn test_is_update_due_gates_on_the_patch_min_interval() {
        assert!(
            is_update_due(None, false),
            "a tile with no prior update must always be due"
        );
        assert!(
            !is_update_due(Some(Instant::now()), false),
            "an update applied moments ago must be held, not reapplied"
        );
        assert!(
            is_update_due(Some(Instant::now()), true),
            "a close must bypass the rate limit and apply immediately"
        );
        let long_ago = Instant::now() - PATCH_MIN_INTERVAL - Duration::from_millis(1);
        assert!(
            is_update_due(Some(long_ago), false),
            "an update past the interval must be due again"
        );
    }

    #[test]
    fn test_tile_key_equality() {
        let pid = PaneId(0);
        let a = TileKey {
            pane_id: pid,
            block_index: 1,
            segment_index: 2,
        };
        let b = TileKey {
            pane_id: pid,
            block_index: 1,
            segment_index: 2,
        };
        assert_eq!(a, b);
    }

    #[test]
    fn test_render_block_html_svg() {
        let theme = winter_render::Theme::dark();
        let html = render_block_html(&svg_emit(), &theme, None, 14.0, false);
        assert!(html.contains("<svg width='10'/>"), "{html}");
        assert!(!html.contains("{{CONTENT}}"), "{html}");
    }

    #[test]
    fn test_render_block_html_fallback() {
        let mut bundle = MimeBundle::new();
        bundle.insert("text/plain", Value::from("hello <world>"));
        let emit = EmitBlock {
            bundle,
            id: BlockId(2),
            trust: TrustTier::Restricted,
        };
        let theme = winter_render::Theme::dark();
        let html = render_block_html(&emit, &theme, None, 14.0, false);
        assert!(html.contains("hello &lt;world&gt;"), "{html}");
    }

    #[test]
    fn test_escape_html() {
        assert_eq!(escape_html("a<b>c&d\"e"), "a&lt;b&gt;c&amp;d&quot;e");
    }

    #[test]
    fn test_richest_content_picks_html_over_svg() {
        let mut bundle = MimeBundle::new();
        bundle.insert("text/html", Value::from("<b>bold</b>"));
        bundle.insert("image/svg+xml", Value::from("<svg/>"));
        let emit = EmitBlock {
            bundle,
            id: BlockId(3),
            trust: TrustTier::Trusted,
        };
        let content = richest_content(&emit, false);
        assert!(content.contains("<b>bold</b>"), "{content}");
    }

    #[test]
    fn test_sandboxed_html_adds_csp_for_restricted() {
        let html = "<html><head></head><body>hi</body></html>";
        let result = sandboxed_html(html, TrustTier::Restricted);
        assert!(result.contains("Content-Security-Policy"), "{result}");
        assert!(result.contains(CSP_RESTRICTED), "{result}");
    }

    #[test]
    fn test_sandboxed_html_adds_csp_for_isolated() {
        let html = "<html><head></head><body>hi</body></html>";
        let result = sandboxed_html(html, TrustTier::Isolated);
        assert!(result.contains("Content-Security-Policy"), "{result}");
        assert!(result.contains(CSP_ISOLATED), "{result}");
    }

    #[test]
    fn test_sandboxed_html_no_csp_for_trusted() {
        let html = "<html><head></head><body>hi</body></html>";
        let result = sandboxed_html(html, TrustTier::Trusted);
        assert!(!result.contains("Content-Security-Policy"), "{result}");
    }

    #[test]
    fn test_sandboxed_html_wraps_fragment_without_head() {
        let html = "<svg width='10'/>";
        let result = sandboxed_html(html, TrustTier::Restricted);
        assert!(result.contains("Content-Security-Policy"), "{result}");
        assert!(result.starts_with("<html>"), "{result}");
        assert!(result.contains("<svg width='10'/>"), "{result}");
    }

    #[test]
    fn test_render_markdown_produces_html() {
        let mut bundle = MimeBundle::new();
        bundle.insert("text/markdown", Value::from("# Hello\nworld"));
        let emit = EmitBlock {
            bundle,
            id: BlockId(10),
            trust: TrustTier::Trusted,
        };
        let theme = winter_render::Theme::dark();
        let html = render_block_html(&emit, &theme, None, 14.0, false);
        assert!(html.contains("<h2>Hello</h2>"), "{html}");
        assert!(html.contains("<p>world</p>"), "{html}");
    }

    #[test]
    fn test_render_json_pretty_prints() {
        let mut bundle = MimeBundle::new();
        bundle.insert("application/json", serde_json::json!({"key": "value"}));
        let emit = EmitBlock {
            bundle,
            id: BlockId(11),
            trust: TrustTier::Restricted,
        };
        let theme = winter_render::Theme::dark();
        let html = render_block_html(&emit, &theme, None, 14.0, false);
        assert!(html.contains("&quot;key&quot;"), "{html}");
    }

    #[test]
    fn test_render_csv_produces_table() {
        let mut bundle = MimeBundle::new();
        bundle.insert("text/csv", Value::from("name,score\nAlice,95"));
        let emit = EmitBlock {
            bundle,
            id: BlockId(12),
            trust: TrustTier::Restricted,
        };
        let theme = winter_render::Theme::dark();
        let html = render_block_html(&emit, &theme, None, 14.0, false);
        assert!(html.contains("<th"), "{html}");
        assert!(html.contains("<td"), "{html}");
        assert!(html.contains("Alice"), "{html}");
    }

    #[test]
    fn test_markdown_to_html_list() {
        let html = markdown_to_html("- one\n- two\n");
        assert!(html.contains("<ul>"), "{html}");
        assert!(html.contains("<li>one</li>"), "{html}");
        assert!(html.contains("</ul>"), "{html}");
    }

    #[test]
    fn test_csv_to_table_single_row() {
        let html = csv_to_table("a,b");
        assert!(html.contains("<th"), "{html}");
        assert!(html.contains("</table>"), "{html}");
    }

    #[test]
    fn test_render_vega_lite_chart_when_remote_assets_are_allowed() {
        let mut bundle = MimeBundle::new();
        bundle.insert(
            "application/vnd.vega-lite+json",
            serde_json::json!({
                "$schema": "https://vega.github.io/schema/vega-lite/v5.json",
                "mark": "bar",
                "data": {"values": [{"a": "A", "b": 28}]}
            }),
        );
        let emit = EmitBlock {
            bundle,
            id: BlockId(15),
            trust: TrustTier::Trusted,
        };
        let theme = winter_render::Theme::dark();
        let html = render_block_html(&emit, &theme, None, 14.0, true);
        assert!(html.contains("id=\"vis\""), "{html}");
        assert!(html.contains("vegaEmbed"), "{html}");
        assert!(html.contains("vega-lite@5"), "{html}");
    }
}
