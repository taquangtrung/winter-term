//! Interactive terminal pane: a cell grid fed by a [`CombinedPerformer`]
//! (unified `vte` performer that drives both the visual grid and the block
//! parser), with bytes arriving either from a locally spawned PTY child
//! or from a named session on an external mux server.

use std::io::Write;
use std::sync::mpsc;
use std::thread;

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::io::Cursor;

use base64::Engine;
use vte::{Params, Perform};
use winter_core::winter_proto::EmitBlock;
use winter_core::{Performer, Scrollback, Segment};
use winter_render::Grid;
#[cfg(test)]
use winter_render::MAX_SCROLLBACK;

use super::block_queue::BlockQueue;
use crate::mux::protocol::ServerMessage;
use crate::mux::resilience::ResilientClient;

// ========================================================================
// Constants
// ========================================================================

const READ_CHUNK: usize = 4096;
const BELL: u8 = 0x07;
const LINE_FEED: u8 = b'\n';
const CARRIAGE_RETURN: u8 = b'\r';
const BACKSPACE: u8 = 0x08;
const HORIZONTAL_TAB: u8 = b'\t';
/// Final byte of the RIS escape sequence (`ESC c`, "Reset to Initial State").
/// `reset` sends it; a full reset must restore legacy xterm keyboard encoding.
const RIS: u8 = b'c';

/// Default grid rows reserved for a content block whose displayed height is not
/// known at emit time (markdown, SVG, HTML). Reserved in-sequence (at the
/// escape) so the shell's subsequent output flows below the block instead of
/// under it, without desyncing the shell's cursor.
pub(crate) const BLOCK_RESERVE_ROWS: usize = 12;

/// Prefix marking a pane label as a mux session name rather than a shell
/// or command path; session restore re-attaches these panes instead of
/// spawning a local shell.
pub(crate) const MUX_COMMAND_PREFIX: &str = "mux:";

/// Prefix marking a pane label as `host|session` for a session attached
/// over SSH; session restore re-attaches these to the remote host instead
/// of the local mux server.
pub(crate) const MUX_REMOTE_COMMAND_PREFIX: &str = "mux-remote:";

/// Maximum APC payload size to accumulate before aborting (guards against
/// malformed or unterminated sequences bloating memory).
const APC_MAX_PAYLOAD: usize = 4 * 1024 * 1024;

/// Upper bound on rows an image block may reserve, so a tall image cannot eat
/// the whole screen. Raster images reserve exactly the rows they occupy, capped
/// here; the app scales them to fit the same cap.
pub(crate) const MAX_IMAGE_ROWS: usize = 24;

/// Raster image MIME types whose displayed height can be computed from their
/// pixel dimensions at emit time (so they reserve an exact band).
const RASTER_MIMES: [&str; 4] = ["image/gif", "image/jpeg", "image/png", "image/webp"];

