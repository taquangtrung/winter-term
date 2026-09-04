//! Tracks rich content blocks surfaced from the core block parser.

use std::collections::HashMap;

use winter_core::winter_proto::{EmitBlock, MimeBundle, TrustTier};
use winter_core::{CommandBlock, LiveBlock, Scrollback, Segment};

// ========================================================================
// Data Structures
// ========================================================================

/// A snapshot of all content blocks currently in the scrollback, with their
/// block-list indices for tracking new arrivals.
#[derive(Clone, Debug, Default)]
pub struct BlockQueue {
    entries: Vec<BlockEntry>,
    known_patches: HashMap<usize, usize>,
    /// Ceiling every entry's requested trust tier is clamped to. Defaults to
    /// the safest tier so a queue built without consulting config can only
    /// ever under-grant; [`BlockQueue::set_max_trust`] raises it from policy.
    max_trust: TrustTier,
    scanned_segments: Vec<usize>,
}

/// One rich block together with where it was anchored in the grid.
#[derive(Clone, Debug)]
pub struct BlockEntry {
    pub block_index: usize,
    /// Set once the live block's `close` has been seen; always `false` for
    /// a one-shot `Content` block.
    pub closed: bool,
    pub emit: EmitBlock,
    pub grid_row: usize,
    pub kind: BlockKind,
    pub segment_index: usize,
    /// The tier this block is actually granted: already clamped against the
    /// configured policy ceiling. Consumers may act on this directly.
    ///
    /// Never read `emit.trust` instead; that is the raw, attacker-controlled
    /// tier the wire asked for.
    pub trust: TrustTier,
}

/// Distinguishes one-shot content blocks from live blocks that accept patches.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockKind {
    Content,
    Live,
}

/// A live block as an emittable snapshot: a single-mime bundle carrying the
/// block's *current* state — the initial spec with every patch folded in —
/// so downstream renderers can treat it like any other block. Live blocks
/// are always Restricted: their content changes after open time, so the
/// trust decision is re-made per render, not granted up front.
fn live_emit(live: &LiveBlock) -> EmitBlock {
    let mut bundle = MimeBundle::new();
    bundle.insert(&live.initial.mime, live.current_spec());
    EmitBlock {
        bundle,
        id: live.id,
        trust: TrustTier::Restricted,
    }
}

// ========================================================================
// BlockQueue
// ========================================================================

impl BlockQueue {
    /// An empty queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Raise the trust ceiling applied to blocks queued from here on, from the
    /// user's `security.block-max-trust` policy.
    ///
    /// Entries already queued keep the ceiling in force when they arrived, so
    /// lowering the policy at runtime does not retroactively re-grant them a
    /// tier they were never rendered at.
    pub fn set_max_trust(&mut self, max_trust: TrustTier) {
        self.max_trust = max_trust;
    }

    /// Scan the scrollback for content segments not yet seen and append them.
    /// `anchors` gives the grid row each new block was reserved at, in emission
    /// order; `fallback_row` is used if anchors run short.
    pub fn update(&mut self, scrollback: &Scrollback, fallback_row: usize, anchors: &[usize]) {
        let mut anchors = anchors.iter();
        let blocks = scrollback.blocks();
        for (block_index, block) in blocks.iter().enumerate() {
            if block_index >= self.scanned_segments.len() {
                self.scanned_segments.push(0);
            }
            let start = self.scanned_segments[block_index];
            for (segment_index, segment) in block.output.iter().enumerate().skip(start) {
                match segment {
                    Segment::Content(emit) => {
                        self.entries.push(BlockEntry {
                            block_index,
                            closed: false,
                            emit: emit.clone(),
                            grid_row: anchors.next().copied().unwrap_or(fallback_row),
                            kind: BlockKind::Content,
                            segment_index,
                            trust: emit.trust.clamp_to(self.max_trust),
                        });
                    }
                    Segment::Live(live) => {
                        self.entries.push(BlockEntry {
                            block_index,
                            closed: live.closed,
                            emit: live_emit(live),
                            grid_row: anchors.next().copied().unwrap_or(fallback_row),
                            kind: BlockKind::Live,
                            segment_index,
                            trust: TrustTier::Restricted.clamp_to(self.max_trust),
                        });
                    }
                    Segment::Link(_) | Segment::Text(_) => {}
                }
            }
            self.scanned_segments[block_index] = block.output.len();
        }
        self.release_elided(blocks);
    }

