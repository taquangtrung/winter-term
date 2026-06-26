//! Winter: emit rich Terminal Block Protocol (TBP) blocks from Rust.
//!
//! Every block carries a `text/plain` fallback, and when Winter is not the
//! active terminal the fallback is printed instead, so programs using this
//! crate stay safe under tmux/ssh/CI. Wire encoding is delegated to
//! [`winter_proto`], the same codec the terminal itself decodes with.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use serde::Serialize;
use serde_json::Value;
use winter_proto::{BlockId, EmitBlock, Message, MimeBundle, OpenBlock, TrustTier, TEXT_PLAIN};

mod diff;

// ========================================================================
// Constants
// ========================================================================

const WINTER_ENV: &str = "WINTER";
const TERM_PROGRAM_ENV: &str = "TERM_PROGRAM";
const WINTER_NAME: &str = "winter";

const SVG_MIME: &str = "image/svg+xml";

static NEXT_BLOCK_ID: AtomicU64 = AtomicU64::new(1);

// ========================================================================
// Data Structures
// ========================================================================

/// Optional presentation hints for a one-shot [`display_html`]-family call.
#[derive(Clone, Debug, Default)]
pub struct DisplayOptions<'a> {
    /// Suggested height in pixels.
    pub height_hint: Option<u16>,
    /// Plain-text fallback shown when Winter is not the active terminal.
    /// Defaults to a short placeholder describing the block.
    pub text: Option<&'a str>,
    /// Block title.
    pub title: Option<&'a str>,
    /// Capability tier the block's content is granted.
    pub trust: TrustTier,
}

/// A handle to an open live TBP block, updated in place via patches.
///
/// Outside Winter the open call writes the fallback once and every later
/// `update`/`update_from`/`patch_ops`/`close` is a no-op, so streaming
/// loops stay safe under tmux/ssh/CI.
pub struct LiveBlock {
    id: BlockId,
    live: bool,
    mime: String,
    stream: Box<dyn Write + Send>,
}

// ========================================================================
// Capability detection
// ========================================================================

/// Whether the current terminal is known to understand TBP.
///
/// For now this is environment-based: Winter exports `WINTER` (and sets
/// `TERM_PROGRAM=winter`).
pub fn supported() -> bool {
    if env::var(WINTER_ENV).is_ok_and(|v| !v.is_empty()) {
        return true;
    }
    env::var(TERM_PROGRAM_ENV).is_ok_and(|v| v == WINTER_NAME)
}

// ========================================================================
// Emission
// ========================================================================

/// Render an HTML fragment inline.
pub fn display_html(html: &str, opts: DisplayOptions) -> io::Result<()> {
    let mut bundle = MimeBundle::new();
    bundle.insert("text/html", Value::from(html));
    bundle.insert(TEXT_PLAIN, Value::from(opts.text.unwrap_or("[html block]")));
    emit(bundle, opts, &mut io::stdout())
}

/// Render an SVG document inline.
pub fn display_svg(svg: &str, opts: DisplayOptions) -> io::Result<()> {
    let mut bundle = MimeBundle::new();
    bundle.insert(SVG_MIME, Value::from(svg));
    bundle.insert(TEXT_PLAIN, Value::from(opts.text.unwrap_or("[svg image]")));
    emit(bundle, opts, &mut io::stdout())
}

/// Render Markdown inline. The raw Markdown is the text fallback by default.
pub fn display_markdown(markdown: &str, opts: DisplayOptions) -> io::Result<()> {
    let mut bundle = MimeBundle::new();
    bundle.insert("text/markdown", Value::from(markdown));
    bundle.insert(TEXT_PLAIN, Value::from(opts.text.unwrap_or(markdown)));
    emit(bundle, opts, &mut io::stdout())
}

/// Render a LaTeX expression inline. The raw source is the text fallback
/// by default.
pub fn display_latex(latex: &str, opts: DisplayOptions) -> io::Result<()> {
    let mut bundle = MimeBundle::new();
    bundle.insert("text/latex", Value::from(latex));
    bundle.insert(TEXT_PLAIN, Value::from(opts.text.unwrap_or(latex)));
    emit(bundle, opts, &mut io::stdout())
}

/// Render an image read from `path`. The MIME type is inferred from the
/// file extension.
pub fn display_image(path: impl AsRef<Path>, opts: DisplayOptions) -> io::Result<()> {
    let path = path.as_ref();
    let data = fs::read(path)?;
    let mime = mime_from_extension(path).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("cannot infer MIME type from suffix {:?}", path.extension()),
        )
    })?;
    display_image_bytes(&data, mime, opts)
}

