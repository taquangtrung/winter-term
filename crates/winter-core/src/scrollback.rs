//! The scrollback state machine: turns semantic terminal events into a block list.
//!
//! This type is independent of `vte` and the PTY so it can be driven directly in
//! tests. The [`crate::parser`] adapter translates byte-stream escapes into the
//! method calls below.

use std::collections::HashMap;

use winter_proto::{BlockId, EmitBlock, OpenBlock, PatchBlock};

use crate::block::{CommandBlock, LiveBlock, Segment};

// ============================================================================
// Constants
// ============================================================================

/// Hard cap on patches retained per live block. Without it, a tool streaming
/// frequent updates to one long-lived block (a progress bar, a log tail)
/// accumulates one entry per update for the block's whole lifetime, growing
/// memory without bound.
const MAX_LIVE_BLOCK_PATCHES: usize = 10_000;

/// Budget for the output bytes one session's block list retains. A terminal is
/// expected to stay open for days, so without a ceiling a pane that streams
/// output (a log tail, a chatty build) grows until the OOM killer intervenes:
/// the GPU grid caps its own scrollback rows (`winter_render::MAX_SCROLLBACK`),
/// but that cap never reached this block list.
///
/// Blocks past the budget are *elided* rather than removed: their output is
/// dropped but the block itself stays, because a block's index is a stable
/// identifier held by the app layer (folds, WebView tiles, image placements),
/// and removing entries would silently renumber every one of them.
const MAX_RETAINED_OUTPUT_BYTES: usize = 8 * 1024 * 1024;

/// How many appended bytes may accumulate before the retention budget is
/// re-measured. Measuring is a walk of every retained block, so it is amortized
/// over this much growth rather than run on every write.
const BUDGET_CHECK_INTERVAL_BYTES: usize = 1024 * 1024;

// ============================================================================
// Data Structures
// ============================================================================

/// A session's output, modeled as an ordered list of command blocks.
#[derive(Clone, Debug)]
pub struct Scrollback {
    active_link: Option<String>,
    blocks: Vec<CommandBlock>,
    cwd: Option<String>,
    /// Bytes appended since the retention budget was last measured.
    bytes_since_budget_check: usize,
    /// Index of the oldest block still holding its output. Everything below
    /// it has been elided, so eviction resumes here instead of rescanning.
    eviction_cursor: usize,
    live_indices: HashMap<BlockId, LiveIndex>,
    phase: Phase,
}

/// Tracks where a live block lives in the block list so `patch`/`close` can
/// find it without scanning.
#[derive(Clone, Copy, Debug)]
struct LiveIndex {
    block_index: usize,
    segment_index: usize,
}

/// Which region of a command the stream is currently writing into. Driven by
/// OSC 133 marks; absent those, the stream stays in [`Phase::Output`] and fills a
/// single rolling block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    /// Between the command-start and output-start marks: the command line itself.
    Input,
    /// Command output (and the default when shell integration is absent).
    Output,
    /// Between the prompt-start and command-start marks: the shell prompt.
    Prompt,
}

// ============================================================================
// Scrollback
// ============================================================================

impl Scrollback {
    /// A fresh scrollback holding one empty block, ready to capture output that
    /// precedes the first prompt.
    pub fn new() -> Self {
        Self {
            active_link: None,
            blocks: vec![CommandBlock::default()],
            bytes_since_budget_check: 0,
            cwd: None,
            eviction_cursor: 0,
            live_indices: HashMap::new(),
            phase: Phase::Output,
        }
    }

    /// The command blocks captured so far, oldest first.
    pub fn blocks(&self) -> &[CommandBlock] {
        &self.blocks
    }

    /// True when the shell is waiting for user input (OSC 133 prompt or command
    /// phase). False when a process is running or shell integration is absent.
    pub fn is_at_prompt(&self) -> bool {
        matches!(self.phase, Phase::Prompt | Phase::Input)
    }

    /// For each block, the starting row offset assuming the first block starts at
    /// row 0. Used by the app layer to scroll to a specific block boundary.
    pub fn block_row_offsets(&self, cols: usize) -> Vec<usize> {
        let mut offsets = Vec::with_capacity(self.blocks.len());
        let mut row = 0;
        for block in &self.blocks {
            offsets.push(row);
            row += block.row_count(cols);
        }
        offsets
    }