    /// Drop the payload of entries whose source block has been elided by the
    /// scrollback's retention budget.
    ///
    /// The block's content is gone, so the entry can never render again; what
    /// is left is a cloned MIME bundle, which for an image block is megabytes.
    /// The entry itself stays because its position in `entries` is an
    /// identifier [`Self::drain_patched_live`] hands out.
    fn release_elided(&mut self, blocks: &[CommandBlock]) {
        for entry in &mut self.entries {
            let elided = blocks.get(entry.block_index).is_some_and(|b| b.elided);
            if elided && !entry.emit.bundle.mime.is_empty() {
                entry.emit.bundle = MimeBundle::new();
            }
        }
    }

    /// Every queued block, in the order it was emitted.
    pub fn entries(&self) -> &[BlockEntry] {
        &self.entries
    }

    /// Shift every entry anchored at or below `row` down by `delta` rows:
    /// a band above them grew, and their anchors must keep pointing at the
    /// same content for the next placement pass.
    pub fn shift_rows_at_or_below(&mut self, row: usize, delta: usize) {
        for entry in &mut self.entries {
            if entry.grid_row >= row {
                entry.grid_row += delta;
            }
        }
    }

    /// Returns indices of live-block entries that have accumulated new
    /// patches, or just closed, since the last call, refreshing each
    /// entry's snapshot (and `closed` flag) so the next render shows new
    /// content. Each call resets the tracked patch count.
    pub fn drain_patched_live(&mut self, blocks: &[CommandBlock]) -> Vec<usize> {
        let mut patched = Vec::new();
        for (idx, entry) in self.entries.iter_mut().enumerate() {
            if entry.kind != BlockKind::Live {
                continue;
            }
            if let Some(block) = blocks.get(entry.block_index) {
                if let Some(Segment::Live(live)) = block.output.get(entry.segment_index) {
                    let patches_grew =
                        live.patches.len() > self.known_patches.get(&idx).copied().unwrap_or(0);
                    let just_closed = live.closed && !entry.closed;
                    if patches_grew || just_closed {
                        self.known_patches.insert(idx, live.patches.len());
                        entry.emit = live_emit(live);
                        entry.closed = live.closed;
                        patched.push(idx);
                    }
                }
            }
        }
        patched
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use winter_core::winter_proto::{BlockId, EmitBlock, MimeBundle, TrustTier};
    use winter_core::Terminal;

    use super::*;

    fn svg_emit() -> EmitBlock {
        let mut bundle = MimeBundle::new();
        bundle.insert("image/svg+xml", Value::from("<svg/>"));
        bundle.insert("text/plain", Value::from("[svg]"));
        EmitBlock {
            bundle,
            id: BlockId(1),
            trust: TrustTier::Restricted,
        }
    }

    fn emit_escape(block: &EmitBlock) -> Vec<u8> {
        winter_core::winter_proto::encode(&winter_core::winter_proto::Message::Emit(block.clone()))
            .into_bytes()
    }

    fn trusted_emit() -> EmitBlock {
        let mut bundle = MimeBundle::new();
        bundle.insert("text/html", Value::from("<b>hi</b>"));
        EmitBlock {
            bundle,
            id: BlockId(9),
            trust: TrustTier::Trusted,
        }
    }

    #[test]
    fn test_wire_requested_trust_is_clamped_to_the_default_ceiling() {
        // Security regression: the tier was copied verbatim off the wire, so
        // any byte stream reaching a PTY (a `cat` of a downloaded file, output
        // piped from `curl`, a host behind ssh) could ask for `trust=trusted`
        // and be granted a WebView with scripting on and no CSP.
        let mut term = Terminal::new();
        term.feed(&emit_escape(&trusted_emit()));

        let mut queue = BlockQueue::new();
        queue.update(term.scrollback(), 0, &[]);

        assert_eq!(queue.entries()[0].emit.trust, TrustTier::Trusted);
        assert_eq!(
            queue.entries()[0].trust,
            TrustTier::Restricted,
            "a wire-requested Trusted tier must not survive the default policy"
        );
    }

    #[test]
    fn test_policy_can_raise_the_ceiling_to_trusted() {
        let mut term = Terminal::new();
        term.feed(&emit_escape(&trusted_emit()));

        let mut queue = BlockQueue::new();
        queue.set_max_trust(TrustTier::Trusted);
        queue.update(term.scrollback(), 0, &[]);

        assert_eq!(queue.entries()[0].trust, TrustTier::Trusted);
    }

    #[test]
    fn test_policy_ceiling_never_raises_a_weaker_request() {
        let mut term = Terminal::new();
        let mut block = trusted_emit();
        block.trust = TrustTier::Isolated;
        term.feed(&emit_escape(&block));

        let mut queue = BlockQueue::new();
        queue.set_max_trust(TrustTier::Trusted);
        queue.update(term.scrollback(), 0, &[]);

        assert_eq!(queue.entries()[0].trust, TrustTier::Isolated);
    }

    #[test]
    fn test_empty_queue_has_no_entries() {
        let queue = BlockQueue::new();
        assert!(queue.entries().is_empty());
    }

    #[test]
    fn test_update_collects_content_segments() {
        let mut term = Terminal::new();
        term.feed(b"before");
        term.feed(&emit_escape(&svg_emit()));
        term.feed(b"after");

        let mut queue = BlockQueue::new();
        queue.update(term.scrollback(), 0, &[]);
        assert_eq!(queue.entries().len(), 1);
        assert_eq!(queue.entries()[0].block_index, 0);
        assert_eq!(queue.entries()[0].segment_index, 1);
    }

    #[test]
    fn test_update_is_incremental() {
        let mut term = Terminal::new();
        term.feed(&emit_escape(&svg_emit()));

        let mut queue = BlockQueue::new();
        queue.update(term.scrollback(), 0, &[]);
        assert_eq!(queue.entries().len(), 1);

        let mut second = svg_emit();
        second.id = BlockId(2);
        term.feed(&emit_escape(&second));
        queue.update(term.scrollback(), 0, &[]);
        assert_eq!(queue.entries().len(), 2);
    }

    #[test]
    fn test_update_surfaces_live_blocks() {
        use winter_core::winter_proto::{Message, OpenBlock};

        let open = Message::Open(OpenBlock {
            id: BlockId(99),
            mime: "text/markdown".to_string(),
            spec: serde_json::json!("# live"),
        });
        let esc = winter_core::winter_proto::encode(&open);

        let mut term = Terminal::new();
        term.feed(esc.as_bytes());

        let mut queue = BlockQueue::new();
        queue.update(term.scrollback(), 5, &[]);
        assert_eq!(queue.entries().len(), 1);
        assert_eq!(queue.entries()[0].kind, BlockKind::Live);
        assert_eq!(queue.entries()[0].grid_row, 5);
    }

    #[test]
    fn test_live_entry_bundle_carries_the_spec_value_directly() {
        // Regression: the initial spec was stringified into the bundle
        // (`Value::String(spec.to_string())`), so a markdown spec rendered
        // as a JSON-quoted string and an object spec (vega) never survived
        // `as_str()` lookups downstream.
        use winter_core::winter_proto::{Message, OpenBlock};

        let open = Message::Open(OpenBlock {
            id: BlockId(7),
            mime: "application/vnd.vega-lite+json".to_string(),
            spec: serde_json::json!({"mark": "bar", "data": {"values": []}}),
        });
        let mut term = Terminal::new();
        term.feed(winter_core::winter_proto::encode(&open).as_bytes());

        let mut queue = BlockQueue::new();
        queue.update(term.scrollback(), 0, &[]);
        let bundle = &queue.entries()[0].emit.bundle;
        assert_eq!(
            bundle.get("application/vnd.vega-lite+json"),
            Some(&serde_json::json!({"mark": "bar", "data": {"values": []}}))
        );
    }

    #[test]
    fn test_shift_rows_at_or_below_moves_only_later_entries() {
        // Growing a band must move the anchors below it — an anchor left
        // behind would draw the next block over the grown band's new rows.
        let mut term = Terminal::new();
        term.feed(&emit_escape(&svg_emit()));
        let mut second = svg_emit();
        second.id = BlockId(2);
        term.feed(&emit_escape(&second));

        let mut queue = BlockQueue::new();
        queue.update(term.scrollback(), 0, &[2, 5]);
        queue.shift_rows_at_or_below(5, 3);

        assert_eq!(queue.entries()[0].grid_row, 2);
        assert_eq!(queue.entries()[1].grid_row, 8);
    }

    #[test]
    fn test_patch_refreshes_the_live_entry_snapshot() {
        // Regression: `drain_patched_live` reported patched entries but left
        // their bundle frozen at the initial spec, so the app re-rendered the
        // same content forever — the update path existed but never showed
        // new bytes.
        use winter_core::winter_proto::{Message, OpenBlock, PatchBlock};

        let mut term = Terminal::new();
        term.feed(
            winter_core::winter_proto::encode(&Message::Open(OpenBlock {
                id: BlockId(42),
                mime: "text/markdown".to_string(),
                spec: serde_json::json!("v0"),
            }))
            .as_bytes(),
        );

        let mut queue = BlockQueue::new();
        queue.update(term.scrollback(), 0, &[]);
        assert_eq!(
            queue.entries()[0].emit.bundle.get("text/markdown"),
            Some(&serde_json::json!("v0"))
        );
        assert!(queue
            .drain_patched_live(term.scrollback().blocks())
            .is_empty());

        term.feed(
            winter_core::winter_proto::encode(&Message::Patch(PatchBlock {
                id: BlockId(42),
                patch: serde_json::json!([{"op": "replace", "path": "", "value": "v1"}]),
            }))
            .as_bytes(),
        );
        let patched = queue.drain_patched_live(term.scrollback().blocks());
        assert_eq!(patched, vec![0]);
        assert_eq!(
            queue.entries()[0].emit.bundle.get("text/markdown"),
            Some(&serde_json::json!("v1")),
            "the snapshot must carry the patched state, not the initial spec"
        );
        assert!(
            queue
                .drain_patched_live(term.scrollback().blocks())
                .is_empty(),
            "an unchanged patch count must not re-report"
        );
    }

    #[test]
    fn test_close_without_a_patch_still_reports_and_flags_the_entry() {
        // A bare `close()` never grows `patches`, so the old patch-count-only
        // check would silently miss it and the entry would never learn it
        // closed — the close affordance would never render.
        use winter_core::winter_proto::{Message, OpenBlock};

        let mut term = Terminal::new();
        term.feed(
            winter_core::winter_proto::encode(&Message::Open(OpenBlock {
                id: BlockId(43),
                mime: "text/plain".to_string(),
                spec: serde_json::json!("v0"),
            }))
            .as_bytes(),
        );
        let mut queue = BlockQueue::new();
        queue.update(term.scrollback(), 0, &[]);
        assert!(!queue.entries()[0].closed);

        term.feed(winter_core::winter_proto::encode(&Message::Close(BlockId(43))).as_bytes());
        let patched = queue.drain_patched_live(term.scrollback().blocks());
        assert_eq!(patched, vec![0]);
        assert!(queue.entries()[0].closed);

        assert!(
            queue
                .drain_patched_live(term.scrollback().blocks())
                .is_empty(),
            "an already-known close must not re-report"
        );
    }
}
