//! Previews: a live picker's in the pane it is about, and the side panel's
//! inside the picker.
//!
//! A live picker (Scout, the outline) moves its pane's view to the selected
//! row's place, and puts it back if the picker is put away. The side panel
//! shows a file read off the window's thread, centred on the row's line;
//! nothing is opened, so a preview leaves no buffer behind.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Instant;

use misty::picker::{PreviewUpdate, Query};
use winter_model::background::{Background, POLL};
use winter_model::editor::JumpStep;
use winter_model::layout::TabId;
use winter_model::mode::Mode;
use winter_model::paint::Style;

use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// How many lines around the row's line the side panel reads of a file.
const PANEL_CONTEXT: usize = 120;

/// The longest file the side panel reads, in bytes: past this a preview
/// would cost more than it shows.
const PANEL_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// The background work's name.
const PANEL_READER: &str = "picker-preview";

// ========================================================================
// Data Structures
// ========================================================================

/// What a preview shows. The window draws it and opens nothing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Preview {
    /// A file, centred on a line.
    File(FilePreview),
    /// One of the pane's own places, by the index its editor gave it.
    Target(usize),
    /// Text to show as it is.
    Text(String),
}

/// A file to preview, and the place in it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilePreview {
    /// The column of the match, from one, when known.
    pub column: Option<usize>,
    /// The line to centre on, from one.
    pub line: usize,
    /// The file.
    pub path: PathBuf,
}

/// The side panel's preview: what it shows, and the read under way.
#[derive(Default)]
pub(in crate::window) struct PanelState {
    reading: Option<Background<PanelText>>,
    /// What the panel shows, once read.
    pub(in crate::window) shown: Option<PanelText>,
}

/// The text the side panel shows: its lines, the one the row is about, and
/// where the query matched on each.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(in crate::window) struct PanelText {
    pub(in crate::window) focus: Option<usize>,
    pub(in crate::window) lines: Vec<String>,
    /// Per line, the char ranges the query matched.
    pub(in crate::window) marks: Vec<Vec<Range<usize>>>,
    /// Per line, the style of each char as its language colours it; empty
    /// for a file in no language Winter knows.
    pub(in crate::window) styles: Vec<Vec<Style>>,
}

// ========================================================================
// PanelState
// ========================================================================

impl PanelState {
    /// Show nothing, and stop any read under way.
    pub(in crate::window) fn clear(&mut self) {
        self.reading = None;
        self.shown = None;
    }
}

// ========================================================================
// App: previews
// ========================================================================

impl App {
    /// Move the pane of tab `focused` to the place a live picker's row is
    /// about: an editor's own place, or a terminal's line.
    pub(in crate::window) fn live_preview(
        &mut self,
        update: PreviewUpdate<Preview>,
        _query: &str,
        focused: TabId,
    ) {
        let PreviewUpdate::Show(Preview::Target(target)) = update else {
            return;
        };
        if let Some(slot) = self.open_tabs.page_mut(&focused) {
            slot.page.on_jump(JumpStep::Preview(target));
        } else {
            self.set_nav_cursor(focused, (target, 0));
            self.reveal_position_in_uncovered(focused, (target, 0));
        }
        self.dirty = true;
    }

    /// Go to `target` for good, from a live picker: an editor makes the
    /// move its own; a terminal's cursor goes to the line, where it started
    /// kept so `C-o` comes back.
    pub(in crate::window) fn confirm_jump(&mut self, target: usize, focused: TabId) {
        if let Some(slot) = self.open_tabs.page_mut(&focused) {
            slot.page.on_jump(JumpStep::Confirm(target));
            return;
        }
        if let Some((_, origin)) = self.term.scout_origin.take() {
            self.vim.tab_mut(focused).jump_list.push(origin);
        }
        self.set_nav_cursor(focused, (target, 0));
        self.reveal_position_centered(focused, (target, 0));
        self.open_tabs.set_mode(&focused, Mode::Normal);
    }

    /// Put the pane of tab `focused` back where it was before a live picker
    /// moved it.
    pub(in crate::window) fn cancel_jump(&mut self, focused: TabId) {
        if let Some(slot) = self.open_tabs.page_mut(&focused) {
            slot.page.on_jump(JumpStep::Cancel);
        } else if let Some((tab, origin)) = self.term.scout_origin.take() {
            if tab == focused {
                self.set_nav_cursor(focused, origin);
                self.reveal_position(focused, origin);
            }
        }
        self.dirty = true;
    }

