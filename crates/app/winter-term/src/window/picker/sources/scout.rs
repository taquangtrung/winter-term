//! The live pickers, about the pane they sit at the bottom of: Scout
//! (`C-j`, or `gs` in a terminal), which searches the tab's text as it is
//! typed and scouts each match in place before going to it, and the outline
//! (`C-S-o`), the file's headings and definitions.
//!
//! Scout lists a terminal's lines, an editor's own places (the text editor's
//! lines, Dir's entries), each row a line's number and its text, matched on
//! the text. Going to a row is a jump, recorded so `C-o` comes back.

use std::time::Duration;

use crate::render::{PickerView, Theme};
use misty::picker::{MatchOn, PreviewKind, Resume, Row, RowKey};
use winter_model::editor::JumpTarget;
use winter_model::layout::TabId;
use winter_model::mode::Mode;

use super::listing::{Entry, Listing};
use crate::window::frame::frame_grid::span_rgb;
use crate::window::layout::open_tabs::OpenTabs;
use crate::window::picker::sources::IconRow;
use crate::window::picker::Preview;
use crate::window::picker::WindowOutcome;
use crate::window::terminal::navigation;
use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// Said when the focused tab has no outline.
const NO_OUTLINE: &str = "nothing to outline here";

/// Scout's picker, by its id.
pub(in crate::window) const SCOUT_PICKER: &str = "scout";

/// The longest a redraw of Scout spends learning the lines above the rows
/// it shows, so they can be colored: a jump deep into a large file colors
/// over a few frames rather than holding one.
const SCOUT_STYLE_BUDGET: Duration = Duration::from_millis(8);

// ========================================================================
// Data Structures
// ========================================================================

/// The editor tab Scout lists the lines of, and the line each of its rows
/// is, so the rows shown can be drawn in the editor's syntax colors.
pub(in crate::window) struct ScoutLines {
    /// The line each row is, by the row's place in the listing.
    pub(in crate::window) targets: Vec<usize>,
    pub(in crate::window) tab: TabId,
}

/// Scout's listing, and the place each of its rows goes to.
struct ScoutListing {
    listing: Listing,
    targets: Vec<usize>,
}

// ========================================================================
// App: the live pickers
// ========================================================================

impl App {
    /// Open Scout over the tab `focused`: its lines or its places, the one
    /// the cursor is on selected.
    pub(in crate::window) fn open_scout(&mut self, focused: TabId) {
        if let Some(slot) = self.open_tabs.page_mut(&focused) {
            let Some(targets) = slot.page.jump_targets() else {
                return;
            };
            let title = format!("Scout: {}", slot.page.title());
            let cursor = slot.page.jump_cursor();
            let listing = scout_listing(title, targets);
            let start = cursor.and_then(|cursor| nearest(&listing.targets, cursor));
            self.pickers.scout = Some(ScoutLines {
                targets: listing.targets.clone(),
                tab: focused,
            });
            self.open_live(listing.listing, start);
            return;
        }
        self.pickers.scout = None;
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let grid = terminal.grid();
        let origin = self
            .nav_cursor(focused)
            .unwrap_or_else(|| (grid.to_absolute_row(grid.cursor().0), grid.cursor().1));
        let lines = navigation::swoop::extract_swoop_lines(grid);
        self.term.scout_origin = Some((focused, origin));
        self.open_tabs.set_mode(&focused, Mode::Normal);
        let targets = lines
            .into_iter()
            .map(|(row, line)| JumpTarget::new(row, format!("{:>5}  {line}", row + 1)))
            .collect();
        let listing = scout_listing("Scout: Terminal".to_string(), targets);
        let start = nearest(&listing.targets, origin.0);
        self.open_live(listing.listing, start);
    }

    /// Open the outline of the editor in `focused`: the cursor follows the
    /// entry picked, and goes back to where it was if the outline is put
    /// away.
    pub(in crate::window) fn open_outline(&mut self, focused: TabId) {
        let Some(slot) = self.open_tabs.page_mut(&focused) else {
            self.refuse(NO_OUTLINE);
            return;
        };
        let targets = slot.page.outline_targets().unwrap_or_default();
        if targets.is_empty() {
            self.set_notice(NO_OUTLINE);
            return;
        }
        let title = format!("Outline: {}", slot.page.title());
        let cursor = slot.page.jump_cursor();
        let mut listing = Listing::new("outline", title)
            .placeholder("Heading or symbol")
            .preview(PreviewKind::Live)
            .resume(Resume::Never);
        // The entry the cursor's line falls under, not the next one below.
        let start =
            cursor.and_then(|cursor| targets.iter().rposition(|target| target.target <= cursor));
        for target in targets {
            let mut row = Row::new(RowKey(0), target.label);
            if let Some(icon) = target.icon {
                row = row.with_icon_kind(icon);
            }
            let entry = Entry::new(row, WindowOutcome::Jump(target.target));
            listing.push(entry.with_preview(Preview::Target(target.target)));
        }
        self.open_live(listing, start);
    }