/// Resolve a bare shell name (e.g. `zsh`, `fish`) to a full path by searching
/// `$PATH`. Returns the input unchanged when it already contains a path
/// separator or when no match is found. This lets users write `shell-linux
/// "zsh"` instead of `shell-linux "/usr/bin/zsh"` in their config.
///
/// `portable_pty`'s own `search_path` runs inside `spawn_command` and can
/// silently fall through in some graphical environments where `PATH` is
/// narrower than the user's login environment. Resolving here, at spawn time,
/// uses the process's live `$PATH` and surfaces the failure early.
#[cfg(unix)]
fn resolve_shell(name: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    if name.contains('/') {
        return name.to_string();
    }
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .map(|dir| std::path::PathBuf::from(dir).join(name))
        .find(|p| {
            p.is_file()
                && std::fs::metadata(p)
                    .map(|m| m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
        })
        .and_then(|p| p.into_os_string().into_string().ok())
        .unwrap_or_else(|| name.to_string())
}

#[cfg(not(unix))]
fn resolve_shell(name: &str) -> String {
    name.to_string()
}

/// Clipboard text an OSC 52 read response may carry, in bytes. Larger
/// clipboards answer with an empty payload: the response is written straight
/// into the PTY (whose kernel buffer is ~64 KiB), so an unbounded reply could
/// block the GUI thread on a tool that isn't reading.
const OSC52_READ_MAX_BYTES: usize = 64 * 1024;

/// The reply to an `OSC 52 ; c ; ?` query: the clipboard's text base64-
/// encoded as `OSC 52 ; c ; <data> ST`. Text over [`OSC52_READ_MAX_BYTES`]
/// answers with an empty payload — xterm's documented "refused" form — so a
/// querying tool never waits on data that will not come.
pub(crate) fn osc52_read_response(text: &str) -> Vec<u8> {
    let payload = if text.len() <= OSC52_READ_MAX_BYTES {
        base64::engine::general_purpose::STANDARD.encode(text)
    } else {
        String::new()
    };
    let mut response = format!("\x1b]52;c;{payload}").into_bytes();
    response.extend_from_slice(b"\x1b\\");
    response
}

/// Whether `url`'s scheme is on the safe-open allowlist (`http`, `https`,
/// `mailto`). Rejects `file://`, `javascript:`, custom app schemes, and
/// anything else that could invoke an unexpected OS handler on Ctrl+click.
fn is_safe_url_scheme(url: &str) -> bool {
    let scheme = url.split(':').next().unwrap_or("").to_ascii_lowercase();
    matches!(scheme.as_str(), "http" | "https" | "mailto")
}

/// Number of renderable (`Content`/`Live`) segments across the scrollback, used
/// to detect how many blocks an escape just produced.
fn content_segment_count(scrollback: &Scrollback) -> usize {
    scrollback
        .blocks()
        .iter()
        .flat_map(|block| &block.output)
        .filter(|segment| matches!(segment, Segment::Content(_) | Segment::Live(_)))
        .count()
}

/// The renderable (`Content`/`Live`) segments added between two
/// [`content_segment_count`] readings, oldest first — the ones a just-parsed
/// escape appended, which the caller must anchor and reserve rows for.
fn new_renderable_segments(scrollback: &Scrollback, before: usize, after: usize) -> Vec<&Segment> {
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

/// Exact rows a raster image occupies fit to the pane width, capped at
/// [`MAX_IMAGE_ROWS`]. `None` when the block is not a raster image (the caller
/// then uses the default band).
fn image_reserve_rows(
    emit: &EmitBlock,
    cols: usize,
    cell_width: f32,
    cell_height: f32,
) -> Option<usize> {
    let value = RASTER_MIMES
        .iter()
        .find_map(|mime| emit.bundle.get(mime).and_then(|v| v.as_str()))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value)
        .ok()?;
    let (nat_w, nat_h) = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()?;
    if nat_w == 0 || nat_h == 0 || cell_height <= 0.0 {
        return None;
    }
    let pane_w = cols as f32 * cell_width;
    let display_w = (nat_w as f32).min(pane_w);
    let display_h = display_w * nat_h as f32 / nat_w as f32;
    let rows = (display_h / cell_height).ceil() as usize;
    Some(rows.clamp(1, MAX_IMAGE_ROWS))
}

// ========================================================================
// Data Structures
// ========================================================================

/// What a call to [`CombinedPerformer::apc_filter`] wants the drain loop to do
/// with the current byte.
enum ApcDecision {
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
struct KittyStack(Vec<u32>);

impl KittyStack {
    fn push(&mut self, flags: u32) {
        self.0.push(flags);
    }

    fn pop(&mut self, n: u32) {
        for _ in 0..n {
            if self.0.is_empty() {
                break;
            }
            self.0.pop();
        }
    }

    fn current(&self) -> u32 {
        self.0.last().copied().unwrap_or(0)
    }

    /// Drop every pushed entry, restoring legacy xterm encoding (flags = 0).
    fn clear(&mut self) {
        self.0.clear();
    }

    /// Mode-based modification (`CSI = flags ; mode u`):
    /// mode 1 = set (replace current), 2 = unset (AND NOT), 3 = OR.
    /// If the stack is empty, a new entry is pushed; otherwise the top is updated.
    fn modify(&mut self, flags: u32, mode: u32) {
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
fn csi_param_or_default(params: &Params, index: usize, default: u32) -> u32 {
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
struct CombinedPerformer {
    /// APC (Application Program Command) payload bytes accumulated between
    /// `ESC _` and the String Terminator `ESC \\` / `\x9c`. Used to parse the
    /// Kitty graphics protocol (`APC G ... ST`) which vte 0.13 silently drops.
    apc_buf: Vec<u8>,
    /// True while we are inside an `ESC _ ... ST` APC string.
    apc_in: bool,
    /// True when the last byte was `ESC` (0x1b) — used for lookahead so we can
    /// intercept `ESC _` without consuming unrelated escape sequences.
    apc_pending_esc: bool,
    bell: bool,
    /// Grid rows (one per emitted block, in emission order) where the block was
    /// anchored, drained by [`Pane::drain_output`] into the block queue.
    block_anchors: Vec<usize>,
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

// ========================================================================
// CombinedPerformer
// ========================================================================

impl CombinedPerformer {
    fn new(cols: usize, rows: usize, max_scrollback: usize) -> Self {
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

    fn kitty_flags(&self) -> u32 {
        if self.grid.is_alt_screen() {
            self.kitty_alt.current()
        } else {
            self.kitty_main.current()
        }
    }

    /// xterm modifyOtherKeys mode: `None` = disabled, `Some(1)` or `Some(2)`.
    fn modify_other_keys(&self) -> Option<i64> {
        self.modify_other_keys
    }

    /// Mutable active stack for the screen the grid is currently displaying.
    fn kitty_active_mut(&mut self) -> &mut KittyStack {
        if self.grid.is_alt_screen() {
            &mut self.kitty_alt
        } else {
            &mut self.kitty_main
        }
    }

    fn take_pending_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending_responses)
    }

    /// Take the decoded clipboard text from a pending `OSC 52 ; c ; <base64>`
    /// write, if any. Called by [`Pane::take_clipboard_write`] after each
    /// parse batch so the app layer can write it to the OS clipboard.
    fn take_clipboard_write(&mut self) -> Option<String> {
        self.pending_clipboard_write.take()
    }

    /// Take the flag raised by an `OSC 52 ; c ; ?` read query, if any. The
    /// pane cannot reach the OS clipboard, so the app layer answers it —
    /// honoring the `clipboard-read` setting — after each parse batch.
    fn take_clipboard_read(&mut self) -> bool {
        std::mem::take(&mut self.pending_clipboard_read)
    }

    /// Handle `OSC 52 ; <selection> ; <data>` clipboard access. Writes
    /// (base64-encoded text) defer to the app layer; read queries (`?`)
    /// raise a flag the app answers from the OS clipboard — and only when
    /// the `clipboard-read` setting allows it, so the default stays silent
    /// (a read would otherwise be an unconditional, invisible exfiltration
    /// channel for any program in the pane, ssh'd or local).
    fn handle_osc52(&mut self, params: &[&[u8]]) {
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

    /// Anchor rows of blocks emitted since the last call, in emission order.
    fn take_block_anchors(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.block_anchors)
    }

    /// Shift pending block anchors at or below `row` down by `delta` (a band
    /// above them grew mid-drain).
    fn shift_block_anchors(&mut self, row: usize, delta: usize) {
        for anchor in &mut self.block_anchors {
            if *anchor >= row {
                *anchor += delta;
            }
        }
    }

    fn set_cell_size(&mut self, width: f32, height: f32) {
        self.cell_width = width;
        self.cell_height = height;
    }

    /// Rows to reserve for each renderable segment added since `before`:
    /// the exact rows a raster image will occupy (capped), else the default
    /// band. Live blocks always get the default band — their size is not
    /// knowable at open time, and their tile scrolls internally.
    fn reserve_rows_for_new_segments(&self, before: usize, after: usize) -> Vec<usize> {
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

    /// Reserve `rows` blank grid rows for a block at the cursor: anchor it
    /// at the cursor row and line-feed past the band. When the reservation
    /// itself scrolls the screen (a band emitted near the bottom), the
    /// anchor is pulled up so it keeps naming the band's visible top — the
    /// rows scrolled off the top are the band's own, clipped ones.
    fn reserve_band_rows(&mut self, rows: usize) {
        let scrolled_before = self.grid.scrollback_len();
        self.block_anchors.push(self.grid.cursor().0);
        for _ in 0..rows {
            self.grid.line_feed();
        }
        let scrolled = self.grid.scrollback_len() - scrolled_before;
        if scrolled > 0 {
            if let Some(anchor) = self.block_anchors.last_mut() {
                *anchor = anchor.saturating_sub(scrolled);
            }
        }
    }

    /// Pre-filter one PTY byte for the Kitty graphics protocol. vte 0.13
    /// silently discards APC sequences (`ESC _ ... ST`), so we intercept them
    /// before the vte parser sees them.
    ///
    /// The caller must act on the returned [`ApcDecision`] to know what (if
    /// anything) to forward to the vte parser.
    fn apc_filter(&mut self, byte: u8) -> ApcDecision {
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

    fn finalize_apc(&mut self) {
        self.apc_in = false;
        self.apc_pending_esc = false;
        if self.apc_buf.first() == Some(&b'G') {
            let payload = std::mem::take(&mut self.apc_buf);
            self.handle_kitty_apc(&payload[1..]);
        } else {
            self.apc_buf.clear();
        }
    }

    fn handle_kitty_apc(&mut self, payload: &[u8]) {
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

    fn finalize_kitty_image(&mut self, b64: &[u8]) {
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
    fn finalize_sixel_image(&mut self, payload: &[u8]) {
        use base64::Engine;
        use winter_core::winter_proto::{EmitBlock, MimeBundle, TrustTier, TEXT_PLAIN};

        let Some(img) = super::sixel::decode(payload) else {
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

    fn grid(&self) -> &Grid {
        &self.grid
    }

    fn grid_mut(&mut self) -> &mut Grid {
        &mut self.grid
    }

    fn scrollback(&self) -> &Scrollback {
        self.performer.scrollback()
    }

    fn take_title(&mut self) -> Option<String> {
        self.performer.take_title()
    }

    fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.bell)
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        self.grid.resize(cols, rows);
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
                // CSI > flags u — push flags onto the stack.
                [b'>'] => {
                    let flags = csi_param_or_default(params, 0, 0);
                    self.kitty_active_mut().push(flags);
                    return;
                }
                // CSI < n u — pop n entries (default 1).
                [b'<'] => {
                    let n = csi_param_or_default(params, 0, 1);
                    self.kitty_active_mut().pop(n);
                    return;
                }
                // CSI ? u — query: respond with current flags.
                [b'?'] => {
                    let flags = self.kitty_active_mut().current();
                    let response = format!("\x1b[?{flags}u");
                    self.pending_responses
                        .extend_from_slice(response.as_bytes());
                    return;
                }
                // CSI = flags ; mode u — mode-based set/unset/or (no stack change).
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
        // the latter — e.g. a completion widget sizing a dropdown to the room
        // left below the prompt — and silently misplace their output if the
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

        // XTVERSION query: CSI > q — report terminal name and version.
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
        // OSC 52 — clipboard read/write. Handled here because it needs arboard
        // access (the core Performer crate does not depend on arboard).
        if params.first() == Some(&b"52".as_slice()) {
            self.handle_osc52(params);
            return;
        }

        // OSC 8 ; params ; URI — open/close a hyperlink on the visual grid.
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
        // RIS (ESC c) — full reset. The Kitty keyboard protocol relies on this
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
// Pane
// ========================================================================

/// One interactive pane: a terminal cell grid whose bytes arrive from a
/// local PTY or a mux session. Local PTY reads happen on a background
/// thread; the main thread drains pending output via [`Pane::drain_output`].
pub struct Pane {
    block_queue: BlockQueue,
    /// The shell or command path used to spawn this pane, or the mux
    /// session label for a pane attached to a local or SSH-remote session.
    command: String,
    combined: CombinedPerformer,
    parser: vte::Parser,
    transport: PaneTransport,
}

/// Where a pane's bytes come from and go to: a locally spawned PTY
/// child, or a named session owned by an external mux server.
enum PaneTransport {
    Local {
        child: Box<dyn portable_pty::Child + Send>,
        master: Box<dyn portable_pty::MasterPty + Send>,
        rx: mpsc::Receiver<Vec<u8>>,
        writer: Box<dyn Write + Send>,
        _read_thread: Option<thread::JoinHandle<()>>,
    },
    Mux {
        client: ResilientClient,
        /// Set once the server reports the session's process exited.
        exited: bool,
        session: String,
    },
}

impl Pane {
    /// Spawn the default shell under a PTY with the given grid dimensions.
    pub fn new(
        cols: usize,
        rows: usize,
        configured_shell: Option<&str>,
        max_scrollback: usize,
    ) -> anyhow::Result<Self> {
        Self::new_with_cwd(cols, rows, configured_shell, max_scrollback, None)
    }

    /// Spawn the default shell under a PTY, starting the child in `cwd` when
    /// given. Used so a split/new tab opens in the same working directory as the
    /// pane it was spawned from instead of the process default (usually `$HOME`).
    pub fn new_with_cwd(
        cols: usize,
        rows: usize,
        configured_shell: Option<&str>,
        max_scrollback: usize,
        cwd: Option<&str>,
    ) -> anyhow::Result<Self> {
        let shell = configured_shell
            .map(|s| s.to_string())
            .or_else(|| std::env::var("WINTER_SHELL").ok())
            .or_else(|| {
                #[cfg(target_os = "windows")]
                {
                    std::env::var("COMSPEC").ok()
                }
                #[cfg(not(target_os = "windows"))]
                {
                    std::env::var("SHELL").ok()
                }
            })
            .unwrap_or_else(|| {
                #[cfg(target_os = "windows")]
                {
                    "powershell.exe".to_string()
                }
                #[cfg(target_os = "macos")]
                {
                    "/bin/zsh".to_string()
                }
                #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
                {
                    "/bin/bash".to_string()
                }
            });

        let mut command = CommandBuilder::new(resolve_shell(&shell));
        if let Some(dir) = cwd {
            command.cwd(dir);
        }
        Self::with_command(cols, rows, command, max_scrollback)
    }

    /// Spawn `command` under a PTY with the given grid dimensions.
    pub fn with_command(
        cols: usize,
        rows: usize,
        command: CommandBuilder,
        max_scrollback: usize,
    ) -> anyhow::Result<Self> {
        let command_str = command
            .get_argv()
            .first()
            .and_then(|a| a.to_str())
            .unwrap_or("sh")
            .to_string();
        Self::with_command_labeled(cols, rows, command, max_scrollback, command_str)
    }

    fn with_command_labeled(
        cols: usize,
        rows: usize,
        mut command: CommandBuilder,
        max_scrollback: usize,
        command_str: String,
    ) -> anyhow::Result<Self> {
        // Advertise Winter to the child so capability-detecting tools emit rich
        // blocks instead of the plain-text fallback.
        command.env("TERM_PROGRAM", "winter");
        command.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
        command.env("WINTER", "1");
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");

        let pty_system = NativePtySystem::default();
        let pair = pty_system.openpty(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let child = pair.slave.spawn_command(command)?;
        drop(pair.slave);

        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        let (tx, rx) = mpsc::channel::<Vec<u8>>();

        let read_thread = thread::Builder::new()
            .name("winter pty read".into())
            .spawn(move || {
                let mut buf = [0u8; READ_CHUNK];
                let mut reader = reader;
                loop {
                    match std::io::Read::read(&mut reader, &mut buf) {
                        Ok(0) => break,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                        Ok(count) => {
                            if tx.send(buf[..count].to_vec()).is_err() {
                                break;
                            }
                        }
                    }
                }
            })?;

        Ok(Self {
            block_queue: BlockQueue::new(),
            command: command_str,
            combined: CombinedPerformer::new(cols, rows, max_scrollback),
            parser: vte::Parser::new(),
            transport: PaneTransport::Local {
                child,
                master: pair.master,
                rx,
                writer,
                _read_thread: Some(read_thread),
            },
        })
    }

    /// Attach a new pane to a named session on the default mux server.
    pub fn new_mux(
        cols: usize,
        rows: usize,
        session: &str,
        max_scrollback: usize,
    ) -> anyhow::Result<Self> {
        Self::new_mux_at(
            &crate::mux::server::default_socket_path(),
            cols,
            rows,
            session,
            max_scrollback,
        )
    }

    /// Attach a new pane to a named session on the mux server at `path`.
    /// The session's buffered output replays into the grid as it arrives.
    pub fn new_mux_at(
        path: &str,
        cols: usize,
        rows: usize,
        session: &str,
        max_scrollback: usize,
    ) -> anyhow::Result<Self> {
        let client = ResilientClient::new(path, session);
        if !client.is_connected() {
            anyhow::bail!("could not connect to the mux server at {path}");
        }
        let mut pane = Self {
            block_queue: BlockQueue::new(),
            command: format!("{MUX_COMMAND_PREFIX}{session}"),
            combined: CombinedPerformer::new(cols.max(1), rows.max(1), max_scrollback),
            parser: vte::Parser::new(),
            transport: PaneTransport::Mux {
                client,
                exited: false,
                session: session.to_string(),
            },
        };
        // The server sizes a fresh session at its default geometry; tell
        // it this pane's real size so the remote PTY matches the grid.
        pane.transport_resize(cols.max(1), rows.max(1));
        Ok(pane)
    }

    /// Attach a new pane to a named session on a mux server reached over
    /// SSH at `host`. The session's buffered output replays into the grid
    /// as it arrives.
    pub fn new_mux_remote(
        host: &str,
        cols: usize,
        rows: usize,
        session: &str,
        max_scrollback: usize,
    ) -> anyhow::Result<Self> {
        let client = ResilientClient::new_remote(host, session);
        if !client.is_connected() {
            anyhow::bail!("could not reach '{host}' over ssh");
        }
        let mut pane = Self {
            block_queue: BlockQueue::new(),
            command: format!("{MUX_REMOTE_COMMAND_PREFIX}{host}|{session}"),
            combined: CombinedPerformer::new(cols.max(1), rows.max(1), max_scrollback),
            parser: vte::Parser::new(),
            transport: PaneTransport::Mux {
                client,
                exited: false,
                session: session.to_string(),
            },
        };
        pane.transport_resize(cols.max(1), rows.max(1));
        Ok(pane)
    }

    /// Drain all pending PTY output into the cell grid and block parser.
    /// Returns `true` if any output was processed.
    pub fn drain_output(&mut self) -> bool {
        let mut chunks = Vec::new();
        // Session geometry reported by the mux server (attach confirmation
        // or arbitration): applied after the transport borrow ends.
        let mut session_geometry: Option<(usize, usize)> = None;
        match &mut self.transport {
            PaneTransport::Local { rx, .. } => {
                while let Ok(chunk) = rx.try_recv() {
                    chunks.push(chunk);
                }
            }
            PaneTransport::Mux { client, exited, .. } => {
                while let Some(msg) = client.recv() {
                    match msg {
                        ServerMessage::Output { bytes, .. }
                        | ServerMessage::Scrollback { bytes, .. } => chunks.push(bytes),
                        ServerMessage::Exit { .. } => *exited = true,
                        // The server owns the session's real geometry — the
                        // smallest among attached clients. Snap the grid to
                        // it (letterboxing within the layout) so a stream
                        // wrapped for a smaller session doesn't double-wrap.
                        ServerMessage::Attached { cols, rows, .. }
                        | ServerMessage::Resized { cols, rows, .. } => {
                            session_geometry = Some((cols as usize, rows as usize));
                        }
                        _ => {}
                    }
                }
            }
        }
        let got_any = !chunks.is_empty() || session_geometry.is_some();
        for chunk in &chunks {
            for &byte in chunk {
                match self.combined.apc_filter(byte) {
                    ApcDecision::Drop => {}
                    ApcDecision::Pass => {
                        self.parser.advance(&mut self.combined, byte);
                    }
                    ApcDecision::ReplayEscThenByte(b) => {
                        self.parser.advance(&mut self.combined, b'\x1b');
                        self.parser.advance(&mut self.combined, b);
                    }
                }
            }
        }
        if got_any {
            let (row, _) = self.combined.grid().cursor();
            let anchors = self.combined.take_block_anchors();
            self.block_queue
                .update(self.combined.scrollback(), row, &anchors);
        }
        if let Some((cols, rows)) = session_geometry {
            self.apply_grid_resize(cols, rows);
        }
        let responses = self.combined.take_pending_responses();
        if !responses.is_empty() {
            self.write(&responses);
        }
        got_any
    }

    /// Current Kitty keyboard protocol flags active in this pane (0 = legacy).
    pub fn kitty_flags(&self) -> u32 {
        self.combined.kitty_flags()
    }

    /// xterm modifyOtherKeys mode: `None` = disabled, `Some(1)` or `Some(2)`.
    pub fn modify_other_keys(&self) -> Option<i64> {
        self.combined.modify_other_keys()
    }

    /// Write bytes to the PTY (keyboard input).
    pub fn write(&mut self, bytes: &[u8]) {
        match &mut self.transport {
            PaneTransport::Local { writer, .. } => {
                let _ = writer.write_all(bytes);
                let _ = writer.flush();
            }
            PaneTransport::Mux { client, .. } => {
                let _ = client.send_input(bytes);
            }
        }
    }

    /// Resize the PTY and the cell grid. Signals the child process via
    /// `SIGWINCH` so the shell knows about the new dimensions.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        // Skip everything when the size is unchanged: a redundant SIGWINCH at
        // the same size leaves the shell's prompt/cursor untouched, and the grid
        // reflow would replace the shell's exact cursor with an approximation
        // (off by one on prompts with trailing markup), so the insert cursor
        // would land before the first typeable column.
        if cols == self.combined.grid().cols() && rows == self.combined.grid().rows() {
            return;
        }
        self.transport_resize(cols, rows);
        self.apply_grid_resize(cols, rows);
    }

    /// Size change sent to the PTY or mux server without the same-size
    /// guard above; used at attach time, when the remote PTY starts at the
    /// server's default geometry regardless of this grid's size.
    fn transport_resize(&mut self, cols: usize, rows: usize) {
        match &mut self.transport {
            PaneTransport::Local { master, .. } => {
                let _ = master.resize(PtySize {
                    rows: rows as u16,
                    cols: cols as u16,
                    pixel_width: 0,
                    pixel_height: 0,
                });
            }
            PaneTransport::Mux { client, .. } => {
                let _ = client.resize(cols as u16, rows as u16);
            }
        }
    }

    /// Apply a grid size change: a no-op at the same size, otherwise a
    /// reflow that preserves any scrolling region the child set (restored
    /// via `CSI r` so full-screen apps keep their bounds). Used both for
    /// layout-driven resizes ([`Pane::resize`]) and for mux
    /// session-geometry sync, where the PTY lives server-side and only the
    /// grid must follow.
    /// Apply a grid size change: a no-op at the same size, otherwise a
    /// reflow that preserves any scrolling region the child set (restored
    /// via `CSI r` so full-screen apps keep their bounds). Used both for
    /// layout-driven resizes ([`Pane::resize`]) and for mux
    /// session-geometry sync, where the PTY lives server-side and only the
    /// grid must follow.
    fn apply_grid_resize(&mut self, cols: usize, rows: usize) {
        if cols == self.combined.grid().cols() && rows == self.combined.grid().rows() {
            return;
        }
        let had_region = {
            let g = self.combined.grid();
            g.scroll_top() != 0 || g.scroll_bottom() != g.rows().saturating_sub(1)
        };
        self.combined.resize(cols, rows);
        if had_region {
            self.write(b"\x1b[r");
        }
    }

    /// The terminal cell grid (read-only for rendering).
    pub fn grid(&self) -> &Grid {
        self.combined.grid()
    }

    /// Set the pixel cell size so image blocks reserve the exact rows they
    /// occupy. Called once the renderer's metrics are known.
    pub fn set_cell_size(&mut self, width: f32, height: f32) {
        self.combined.set_cell_size(width, height);
    }

    /// The terminal cell grid (mutable, for scrollback navigation).
    pub fn grid_mut(&mut self) -> &mut Grid {
        self.combined.grid_mut()
    }

    /// The scrollback parsed so far.
    pub fn scrollback(&self) -> &Scrollback {
        self.combined.scrollback()
    }

    /// True when no full-screen process is running. Uses the alternate screen as
    /// a proxy: full-screen apps (vim, fzf, less) enter it; the shell prompt does not.
    pub fn is_at_prompt(&self) -> bool {
        !self.combined.grid().is_alt_screen()
    }

    /// True while a foreground process other than the shell itself owns the
    /// pane: a full-screen program (via [`Self::is_at_prompt`]) or, on Linux,
    /// any other foreground process group leader (via
    /// [`Self::foreground_process_name`]). A bare Escape in Insert mode is
    /// forwarded to it instead of switching to Normal mode.
    pub fn has_foreground_process(&self) -> bool {
        !self.is_at_prompt() || self.foreground_process_name().is_some()
    }

    /// Whether bracketed paste mode (CSI ?2004h) is active.
    pub fn bracketed_paste(&self) -> bool {
        self.combined.grid().bracketed_paste()
    }

    /// Whether any mouse tracking mode is active.
    pub fn mouse_tracking(&self) -> bool {
        self.combined.grid().mouse_tracking()
    }

    /// Whether drag tracking specifically is active.
    pub fn mouse_drag_tracking(&self) -> bool {
        self.combined.grid().mouse_drag_tracking()
    }

    /// Whether focus event mode (CSI ?1004h) is active.
    pub fn focus_event(&self) -> bool {
        self.combined.grid().focus_event()
    }

    /// Whether SGR extended mouse mode is active.
    pub fn mouse_sgr(&self) -> bool {
        self.combined.grid().mouse_sgr()
    }

    /// Take the pending window title set by OSC 0/2, if any.
    pub fn take_title(&mut self) -> Option<String> {
        self.combined.take_title()
    }

    /// Take the clipboard text from a pending `OSC 52` write, if any.
    pub fn take_clipboard_write(&mut self) -> Option<String> {
        self.combined.take_clipboard_write()
    }

    /// Take the flag raised by an `OSC 52 ; c ; ?` clipboard read query —
    /// the app layer answers it from the OS clipboard.
    pub fn take_clipboard_read(&mut self) -> bool {
        self.combined.take_clipboard_read()
    }

    /// Whether a bell character was received since the last check.
    pub fn take_bell(&mut self) -> bool {
        self.combined.take_bell()
    }

    pub fn block_queue(&self) -> &BlockQueue {
        &self.block_queue
    }

    pub fn block_queue_mut(&mut self) -> &mut BlockQueue {
        &mut self.block_queue
    }

    pub fn drain_live_patches(&mut self) -> Vec<usize> {
        let blocks = self.combined.scrollback().blocks().to_vec();
        self.block_queue.drain_patched_live(&blocks)
    }

    /// Grow a reserved band by inserting `extra` blank rows at screen row
    /// `row` (the first row past the band): the rows, cursor, and every block
    /// anchor at or below `row` shift down, so the content beneath a patched
    /// block is overdrawn by neither the block nor the shell.
    pub fn insert_band_rows(&mut self, row: usize, extra: usize) {
        self.combined.grid_mut().insert_rows_at(row, extra);
        self.combined.shift_block_anchors(row, extra);
        self.block_queue.shift_rows_at_or_below(row, extra);
    }

    /// Whether the child process has exited.
    pub fn is_alive(&mut self) -> bool {
        match &mut self.transport {
            PaneTransport::Local { child, .. } => match child.try_wait() {
                Ok(Some(_)) => false,
                Ok(None) => true,
                Err(_) => false,
            },
            PaneTransport::Mux { exited, .. } => !*exited,
        }
    }

    /// The shell or command path used to spawn this pane.
    pub fn shell_command(&self) -> &str {
        &self.command
    }

    /// The mux session this pane is attached to, if any.
    pub fn mux_session(&self) -> Option<&str> {
        match &self.transport {
            PaneTransport::Mux { session, .. } => Some(session),
            PaneTransport::Local { .. } => None,
        }
    }

    /// Working directory of the foreground process running in this pane.
    /// On Linux this reads `/proc/{pid}/cwd`; returns `None` on other
    /// platforms, for a mux pane (the process runs on the server's
    /// machine), or when the PID is not available.
    pub fn cwd(&self) -> Option<String> {
        #[cfg(target_os = "linux")]
        {
            let PaneTransport::Local { child, .. } = &self.transport else {
                return None;
            };
            let pid = child.process_id()?;
            std::fs::read_link(format!("/proc/{pid}/cwd"))
                .ok()
                .and_then(|p| p.into_os_string().into_string().ok())
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }

    /// Name of the foreground process running in this pane, if any. A mux
    /// pane has no local foreground process, so it reports none.
    pub fn foreground_process_name(&self) -> Option<String> {
        #[cfg(target_os = "linux")]
        {
            let PaneTransport::Local { child, .. } = &self.transport else {
                return None;
            };
            let shell_pid = child.process_id()?;
            let stat = std::fs::read_to_string(format!("/proc/{shell_pid}/stat")).ok()?;
            let tpgid = parse_foreground_process(&stat)?;
            let comm = std::fs::read_to_string(format!("/proc/{tpgid}/comm")).ok()?;
            let name = comm.trim().to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
        None
    }
}

#[cfg(target_os = "linux")]
fn parse_foreground_process(stat_content: &str) -> Option<i32> {
    let rparen = stat_content.rfind(')')?;
    let fields: Vec<&str> = stat_content[rparen + 1..].split_whitespace().collect();
    if fields.len() > 5 {
        let pgrp: i32 = fields[2].parse().ok()?;
        let tpgid: i32 = fields[5].parse().ok()?;
        if tpgid > 0 && pgrp != tpgid {
            return Some(tpgid);
        }
    }
    None
}

impl Drop for Pane {
    fn drop(&mut self) {
        if let PaneTransport::Local { child, writer, .. } = &mut self.transport {
            writer.flush().ok();
            let _ = child.kill();
            let _ = child.wait();
        }
        // A mux pane detaches implicitly: dropping the client closes the
        // connection, and the session keeps running server-side.
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pane_echo() {
        let mut pane = Pane::with_command(40, 10, CommandBuilder::new("bash"), MAX_SCROLLBACK)
            .expect("test pane spawn");
        pane.write(b"echo hello\n");
        thread::sleep(std::time::Duration::from_millis(100));
        pane.drain_output();
        let text = pane.grid().to_text();
        assert!(
            text.contains("hello"),
            "expected 'hello' in output, got: {text}"
        );
    }

    #[test]
    fn test_with_command_returns_an_error_instead_of_panicking_on_a_bad_command() {
        // Regression: PTY spawn failures used to be `.expect()`-ed, crashing
        // the whole app on an ordinary misconfiguration (a bad shell path,
        // fd exhaustion, ...). A nonexistent executable must surface as an
        // `Err`, not a panic, so callers can fall back or show an error.
        let result = Pane::with_command(
            20,
            5,
            CommandBuilder::new("/nonexistent/winter-test-executable-xyz"),
            MAX_SCROLLBACK,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_pane_resize_signals_pty() {
        let mut pane = Pane::with_command(20, 5, CommandBuilder::new("bash"), MAX_SCROLLBACK)
            .expect("test pane spawn");
        pane.resize(40, 10);
        thread::sleep(std::time::Duration::from_millis(50));
        assert!(pane.is_alive());
    }

    #[test]
    fn test_mux_pane_replays_scrollback_and_sends_input() {
        use crate::mux::protocol::{self, ClientMessage};
        use std::io::{Read, Write};
        #[cfg(unix)]
        use std::os::unix::net::UnixListener;
        #[cfg(windows)]
        use uds_windows::UnixListener;

        fn read_frame(conn: &mut impl Read) -> Vec<u8> {
            let mut len_bytes = [0u8; 4];
            conn.read_exact(&mut len_bytes).unwrap();
            let mut framed = len_bytes.to_vec();
            let mut body = vec![0u8; u32::from_be_bytes(len_bytes) as usize];
            conn.read_exact(&mut body).unwrap();
            framed.extend(body);
            framed
        }

        let path =
            std::env::temp_dir().join(format!("winter-mux-pane-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let socket_path = path.to_string_lossy().to_string();

        let (input_tx, input_rx) = mpsc::channel();
        let (resize_tx, resize_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let _attach = read_frame(&mut conn); // client's Attach
            let session = "work";
            for frame in [
                protocol::encode(&ServerMessage::Attached {
                    session: session.into(),
                    cols: 80,
                    rows: 24,
                }),
                protocol::encode(&ServerMessage::Scrollback {
                    session: session.into(),
                    bytes: b"replayed\n".to_vec(),
                }),
                protocol::encode(&ServerMessage::Output {
                    session: session.into(),
                    bytes: b"live\n".to_vec(),
                }),
            ] {
                conn.write_all(&frame).unwrap();
            }
            // The pane sends a Resize for its real geometry right after
            // attach, then whatever the user types; wait for the Input.
            let mut saw_matching_resize = false;
            loop {
                let frame = read_frame(&mut conn);
                let Some(msg) = protocol::decode::<ClientMessage>(&frame) else {
                    continue;
                };
                match msg {
                    ClientMessage::Input { bytes, .. } => {
                        let _ = input_tx.send(bytes);
                        break;
                    }
                    ClientMessage::Resize {
                        cols: 40, rows: 10, ..
                    } => saw_matching_resize = true,
                    _ => {}
                }
            }
            let _ = resize_tx.send(saw_matching_resize);
        });

        let mut pane = Pane::new_mux_at(&socket_path, 40, 10, "work", MAX_SCROLLBACK).unwrap();
        assert_eq!(pane.mux_session(), Some("work"));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut text = String::new();
        while std::time::Instant::now() < deadline {
            pane.drain_output();
            text = pane.grid().to_text();
            if text.contains("replayed") && text.contains("live") {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            text.contains("replayed"),
            "scrollback must replay, got: {text}"
        );
        assert!(
            text.contains("live"),
            "live output must render, got: {text}"
        );
        assert!(pane.is_alive());

        pane.write(b"echo hi\n");
        let sent = input_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(sent, b"echo hi\n");
        assert!(
            resize_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            "attach must tell the server the pane's real geometry"
        );
        let _ = server.join();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_mux_session_geometry_snaps_the_grid() {
        // The server owns the session's real geometry (the smallest among
        // attached clients). A pane whose layout is larger must follow it —
        // letterboxing within the layout — instead of rendering a stream
        // wrapped for a width it doesn't have.
        use crate::mux::protocol;
        use std::io::{Read, Write};
        #[cfg(unix)]
        use std::os::unix::net::UnixListener;
        #[cfg(windows)]
        use uds_windows::UnixListener;

        fn read_frame(conn: &mut impl Read) -> Vec<u8> {
            let mut len_bytes = [0u8; 4];
            conn.read_exact(&mut len_bytes).unwrap();
            let mut framed = len_bytes.to_vec();
            let mut body = vec![0u8; u32::from_be_bytes(len_bytes) as usize];
            conn.read_exact(&mut body).unwrap();
            framed.extend(body);
            framed
        }

        let path =
            std::env::temp_dir().join(format!("winter-mux-geo-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let socket_path = path.to_string_lossy().to_string();

        let server = thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let _attach = read_frame(&mut conn); // the pane's Attach
            for frame in [
                protocol::encode(&ServerMessage::Attached {
                    session: "s".into(),
                    cols: 20,
                    rows: 5,
                }),
                protocol::encode(&ServerMessage::Output {
                    session: "s".into(),
                    bytes: b"hello\n".to_vec(),
                }),
            ] {
                conn.write_all(&frame).unwrap();
            }
            let _resize = read_frame(&mut conn); // the attach-time Resize
                                                 // Then the arbitration result: another client's smaller
                                                 // geometry won.
            conn.write_all(&protocol::encode(&ServerMessage::Resized {
                session: "s".into(),
                cols: 10,
                rows: 3,
            }))
            .unwrap();
            // Hold the connection open until the pane has drained.
            thread::sleep(std::time::Duration::from_millis(300));
        });

        let mut pane = Pane::new_mux_at(&socket_path, 30, 10, "s", MAX_SCROLLBACK).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            pane.drain_output();
            if (pane.grid().cols(), pane.grid().rows()) == (10, 3) {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            (pane.grid().cols(), pane.grid().rows()),
            (10, 3),
            "the grid must follow the server-arbitrated session geometry"
        );
        assert!(pane.grid().to_text().contains("hello"));

        let _ = server.join();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_new_mux_reports_missing_server() {
        assert!(Pane::new_mux_at(
            "/tmp/winter-mux-pane-missing-test.sock",
            20,
            5,
            "s",
            MAX_SCROLLBACK
        )
        .is_err());
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
            parser.advance(&mut cp, b);
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
            parser.advance(&mut cp, b);
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
            parser.advance(&mut cp, b);
        }
        assert_eq!(
            cp.grid().cursor().0 - after_image,
            BLOCK_RESERVE_ROWS,
            "a live block gets the default band, not the previous image's rows"
        );
    }

    #[test]
    fn test_band_anchor_names_the_visible_top_when_emission_scrolls() {
        // Regression: a band reserved while the cursor sits at the bottom
        // scrolls the screen during its own line feeds; the anchor kept the
        // pre-scroll cursor row and pointed past the band's real top,
        // misplacing the block and every later anchor.
        use winter_core::winter_proto::{BlockId, Message, OpenBlock};

        let mut cp = CombinedPerformer::new(20, 10, MAX_SCROLLBACK);
        for _ in 0..9 {
            cp.grid_mut().line_feed();
        }
        assert_eq!(cp.grid().cursor().0, 9, "cursor parked at the bottom row");

        let escape = winter_core::winter_proto::encode(&Message::Open(OpenBlock {
            id: BlockId(1),
            mime: "text/markdown".to_string(),
            spec: serde_json::json!("# live"),
        }));
        let mut parser = vte::Parser::new();
        for &b in escape.as_bytes() {
            parser.advance(&mut cp, b);
        }
        let anchors = cp.take_block_anchors();
        let scrolled = cp.grid().scrollback_len();
        assert!(scrolled >= BLOCK_RESERVE_ROWS);
        assert_eq!(anchors, vec![9usize.saturating_sub(scrolled)]);
        assert_eq!(anchors[0], 0, "the band's visible top after the scroll");
    }

    #[test]
    fn test_shift_block_anchors_moves_only_rows_at_or_below() {
        let mut cp = CombinedPerformer::new(20, 30, MAX_SCROLLBACK);
        cp.block_anchors = vec![2, 5, 9];
        cp.shift_block_anchors(5, 3);
        assert_eq!(cp.block_anchors, vec![2, 8, 12]);
    }

    #[test]
    fn test_osc52_read_query_raises_the_pending_flag_once() {
        // The pane cannot read the clipboard itself; a `?` query must surface
        // as a flag the app answers (when `clipboard-read` allows), while
        // primary-selection queries and plain writes must not raise it.
        let mut cp = CombinedPerformer::new(20, 10, MAX_SCROLLBACK);
        let mut parser = vte::Parser::new();
        for &b in b"\x1b]52;c;?\x07" {
            parser.advance(&mut cp, b);
        }
        assert!(cp.take_clipboard_read());
        assert!(!cp.take_clipboard_read(), "the flag is edge-triggered");

        for &b in b"\x1b]52;p;?\x07" {
            parser.advance(&mut cp, b);
        }
        assert!(
            !cp.take_clipboard_read(),
            "primary-selection queries are not answerable"
        );

        let payload = base64::engine::general_purpose::STANDARD.encode("hi");
        let write = format!("\x1b]52;c;{payload}\x07");
        for &b in write.as_bytes() {
            parser.advance(&mut cp, b);
        }
        assert!(!cp.take_clipboard_read(), "a write is not a read");
        assert_eq!(cp.take_clipboard_write().as_deref(), Some("hi"));
    }

    #[test]
    fn test_osc52_read_response_encodes_caps_and_terminates_with_st() {
        // The reply rides the same OSC 52 grammar: base64 payload, ST
        // terminator. Oversized text answers with the empty "refused"
        // payload so a tool never waits on data that will not come.
        assert_eq!(
            osc52_read_response("hello"),
            b"\x1b]52;c;aGVsbG8=\x1b\\".to_vec()
        );
        assert_eq!(
            osc52_read_response(""),
            b"\x1b]52;c;\x1b\\".to_vec(),
            "an empty clipboard is still a reply"
        );
        let oversized = "x".repeat(OSC52_READ_MAX_BYTES + 1);
        assert_eq!(
            osc52_read_response(&oversized),
            b"\x1b]52;c;\x1b\\".to_vec(),
            "oversized clipboards are refused, not truncated"
        );
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
            parser.advance(&mut cp, b);
        }
        assert_eq!(cp.kitty_flags(), 1);
        // `reset` sends RIS (ESC c); legacy xterm encoding must return so shell
        // editing keys (Ctrl+W, Ctrl+C, ...) reach the PTY as raw control bytes.
        for &b in b"\x1bc" {
            parser.advance(&mut cp, b);
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
            parser.advance(&mut cp, b);
        }
        assert_eq!(cp.kitty_flags(), 1);
        // Returning to the main screen must expose the shell's own (empty) stack,
        // not the flags the alt-screen app left behind.
        for &b in b"\x1b[?1049l" {
            parser.advance(&mut cp, b);
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
            parser.advance(&mut cp, b);
        }
        assert_eq!(cp.kitty_flags(), 7);
        for &b in b"\x1b[<u" {
            parser.advance(&mut cp, b);
        }
        assert_eq!(cp.kitty_flags(), 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_parse_foreground_process() {
        // Active foreground process: pgrp = 12345, tpgid = 12346
        let stat_active = "12345 (bash) S 12344 12345 12345 34816 12346";
        assert_eq!(parse_foreground_process(stat_active), Some(12346));

        // Idle shell: pgrp = 12345, tpgid = 12345
        let stat_idle = "12345 (bash) S 12344 12345 12345 34816 12345";
        assert_eq!(parse_foreground_process(stat_idle), None);

        // Invalid fields
        let stat_invalid = "12345 (bash) S 12344";
        assert_eq!(parse_foreground_process(stat_invalid), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_has_foreground_process_detects_a_running_foreground_command() {
        // Regression: Escape in Insert mode should only be forwarded to a
        // running foreground command instead of switching straight to Normal
        // mode, so `has_foreground_process` must actually track a command
        // that isn't a full-screen (alt-screen) app.
        let mut pane = Pane::with_command(40, 10, CommandBuilder::new("bash"), MAX_SCROLLBACK)
            .expect("test pane spawn");
        for _ in 0..100 {
            pane.drain_output();
            if !pane.has_foreground_process() {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            !pane.has_foreground_process(),
            "an idle shell prompt has no foreground process"
        );

        pane.write(b"sleep 2\n");
        let mut detected = false;
        for _ in 0..100 {
            pane.drain_output();
            if pane.has_foreground_process() {
                detected = true;
                break;
            }
            thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            detected,
            "a running `sleep` should count as a foreground process"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_resolve_shell_absolute_path_unchanged() {
        assert_eq!(resolve_shell("/bin/zsh"), "/bin/zsh");
        assert_eq!(resolve_shell("/usr/bin/fish"), "/usr/bin/fish");
    }

    #[cfg(unix)]
    #[test]
    fn test_resolve_shell_bare_bash_finds_executable() {
        let resolved = resolve_shell("bash");
        assert!(
            resolved.contains('/'),
            "expected bash to resolve to full path, got: {resolved}"
        );
        assert!(
            std::path::Path::new(&resolved).is_file(),
            "resolved path does not exist: {resolved}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_resolve_shell_unknown_name_returns_as_is() {
        let name = "this-shell-does-not-exist-12345";
        assert_eq!(resolve_shell(name), name);
    }

    #[test]
    fn test_da1_response_advertises_vt220_class() {
        // CSI c (Primary Device Attributes) must produce a non-empty response
        // so apps detect a capable terminal instead of falling back to VT100.
        let mut cp = CombinedPerformer::new(10, 2, MAX_SCROLLBACK);
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[c" {
            parser.advance(&mut cp, b);
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
            parser.advance(&mut cp, b);
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
            parser.advance(&mut cp, b);
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
            parser.advance(&mut cp, b);
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
            parser.advance(&mut cp, b);
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
            parser.advance(&mut cp, b);
        }
        assert_eq!(cp.take_pending_responses(), b"\x1b[0n");
    }

    #[test]
    fn test_dsr_6n_reports_the_cursor_position_after_a_move() {
        // CSI 6 n must answer with the cursor's *current* screen position
        // (1-based row;col), not a fixed origin — callers like a shell's
        // completion widget use it to size output around the real cursor.
        let mut cp = CombinedPerformer::new(10, 5, MAX_SCROLLBACK);
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[3;5H\x1b[6n" {
            parser.advance(&mut cp, b);
        }
        assert_eq!(cp.take_pending_responses(), b"\x1b[3;5R");
    }

    #[test]
    fn test_modify_other_keys_mode2_is_parsed() {
        // CSI > 4;2 m sets modifyOtherKeys to mode 2.
        let mut cp = CombinedPerformer::new(10, 2, MAX_SCROLLBACK);
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[>4;2m" {
            parser.advance(&mut cp, b);
        }
        assert_eq!(cp.modify_other_keys(), Some(2));
    }

    #[test]
    fn test_modify_other_keys_reset_to_none() {
        // CSI > 4 m (no value) resets modifyOtherKeys to None.
        let mut cp = CombinedPerformer::new(10, 2, MAX_SCROLLBACK);
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[>4;2m" {
            parser.advance(&mut cp, b);
        }
        assert_eq!(cp.modify_other_keys(), Some(2));
        for &b in b"\x1b[>4m" {
            parser.advance(&mut cp, b);
        }
        assert_eq!(cp.modify_other_keys(), None);
    }

    #[test]
    fn test_ris_resets_modify_other_keys() {
        // ESC c (RIS) resets modifyOtherKeys along with Kitty keyboard flags.
        let mut cp = CombinedPerformer::new(10, 2, MAX_SCROLLBACK);
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[>4;2m" {
            parser.advance(&mut cp, b);
        }
        assert_eq!(cp.modify_other_keys(), Some(2));
        for &b in b"\x1bc" {
            parser.advance(&mut cp, b);
        }
        assert_eq!(cp.modify_other_keys(), None);
    }
}