    /// Start reading what the side panel shows for the selected row.
    pub(in crate::window) fn panel_preview(&mut self, update: PreviewUpdate<Preview>, query: &str) {
        let preview = match update {
            PreviewUpdate::Clear => None,
            PreviewUpdate::Show(preview) => Some(preview),
        };
        let panel = &mut self.pickers.panel;
        match preview {
            Some(Preview::File(file)) => {
                let query = Query::parse(query);
                let (path, line) = (file.path, file.line);
                panel.reading = Some(Background::start(PANEL_READER, move |cancel| {
                    read_panel(&path, line, &query, cancel)
                }));
            }
            Some(Preview::Text(text)) => {
                panel.reading = None;
                panel.shown = Some(PanelText {
                    focus: None,
                    marks: Vec::new(),
                    lines: text.lines().map(str::to_string).collect(),
                    styles: Vec::new(),
                });
            }
            Some(Preview::Target(_)) | None => panel.clear(),
        }
        self.dirty = true;
    }

    /// Take in a side panel read that finished; when to look again for one
    /// still under way.
    pub(in crate::window) fn take_panel_preview(&mut self) -> Option<Instant> {
        let reading = self.pickers.panel.reading.as_ref()?;
        match reading.poll() {
            Some(text) => {
                self.pickers.panel.reading = None;
                self.pickers.panel.shown = Some(text);
                self.dirty = true;
                None
            }
            None => Some(Instant::now() + POLL),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The lines of `path` around `line` (from one), with where `query`
/// matched each: what the side panel shows for a row at that place. A file
/// that cannot be read, or is too long, shows nothing.
fn read_panel(
    path: &PathBuf,
    line: usize,
    query: &Query,
    cancel: &std::sync::atomic::AtomicBool,
) -> PanelText {
    let too_long = std::fs::metadata(path).is_ok_and(|meta| meta.len() > PANEL_MAX_BYTES);
    let text = match too_long {
        true => None,
        false => std::fs::read(path).ok(),
    };
    let Some(text) = text else {
        return PanelText::default();
    };
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return PanelText::default();
    }
    let text = String::from_utf8_lossy(&text);
    let at = line.saturating_sub(1);
    let first = at.saturating_sub(PANEL_CONTEXT / 2);
    let lines: Vec<String> = text
        .lines()
        .skip(first)
        .take(PANEL_CONTEXT)
        .map(|line| line.replace('\t', "    "))
        .collect();
    let marks = lines.iter().map(|line| query.spans(line)).collect();
    PanelText {
        focus: Some(at - first),
        styles: colour_lines(path, &text, first, &lines),
        lines,
        marks,
    }
}

/// The style of each char of `shown`, the lines of `text` from `first`, in
/// the language `path` or its first line names: the lines above read too, so
/// a string or comment open across them is coloured as it is.
fn colour_lines(path: &Path, text: &str, first: usize, shown: &[String]) -> Vec<Vec<Style>> {
    let grammar = winter_syntax::of_path(path)
        .or_else(|| text.lines().next().and_then(winter_syntax::of_first_line));
    let Some(grammar) = grammar else {
        return Vec::new();
    };
    let mut state = winter_syntax::LineState::start(grammar);
    for line in text.lines().take(first) {
        state = winter_syntax::next_state(line, &state);
    }
    shown
        .iter()
        .map(|line| {
            let (styles, next) = winter_syntax::highlight(line, &state);
            state = next;
            styles
        })
        .collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;

    #[test]
    fn test_the_panel_reads_the_lines_around_the_row_and_marks_the_query() {
        let dir = std::env::temp_dir().join(format!("winter-panel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("a.rs");
        let body: String = (1..=300).map(|n| format!("line {n}\n")).collect();
        std::fs::write(&path, body).expect("temp file");
        let text = read_panel(
            &path,
            200,
            &Query::parse("line 200"),
            &AtomicBool::new(false),
        );
        let focus = text.focus.expect("the row's line is marked");
        assert_eq!(text.lines[focus], "line 200");
        assert_eq!(text.marks[focus], [0..4, 5..8]);
        assert!(text.lines.len() <= PANEL_CONTEXT);
        assert!(!text.styles.is_empty(), "a Rust file is coloured");
    }
}