    /// Search blocks for `query` (case-insensitive) and return indices of matching
    /// blocks. Text segments are searched; content blocks' text fallbacks are included.
    pub fn search(&self, query: &str) -> Vec<usize> {
        let lower = query.to_lowercase();
        self.blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| block.plain_text().to_lowercase().contains(&lower))
            .map(|(i, _)| i)
            .collect()
    }

    /// The block list serialized as JSON, the boundary form consumed by the Node
    /// addon and the `dump_session` harness.
    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.blocks).expect("command blocks are always serializable")
    }

    /// All plain text across every block's output, concatenated in stream order.
    /// Content blocks are skipped. Useful for search and smoke tests.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            for segment in &block.output {
                if let Segment::Text(text) = segment {
                    out.push_str(text);
                }
            }
        }
        out
    }

    pub(crate) fn print(&mut self, text: &str) {
        match self.phase {
            Phase::Input => self.current_mut().command.push_str(text),
            Phase::Output => match self.active_link.clone() {
                Some(url) => self.current_mut().append_link(text, &url),
                None => self.current_mut().append_text(text),
            },
            Phase::Prompt => {}
        }
        self.note_growth(text.len());
    }

    pub(crate) fn line_break(&mut self) {
        if self.phase == Phase::Output {
            self.current_mut().append_text("\n");
            self.note_growth(1);
        }
    }

    pub(crate) fn prompt_start(&mut self) {
        if !self.current_mut().is_empty() {
            self.blocks.push(CommandBlock::default());
        }
        let cwd = self.cwd.clone();
        self.current_mut().cwd = cwd;
        self.active_link = None;
        self.phase = Phase::Prompt;
    }

    pub(crate) fn command_start(&mut self) {
        self.phase = Phase::Input;
    }

    pub(crate) fn output_start(&mut self) {
        self.phase = Phase::Output;
    }

    pub(crate) fn command_end(&mut self, exit_code: Option<i32>) {
        self.current_mut().exit_code = exit_code;
        self.phase = Phase::Output;
    }

    pub(crate) fn set_cwd(&mut self, cwd: String) {
        if self.current_mut().cwd.is_none() {
            self.current_mut().cwd = Some(cwd.clone());
        }
        self.cwd = Some(cwd);
    }

    pub(crate) fn emit(&mut self, block: EmitBlock) {
        let bytes = block
            .bundle
            .mime
            .iter()
            .map(|(mime, value)| mime.len() + value.to_string().len())
            .sum();
        self.current_mut().output.push(Segment::Content(block));
        self.note_growth(bytes);
    }

    pub(crate) fn open(&mut self, open: OpenBlock) {
        let id = open.id;
        // Reject a duplicate open for an ID that's already open: installing a
        // second live segment would silently orphan the first, frozen in the
        // output and unreachable by any further patch/close for that ID once
        // `live_indices` starts pointing at the new one instead.
        if self.live_indices.contains_key(&id) {
            return;
        }
        let live = LiveBlock {
            closed: false,
            id,
            initial: open,
            patches: Vec::new(),
        };
        let block_index = self.blocks.len() - 1;
        let segment_index = self.current_mut().output.len();
        self.current_mut().output.push(Segment::Live(live));
        self.live_indices.insert(
            id,
            LiveIndex {
                block_index,
                segment_index,
            },
        );
    }

    pub(crate) fn patch(&mut self, patch: PatchBlock) {
        if let Some(idx) = self.live_indices.get(&patch.id).copied() {
            if let Some(Segment::Live(live)) = self
                .blocks
                .get_mut(idx.block_index)
                .and_then(|b| b.output.get_mut(idx.segment_index))
            {
                if live.patches.len() < MAX_LIVE_BLOCK_PATCHES {
                    let bytes = patch.patch.to_string().len();
                    live.patches.push(patch.patch);
                    self.note_growth(bytes);
                }
            }
        }
    }

    pub(crate) fn close(&mut self, id: BlockId) {
        if let Some(idx) = self.live_indices.remove(&id) {
            if let Some(Segment::Live(live)) = self
                .blocks
                .get_mut(idx.block_index)
                .and_then(|b| b.output.get_mut(idx.segment_index))
            {
                live.closed = true;
            }
        }
    }

    /// Open (`Some`) or close (`None`) the active OSC 8 hyperlink target.
    pub(crate) fn set_link(&mut self, url: Option<String>) {
        self.active_link = url;
    }

    fn current_mut(&mut self) -> &mut CommandBlock {
        self.blocks
            .last_mut()
            .expect("scrollback always retains at least one block")
    }

    /// Record `bytes` of growth and re-measure the retention budget once
    /// enough has accumulated to be worth the walk.
    fn note_growth(&mut self, bytes: usize) {
        self.bytes_since_budget_check += bytes;
        if self.bytes_since_budget_check >= BUDGET_CHECK_INTERVAL_BYTES {
            self.bytes_since_budget_check = 0;
            self.enforce_retention_budget();
        }
    }

    /// Elide the oldest blocks until retained output fits in
    /// [`MAX_RETAINED_OUTPUT_BYTES`].
    ///
    /// The block currently being written is never elided, so a single command
    /// producing more than the whole budget keeps its output and simply
    /// exceeds it; the alternative is discarding the output the user is
    /// actively looking at.
    fn enforce_retention_budget(&mut self) {
        let mut retained: usize = self.blocks.iter().map(CommandBlock::retained_bytes).sum();
        if retained <= MAX_RETAINED_OUTPUT_BYTES {
            return;
        }
        let last = self.blocks.len() - 1;
        while retained > MAX_RETAINED_OUTPUT_BYTES && self.eviction_cursor < last {
            let index = self.eviction_cursor;
            self.eviction_cursor += 1;
            let block = &mut self.blocks[index];
            if block.elided {
                continue;
            }
            retained -= block.retained_bytes();
            block.elide();
            // A live block whose output just went away can no longer be
            // patched or closed; drop its index so later messages for that ID
            // are ignored rather than landing on a stale segment.
            self.live_indices.retain(|_, idx| idx.block_index != index);
        }
    }
}

