//! The block-list scrollback model: typed output units.
//!
//! Scrollback is a list of [`CommandBlock`]s, not a flat line buffer. Each block
//! holds one command's output as an ordered sequence of [`Segment`]s, where a
//! segment is either plain terminal text or a rich content block emitted via TBP.

use serde::Serialize;
use winter_proto::{EmitBlock, OpenBlock};

// ============================================================================
// Data Structures
// ============================================================================

/// One command's output unit: the command line, its working directory, exit
/// status, and the ordered segments it produced.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CommandBlock {
    /// The command line as the shell reported it (OSC 133 `B`..`C` region).
    /// Empty when shell integration is absent.
    pub command: String,
    /// Working directory the command ran in, from OSC 7. `None` when the
    /// shell does not report it.
    pub cwd: Option<String>,
    /// Exit status from OSC 133 `D`. `None` while the command is still
    /// running, or when the shell does not report it.
    pub exit_code: Option<i32>,
    /// True once this block's output has been dropped to keep the session's
    /// retained bytes under budget. The block itself is kept so its index
    /// stays a stable identifier for callers that recorded one.
    pub elided: bool,
    /// Everything the command wrote, in emission order. Empty when
    /// [`Self::elided`] is set.
    pub output: Vec<Segment>,
}

/// One run of output within a command block.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase", tag = "kind", content = "data")]
pub enum Segment {
    /// A rich block emitted via TBP (`OSC 9001 ; emit`).
    Content(EmitBlock),
    /// A hyperlinked run of text (`OSC 8`).
    Link(LinkSpan),
    /// A live block opened via TBP (`OSC 9001 ; open`), updated via `patch`.
    Live(LiveBlock),
    /// Plain terminal text.
    Text(String),
}

/// A live block that can be updated in-place via `patch` messages.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LiveBlock {
    /// Set once a `close` message arrives; the block keeps its last state
    /// and stops accepting further patches.
    pub closed: bool,
    /// The emitter-chosen id `patch` and `close` messages refer back to.
    pub id: winter_proto::BlockId,
    /// The `open` message's spec, before any patch is folded in.
    pub initial: OpenBlock,
    /// Patches received so far, in arrival order. Capped by the scrollback's
    /// per-block patch limit so a fast-streaming emitter cannot grow this
    /// without bound.
    pub patches: Vec<serde_json::Value>,
}

impl LiveBlock {
    /// The block's current state: the initial spec with every received
    /// patch folded in.
    ///
    /// Folding is best-effort (`crate::patch`): a malformed operation is
    /// skipped rather than freezing the display at the initial state — the
    /// pragmatic reading of RFC 6902's atomicity for a streaming terminal.
    pub fn current_spec(&self) -> serde_json::Value {
        let mut spec = self.initial.spec.clone();
        for patch in &self.patches {
            crate::patch::apply(&mut spec, patch);
        }
        spec
    }
}

/// A run of text carrying an OSC 8 hyperlink target.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LinkSpan {
    pub text: String,
    pub url: String,
}

// ============================================================================
// CommandBlock
// ============================================================================

impl CommandBlock {
    /// Append text to the output, coalescing with a trailing text segment so a
    /// run of `print` calls does not fragment into one segment per character.
    pub(crate) fn append_text(&mut self, text: &str) {
        match self.output.last_mut() {
            Some(Segment::Text(buf)) => buf.push_str(text),
            Some(Segment::Content(_)) | Some(Segment::Link(_)) | Some(Segment::Live(_)) | None => {
                self.output.push(Segment::Text(text.to_string()))
            }
        }
    }

    /// Append hyperlinked text, coalescing with a trailing link to the same URL.
    pub(crate) fn append_link(&mut self, text: &str, url: &str) {
        match self.output.last_mut() {
            Some(Segment::Link(span)) if span.url == url => span.text.push_str(text),
            Some(Segment::Content(_))
            | Some(Segment::Link(_))
            | Some(Segment::Live(_))
            | Some(Segment::Text(_))
            | None => self.output.push(Segment::Link(LinkSpan {
                text: text.to_string(),
                url: url.to_string(),
            })),
        }
    }

    /// Whether the block has captured neither a command line nor any output, so
    /// it can be reused rather than leaving an empty block in the scrollback.
    pub(crate) fn is_empty(&self) -> bool {
        self.command.is_empty() && self.output.is_empty()
    }

    /// Bytes of payload this block holds, used to keep a session's total
    /// retained output under budget. Counts the text a segment carries, not
    /// the `Vec`/`String` overhead around it, so it is an estimate of the
    /// dominant term rather than an exact heap measurement.
    pub(crate) fn retained_bytes(&self) -> usize {
        let mut total = self.command.len();
        for segment in &self.output {
            total += match segment {
                Segment::Content(emit) => emit
                    .bundle
                    .mime
                    .iter()
                    .map(|(mime, value)| mime.len() + value.to_string().len())
                    .sum(),
                Segment::Link(span) => span.text.len() + span.url.len(),
                Segment::Live(live) => {
                    live.initial.spec.to_string().len()
                        + live
                            .patches
                            .iter()
                            .map(|p| p.to_string().len())
                            .sum::<usize>()
                }
                Segment::Text(text) => text.len(),
            };
        }
        total
    }

    /// Drop this block's output, keeping its command line, cwd, and exit
    /// status so the block still reads as a navigable history entry.
    pub(crate) fn elide(&mut self) {
        self.elided = true;
        self.output.clear();
    }

