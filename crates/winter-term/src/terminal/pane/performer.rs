//! The unified vte performer driving both the cell grid and the block parser.

use super::shell::is_safe_url_scheme;
use super::{
    BACKSPACE, BELL, BLOCK_RESERVE_ROWS, CARRIAGE_RETURN, HORIZONTAL_TAB, LINE_FEED,
    MAX_IMAGE_ROWS, RIS,
};
use crate::terminal::block_queue::BlockBand;
use base64::Engine;
use std::io::Cursor;
use vte::{Params, Perform};
use winter_core::winter_proto::EmitBlock;
use winter_core::{Performer, Scrollback, Segment};
use winter_render::Grid;

// ========================================================================
// CombinedPerformer
// ========================================================================

/// Maximum APC payload size to accumulate before aborting (guards against
/// malformed or unterminated sequences bloating memory).
pub(super) const APC_MAX_PAYLOAD: usize = 4 * 1024 * 1024;
/// Raster image MIME types whose displayed height can be computed from their
/// pixel dimensions at emit time (so they reserve an exact band).
pub(super) const RASTER_MIMES: [&str; 4] = ["image/gif", "image/jpeg", "image/png", "image/webp"];
/// SVG, whose intrinsic size is readable from the markup, so it reserves an
/// exact band like a raster image rather than falling back to the default.
pub(super) const SVG_MIME: &str = "image/svg+xml";
/// Number of renderable (`Content`/`Live`) segments across the scrollback, used
/// to detect how many blocks an escape just produced.
pub(super) fn content_segment_count(scrollback: &Scrollback) -> usize {
    scrollback
        .blocks()
        .iter()
        .flat_map(|block| &block.output)
        .filter(|segment| matches!(segment, Segment::Content(_) | Segment::Live(_)))
        .count()
}
/// The renderable (`Content`/`Live`) segments added between two
/// [`content_segment_count`] readings, oldest first: the ones a just-parsed
/// escape appended, which the caller must anchor and reserve rows for.
pub(super) fn new_renderable_segments(
    scrollback: &Scrollback,
    before: usize,
    after: usize,
) -> Vec<&Segment> {
    let mut added = Vec::new();
    let mut seen = 0usize;
    for segment in scrollback
        .blocks()
        .iter()
        .flat_map(|block| block.output.iter())
        .filter(|segment| matches!(segment, Segment::Content(_) | Segment::Live(_)))
    {
        seen += 1;
        if seen > before {
            added.push(segment);
            if added.len() == after - before {
                break;
            }
        }
    }
    added
}
/// The intrinsic pixel size of an SVG document, from the root element's
/// `width`/`height`, falling back to the last two numbers of its `viewBox`.
///
/// Deliberately a shallow attribute scan rather than a real parse: this runs on
/// the PTY drain path, and all it has to decide is how many grid rows to
/// reserve. Anything it cannot read falls back to the default band.
fn svg_intrinsic_size(markup: &str) -> Option<(f32, f32)> {
    let open = markup.find("<svg")?;
    let root = &markup[open..markup[open..].find('>').map(|i| open + i)?];

    fn attr(root: &str, name: &str) -> Option<f32> {
        // Match the attribute only at a word boundary, so `width` does not
        // also match the `stroke-width` that most real documents carry.
        let mut from = 0;
        let at = loop {
            let hit = from + root[from..].find(name)?;
            let boundary = hit == 0
                || root[..hit]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_whitespace());
            if boundary {
                break hit;
            }
            from = hit + name.len();
        };
        let rest = root[at + name.len()..].trim_start();
        let rest = rest.strip_prefix('=')?.trim_start();
        let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let close = rest[1..].find(quote)? + 1;
        let value = &rest[1..close];
        // Percentages are relative to a viewport this has no notion of, so they
        // read as unknown rather than as a wrong number.
        if value.trim_end().ends_with('%') {
            return None;
        }
        // Trailing CSS unit ("120px") is dropped; the leading number is the size.
        let digits: String = value
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        digits.parse().ok().filter(|n: &f32| *n > 0.0)
    }

    if let (Some(w), Some(h)) = (attr(root, "width"), attr(root, "height")) {
        return Some((w, h));
    }
    let vb_at = root.find("viewBox")?;
    let rest = root[vb_at..].split('"').nth(1)?;
    let nums: Vec<f32> = rest
        .split([' ', ','])
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    match nums[..] {
        [_, _, w, h] if w > 0.0 && h > 0.0 => Some((w, h)),
        _ => None,
    }
}

/// Exact rows an image block occupies fit to the pane width, capped at
/// [`MAX_IMAGE_ROWS`]. `None` when the block's height is not knowable at emit
/// time (markdown, text, live blocks), and the caller uses the default band.
///
/// Worth the work per block: whatever this over-reserves stays on screen as
/// blank rows between the block and the next prompt, since nothing shrinks a
/// band back down once the content turns out to be smaller.
pub(super) fn image_reserve_rows(
    emit: &EmitBlock,
    cols: usize,
    cell_width: f32,
    cell_height: f32,
) -> Option<usize> {
    let (nat_w, nat_h) = raster_dimensions(emit).or_else(|| {
        let markup = emit.bundle.get(SVG_MIME).and_then(|v| v.as_str())?;
        svg_intrinsic_size(markup)
    })?;
    if nat_w <= 0.0 || nat_h <= 0.0 || cell_height <= 0.0 {
        return None;
    }
    let pane_w = cols as f32 * cell_width;
    let display_w = nat_w.min(pane_w);
    let display_h = display_w * nat_h / nat_w;
    let rows = (display_h / cell_height).ceil() as usize;
    Some(rows.clamp(1, MAX_IMAGE_ROWS))
}

/// The pixel dimensions of a raster block, read from the encoded header.
fn raster_dimensions(emit: &EmitBlock) -> Option<(f32, f32)> {
    let value = RASTER_MIMES
        .iter()
        .find_map(|mime| emit.bundle.get(mime).and_then(|v| v.as_str()))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value)
        .ok()?;
    let (w, h) = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()?;
    (w > 0 && h > 0).then_some((w as f32, h as f32))
}
/// What a call to [`CombinedPerformer::apc_filter`] wants the drain loop to do
/// with the current byte.
pub(super) enum ApcDecision {
    /// Byte was consumed by the APC state machine; do not forward to vte.
    Drop,
    /// Byte was not APC-related; forward it to vte as-is.
    Pass,
    /// The APC filter had buffered an ESC that turned out not to start an APC
    /// sequence. Forward ESC followed by the current byte to vte.
    ReplayEscThenByte(u8),
}
/// Kitty keyboard protocol flag stack. Apps push a flags bitmask to opt in to
/// progressive keyboard enhancement, then pop it on exit. The current top of
/// the stack is the active mode; an empty stack means legacy xterm encoding.
#[derive(Default)]
pub(super) struct KittyStack(Vec<u32>);
impl KittyStack {
    pub(super) fn push(&mut self, flags: u32) {
        self.0.push(flags);
    }