impl Default for Scrollback {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use winter_proto::{BlockId, EmitBlock, MimeBundle, OpenBlock, PatchBlock, TrustTier};

    use super::*;
    use crate::block::LinkSpan;

    fn svg_block() -> EmitBlock {
        let mut bundle = MimeBundle::new();
        bundle.insert("image/svg+xml", Value::from("<svg/>"));
        EmitBlock {
            bundle,
            id: BlockId(1),
            trust: TrustTier::Restricted,
        }
    }

    #[test]
    fn test_text_without_marks_fills_one_rolling_block() {
        let mut sb = Scrollback::new();
        sb.print("hello");
        sb.line_break();
        assert_eq!(sb.blocks().len(), 1);
        assert_eq!(
            sb.blocks()[0].output,
            vec![Segment::Text("hello\n".to_string())]
        );
    }

    #[test]
    fn test_osc133_sequence_captures_command_and_exit() {
        let mut sb = Scrollback::new();
        sb.prompt_start();
        sb.print("user@host$ "); // prompt text is dropped
        sb.command_start();
        sb.print("ls");
        sb.output_start();
        sb.print("a\nb\n");
        sb.command_end(Some(0));

        let block = &sb.blocks()[0];
        assert_eq!(block.command, "ls");
        assert_eq!(block.output, vec![Segment::Text("a\nb\n".to_string())]);
        assert_eq!(block.exit_code, Some(0));
    }

    #[test]
    fn test_second_prompt_opens_a_new_block() {
        let mut sb = Scrollback::new();
        sb.prompt_start();
        sb.command_start();
        sb.print("true");
        sb.output_start();
        sb.command_end(Some(0));
        sb.prompt_start();
        sb.command_start();
        sb.print("false");
        assert_eq!(sb.blocks().len(), 2);
        assert_eq!(sb.blocks()[1].command, "false");
    }

    #[test]
    fn test_emitted_block_appends_as_content_segment() {
        let mut sb = Scrollback::new();
        sb.print("before");
        sb.emit(svg_block());
        assert_eq!(
            sb.blocks()[0].output,
            vec![
                Segment::Text("before".to_string()),
                Segment::Content(svg_block())
            ]
        );
    }

    #[test]
    fn test_active_link_groups_consecutive_text() {
        let mut sb = Scrollback::new();
        sb.set_link(Some("https://example.com".to_string()));
        sb.print("click ");
        sb.print("here");
        sb.set_link(None);
        sb.print(" plain");

        assert_eq!(
            sb.blocks()[0].output,
            vec![
                Segment::Link(LinkSpan {
                    text: "click here".to_string(),
                    url: "https://example.com".to_string(),
                }),
                Segment::Text(" plain".to_string()),
            ]
        );
    }