/// Render an image from raw bytes with an explicit MIME type.
pub fn display_image_bytes(data: &[u8], mime: &str, opts: DisplayOptions) -> io::Result<()> {
    let payload = if mime == SVG_MIME {
        String::from_utf8(data.to_vec())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
    } else {
        BASE64_STANDARD.encode(data)
    };
    let fallback = opts
        .text
        .map(str::to_string)
        .unwrap_or_else(|| format!("[{mime} image, {} bytes]", data.len()));
    let mut bundle = MimeBundle::new();
    bundle.insert(mime, Value::from(payload));
    bundle.insert(TEXT_PLAIN, Value::from(fallback));
    emit(bundle, opts, &mut io::stdout())
}

fn emit(mut bundle: MimeBundle, opts: DisplayOptions, out: &mut dyn Write) -> io::Result<()> {
    bundle.meta.height_hint = opts.height_hint;
    bundle.meta.title = opts.title.map(str::to_string);

    if !supported() {
        if let Some(text) = bundle.text_plain() {
            out.write_all(text.as_bytes())?;
        }
        return out.flush();
    }
    let message = Message::Emit(EmitBlock {
        bundle,
        id: next_block_id(),
        trust: opts.trust,
    });
    out.write_all(winter_proto::encode(&message).as_bytes())?;
    out.flush()
}