    pub(super) fn pop(&mut self, n: u32) {
        for _ in 0..n {
            if self.0.is_empty() {
                break;
            }
            self.0.pop();
        }
    }

    pub(super) fn current(&self) -> u32 {
        self.0.last().copied().unwrap_or(0)
    }

    /// Drop every pushed entry, restoring legacy xterm encoding (flags = 0).
    pub(super) fn clear(&mut self) {
        self.0.clear();
    }

    /// Mode-based modification (`CSI = flags ; mode u`):
    /// mode 1 = set (replace current), 2 = unset (AND NOT), 3 = OR.
    /// If the stack is empty, a new entry is pushed; otherwise the top is updated.
    pub(super) fn modify(&mut self, flags: u32, mode: u32) {
        let current = self.0.last().copied().unwrap_or(0);
        let new = match mode {
            1 => flags,
            2 => current & !flags,
            3 => current | flags,
            _ => return,
        };
        match self.0.last_mut() {
            Some(top) => *top = new,
            None => self.0.push(new),
        }
    }
}
/// Read the CSI parameter group at `index`, falling back to `default` when
/// it is either absent or explicitly `0`. `vte::Params` yields a present `0`
/// for an omitted parameter (e.g. the bare `CSI < u`, no digits) rather than
/// an empty iterator, so `Some(0)` and "omitted" are indistinguishable;
/// this matches the ANSI/Kitty convention that a `0` parameter means "default".
pub(super) fn csi_param_or_default(params: &Params, index: usize, default: u32) -> u32 {
    params
        .iter()
        .nth(index)
        .and_then(|p| p.first())
        .map(|&v| v as u32)
        .filter(|&v| v != 0)
        .unwrap_or(default)
}
/// A single `vte::Perform` that fans out every callback to both a [`Grid`]
/// (visual cell grid) and a core [`Performer`] (block parser). This replaces
/// the previous dual-parser setup where every PTY byte was parsed twice.
pub(super) struct CombinedPerformer {
    /// APC (Application Program Command) payload bytes accumulated between
    /// `ESC _` and the String Terminator `ESC \\` / `\x9c`. Used to parse the
    /// Kitty graphics protocol (`APC G ... ST`) which vte 0.13 silently drops.
    apc_buf: Vec<u8>,
    /// True while we are inside an `ESC _ ... ST` APC string.
    apc_in: bool,
    /// True when the last byte was `ESC` (0x1b): used for lookahead so we can
    /// intercept `ESC _` without consuming unrelated escape sequences.
    apc_pending_esc: bool,
    bell: bool,
    /// Grid rows (one per emitted block, in emission order) where the block was
    /// anchored, drained by [`Pane::drain_output`] into the block queue.
    block_anchors: Vec<BlockBand>,
    /// Pixel cell size, used to convert an image's pixel height into reserved
    /// rows. Set by the app once the renderer is up; defaults are close enough
    /// until then.
    cell_height: f32,
    cell_width: f32,
    grid: Grid,
    /// Accumulated base64 payload across Kitty graphics chunks (`m=1` packets).
    kitty_b64: Vec<u8>,
    /// Pixel dimensions from the first Kitty chunk header (`s=`, `v=`).
    kitty_px_h: u32,
    kitty_px_w: u32,
    /// Format code from the first Kitty chunk header (`f=`).
    kitty_format: u32,
    /// Kitty keyboard protocol flag stacks, one per screen. The protocol
    /// mandates separate stacks for the main and alternate screens so a
    /// full-screen app's pushed flags cannot leak back to the shell prompt
    /// when it exits. The active stack is selected by the grid's screen state.
    kitty_alt: KittyStack,
    kitty_main: KittyStack,
    /// xterm modifyOtherKeys mode: `None` = disabled, `Some(1)` = mode 1,
    /// `Some(2)` = mode 2. Set by `CSI > 4;N m`, cleared by `CSI > 4 m` or
    /// soft reset (DECSTR). When active, modified character keys use the
    /// unambiguous `\x1b[27;<modifier>;<codepoint>~` encoding instead of the
    /// legacy `\x1b<char>` (whose ESC prefix is timing-ambiguous).
    modify_other_keys: Option<i64>,
    performer: Performer,
    /// Response bytes queued by `CSI ? u` queries, drained into the PTY
    /// writer by [`Pane::drain_output`] after each parse batch.
    pending_responses: Vec<u8>,
    /// Text written via `OSC 52 ; c ; <base64>`, drained into the host
    /// clipboard by [`Pane::take_clipboard_write`] after each parse batch.
    pending_clipboard_read: bool,
    pending_clipboard_write: Option<String>,
    /// Accumulated Sixel payload between `DCS <params> q` and the String
    /// Terminator. Decoded and emitted as an image block on `unhook`.
    sixel_buf: Vec<u8>,
    /// True while inside a `DCS ... q ... ST` Sixel string.
    sixel_in: bool,
}
impl CombinedPerformer {
    pub(super) fn new(cols: usize, rows: usize, max_scrollback: usize) -> Self {
        Self {
            apc_buf: Vec::new(),
            apc_in: false,
            apc_pending_esc: false,
            bell: false,
            block_anchors: Vec::new(),
            // Approximate defaults until the app sets the real cell size.
            cell_height: 20.0,
            cell_width: 9.0,
            grid: Grid::new(cols, rows).with_max_scrollback(max_scrollback),
            kitty_b64: Vec::new(),
            kitty_format: 100,
            kitty_px_h: 0,
            kitty_px_w: 0,
            kitty_alt: KittyStack::default(),
            kitty_main: KittyStack::default(),
            modify_other_keys: None,
            performer: Performer::new(),
            pending_clipboard_read: false,
            pending_clipboard_write: None,
            pending_responses: Vec::new(),
            sixel_buf: Vec::new(),
            sixel_in: false,
        }
    }

    pub(super) fn kitty_flags(&self) -> u32 {
        if self.grid.is_alt_screen() {
            self.kitty_alt.current()
        } else {
            self.kitty_main.current()
        }
    }

    /// xterm modifyOtherKeys mode: `None` = disabled, `Some(1)` or `Some(2)`.
    pub(super) fn modify_other_keys(&self) -> Option<i64> {
        self.modify_other_keys
    }

    /// Mutable active stack for the screen the grid is currently displaying.
    pub(super) fn kitty_active_mut(&mut self) -> &mut KittyStack {
        if self.grid.is_alt_screen() {
            &mut self.kitty_alt
        } else {
            &mut self.kitty_main
        }
    }