    #[test]
    fn test_to_json_tags_text_and_content_segments() {
        let mut sb = Scrollback::new();
        sb.print("hi");
        sb.emit(svg_block());
        let json = sb.to_json();
        assert!(json.contains(r#""kind":"text""#), "{json}");
        assert!(json.contains(r#""kind":"content""#), "{json}");
    }

    #[test]
    fn test_cwd_is_inherited_by_next_block() {
        let mut sb = Scrollback::new();
        sb.set_cwd("/home/user".to_string());
        sb.prompt_start();
        assert_eq!(
            sb.blocks().last().unwrap().cwd,
            Some("/home/user".to_string())
        );
    }

    #[test]
    fn test_open_creates_live_segment() {
        let mut sb = Scrollback::new();
        sb.open(OpenBlock {
            id: BlockId(10),
            mime: "text/markdown".to_string(),
            spec: serde_json::json!("initial"),
        });
        assert_eq!(sb.blocks()[0].output.len(), 1);
        match &sb.blocks()[0].output[0] {
            Segment::Live(live) => {
                assert_eq!(live.id, BlockId(10));
                assert!(live.patches.is_empty());
            }
            other => panic!("expected Live, got {other:?}"),
        }
    }

    #[test]
    fn test_patch_appends_to_live_block() {
        let mut sb = Scrollback::new();
        sb.open(OpenBlock {
            id: BlockId(20),
            mime: "text/plain".to_string(),
            spec: serde_json::json!("v0"),
        });
        sb.patch(PatchBlock {
            id: BlockId(20),
            patch: serde_json::json!([{"op": "replace", "path": "/0", "value": "v1"}]),
        });
        match &sb.blocks()[0].output[0] {
            Segment::Live(live) => assert_eq!(live.patches.len(), 1),
            other => panic!("expected Live, got {other:?}"),
        }
    }

    #[test]
    fn test_output_retention_is_capped_instead_of_unbounded() {
        // Regression: the GPU grid capped its own scrollback rows, but this
        // block list never had a ceiling, so a long-lived pane grew until the
        // process died. Stream well past the budget across many commands.
        let mut scrollback = Scrollback::new();
        let chunk = "x".repeat(64 * 1024);
        for _ in 0..400 {
            scrollback.prompt_start();
            scrollback.command_start();
            scrollback.print("stream");
            scrollback.output_start();
            scrollback.print(&chunk);
            scrollback.command_end(Some(0));
        }
        let retained: usize = scrollback
            .blocks()
            .iter()
            .map(CommandBlock::retained_bytes)
            .sum();
        assert!(
            retained <= MAX_RETAINED_OUTPUT_BYTES,
            "retained {retained} bytes, budget is {MAX_RETAINED_OUTPUT_BYTES}"
        );
    }

    #[test]
    fn test_eviction_keeps_block_indices_stable() {
        // Block indices are stable identifiers held by the app layer (folds,
        // WebView tiles, image placements). Eviction must elide output in
        // place, never remove entries and renumber every one of them.
        let mut scrollback = Scrollback::new();
        let chunk = "y".repeat(64 * 1024);
        for _ in 0..400 {
            scrollback.prompt_start();
            scrollback.output_start();
            scrollback.print(&chunk);
        }
        let count_before = scrollback.blocks().len();
        scrollback.prompt_start();
        scrollback.output_start();
        scrollback.print(&chunk);
        assert_eq!(scrollback.blocks().len(), count_before + 1);
        assert!(
            scrollback.blocks()[0].elided,
            "the oldest block should have been elided"
        );
        assert!(
            scrollback.blocks()[0].output.is_empty(),
            "an elided block keeps no output"
        );
    }

    #[test]
    fn test_the_block_being_written_is_never_elided() {
        // Eliding the block the user is currently watching would discard the
        // output they are looking at; exceeding the budget is the better trade.
        let mut scrollback = Scrollback::new();
        scrollback.prompt_start();
        scrollback.output_start();
        let huge = "z".repeat(MAX_RETAINED_OUTPUT_BYTES + BUDGET_CHECK_INTERVAL_BYTES);
        scrollback.print(&huge);
        let last = scrollback.blocks().last().expect("a block exists");
        assert!(!last.elided);
        assert!(!last.output.is_empty());
    }

    #[test]
    fn test_patch_growth_is_capped_instead_of_unbounded() {
        // Regression: a tool streaming frequent updates to one long-lived
        // block (a progress bar, a log tail) pushed one patch per update for
        // the block's whole lifetime with no ceiling, growing memory without
        // bound.
        let mut sb = Scrollback::new();
        sb.open(OpenBlock {
            id: BlockId(21),
            mime: "text/plain".to_string(),
            spec: serde_json::json!("v0"),
        });
        for i in 0..(MAX_LIVE_BLOCK_PATCHES + 50) {
            sb.patch(PatchBlock {
                id: BlockId(21),
                patch: serde_json::json!([{"op": "replace", "path": "/0", "value": i}]),
            });
        }
        match &sb.blocks()[0].output[0] {
            Segment::Live(live) => assert_eq!(live.patches.len(), MAX_LIVE_BLOCK_PATCHES),
            other => panic!("expected Live, got {other:?}"),
        }
    }

    #[test]
    fn test_reopening_an_unclosed_id_does_not_orphan_the_first_segment() {
        // Regression: `open` unconditionally pushed a new live segment and
        // repointed `live_indices` at it, so a duplicate open for an ID that
        // was never closed left the first segment stuck in the output,
        // permanently unreachable by any later patch/close for that ID, with
        // no error surfaced anywhere.
        let mut sb = Scrollback::new();
        sb.open(OpenBlock {
            id: BlockId(22),
            mime: "text/plain".to_string(),
            spec: serde_json::json!("first"),
        });
        sb.open(OpenBlock {
            id: BlockId(22),
            mime: "text/plain".to_string(),
            spec: serde_json::json!("second"),
        });
        assert_eq!(
            sb.blocks()[0].output.len(),
            1,
            "the duplicate open must not add a second segment"
        );
        match &sb.blocks()[0].output[0] {
            Segment::Live(live) => assert_eq!(live.initial.spec, serde_json::json!("first")),
            other => panic!("expected Live, got {other:?}"),
        }

        // The original open is still tracked and reachable.
        sb.patch(PatchBlock {
            id: BlockId(22),
            patch: serde_json::json!([{"op": "replace", "path": "/0", "value": "v1"}]),
        });
        match &sb.blocks()[0].output[0] {
            Segment::Live(live) => assert_eq!(live.patches.len(), 1),
            other => panic!("expected Live, got {other:?}"),
        }
    }

    #[test]
    fn test_close_removes_live_index() {
        let mut sb = Scrollback::new();
        sb.open(OpenBlock {
            id: BlockId(30),
            mime: "text/plain".to_string(),
            spec: serde_json::json!(null),
        });
        assert!(sb.live_indices.contains_key(&BlockId(30)));
        sb.close(BlockId(30));
        assert!(!sb.live_indices.contains_key(&BlockId(30)));
    }

    #[test]
    fn test_close_flags_the_segment_as_closed() {
        // The segment itself must record `closed`, not just fall out of
        // `live_indices`: the GUI reads this to render a closed block
        // differently, and it can only reach the segment, not the index.
        let mut sb = Scrollback::new();
        sb.open(OpenBlock {
            id: BlockId(31),
            mime: "text/plain".to_string(),
            spec: serde_json::json!(null),
        });
        let Segment::Live(live) = &sb.blocks()[0].output[0] else {
            panic!("expected a live segment");
        };
        assert!(!live.closed);

        sb.close(BlockId(31));
        let Segment::Live(live) = &sb.blocks()[0].output[0] else {
            panic!("expected a live segment");
        };
        assert!(live.closed);
    }

    #[test]
    fn test_patch_for_unknown_id_is_ignored() {
        let mut sb = Scrollback::new();
        sb.patch(PatchBlock {
            id: BlockId(999),
            patch: serde_json::json!([{"op": "add", "path": "/x", "value": 1}]),
        });
        assert!(sb.blocks()[0].output.is_empty());
    }

    #[test]
    fn test_block_row_offsets_tracks_cumulative_rows() {
        let mut sb = Scrollback::new();
        sb.print("aaa\nbbb\n");
        sb.prompt_start();
        sb.command_start();
        sb.print("ls");
        sb.output_start();
        sb.print("output\n");
        let offsets = sb.block_row_offsets(80);
        assert_eq!(offsets.len(), 2);
        assert_eq!(offsets[0], 0);
        assert!(offsets[1] > 0);
    }

    #[test]
    fn test_search_finds_matching_blocks() {
        let mut sb = Scrollback::new();
        sb.print("hello world\n");
        sb.prompt_start();
        sb.command_start();
        sb.print("ls");
        sb.output_start();
        sb.print("goodbye\n");
        let results = sb.search("hello");
        assert_eq!(results, vec![0]);
        let results = sb.search("goodbye");
        assert_eq!(results, vec![1]);
    }

    #[test]
    fn test_search_is_case_insensitive() {
        let mut sb = Scrollback::new();
        sb.print("Hello World\n");
        let results = sb.search("hello");
        assert_eq!(results, vec![0]);
    }

    #[test]
    fn test_search_returns_empty_when_no_match() {
        let mut sb = Scrollback::new();
        sb.print("hello\n");
        let results = sb.search("xyz");
        assert!(results.is_empty());
    }
}
