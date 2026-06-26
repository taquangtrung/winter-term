//! WebView tile manager: creates and positions child WebViews for rich blocks.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use winit::window::Window;
use winter_core::winter_proto::{EmitBlock, TrustTier};
use wry::{WebView, WebViewBuilder};

use super::block_queue::BlockEntry;
use super::pane::BLOCK_RESERVE_ROWS;

// ========================================================================
// Constants
// ========================================================================

const BLOCK_HEIGHT_ROWS: usize = 8;
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

/// Re-measures content height after a patch: a `<script>` tag assigned via
/// `innerHTML` never runs, so a patch must post its own measurement rather
/// than relying on the shell's initial-load one.
const HEIGHT_REPORT_JS: &str =
    "try{window.ipc.postMessage(String(document.documentElement.scrollHeight));}catch(e){}";

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

/// Everything a tile needs in order to be created and positioned.
pub struct TileParams {
    /// Grid row the tile is anchored to.
    pub grid_row: usize,
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
    pub pane_id: crate::model::layout::PaneId,
    /// Index of the segment within the command block.
    pub segment_index: usize,
}

/// Manages child WebViews that render rich content blocks inline in the
/// terminal. Each content block gets its own WebView positioned at the
/// block's pixel coordinates within the parent window.
pub struct WebViewManager {
    report_rx: mpsc::Receiver<HeightReport>,
    report_tx: mpsc::Sender<HeightReport>,
    tiles: HashMap<TileKey, TileSlot>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct TileKey {
    pane_id: crate::model::layout::PaneId,
    block_index: usize,
    segment_index: usize,
}

/// A patch received too soon after the last applied one, held for the next
/// due tick instead of being dropped.
struct PendingUpdate {
    closed: bool,
    html: String,
}

struct TileSlot {
    grid_row: usize,
    /// When the WebView's content was last actually updated, for the patch
    /// rate limit; `None` before the first update.
    last_applied: Option<Instant>,
    pane_id: crate::model::layout::PaneId,
    /// A patch rate-limited past the last update, applied on the next due
    /// frame instead of being lost.
    pending: Option<PendingUpdate>,
    /// Grid rows currently reserved for this tile's band; grows as its
    /// content grows.
    reserved_rows: usize,
    trust: TrustTier,
    webview: WebView,
}

// ========================================================================
// WebViewManager
// ========================================================================

impl WebViewManager {
    /// A manager owning no tiles yet.
    pub fn new() -> Self {
        let (report_tx, report_rx) = mpsc::channel();
        Self {
            report_rx,
            report_tx,
            tiles: HashMap::new(),
        }
    }