    /// How many terminal rows this block occupies, given a column width.
    pub fn row_count(&self, cols: usize) -> usize {
        let text = self.plain_text();
        if text.is_empty() {
            return if self.command.is_empty() { 0 } else { 1 };
        }
        let mut rows = 0;
        for line in text.split('\n') {
            if cols == 0 {
                rows += 1;
            } else {
                rows += line.len().max(1).div_ceil(cols);
            }
        }
        rows.max(1)
    }

    /// All plain text in this block's output, concatenated.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for segment in &self.output {
            if let Segment::Text(text) = segment {
                out.push_str(text);
            }
        }
        out
    }

    /// Look for an SVG content segment in this block's output.
    pub fn svg_content(&self) -> Option<String> {
        for segment in &self.output {
            if let Segment::Content(emit) = segment {
                if let Some(svg) = emit.bundle.get("image/svg+xml") {
                    if let Some(s) = svg.as_str() {
                        return Some(s.to_string());
                    }
                }
            }
        }
        None
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use winter_proto::{BlockId, EmitBlock, MimeBundle, TrustTier};

    use super::*;

    #[test]
    fn test_append_text_coalesces_consecutive_prints() {
        let mut block = CommandBlock::default();
        block.append_text("hel");
        block.append_text("lo");
        assert_eq!(block.output, vec![Segment::Text("hello".to_string())]);
    }

    #[test]
    fn test_append_text_separates_after_content() {
        let mut block = CommandBlock::default();
        block.append_text("before");
        let mut bundle = MimeBundle::new();
        bundle.insert("text/plain", serde_json::Value::from("img"));
        block.output.push(Segment::Content(EmitBlock {
            bundle,
            id: BlockId(1),
            trust: TrustTier::Restricted,
        }));
        block.append_text("after");
        assert_eq!(block.output.len(), 3);
        assert_eq!(&block.output[0], &Segment::Text("before".to_string()));
        assert_eq!(&block.output[2], &Segment::Text("after".to_string()));
    }

    #[test]
    fn test_append_link_coalesces_same_url() {
        let mut block = CommandBlock::default();
        block.append_link("click ", "https://example.com");
        block.append_link("here", "https://example.com");
        assert_eq!(
            block.output,
            vec![Segment::Link(LinkSpan {
                text: "click here".to_string(),
                url: "https://example.com".to_string(),
            })]
        );
    }

    #[test]
    fn test_append_link_separates_different_urls() {
        let mut block = CommandBlock::default();
        block.append_link("a", "https://a.com");
        block.append_link("b", "https://b.com");
        assert_eq!(block.output.len(), 2);
    }

    #[test]
    fn test_is_empty_on_fresh_block() {
        let block = CommandBlock::default();
        assert!(block.is_empty());
    }

    #[test]
    fn test_is_not_empty_after_appending_text() {
        let mut block = CommandBlock::default();
        block.append_text("x");
        assert!(!block.is_empty());
    }

    #[test]
    fn test_row_count_single_line() {
        let mut block = CommandBlock::default();
        block.append_text("hello");
        assert_eq!(block.row_count(80), 1);
    }

    #[test]
    fn test_row_count_wraps_long_line() {
        let mut block = CommandBlock::default();
        block.append_text("abcdefgh");
        assert_eq!(block.row_count(4), 2);
    }

    #[test]
    fn test_row_count_multi_line() {
        let mut block = CommandBlock::default();
        block.append_text("abc\ndef\nghi");
        assert_eq!(block.row_count(80), 3);
    }

    #[test]
    fn test_row_count_empty_block() {
        let block = CommandBlock::default();
        assert_eq!(block.row_count(80), 0);
    }

    #[test]
    fn test_row_count_command_only() {
        let block = CommandBlock {
            command: "ls".to_string(),
            ..CommandBlock::default()
        };
        assert_eq!(block.row_count(80), 1);
    }

    #[test]
    fn test_svg_content_finds_svg_segment() {
        let mut block = CommandBlock::default();
        assert_eq!(block.svg_content(), None);

        let mut bundle = MimeBundle::new();
        bundle.insert("image/svg+xml", serde_json::Value::from("<svg></svg>"));
        block.output.push(Segment::Content(EmitBlock {
            id: BlockId(1),
            bundle,
            trust: TrustTier::Trusted,
        }));
        assert_eq!(block.svg_content().as_deref(), Some("<svg></svg>"));
    }

    #[test]
    fn test_live_block_current_spec_folds_patches() {
        use winter_proto::OpenBlock;

        let live = LiveBlock {
            closed: false,
            id: BlockId(7),
            initial: OpenBlock {
                id: BlockId(7),
                mime: "application/vnd.vega-lite+json".to_string(),
                spec: serde_json::json!({"values": [1]}),
            },
            patches: vec![
                serde_json::json!([{"op": "add", "path": "/values/-", "value": 2}]),
                serde_json::json!([{"op": "add", "path": "/values/-", "value": 3}]),
            ],
        };
        assert_eq!(
            live.current_spec(),
            serde_json::json!({"values": [1, 2, 3]})
        );

        // No patches: the initial spec, unchanged.
        let fresh = LiveBlock {
            closed: false,
            id: BlockId(8),
            initial: OpenBlock {
                id: BlockId(8),
                mime: "text/plain".to_string(),
                spec: serde_json::json!("hello"),
            },
            patches: Vec::new(),
        };
        assert_eq!(fresh.current_spec(), serde_json::json!("hello"));
    }
}