    pub(super) fn take_pending_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending_responses)
    }

    /// Take the decoded clipboard text from a pending `OSC 52 ; c ; <base64>`
    /// write, if any. Called by [`Pane::take_clipboard_write`] after each
    /// parse batch so the app layer can write it to the OS clipboard.
    pub(super) fn take_clipboard_write(&mut self) -> Option<String> {
        self.pending_clipboard_write.take()
    }

    /// Take the flag raised by an `OSC 52 ; c ; ?` read query, if any. The
    /// pane cannot reach the OS clipboard, so the app layer answers it,
    /// honoring the `clipboard-read` setting: after each parse batch.
    pub(super) fn take_clipboard_read(&mut self) -> bool {
        std::mem::take(&mut self.pending_clipboard_read)
    }

    /// Handle `OSC 52 ; <selection> ; <data>` clipboard access. Writes
    /// (base64-encoded text) defer to the app layer; read queries (`?`)
    /// raise a flag the app answers from the OS clipboard, and only when
    /// the `clipboard-read` setting allows it, so the default stays silent
    /// (a read would otherwise be an unconditional, invisible exfiltration
    /// channel for any program in the pane, ssh'd or local).
    pub(super) fn handle_osc52(&mut self, params: &[&[u8]]) {
        let data = match params.get(2) {
            Some(d) => *d,
            None => return,
        };
        if data == b"?" {
            // Only the clipboard selection is answerable; primary-selection
            // queries (`p`, `q`) target a buffer winter does not track.
            if params.get(1) == Some(&b"c".as_slice()) {
                self.pending_clipboard_read = true;
            }
            return;
        }
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) {
            if let Ok(text) = String::from_utf8(bytes) {
                self.pending_clipboard_write = Some(text);
            }
        }
    }

    /// Absolute anchor rows of blocks emitted since the last call, in emission
    /// order (see [`winter_render::Grid::to_absolute_row`]).
    pub(super) fn take_block_anchors(&mut self) -> Vec<BlockBand> {
        std::mem::take(&mut self.block_anchors)
    }

    /// Move pending block anchors at or below absolute `row` by `delta` (a band
    /// above them grew or shrank mid-drain).
    pub(super) fn shift_block_anchors(&mut self, row: usize, delta: isize) {
        for anchor in &mut self.block_anchors {
            if anchor.abs_row >= row {
                anchor.abs_row = anchor.abs_row.saturating_add_signed(delta);
            }
        }
    }

    pub(super) fn set_cell_size(&mut self, width: f32, height: f32) {
        self.cell_width = width;
        self.cell_height = height;
    }

    /// Rows to reserve for each renderable segment added since `before`:
    /// the exact rows a raster image will occupy (capped), else the default
    /// band. Live blocks always get the default band: their size is not
    /// knowable at open time, and their tile scrolls internally.
    pub(super) fn reserve_rows_for_new_segments(&self, before: usize, after: usize) -> Vec<usize> {
        new_renderable_segments(self.performer.scrollback(), before, after)
            .iter()
            .map(|segment| match segment {
                Segment::Content(emit) => {
                    image_reserve_rows(emit, self.grid.cols(), self.cell_width, self.cell_height)
                        .unwrap_or(BLOCK_RESERVE_ROWS)
                }
                _ => BLOCK_RESERVE_ROWS,
            })
            .collect()
    }

    /// Reserve `rows` blank grid rows for a block at the cursor: anchor it at
    /// the cursor row and line-feed past the band.
    ///
    /// The anchor is *absolute* (see [`winter_render::Grid::to_absolute_row`]),
    /// not a screen row. A screen row names the band correctly only until the
    /// next line of output scrolls the grid, after which the block would be
    /// drawn over whatever live text had moved into that row; an absolute row
    /// keeps naming the band's own first line for as long as the line is
    /// retained, including while the reservation's own line feeds scroll the
    /// screen out from under it.
    pub(super) fn reserve_band_rows(&mut self, rows: usize) {
        self.block_anchors.push(BlockBand {
            abs_row: self.grid.absolute_cursor_row(),
            rows,
        });
        for _ in 0..rows {
            self.grid.line_feed();
        }
    }

    /// Pre-filter one PTY byte for the Kitty graphics protocol. vte 0.13
    /// silently discards APC sequences (`ESC _ ... ST`), so we intercept them
    /// before the vte parser sees them.
    ///
    /// The caller must act on the returned [`ApcDecision`] to know what (if
    /// anything) to forward to the vte parser.
    pub(super) fn apc_filter(&mut self, byte: u8) -> ApcDecision {
        if self.apc_in {
            if self.apc_pending_esc {
                self.apc_pending_esc = false;
                if byte == b'\\' {
                    self.finalize_apc();
                } else {
                    // ESC inside APC not followed by '\\': keep both bytes.
                    self.apc_buf.push(b'\x1b');
                    self.apc_buf.push(byte);
                }
                return ApcDecision::Drop;
            }
            match byte {
                0x9c | 0x07 => self.finalize_apc(),
                b'\x1b' => self.apc_pending_esc = true,
                _ => {
                    if self.apc_buf.len() < APC_MAX_PAYLOAD {
                        self.apc_buf.push(byte);
                    } else {
                        self.apc_buf.clear();
                        self.apc_in = false;
                    }
                }
            }
            return ApcDecision::Drop;
        }
        if self.apc_pending_esc {
            self.apc_pending_esc = false;
            if byte == b'_' {
                self.apc_in = true;
                self.apc_buf.clear();
                return ApcDecision::Drop;
            }
            // Not APC: replay the buffered ESC then the current byte.
            return ApcDecision::ReplayEscThenByte(byte);
        }
        if byte == b'\x1b' {
            self.apc_pending_esc = true;
            return ApcDecision::Drop; // buffer ESC until we see the next byte
        }
        ApcDecision::Pass
    }

    pub(super) fn finalize_apc(&mut self) {
        self.apc_in = false;
        self.apc_pending_esc = false;
        if self.apc_buf.first() == Some(&b'G') {
            let payload = std::mem::take(&mut self.apc_buf);
            self.handle_kitty_apc(&payload[1..]);
        } else {
            self.apc_buf.clear();
        }
    }

    pub(super) fn handle_kitty_apc(&mut self, payload: &[u8]) {
        let Ok(text) = std::str::from_utf8(payload) else {
            return;
        };
        let (ctrl_str, b64_data) = text.split_once(';').unwrap_or((text, ""));

        let mut format: u32 = 100;
        let mut more: u32 = 0;
        let mut px_w: u32 = 0;
        let mut px_h: u32 = 0;

        for kv in ctrl_str.split(',') {
            if let Some((k, v)) = kv.split_once('=') {
                match k.trim() {
                    "f" => format = v.trim().parse().unwrap_or(100),
                    "m" => more = v.trim().parse().unwrap_or(0),
                    "s" => px_w = v.trim().parse().unwrap_or(0),
                    "v" => px_h = v.trim().parse().unwrap_or(0),
                    _ => {}
                }
            }
        }

        if self.kitty_b64.is_empty() {
            self.kitty_format = format;
            self.kitty_px_w = px_w;
            self.kitty_px_h = px_h;
        }
        self.kitty_b64.extend_from_slice(b64_data.as_bytes());

        if more == 0 {
            let b64 = std::mem::take(&mut self.kitty_b64);
            self.finalize_kitty_image(&b64);
        }
    }

    pub(super) fn finalize_kitty_image(&mut self, b64: &[u8]) {
        use base64::Engine;
        use winter_core::winter_proto::{EmitBlock, MimeBundle, TrustTier, TEXT_PLAIN};

        let decoded = match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(d) => d,
            Err(_) => return,
        };

        let (mime, bytes): (&str, Vec<u8>) = match self.kitty_format {
            100 => ("image/png", decoded),
            1 => ("image/jpeg", decoded),
            32 => {
                let (w, h) = (self.kitty_px_w, self.kitty_px_h);
                if w == 0 || h == 0 {
                    return;
                }
                let Some(img) = image::RgbaImage::from_raw(w, h, decoded) else {
                    return;
                };
                let mut png: Vec<u8> = Vec::new();
                if image::DynamicImage::ImageRgba8(img)
                    .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
                    .is_err()
                {
                    return;
                }
                ("image/png", png)
            }
            24 => {
                let (w, h) = (self.kitty_px_w, self.kitty_px_h);
                if w == 0 || h == 0 {
                    return;
                }
                let Some(img) = image::RgbImage::from_raw(w, h, decoded) else {
                    return;
                };
                let mut png: Vec<u8> = Vec::new();
                if image::DynamicImage::ImageRgb8(img)
                    .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
                    .is_err()
                {
                    return;
                }
                ("image/png", png)
            }
            _ => return,
        };

        let data_b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let mut bundle = MimeBundle::new();
        bundle.insert(mime, serde_json::Value::from(data_b64.as_str()));
        bundle.insert(TEXT_PLAIN, serde_json::Value::from("[image]"));
        let block = EmitBlock {
            bundle,
            id: self.performer.alloc_block_id(),
            trust: TrustTier::default(),
        };
        let before = content_segment_count(self.performer.scrollback());
        self.performer.emit(block);
        let after = content_segment_count(self.performer.scrollback());
        for rows in self.reserve_rows_for_new_segments(before, after) {
            self.reserve_band_rows(rows);
        }
    }

    /// Decode a Sixel payload and emit it as an inline PNG image block, reusing
    /// the same block-emission path as Kitty graphics.
    pub(super) fn finalize_sixel_image(&mut self, payload: &[u8]) {
        use base64::Engine;
        use winter_core::winter_proto::{EmitBlock, MimeBundle, TrustTier, TEXT_PLAIN};

        let Some(img) = crate::terminal::sixel::decode(payload) else {
            return;
        };
        let mut png: Vec<u8> = Vec::new();
        if image::DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .is_err()
        {
            return;
        }

        let data_b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let mut bundle = MimeBundle::new();
        bundle.insert("image/png", serde_json::Value::from(data_b64.as_str()));
        bundle.insert(TEXT_PLAIN, serde_json::Value::from("[image]"));
        let block = EmitBlock {
            bundle,
            id: self.performer.alloc_block_id(),
            trust: TrustTier::default(),
        };
        let before = content_segment_count(self.performer.scrollback());
        self.performer.emit(block);
        let after = content_segment_count(self.performer.scrollback());
        for rows in self.reserve_rows_for_new_segments(before, after) {
            self.reserve_band_rows(rows);
        }
    }

    pub(super) fn grid(&self) -> &Grid {
        &self.grid
    }

    pub(super) fn grid_mut(&mut self) -> &mut Grid {
        &mut self.grid
    }

    pub(super) fn scrollback(&self) -> &Scrollback {
        self.performer.scrollback()
    }

    pub(super) fn take_title(&mut self) -> Option<String> {
        self.performer.take_title()
    }

    pub(super) fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.bell)
    }

    pub(super) fn resize(&mut self, cols: usize, rows: usize) -> Option<winter_render::RowRemap> {
        self.grid.resize(cols, rows);
        let remap = self.grid.take_row_remap();
        // Pending block anchors point into the live screen the reflow just
        // rebuilt; push them onto the rows their bands now sit at.
        if let Some(remap) = &remap {
            for anchor in &mut self.block_anchors {
                anchor.abs_row = remap.map(anchor.abs_row);
            }
        }
        remap
    }
}
impl Perform for CombinedPerformer {
    fn print(&mut self, c: char) {
        self.grid.print(c);
        Perform::print(&mut self.performer, c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            LINE_FEED => self.grid.line_feed(),
            CARRIAGE_RETURN => self.grid.carriage_return(),
            BACKSPACE => self.grid.backspace(),
            HORIZONTAL_TAB => self.grid.tab(),
            _ => {}
        }
        Perform::execute(&mut self.performer, byte);
        if byte == BELL {
            self.bell = true;
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        // Kitty keyboard protocol negotiation (final byte 'u').
        if action == 'u' {
            match intermediates {
                // CSI > flags u: push flags onto the stack.
                [b'>'] => {
                    let flags = csi_param_or_default(params, 0, 0);
                    self.kitty_active_mut().push(flags);
                    return;
                }
                // CSI < n u: pop n entries (default 1).
                [b'<'] => {
                    let n = csi_param_or_default(params, 0, 1);
                    self.kitty_active_mut().pop(n);
                    return;
                }
                // CSI ? u: query: respond with current flags.
                [b'?'] => {
                    let flags = self.kitty_active_mut().current();
                    let response = format!("\x1b[?{flags}u");
                    self.pending_responses
                        .extend_from_slice(response.as_bytes());
                    return;
                }
                // CSI = flags ; mode u: mode-based set/unset/or (no stack change).
                [b'='] => {
                    let flags = csi_param_or_default(params, 0, 0);
                    let mode = csi_param_or_default(params, 1, 1);
                    self.kitty_active_mut().modify(flags, mode);
                    return;
                }
                _ => {}
            }
        }

        // Device Attributes queries (DA1/DA2/DA3) and XTVERSION.
        //
        // Apps (including Claude Code's Node.js input layer) send these at
        // startup to identify the terminal and decide which keyboard protocol
        // to use. A missing response makes many apps fall back to a bare VT100
        // assumption and use the legacy `\x1b<char>` Alt encoding, whose ESC
        // prefix is timing-ambiguous: `\x1bE` can parse as Alt+Shift+E *or* as
        // a standalone Escape (which cancels the current operation) followed by
        // `E`. By advertising a modern Kitty-compatible terminal identity we
        // let these apps switch to the unambiguous CSI-u encoding so modifier
        // combos like Shift+Alt+E/H/L arrive intact.
        if action == 'c' {
            match intermediates {
                // DA1 (Primary Device Attributes): CSI c or CSI 0 c.
                // Advertise VT220-class (62) with ANSI color (22) so apps
                // enable their full feature set (256/truecolor, mouse, ...).
                [] => {
                    self.pending_responses
                        .extend_from_slice(b"\x1b[?62;1;2;4;6;9;15;16;17;22c");
                    return;
                }
                // DA2 (Secondary Device Attributes): CSI > c.
                // Respond with VT220-class (1) and xterm patch level 277 so
                // xterm-compatible apps enable SGR mouse (pv >= 277) and probe
                // for modifyOtherKeys support (pv >= 279). WezTerm uses the
                // same values for maximum compatibility.
                [b'>'] => {
                    self.pending_responses.extend_from_slice(b"\x1b[>1;277;0c");
                    return;
                }
                // DA3 (Tertiary Device Attributes): CSI = c.
                // Response is DCS ! | <8 hex digits> ST. "00000000" is a safe
                // generic identifier (xterm's default).
                [b'='] => {
                    self.pending_responses
                        .extend_from_slice(b"\x1bP!|00000000\x1b\\");
                    return;
                }
                _ => {}
            }
        }

        // Device Status Report: CSI 5 n ("are you OK?", answered with the only
        // status VT100 defines, `ESC [ 0 n` for "fine") and CSI 6 n (cursor
        // position report, `ESC [ row ; col R`, 1-based). Shells and TUIs that
        // redraw relative to the cursor without tracking it themselves rely on
        // the latter: e.g. a completion widget sizing a dropdown to the room
        // left below the prompt, and silently misplace their output if the
        // query times out unanswered instead of erroring.
        if action == 'n' && intermediates.is_empty() {
            match csi_param_or_default(params, 0, 0) {
                5 => {
                    self.pending_responses.extend_from_slice(b"\x1b[0n");
                    return;
                }
                6 => {
                    let (row, col) = self.grid.cursor();
                    let resp = format!("\x1b[{};{}R", row + 1, col + 1);
                    self.pending_responses.extend_from_slice(resp.as_bytes());
                    return;
                }
                _ => {}
            }
        }

        // XTVERSION query: CSI > q: report terminal name and version.
        // Distinguished from the cursor-shape query `CSI <SP> q` by the '>'
        // intermediate byte. Apps use the name+version to pick the right
        // keyboard encoding and capability set.
        if action == 'q' && intermediates == [b'>'] {
            let resp = format!("\x1bP>|winter {}\x1b\\", env!("CARGO_PKG_VERSION"));
            self.pending_responses.extend_from_slice(resp.as_bytes());
            return;
        }

        // xterm modifyOtherKeys: `CSI > 4 ; N m` (set mode N) or `CSI > 4 m`
        // (reset to None). Mode 2 enables the unambiguous
        // `\x1b[27;<modifier>;<codepoint>~` encoding for modified keys so apps
        // like Claude Code can reliably distinguish Shift+Alt+E from Escape+E.
        // Mirrors WezTerm's `XtermKeyMode { resource: OtherKeys, value }`.
        if action == 'm' && intermediates == [b'>'] {
            let p0 = params
                .iter()
                .next()
                .and_then(|p| p.first())
                .copied()
                .unwrap_or(0);
            if p0 == 4 {
                let value = params.iter().nth(1).and_then(|p| p.first()).copied();
                self.modify_other_keys = match value {
                    Some(0) | None => None,
                    Some(v) => Some(v as i64),
                };
                return;
            }
        }

        Perform::csi_dispatch(&mut self.grid, params, intermediates, ignore, action);
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        // OSC 52: clipboard read/write. Handled here because it needs arboard
        // access (the core Performer crate does not depend on arboard).
        if params.first() == Some(&b"52".as_slice()) {
            self.handle_osc52(params);
            return;
        }

        // OSC 8 ; params ; URI: open/close a hyperlink on the visual grid.
        // Only store URLs whose scheme is on the allowlist so that rogue
        // sequences cannot cause Ctrl+click to invoke arbitrary OS handlers
        // (e.g. `file://`, `javascript:`, custom app schemes).
        if params.first() == Some(&b"8".as_slice()) {
            let uri: String = params
                .get(2..)
                .unwrap_or_default()
                .iter()
                .map(|b| String::from_utf8_lossy(b))
                .collect::<Vec<_>>()
                .join(";");
            let safe = !uri.is_empty() && is_safe_url_scheme(&uri);
            self.grid.set_active_link(safe.then_some(uri.as_str()));
        }

        let before = content_segment_count(self.performer.scrollback());
        Perform::osc_dispatch(&mut self.performer, params, bell_terminated);
        let after = content_segment_count(self.performer.scrollback());

        // For each block this escape produced, anchor it at the current row and
        // reserve rows so the shell's following output flows below it.
        for rows in self.reserve_rows_for_new_segments(before, after) {
            self.reserve_band_rows(rows);
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        // RIS (ESC c): full reset. The Kitty keyboard protocol relies on this
        // as the recovery path when a TUI that pushed enhancement flags crashes
        // without popping them: `reset` sends ESC c, and legacy xterm encoding
        // must return so shell editing keys (Ctrl+W, Ctrl+C, ...) work again.
        if intermediates.is_empty() && byte == RIS {
            self.kitty_main.clear();
            self.kitty_alt.clear();
            self.modify_other_keys = None;
        }
        Perform::esc_dispatch(&mut self.grid, intermediates, ignore, byte);
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, action: char) {
        // `DCS <params> q` begins a Sixel image; accumulate its payload until ST.
        if action == 'q' {
            self.sixel_in = true;
            self.sixel_buf.clear();
        }
    }

    fn put(&mut self, byte: u8) {
        if self.sixel_in {
            // Cap the payload to the same budget as APC images.
            if self.sixel_buf.len() < APC_MAX_PAYLOAD {
                self.sixel_buf.push(byte);
            }
        }
    }

    fn unhook(&mut self) {
        if self.sixel_in {
            self.sixel_in = false;
            let buf = std::mem::take(&mut self.sixel_buf);
            self.finalize_sixel_image(&buf);
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::MAX_SCROLLBACK;
    use super::*;

    /// Each control byte must reach its own grid operation. A refactor that
    /// left these constants unimported turned every arm into a catch-all
    /// binding, so every control byte ran `line_feed`, and the whole suite
    /// still passed.
    #[test]
    fn test_execute_dispatches_each_control_byte_distinctly() {
        let mut cp = CombinedPerformer::new(10, 3, MAX_SCROLLBACK);
        for c in "abc".chars() {
            cp.print(c);
        }

        // Carriage return returns to column 0 without changing row.
        cp.execute(CARRIAGE_RETURN);
        assert_eq!(cp.grid().cursor().1, 0, "CR must not move the row");
        assert_eq!(cp.grid().cursor().0, 0, "CR must not line-feed");

        // Tab advances within the row rather than feeding a line.
        cp.execute(HORIZONTAL_TAB);
        assert!(cp.grid().cursor().1 > 0, "tab must advance the column");
        assert_eq!(cp.grid().cursor().0, 0, "tab must not line-feed");

        // Backspace steps back one column, still on the same row.
        let before = cp.grid().cursor().1;
        cp.execute(BACKSPACE);
        assert_eq!(cp.grid().cursor().1, before - 1, "backspace moves left one");
        assert_eq!(cp.grid().cursor().0, 0, "backspace must not line-feed");

        // Only line feed advances the row.
        cp.execute(LINE_FEED);
        assert_eq!(cp.grid().cursor().0, 1, "line feed must advance the row");
    }

    #[test]
    fn test_combined_performer_print_feeds_both() {
        let mut cp = CombinedPerformer::new(10, 2, MAX_SCROLLBACK);
        cp.print('x');
        assert_eq!(cp.grid().cell(0, 0).map(|c| c.ch), Some('x'));
        assert!(cp.scrollback().plain_text().contains('x'));
    }
    #[test]
    fn test_combined_performer_csi_moves_cursor() {
        let mut cp = CombinedPerformer::new(5, 2, MAX_SCROLLBACK);
        cp.print('a');
        cp.print('b');
        cp.print('c');
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[1;1HX" {
            parser.advance(&mut cp, &[b]);
        }
        assert_eq!(cp.grid().cell(0, 0).map(|c| c.ch), Some('X'));
        assert_eq!(cp.grid().cell(0, 1).map(|c| c.ch), Some('b'));
    }
    #[test]
    fn test_combined_performer_bell() {
        let mut cp = CombinedPerformer::new(10, 1, MAX_SCROLLBACK);
        cp.execute(BELL);
        assert!(cp.take_bell());
        assert!(!cp.take_bell());
    }
    #[test]
    fn test_live_open_reserves_default_band_not_the_previous_image_rows() {
        // Regression: row reservation consulted the last *content* block, so
        // a live block opened after a tall raster image inherited the image's
        // (capped) row count instead of the default band, misaligning every
        // anchor and band that followed.
        use winter_core::winter_proto::{
            BlockId, EmitBlock, Message, MimeBundle, OpenBlock, TrustTier,
        };

        let mut png: Vec<u8> = Vec::new();
        image::RgbaImage::from_pixel(10, 300, image::Rgba([0, 0, 0, 255]))
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let data_b64 = base64::engine::general_purpose::STANDARD.encode(&png);

        let mut cp = CombinedPerformer::new(20, 200, MAX_SCROLLBACK);
        cp.set_cell_size(10.0, 10.0);
        let mut parser = vte::Parser::new();

        let mut bundle = MimeBundle::new();
        bundle.insert("image/png", serde_json::Value::from(data_b64.as_str()));
        let emit_escape = winter_core::winter_proto::encode(&Message::Emit(EmitBlock {
            bundle,
            id: BlockId(1),
            trust: TrustTier::Restricted,
        }));
        for &b in emit_escape.as_bytes() {
            parser.advance(&mut cp, &[b]);
        }
        let after_image = cp.grid().cursor().0;
        assert_eq!(
            after_image, MAX_IMAGE_ROWS,
            "the tall raster reserves its own capped rows"
        );

        let open_escape = winter_core::winter_proto::encode(&Message::Open(OpenBlock {
            id: BlockId(2),
            mime: "text/markdown".to_string(),
            spec: serde_json::json!("# live"),
        }));
        for &b in open_escape.as_bytes() {
            parser.advance(&mut cp, &[b]);
        }
        assert_eq!(
            cp.grid().cursor().0 - after_image,
            BLOCK_RESERVE_ROWS,
            "a live block gets the default band, not the previous image's rows"
        );
    }
    #[test]
    fn test_band_anchor_names_the_bands_own_line_after_later_output_scrolls() {
        // Regression: the anchor used to be a screen row, so it kept naming a
        // fixed position on the display instead of the band's first line. Once
        // a screenful of further output scrolled past, the block was drawn over
        // whatever unrelated text had moved into that row, and scrolling back
        // moved it the wrong way. An absolute anchor keeps naming the band.
        use winter_core::winter_proto::{BlockId, Message, OpenBlock};

        let mut cp = CombinedPerformer::new(20, 10, MAX_SCROLLBACK);
        for _ in 0..9 {
            cp.grid_mut().line_feed();
        }
        assert_eq!(cp.grid().cursor().0, 9, "cursor parked at the bottom row");
        let band_top = cp.grid().absolute_cursor_row();

        let escape = winter_core::winter_proto::encode(&Message::Open(OpenBlock {
            id: BlockId(1),
            mime: "text/markdown".to_string(),
            spec: serde_json::json!("# live"),
        }));
        let mut parser = vte::Parser::new();
        for &b in escape.as_bytes() {
            parser.advance(&mut cp, &[b]);
        }
        let anchors = cp.take_block_anchors();
        assert!(cp.grid().scrollback_len() >= BLOCK_RESERVE_ROWS);
        assert_eq!(
            anchors,
            vec![BlockBand {
                abs_row: band_top,
                rows: BLOCK_RESERVE_ROWS
            }],
            "the anchor names the row the band started on, scroll or no scroll"
        );

        // A screenful more output moves the band deep into history: it must
        // track that scroll row for row, where a screen-row anchor would have
        // sat still and left the block drawn over live text.
        let before = cp.grid().to_viewport_row(band_top);
        for _ in 0..30 {
            cp.grid_mut().line_feed();
        }
        let after = cp.grid().to_viewport_row(band_top);
        assert_eq!(after, before - 30, "the band scrolled with its content");
        assert!(after < 0, "and has left the top of the screen");
    }
    #[test]
    fn test_svg_intrinsic_size_reads_width_height_then_falls_back_to_viewbox() {
        // The band an SVG reserves comes from these numbers, so a miss here
        // costs the block a dozen blank rows of dead space under it.
        assert_eq!(
            svg_intrinsic_size(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="360" height="90">"#
            ),
            Some((360.0, 90.0))
        );
        // CSS units, single quotes, and attribute order must all still read.
        assert_eq!(
            svg_intrinsic_size(r#"<svg height='40px' width='80px'>"#),
            Some((80.0, 40.0))
        );
        // No width/height: the viewBox's last two numbers are the size.
        assert_eq!(
            svg_intrinsic_size(r#"<svg viewBox="0 0 200 50">"#),
            Some((200.0, 50.0))
        );
        // Percentage sizes are relative to a viewport this cannot see, and a
        // document with neither is unknowable: both fall back to the default
        // band rather than reserving a wrong one.
        assert_eq!(
            svg_intrinsic_size(r#"<svg width="100%" height="100%">"#),
            None
        );
        assert_eq!(svg_intrinsic_size(r#"<svg>"#), None);
        assert_eq!(svg_intrinsic_size("not svg at all"), None);
    }

    #[test]
    fn test_image_reserve_rows_sizes_an_svg_band_to_its_content() {
        // Regression: SVG fell through to the default 12-row band whatever its
        // height, so the demo's 90px graphic reserved more than twice the rows
        // it drew into and left the rest as blank space above the next prompt.
        use winter_core::winter_proto::{BlockId, MimeBundle};

        let mut bundle = MimeBundle::new();
        bundle.insert(
            SVG_MIME,
            serde_json::json!(r#"<svg width="360" height="90"></svg>"#),
        );
        let emit = EmitBlock {
            bundle,
            id: BlockId(1),
            trust: winter_core::winter_proto::TrustTier::Restricted,
        };
        // 90px tall at a 20px cell, inside a pane wide enough not to scale it.
        assert_eq!(image_reserve_rows(&emit, 200, 10.0, 20.0), Some(5));
        // Narrow pane: the image scales down to the width, and the band with it.
        assert_eq!(image_reserve_rows(&emit, 18, 10.0, 20.0), Some(3));
    }

    #[test]
    fn test_shift_block_anchors_moves_only_rows_at_or_below() {
        let mut cp = CombinedPerformer::new(20, 30, MAX_SCROLLBACK);
        cp.block_anchors = [2, 5, 9]
            .map(|abs_row| BlockBand { abs_row, rows: 4 })
            .to_vec();
        cp.shift_block_anchors(5, 3);
        let rows: Vec<usize> = cp.block_anchors.iter().map(|a| a.abs_row).collect();
        assert_eq!(rows, vec![2, 8, 12]);
    }
    #[test]
    fn test_osc52_read_query_raises_the_pending_flag_once() {
        // The pane cannot read the clipboard itself; a `?` query must surface
        // as a flag the app answers (when `clipboard-read` allows), while
        // primary-selection queries and plain writes must not raise it.
        let mut cp = CombinedPerformer::new(20, 10, MAX_SCROLLBACK);
        let mut parser = vte::Parser::new();
        for &b in b"\x1b]52;c;?\x07" {
            parser.advance(&mut cp, &[b]);
        }
        assert!(cp.take_clipboard_read());
        assert!(!cp.take_clipboard_read(), "the flag is edge-triggered");

        for &b in b"\x1b]52;p;?\x07" {
            parser.advance(&mut cp, &[b]);
        }
        assert!(
            !cp.take_clipboard_read(),
            "primary-selection queries are not answerable"
        );

        let payload = base64::engine::general_purpose::STANDARD.encode("hi");
        let write = format!("\x1b]52;c;{payload}\x07");
        for &b in write.as_bytes() {
            parser.advance(&mut cp, &[b]);
        }
        assert!(!cp.take_clipboard_read(), "a write is not a read");
        assert_eq!(cp.take_clipboard_write().as_deref(), Some("hi"));
    }
    #[test]
    fn test_kitty_stack_empty_returns_zero() {
        let stack = KittyStack::default();
        assert_eq!(stack.current(), 0);
    }
    #[test]
    fn test_kitty_stack_push_pop() {
        let mut stack = KittyStack::default();
        stack.push(1);
        stack.push(3);
        assert_eq!(stack.current(), 3);
        stack.pop(1);
        assert_eq!(stack.current(), 1);
        stack.pop(1);
        assert_eq!(stack.current(), 0);
        // Pop on empty stack is a no-op.
        stack.pop(1);
        assert_eq!(stack.current(), 0);
    }
    #[test]
    fn test_kitty_stack_modify_set_replaces_top() {
        let mut stack = KittyStack::default();
        stack.push(3);
        stack.modify(5, 1); // mode 1 = set
        assert_eq!(stack.current(), 5);
    }
    #[test]
    fn test_kitty_stack_modify_unset_clears_bits() {
        let mut stack = KittyStack::default();
        stack.push(7); // 0b111
        stack.modify(2, 2); // mode 2 = AND NOT: 7 & !2 = 5
        assert_eq!(stack.current(), 5);
    }
    #[test]
    fn test_kitty_stack_modify_or_adds_bits() {
        let mut stack = KittyStack::default();
        stack.push(1);
        stack.modify(6, 3); // mode 3 = OR: 1 | 6 = 7
        assert_eq!(stack.current(), 7);
    }
    #[test]
    fn test_kitty_stack_modify_on_empty_stack_pushes_entry() {
        let mut stack = KittyStack::default();
        stack.modify(3, 1); // set on empty: pushes 3
        assert_eq!(stack.current(), 3);
    }
    #[test]
    fn test_kitty_stack_modify_unknown_mode_is_noop() {
        let mut stack = KittyStack::default();
        stack.push(1);
        stack.modify(99, 99); // unknown mode
        assert_eq!(stack.current(), 1); // unchanged
    }
    #[test]
    fn test_kitty_stack_clear_empties_and_stays_usable() {
        let mut stack = KittyStack::default();
        stack.push(1);
        stack.push(3);
        stack.clear();
        assert_eq!(stack.current(), 0);
        // A fresh push works after clearing, proving clear did not corrupt state.
        stack.push(7);
        assert_eq!(stack.current(), 7);
    }
    #[test]
    fn test_ris_resets_kitty_keyboard_flags() {
        let mut cp = CombinedPerformer::new(10, 2, MAX_SCROLLBACK);
        let mut parser = vte::Parser::new();
        // A TUI pushes the disambigulate flag, then crashes without popping it.
        for &b in b"\x1b[>1u" {
            parser.advance(&mut cp, &[b]);
        }
        assert_eq!(cp.kitty_flags(), 1);
        // `reset` sends RIS (ESC c); legacy xterm encoding must return so shell
        // editing keys (Ctrl+W, Ctrl+C, ...) reach the PTY as raw control bytes.
        for &b in b"\x1bc" {
            parser.advance(&mut cp, &[b]);
        }
        assert_eq!(cp.kitty_flags(), 0);
    }
    #[test]
    fn test_kitty_flags_isolated_per_screen() {
        let mut cp = CombinedPerformer::new(10, 2, MAX_SCROLLBACK);
        let mut parser = vte::Parser::new();
        // Shell prompt on the main screen: no flags.
        assert_eq!(cp.kitty_flags(), 0);
        // A full-screen app enters the alt screen and pushes the disambiguate flag.
        for &b in b"\x1b[?1049h\x1b[>1u" {
            parser.advance(&mut cp, &[b]);
        }
        assert_eq!(cp.kitty_flags(), 1);
        // Returning to the main screen must expose the shell's own (empty) stack,
        // not the flags the alt-screen app left behind.
        for &b in b"\x1b[?1049l" {
            parser.advance(&mut cp, &[b]);
        }
        assert_eq!(cp.kitty_flags(), 0);
    }
    #[test]
    fn test_kitty_pop_bare_defaults_to_one() {
        // The Kitty spec's pop count defaults to 1 when omitted, i.e. a bare
        // `CSI < u` with no digits. vte's `Params` represents an omitted
        // parameter as a present `0`, not an empty iterator, so a naive
        // `params.iter().next()` read sees `Some(0)` and never falls back to
        // the intended default, so the pop silently becomes a no-op and an
        // app's flags stay stuck on the stack after it exits.
        let mut cp = CombinedPerformer::new(120, 40, MAX_SCROLLBACK);
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[>7u" {
            parser.advance(&mut cp, &[b]);
        }
        assert_eq!(cp.kitty_flags(), 7);
        for &b in b"\x1b[<u" {
            parser.advance(&mut cp, &[b]);
        }
        assert_eq!(cp.kitty_flags(), 0);
    }
    #[test]
    fn test_da1_response_advertises_vt220_class() {
        // CSI c (Primary Device Attributes) must produce a non-empty response
        // so apps detect a capable terminal instead of falling back to VT100.
        let mut cp = CombinedPerformer::new(10, 2, MAX_SCROLLBACK);
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[c" {
            parser.advance(&mut cp, &[b]);
        }
        let resp = cp.take_pending_responses();
        assert!(
            resp.starts_with(b"\x1b[?62"),
            "DA1 response should advertise VT220-class (62), got: {resp:?}"
        );
        assert!(resp.ends_with(b"c"), "DA1 response must end with 'c'");
    }
    #[test]
    fn test_da2_response_identifies_vt220_with_xterm_patch_level() {
        // CSI > c (Secondary Device Attributes) must respond with VT220-class
        // (1) and an xterm patch level >= 277 so apps enable SGR mouse and
        // probe for modifyOtherKeys support. Mirrors WezTerm's response.
        let mut cp = CombinedPerformer::new(10, 2, MAX_SCROLLBACK);
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[>c" {
            parser.advance(&mut cp, &[b]);
        }
        let resp = cp.take_pending_responses();
        assert!(
            resp.starts_with(b"\x1b[>1;277;"),
            "DA2 response should be VT220-class with xterm patch 277, got: {resp:?}"
        );
        assert!(resp.ends_with(b"c"), "DA2 response must end with 'c'");
    }
    #[test]
    fn test_da3_response_uses_dcs_format() {
        // CSI = c (Tertiary Device Attributes) responds via DCS ! | <hex> ST.
        let mut cp = CombinedPerformer::new(10, 2, MAX_SCROLLBACK);
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[=c" {
            parser.advance(&mut cp, &[b]);
        }
        let resp = cp.take_pending_responses();
        assert!(
            resp.starts_with(b"\x1bP!|") && resp.ends_with(b"\x1b\\"),
            "DA3 response should be DCS ! | <hex> ST, got: {resp:?}"
        );
    }
    #[test]
    fn test_xtversion_response_reports_winter_name() {
        // CSI > q (XTVERSION) responds with DCS > | <name> ST.
        let mut cp = CombinedPerformer::new(10, 2, MAX_SCROLLBACK);
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[>q" {
            parser.advance(&mut cp, &[b]);
        }
        let resp = cp.take_pending_responses();
        let text = String::from_utf8_lossy(&resp);
        assert!(
            text.contains("winter"),
            "XTVERSION response should contain 'winter', got: {text}"
        );
        assert!(
            text.starts_with("\x1bP>|") && text.ends_with("\x1b\\"),
            "XTVERSION response should be DCS > | <name> ST, got: {text:?}"
        );
    }
    #[test]
    fn test_xtversion_does_not_interfere_with_cursor_shape_query() {
        // CSI <SP> q is the cursor-shape query; it must NOT produce an
        // XTVERSION response. Only CSI > q does.
        let mut cp = CombinedPerformer::new(10, 2, MAX_SCROLLBACK);
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[2 q" {
            parser.advance(&mut cp, &[b]);
        }
        let resp = cp.take_pending_responses();
        assert!(
            resp.is_empty(),
            "Cursor-shape query must not produce a version response, got: {resp:?}"
        );
    }
    #[test]
    fn test_dsr_5n_reports_device_ok() {
        // CSI 5 n ("are you OK?") must be answered with the fixed "fine" status,
        // or an app that gates on it before proceeding hangs waiting for a reply
        // that never comes.
        let mut cp = CombinedPerformer::new(10, 2, MAX_SCROLLBACK);
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[5n" {
            parser.advance(&mut cp, &[b]);
        }
        assert_eq!(cp.take_pending_responses(), b"\x1b[0n");
    }
    #[test]
    fn test_dsr_6n_reports_the_cursor_position_after_a_move() {
        // CSI 6 n must answer with the cursor's *current* screen position
        // (1-based row;col), not a fixed origin: callers like a shell's
        // completion widget use it to size output around the real cursor.
        let mut cp = CombinedPerformer::new(10, 5, MAX_SCROLLBACK);
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[3;5H\x1b[6n" {
            parser.advance(&mut cp, &[b]);
        }
        assert_eq!(cp.take_pending_responses(), b"\x1b[3;5R");
    }
    #[test]
    fn test_modify_other_keys_mode2_is_parsed() {
        // CSI > 4;2 m sets modifyOtherKeys to mode 2.
        let mut cp = CombinedPerformer::new(10, 2, MAX_SCROLLBACK);
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[>4;2m" {
            parser.advance(&mut cp, &[b]);
        }
        assert_eq!(cp.modify_other_keys(), Some(2));
    }
    #[test]
    fn test_modify_other_keys_reset_to_none() {
        // CSI > 4 m (no value) resets modifyOtherKeys to None.
        let mut cp = CombinedPerformer::new(10, 2, MAX_SCROLLBACK);
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[>4;2m" {
            parser.advance(&mut cp, &[b]);
        }
        assert_eq!(cp.modify_other_keys(), Some(2));
        for &b in b"\x1b[>4m" {
            parser.advance(&mut cp, &[b]);
        }
        assert_eq!(cp.modify_other_keys(), None);
    }
    #[test]
    fn test_ris_resets_modify_other_keys() {
        // ESC c (RIS) resets modifyOtherKeys along with Kitty keyboard flags.
        let mut cp = CombinedPerformer::new(10, 2, MAX_SCROLLBACK);
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[>4;2m" {
            parser.advance(&mut cp, &[b]);
        }
        assert_eq!(cp.modify_other_keys(), Some(2));
        for &b in b"\x1bc" {
            parser.advance(&mut cp, &[b]);
        }
        assert_eq!(cp.modify_other_keys(), None);
    }
}