    /// Open a live picker over `listing`, selected on its row `start`.
    fn open_live(&mut self, listing: Listing, start: Option<usize>) {
        self.open_picker(Box::new(listing));
        if let (Some(picker), Some(start)) = (self.chrome.picker_mut(), start) {
            picker.preselect(RowKey(start as u64));
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Scout over `targets`: a line's number as the label and its text as the
/// description, matched on the text; an entry of a listing (an icon says
/// it is one) by its name.
fn scout_listing(title: String, targets: Vec<JumpTarget>) -> ScoutListing {
    let listing_rows = targets.iter().any(|target| target.icon.is_some());
    let on = match listing_rows {
        true => MatchOn::LABEL,
        false => MatchOn::DESCRIPTION,
    };
    let mut listing = Listing::new(SCOUT_PICKER, title)
        .placeholder("Text on the line")
        .takes_word_at_point()
        .match_on(on)
        .preview(PreviewKind::Live)
        .resume(Resume::Never);
    let mut places = Vec::with_capacity(targets.len());
    for target in targets {
        let (number, text) = split_number(&target.label);
        let row = match listing_rows {
            true => Row::new(RowKey(0), text.trim_start()).with_hint(number),
            false => Row::new(RowKey(0), number).with_description(text),
        };
        let row = match target.icon {
            Some(icon) => row.with_icon_kind(icon),
            None => row,
        };
        places.push(target.target);
        let entry = Entry::new(row, WindowOutcome::Jump(target.target));
        listing.push(entry.with_preview(Preview::Target(target.target)));
    }
    ScoutListing {
        listing,
        targets: places,
    }
}

/// Draw the rows `view` shows, whose places in Scout's listing are `shown`,
/// in the colors the editor `scout` lists draws their lines in: only those
/// rows are colored, each redraw. Whether some were not reached in the time
/// a redraw gives, so the next redraw colors more.
pub(in crate::window) fn color_scout_rows(
    open_tabs: &mut OpenTabs,
    scout: &ScoutLines,
    shown: &[usize],
    view: &mut PickerView,
    theme: &Theme,
) -> bool {
    let Some(slot) = open_tabs.page_mut(&scout.tab) else {
        return false;
    };
    let lines: Vec<usize> = shown
        .iter()
        .filter_map(|&at| scout.targets.get(at).copied())
        .collect();
    let styles = slot.page.jump_styles(&lines, SCOUT_STYLE_BUDGET);
    for (item, line) in view.items.iter_mut().zip(&lines) {
        let Some(found) = styles.styles.iter().find(|style| style.target == *line) else {
            continue;
        };
        item.description_colors = found
            .runs
            .iter()
            .map(|run| (run.chars, span_rgb(run.style, theme)))
            .collect();
    }
    styles.pending
}

/// A target's label taken apart: the line number it leads with, and the
/// rest after the two spaces that follow it.
fn split_number(label: &str) -> (&str, &str) {
    let trimmed = label.trim_start();
    match trimmed.split_once("  ") {
        Some((number, text)) if number.chars().all(|c| c.is_ascii_digit()) => (number, text),
        Some(_) | None => ("", label),
    }
}

/// The place in `targets` nearest `at`.
fn nearest(targets: &[usize], at: usize) -> Option<usize> {
    (0..targets.len()).min_by_key(|&index| targets[index].abs_diff(at))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use misty::picker::{Matching, Source};

    use super::*;

    #[test]
    fn test_a_line_is_its_number_then_its_text_matched_on_the_text() {
        let targets = vec![JumpTarget::new(11, "   12  fn twelve() {}".to_string())];
        let mut scout = scout_listing("Scout".to_string(), targets);
        assert_eq!(
            scout.listing.matching(),
            Matching::Words(MatchOn::DESCRIPTION)
        );
        let misty::picker::Feed::Fixed(rows) = scout.listing.feed("") else {
            panic!("fixed rows");
        };
        assert_eq!(rows[0].label, "12");
        assert_eq!(rows[0].description, "fn twelve() {}");
    }

    #[test]
    fn test_scout_over_a_listing_matches_the_entry_name() {
        let icon = winter_model::paint::IconKind::File {
            name: "a.rs".to_string(),
        };
        let targets = vec![JumpTarget::new(0, "    1    a.rs".to_string()).with_icon(icon)];
        let scout = scout_listing("Scout".to_string(), targets);
        assert_eq!(scout.listing.matching(), Matching::Words(MatchOn::LABEL));
    }

    #[test]
    fn test_scout_starts_on_the_place_nearest_the_cursor() {
        assert_eq!(nearest(&[2, 10, 30], 12), Some(1));
        assert_eq!(nearest(&[], 12), None);
    }
}