    /// Create a tile for a block and place it over the grid.
    pub fn create_block_tile(
        &mut self,
        pane_id: crate::model::layout::PaneId,
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
            .with_bounds(wry::Rect {
                position: wry::dpi::LogicalPosition::new(params.x, params.y).into(),
                size: wry::dpi::LogicalSize::new(params.width, params.height).into(),
            })
            .with_visible(true)
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
                grid_row: params.grid_row,
                last_applied: Some(Instant::now()),
                pane_id,
                pending: None,
                reserved_rows: BLOCK_RESERVE_ROWS,
                trust: entry.trust,
                webview,
            },
        );
        Ok(())
    }

    /// Reposition all tiles based on scroll offset. Tiles whose pane is not in
    /// `active_panes` (i.e. belong to a background tab) are hidden, as are tiles
    /// that scroll offscreen; tiles that come back are re-shown.
    pub fn reposition_tiles(
        &mut self,
        scroll_offset: usize,
        grid_rows: usize,
        cell_height: f32,
        pane_y_offset: f32,
        active_panes: &std::collections::HashSet<crate::model::layout::PaneId>,
    ) {
        for (key, slot) in self.tiles.iter_mut() {
            if !active_panes.contains(&key.pane_id) {
                let _ = slot.webview.set_visible(false);
                continue;
            }
            let visible_row = slot.grid_row as isize - scroll_offset as isize;
            if visible_row < 0 || visible_row as usize >= grid_rows {
                let _ = slot.webview.set_visible(false);
            } else {
                let new_y = pane_y_offset + visible_row as f32 * cell_height;
                if let Ok(current_bounds) = slot.webview.bounds() {
                    let current_y = current_bounds.position.to_logical::<i32>(1.0).y;
                    if (current_y as f32 - new_y).abs() > 0.5 {
                        let _ = slot.webview.set_bounds(wry::Rect {
                            position: wry::dpi::LogicalPosition::new(
                                current_bounds.position.to_logical::<i32>(1.0).x,
                                new_y as i32,
                            )
                            .into(),
                            size: current_bounds.size,
                        });
                    }
                }
                let _ = slot.webview.set_visible(true);
            }
        }
    }

    /// The default block height in pixels given a cell height.
    pub fn block_pixel_height(cell_height: f32) -> u32 {
        (BLOCK_HEIGHT_ROWS as f32 * cell_height) as u32
    }

    /// Shift tiles of `pane_id` anchored at or below `row` down by `delta`
    /// rows: a band above them grew, so their anchors must follow the content
    /// they point at. The next `reposition_tiles` moves the views.
    pub fn shift_tiles_at_or_below(
        &mut self,
        pane_id: crate::model::layout::PaneId,
        row: usize,
        delta: usize,
    ) {
        for slot in self.tiles.values_mut() {
            if slot.pane_id == pane_id && slot.grid_row >= row {
                slot.grid_row += delta;
            }
        }
    }

    /// Update the HTML content of an existing tile (for live-block patches).
    /// A patch arriving too soon after the last applied one is held rather
    /// than firing immediately; a close always fires right away.
    pub fn update_tile_html(
        &mut self,
        pane_id: crate::model::layout::PaneId,
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
        let js = format!(
            "document.documentElement.innerHTML = {};{HEIGHT_REPORT_JS}{closed_js}",
            serde_json::to_string(&sandboxed).unwrap_or_default()
        );
        let _ = slot.webview.evaluate_script(&js);
        slot.last_applied = Some(Instant::now());
    }

    /// Every tile content-height report queued since the last drain.
    pub fn drain_height_reports(&mut self) -> Vec<HeightReport> {
        self.report_rx.try_iter().collect()
    }

    /// A tile's current `(grid_row, reserved_rows)`, for computing how much
    /// further a height report can grow it before touching the grid.
    /// `None` when no matching tile exists.
    pub fn tile_band(
        &self,
        pane_id: crate::model::layout::PaneId,
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
            .map(|slot| (slot.grid_row, slot.reserved_rows))
    }

    /// Grow a tile's own bounds to `reserved_rows` grid rows tall. Returns
    /// `false` when no matching tile exists (e.g. it closed after the
    /// report was queued).
    pub fn resize_tile(
        &mut self,
        pane_id: crate::model::layout::PaneId,
        block_index: usize,
        segment_index: usize,
        reserved_rows: usize,
        cell_height: f32,
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
        if let Ok(bounds) = slot.webview.bounds() {
            let position = bounds.position.to_logical::<i32>(1.0);
            let width = bounds.size.to_logical::<u32>(1.0).width;
            let _ = slot.webview.set_bounds(wry::Rect {
                position: position.into(),
                size: wry::dpi::LogicalSize::new(
                    width,
                    (reserved_rows as f32 * cell_height) as u32,
                )
                .into(),
            });
        }
        true
    }

    /// Remove all WebView tiles belonging to a closed pane.
    pub fn remove_tiles_for_pane(&mut self, pane_id: crate::model::layout::PaneId) {
        self.tiles.retain(|key, _| key.pane_id != pane_id);
    }

    /// Hide every tile, e.g. while a full-window overlay (the settings page) is
    /// up. Tiles are re-shown by the next `reposition_tiles` after it closes.
    pub fn hide_all(&self) {
        for slot in self.tiles.values() {
            let _ = slot.webview.set_visible(false);
        }
    }

    /// Hide all WebView tiles for a folded block.
    pub fn fold_block(&mut self, pane_id: crate::model::layout::PaneId, block_index: usize) {
        for (key, slot) in self.tiles.iter_mut() {
            if key.pane_id == pane_id && key.block_index == block_index {
                let _ = slot.webview.set_visible(false);
            }
        }
    }

    /// Show all WebView tiles for an unfolded block.
    pub fn unfold_block(&mut self, pane_id: crate::model::layout::PaneId, block_index: usize) {
        for (key, slot) in self.tiles.iter_mut() {
            if key.pane_id == pane_id && key.block_index == block_index {
                let _ = slot.webview.set_visible(true);
            }
        }
    }

    /// Forward a key event to the focused block's WebView by dispatching a
    /// synthetic KeyboardEvent via JavaScript. Returns true if a tile existed
    /// for the focused pane.
    pub fn forward_key_event(
        &mut self,
        pane_id: crate::model::layout::PaneId,
        bytes: &[u8],
    ) -> bool {
        let key = String::from_utf8_lossy(bytes);
        let js = format!(
            "if(document.activeElement)document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{{key:{},bubbles:true}}));",
            serde_json::to_string(&key).unwrap_or_default()
        );
        let mut dispatched = false;
        for slot in self.tiles.values_mut() {
            if slot.pane_id == pane_id {
                let _ = slot.webview.evaluate_script(&js);
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
    fn test_block_pixel_height() {
        let h = WebViewManager::block_pixel_height(20.0);
        assert_eq!(h, 160);
    }

    #[test]
    fn test_drain_height_reports_returns_queued_reports() {
        // A tile's IPC handler only has a `Sender` to push through; this
        // pins the receiving half without needing a real WebView/window.
        let mut mgr = WebViewManager::new();
        let pid = crate::model::layout::PaneId(0);
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
        let pid = crate::model::layout::PaneId(0);
        assert_eq!(mgr.tile_band(pid, 0, 0), None);
        assert!(!mgr.resize_tile(pid, 0, 0, 12, 20.0));
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
        let pid = crate::model::layout::PaneId(0);
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