fn mime_from_extension(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "gif" => Some("image/gif"),
        "jpeg" | "jpg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "svg" => Some(SVG_MIME),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

// ========================================================================
// Live blocks
// ========================================================================

/// Open a live block, writing to stdout, and return a handle for streaming
/// updates. `spec` is the block's initial state.
pub fn live_block(mime: &str, spec: impl Serialize) -> io::Result<LiveBlock> {
    live_block_to(mime, spec, None, Box::new(io::stdout()))
}

/// Open a live block, writing to `stream` and showing `text` once when
/// Winter is not the active terminal, and return a handle for streaming
/// updates.
pub fn live_block_to(
    mime: &str,
    spec: impl Serialize,
    text: Option<&str>,
    mut stream: Box<dyn Write + Send>,
) -> io::Result<LiveBlock> {
    if !supported() {
        stream.write_all(text.unwrap_or("").as_bytes())?;
        stream.flush()?;
        return Ok(LiveBlock {
            id: BlockId(0),
            live: false,
            mime: mime.to_string(),
            stream,
        });
    }
    let id = next_block_id();
    let spec = serde_json::to_value(spec).map_err(io::Error::other)?;
    let message = Message::Open(OpenBlock {
        id,
        mime: mime.to_string(),
        spec,
    });
    stream.write_all(winter_proto::encode(&message).as_bytes())?;
    stream.flush()?;
    Ok(LiveBlock {
        id,
        live: true,
        mime: mime.to_string(),
        stream,
    })
}

impl LiveBlock {
    /// The block id correlating this handle's `open`/`patch`/`close` frames.
    pub fn id(&self) -> BlockId {
        self.id
    }

    /// The block's MIME type, as given to [`live_block`].
    pub fn mime(&self) -> &str {
        &self.mime
    }

    /// Replace the block's whole spec with one patch.
    pub fn update(&mut self, spec: impl Serialize) -> io::Result<()> {
        let value = serde_json::to_value(spec).map_err(io::Error::other)?;
        self.patch_ops(&[serde_json::json!({"op": "add", "path": "", "value": value})])
    }

    /// Patch from `old`'s shape to `new`'s with a minimal diff.
    ///
    /// Object keys are added/removed/replaced (recursing into matching
    /// keys, so a change to one nested field emits one small op instead of
    /// replacing the whole object); an array that only grew a tail emits
    /// one `add` per appended item; anything else emits a single
    /// `replace`.
    pub fn update_from(&mut self, old: &Value, new: &Value) -> io::Result<()> {
        let ops = diff::diff(old, new, "");
        if ops.is_empty() {
            return Ok(());
        }
        self.patch_ops(&ops)
    }

    /// Apply RFC 6902 operations to the block's current spec.
    pub fn patch_ops(&mut self, ops: &[Value]) -> io::Result<()> {
        if !self.live {
            return Ok(());
        }
        let message = Message::Patch(winter_proto::PatchBlock {
            id: self.id,
            patch: Value::Array(ops.to_vec()),
        });
        self.stream
            .write_all(winter_proto::encode(&message).as_bytes())?;
        self.stream.flush()
    }

    /// End the block; the terminal freezes its last state.
    pub fn close(&mut self) -> io::Result<()> {
        if !self.live {
            return Ok(());
        }
        self.stream
            .write_all(winter_proto::encode(&Message::Close(self.id)).as_bytes())?;
        self.stream.flush()?;
        self.live = false;
        Ok(())
    }
}

fn next_block_id() -> BlockId {
    BlockId(NEXT_BLOCK_ID.fetch_add(1, Ordering::Relaxed))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use winter_proto::PatchBlock;

    use super::*;

    /// Serializes every test that reads or writes the `WINTER`/`TERM_PROGRAM`
    /// env vars: `cargo test` runs tests in parallel threads, and these vars
    /// are process-global state, not per-test.
    static ENV_GUARD: Mutex<()> = Mutex::new(());

    /// A `Write` sink that stays readable after being moved into a
    /// [`LiveBlock`], by sharing its buffer through an `Arc<Mutex<_>>`.
    #[derive(Clone, Default)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl SharedBuf {
        /// The bytes written since the last call, draining the buffer -
        /// mirrors `client-py`'s tests truncating an `io.StringIO` between
        /// assertions.
        fn take(&self) -> Vec<u8> {
            std::mem::take(&mut self.0.lock().unwrap())
        }
    }

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn strip_frame(escape: &str) -> &str {
        escape
            .strip_prefix("\x1b]")
            .and_then(|rest| rest.strip_suffix("\x1b\\"))
            .expect("encoded escape is OSC-framed")
    }

    fn decode_one(bytes: &[u8]) -> Message {
        let text = std::str::from_utf8(bytes).expect("escape is UTF-8");
        winter_proto::decode(strip_frame(text)).expect("re-decodes")
    }

    #[test]
    fn test_open_frames_mime_and_spec() {
        let _guard = ENV_GUARD.lock().unwrap();
        env::set_var(WINTER_ENV, "1");

        let buf = SharedBuf::default();
        let block = live_block_to("text/markdown", "# v0", None, Box::new(buf.clone())).unwrap();

        match decode_one(&buf.take()) {
            Message::Open(open) => {
                assert_eq!(open.id, block.id());
                assert_eq!(open.mime, "text/markdown");
                assert_eq!(open.spec, Value::from("# v0"));
            }
            other => panic!("expected Open, got {other:?}"),
        }
    }

    #[test]
    fn test_update_emits_a_root_add_patch() {
        let _guard = ENV_GUARD.lock().unwrap();
        env::set_var(WINTER_ENV, "1");

        let buf = SharedBuf::default();
        let mut block =
            live_block_to("text/markdown", "# v0", None, Box::new(buf.clone())).unwrap();
        buf.take();

        block.update("# v1").unwrap();

        match decode_one(&buf.take()) {
            Message::Patch(PatchBlock { id, patch }) => {
                assert_eq!(id, block.id());
                assert_eq!(
                    patch,
                    serde_json::json!([{"op": "add", "path": "", "value": "# v1"}])
                );
            }
            other => panic!("expected Patch, got {other:?}"),
        }
    }

    #[test]
    fn test_patch_ops_pass_through() {
        let _guard = ENV_GUARD.lock().unwrap();
        env::set_var(WINTER_ENV, "1");

        let buf = SharedBuf::default();
        let mut block = live_block_to(
            "application/vnd.vega-lite+json",
            serde_json::json!({"values": [1]}),
            None,
            Box::new(buf.clone()),
        )
        .unwrap();
        buf.take();

        let ops = [serde_json::json!({"op": "add", "path": "/values/-", "value": 2})];
        block.patch_ops(&ops).unwrap();

        match decode_one(&buf.take()) {
            Message::Patch(PatchBlock { patch, .. }) => {
                assert_eq!(patch, serde_json::json!(ops));
            }
            other => panic!("expected Patch, got {other:?}"),
        }
    }

    #[test]
    fn test_update_from_round_trips_through_patch_ops() {
        let _guard = ENV_GUARD.lock().unwrap();
        env::set_var(WINTER_ENV, "1");

        let buf = SharedBuf::default();
        let mut block = live_block_to(
            "application/json",
            serde_json::json!({"count": 1}),
            None,
            Box::new(buf.clone()),
        )
        .unwrap();
        buf.take();

        block
            .update_from(
                &serde_json::json!({"count": 1}),
                &serde_json::json!({"count": 2}),
            )
            .unwrap();

        match decode_one(&buf.take()) {
            Message::Patch(PatchBlock { patch, .. }) => {
                assert_eq!(
                    patch,
                    serde_json::json!([{"op": "replace", "path": "/count", "value": 2}])
                );
            }
            other => panic!("expected Patch, got {other:?}"),
        }
    }

    #[test]
    fn test_update_from_writes_nothing_when_the_values_are_equal() {
        let _guard = ENV_GUARD.lock().unwrap();
        env::set_var(WINTER_ENV, "1");

        let buf = SharedBuf::default();
        let mut block = live_block_to(
            "application/json",
            serde_json::json!({"count": 1}),
            None,
            Box::new(buf.clone()),
        )
        .unwrap();
        buf.take();

        block
            .update_from(
                &serde_json::json!({"count": 1}),
                &serde_json::json!({"count": 1}),
            )
            .unwrap();

        assert!(
            buf.take().is_empty(),
            "an unchanged value must not emit an empty patch frame"
        );
    }

    #[test]
    fn test_close_is_terminal_and_idempotent() {
        let _guard = ENV_GUARD.lock().unwrap();
        env::set_var(WINTER_ENV, "1");

        let buf = SharedBuf::default();
        let mut block = live_block_to("text/plain", "x", None, Box::new(buf.clone())).unwrap();
        buf.take();

        block.close().unwrap();
        assert!(matches!(decode_one(&buf.take()), Message::Close(_)));

        block.close().unwrap();
        block.update("ignored").unwrap();
        assert!(
            buf.take().is_empty(),
            "a closed handle must write nothing further"
        );
    }

    #[test]
    fn test_two_blocks_get_distinct_ids() {
        let _guard = ENV_GUARD.lock().unwrap();
        env::set_var(WINTER_ENV, "1");

        let a = live_block_to("text/plain", "a", None, Box::new(SharedBuf::default())).unwrap();
        let b = live_block_to("text/plain", "b", None, Box::new(SharedBuf::default())).unwrap();
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn test_fallback_outside_winter_writes_text_once() {
        let _guard = ENV_GUARD.lock().unwrap();
        env::remove_var(WINTER_ENV);
        env::remove_var(TERM_PROGRAM_ENV);

        let buf = SharedBuf::default();
        let mut block = live_block_to(
            "text/markdown",
            "# v0",
            Some("just text"),
            Box::new(buf.clone()),
        )
        .unwrap();
        block.update("# v1").unwrap();
        block.close().unwrap();

        assert_eq!(buf.take(), b"just text");
    }

    #[test]
    fn test_emit_outside_winter_writes_the_text_plain_fallback() {
        let _guard = ENV_GUARD.lock().unwrap();
        env::remove_var(WINTER_ENV);
        env::remove_var(TERM_PROGRAM_ENV);

        let mut bundle = MimeBundle::new();
        bundle.insert("text/html", Value::from("<b>hi</b>"));
        bundle.insert(TEXT_PLAIN, Value::from("[html block]"));
        let mut out: Vec<u8> = Vec::new();
        emit(bundle, DisplayOptions::default(), &mut out).unwrap();

        assert_eq!(out, b"[html block]");
    }

    #[test]
    fn test_emit_inside_winter_frames_the_bundle_with_meta() {
        let _guard = ENV_GUARD.lock().unwrap();
        env::set_var(WINTER_ENV, "1");

        let mut bundle = MimeBundle::new();
        bundle.insert("text/html", Value::from("<b>hi</b>"));
        let mut out: Vec<u8> = Vec::new();
        let opts = DisplayOptions {
            title: Some("Report"),
            ..Default::default()
        };
        emit(bundle, opts, &mut out).unwrap();

        match decode_one(&out) {
            Message::Emit(block) => {
                assert_eq!(block.bundle.meta.title.as_deref(), Some("Report"));
                assert_eq!(
                    block.bundle.get("text/html"),
                    Some(&Value::from("<b>hi</b>"))
                );
                assert_eq!(block.trust, TrustTier::Restricted);
            }
            other => panic!("expected Emit, got {other:?}"),
        }
    }

    #[test]
    fn test_mime_from_extension_recognizes_common_image_types() {
        assert_eq!(
            mime_from_extension(Path::new("chart.png")),
            Some("image/png")
        );
        assert_eq!(mime_from_extension(Path::new("chart.SVG")), Some(SVG_MIME));
        assert_eq!(mime_from_extension(Path::new("chart.bmp")), None);
        assert_eq!(mime_from_extension(Path::new("chart")), None);
    }
}
