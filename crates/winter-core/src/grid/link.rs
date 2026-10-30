//! OSC 8 hyperlinks and the URL scanner that finds bare links.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::Cell;
use super::Grid;
use super::Style;

// ========================================================================
// Grid: hyperlinks and URL scanning
// ========================================================================

impl Grid {
    /// Open or close an OSC 8 hyperlink. `None` or an empty string clears the
    /// active link; any other value is interned and stamped into future cells.
    pub fn set_active_link(&mut self, url: Option<&str>) {
        self.active_link = match url {
            None | Some("") => 0,
            Some(u) => self.intern_link(u),
        };
    }
    /// Intern `url` into the link table, returning its ID (>0). Reuses an
    /// existing slot when the same URL has been seen before. Once every ID is
    /// taken, a new URL gets 0 (no link): an ID that wrapped around would
    /// alias an older link, and Ctrl+click would open a URL other than the
    /// one shown.
    pub(super) fn intern_link(&mut self, url: &str) -> u16 {
        if let Some(&id) = self.link_ids.get(url) {
            return id;
        }
        if u16::try_from(self.link_table.len()).is_err() && !self.links_saturated {
            self.compact_links();
        }
        let Ok(id) = u16::try_from(self.link_table.len()) else {
            return 0;
        };
        self.link_table.push(url.to_string());
        self.link_ids.insert(url.to_string(), id);
        id
    }
    /// Drop every link no cell, history row, or pen still carries, and
    /// renumber the rest. The table only grows as URLs are seen, so a pane
    /// that has shown many in its life fills it, and every link after that
    /// would be no link at all.
    fn compact_links(&mut self) {
        let mut used = vec![false; self.link_table.len()];
        let mut mark = |style: &Style| {
            if let Some(slot) = used.get_mut(style.link as usize) {
                *slot = true;
            }
        };
        self.for_each_style(&mut |style| mark(style));
        if let Some(slot) = used.get_mut(self.active_link as usize) {
            *slot = true;
        }
        let mut renumbered: HashMap<u16, u16> = HashMap::new();
        let mut table = vec![String::new()];
        for (old, url) in self.link_table.iter().enumerate().skip(1) {
            if used[old] {
                renumbered.insert(old as u16, table.len() as u16);
                table.push(url.clone());
            }
        }
        if table.len() == self.link_table.len() {
            self.links_saturated = true;
            return;
        }
        let renumber = |id: u16| renumbered.get(&id).copied().unwrap_or(0);
        self.for_each_style(&mut |style| style.link = renumber(style.link));
        self.active_link = renumber(self.active_link);
        self.link_ids = table
            .iter()
            .enumerate()
            .skip(1)
            .map(|(id, url)| (url.clone(), id as u16))
            .collect();
        self.link_table = table;
    }

    /// Run `visit` over every style the grid keeps: each cell's on the
    /// screen, in the history, and on the screen put aside, and the pens.
    fn for_each_style(&mut self, visit: &mut dyn FnMut(&mut Style)) {
        self.cells.iter_mut().for_each(|cell| visit(&mut cell.style));
        for row in &mut self.scrollback {
            row.cells.iter_mut().for_each(|cell| visit(&mut cell.style));
        }
        visit(&mut self.style);
        if let Some(saved) = &mut self.saved_cursor {
            visit(&mut saved.style);
        }
        if let Some(alt) = &mut self.alt_buffer {
            alt.cells.iter_mut().for_each(|cell| visit(&mut cell.style));
            visit(&mut alt.style);
            if let Some(saved) = &mut alt.saved_cursor {
                visit(&mut saved.style);
            }
        }
    }

    /// Resolve a link ID to its URL. Returns `None` for id 0 (no link).
    pub fn link_url(&self, id: u16) -> Option<&str> {
        if id == 0 {
            return None;
        }
        self.link_table.get(id as usize).map(String::as_str)
    }
    /// The hyperlink URL of the visible cell at (row, col), if any.
    pub fn cell_link(&self, row: usize, col: usize) -> Option<&str> {
        let cell = self.visible_cell(row, col)?;
        self.link_url(cell.style.link)
    }
    /// Return the link ID (non-zero) for the given URL, or 0 if it has never
    /// been interned. Used to resolve a URL string back to its rendering ID so
    /// the renderer can highlight all cells belonging to the hovered link.
    pub fn find_link_id(&self, url: &str) -> u16 {
        self.link_ids.get(url).copied().unwrap_or(0)
    }
    /// Scan the live cell buffer for plain-text `http://` / `https://` patterns
    /// and stamp matching cells with auto-detected link IDs. Cells that already
    /// carry an OSC 8 link are left untouched. Only the live (non-scrollback)
    /// rows are scanned; scrollback is read-only.
    pub fn detect_urls(&mut self) {
        let now = Instant::now();
        if self.next_url_scan.is_some_and(|next| now < next) {
            return;
        }
        self.next_url_scan = Some(now + URL_SCAN_INTERVAL);

        // Phase 1: collect (cell_start_idx, span_len, url_string) triples by
        // reading self.cells without taking any long-lived borrows.
        let mut spans: Vec<(usize, usize, String)> = Vec::new();

        for row in 0..self.rows {
            let row_start = row * self.cols;
            let mut col = 0;
            while col < self.cols {
                let prefix_len = url_prefix_len(&self.cells, row_start, col, self.cols);
                if prefix_len == 0 {
                    col += 1;
                    continue;
                }
                let start = col;
                let mut end = col + prefix_len;
                while end < self.cols {
                    let ch = self.cells[row_start + end].ch;
                    if is_url_stop(ch) {
                        break;
                    }
                    end += 1;
                }
                if end > start + prefix_len {
                    let url: String = (start..end).map(|c| self.cells[row_start + c].ch).collect();
                    spans.push((row_start + start, end - start, url));
                }
                col = end;
            }
        }

        // Phase 2: intern collected URLs and stamp cells (link_table borrow ends
        // before each cells access).
        for (cell_start, len, url) in spans {
            let link_id = self.intern_link(&url);
            for i in 0..len {
                let idx = cell_start + i;
                if self.cells[idx].style.link == 0 {
                    self.cells[idx].style.link = link_id;
                }
            }
        }
    }
}

/// Minimum time between full-grid [`Grid::detect_urls`] scans. URL highlighting
/// is a passive affordance, not something that needs sub-frame freshness, so a
/// small bounded delay is imperceptible, but skipping the O(rows*cols) rescan
/// on every call matters a lot for a shell whose line-editor retypes the whole
/// input line on every keystroke (e.g. cmd.exe under ConPTY), which would
/// otherwise trigger that scan on nearly every keystroke.
pub(super) const URL_SCAN_INTERVAL: Duration = Duration::from_millis(100);
/// Number of characters in the URL scheme+authority prefix starting at `col`,
/// or 0 if the cell sequence does not begin `https://` or `http://`.
pub(super) fn url_prefix_len(cells: &[Cell], row_start: usize, col: usize, cols: usize) -> usize {
    let matches = |pat: &[u8]| -> bool {
        pat.iter().enumerate().all(|(i, &b)| {
            cells
                .get(row_start + col + i)
                .is_some_and(|c| c.ch as u8 == b && c.ch.is_ascii())
        })
    };
    if col + 8 <= cols && matches(b"https://") {
        8
    } else if col + 7 <= cols && matches(b"http://") {
        7
    } else {
        0
    }
}
/// Returns true for characters that terminate a URL in plain terminal text.
pub(super) fn is_url_stop(ch: char) -> bool {
    ch == '\0'
        || ch == ' '
        || ch == '\t'
        || ch == '"'
        || ch == '\''
        || ch == '<'
        || ch == '>'
        || (ch as u32) < 0x20
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intern_link_deduplicates_same_url() {
        let mut grid = Grid::new(5, 1);
        let id1 = grid.intern_link("https://a.com");
        let id2 = grid.intern_link("https://a.com");
        assert!(id1 > 0);
        assert_eq!(id1, id2);
    }
    #[test]
    fn test_a_full_link_table_makes_room_from_links_no_longer_shown() {
        // The table only grew: a pane that had shown 65,535 distinct URLs
        // gave every later link id 0, so links stopped working for good.
        let mut grid = Grid::new(5, 1);
        grid.set_active_link(Some("https://kept.example"));
        grid.print('k');
        grid.set_active_link(None);
        for n in 0..u16::MAX as usize {
            grid.intern_link(&format!("https://gone.example/{n}"));
        }
        let fresh = grid.intern_link("https://fresh.example");
        assert_ne!(fresh, 0, "room was made");
        assert_eq!(grid.link_url(fresh), Some("https://fresh.example"));
        assert_eq!(grid.cell_link(0, 0), Some("https://kept.example"), "a link on screen survives");
    }

    #[test]
    fn test_intern_link_different_urls_get_different_ids() {
        let mut grid = Grid::new(5, 1);
        let id1 = grid.intern_link("https://a.com");
        let id2 = grid.intern_link("https://b.com");
        assert_ne!(id1, id2);
    }
    #[test]
    fn test_link_url_zero_returns_none() {
        let grid = Grid::new(5, 1);
        assert_eq!(grid.link_url(0), None);
    }
    #[test]
    fn test_set_active_link_stamps_cells() {
        let mut grid = Grid::new(5, 1);
        grid.set_active_link(Some("https://example.com"));
        grid.print('h');
        grid.print('i');
        grid.set_active_link(None);
        grid.print('!');

        let id = grid.cell(0, 0).unwrap().style.link;
        assert!(id > 0);
        assert_eq!(grid.link_url(id), Some("https://example.com"));
        assert_eq!(grid.cell(0, 1).unwrap().style.link, id);
        assert_eq!(grid.cell(0, 2).unwrap().style.link, 0);
    }
    #[test]
    fn test_cell_link_returns_url_for_linked_cell() {
        let mut grid = Grid::new(5, 1);
        grid.set_active_link(Some("https://x.io"));
        grid.print('x');
        assert_eq!(grid.cell_link(0, 0), Some("https://x.io"));
    }
    #[test]
    fn test_cell_link_returns_none_for_unlinked_cell() {
        let mut grid = Grid::new(5, 1);
        grid.print('x');
        assert_eq!(grid.cell_link(0, 0), None);
    }
    #[test]
    fn test_detect_urls_stamps_https_link() {
        let mut grid = Grid::new(40, 1);
        for ch in "visit https://example.com/page here".chars() {
            grid.print(ch);
        }
        grid.detect_urls();
        // The 'h' of 'https' starts the link; every char until space gets the ID.
        let link_id = grid.cells[6].style.link;
        assert!(link_id > 0, "https:// cell should have a link ID");
        let url = grid.link_url(link_id).unwrap();
        assert_eq!(url, "https://example.com/page");
        // Cells before and after the URL have no link.
        assert_eq!(grid.cells[0].style.link, 0);
        assert_eq!(grid.cells[30].style.link, 0);
    }
    #[test]
    fn test_detect_urls_http_scheme() {
        let mut grid = Grid::new(30, 1);
        for ch in "http://foo.io end".chars() {
            grid.print(ch);
        }
        grid.detect_urls();
        let link_id = grid.cells[0].style.link;
        assert!(link_id > 0);
        assert_eq!(grid.link_url(link_id), Some("http://foo.io"));
        // Space terminates the URL; "end" has no link.
        assert_eq!(grid.cells[14].style.link, 0);
    }
    #[test]
    fn test_detect_urls_does_not_override_osc8_link() {
        let mut grid = Grid::new(40, 1);
        grid.set_active_link(Some("https://osc8.io"));
        for ch in "https://osc8.io".chars() {
            grid.print(ch);
        }
        grid.set_active_link(None);
        // Manually store the osc8 link id before calling detect_urls.
        let osc8_id = grid.cells[0].style.link;
        assert!(osc8_id > 0);
        grid.detect_urls();
        // detect_urls should not replace the existing osc8 link.
        assert_eq!(grid.cells[0].style.link, osc8_id);
    }
    #[test]
    fn test_detect_urls_plain_text_no_urls_unchanged() {
        let mut grid = Grid::new(20, 1);
        for ch in "no links here today".chars() {
            grid.print(ch);
        }
        grid.detect_urls();
        for i in 0..19 {
            assert_eq!(grid.cells[i].style.link, 0);
        }
    }

    /// Id 0 means "no hyperlink", so an interned URL must never receive it.
    #[test]
    fn test_find_link_id_is_zero_until_the_url_is_interned() {
        let mut grid = Grid::new(8, 2);
        assert_eq!(grid.find_link_id("https://example.com"), 0);

        grid.set_active_link(Some("https://example.com"));
        grid.print('x');
        let id = grid.find_link_id("https://example.com");
        assert_ne!(id, 0, "0 is reserved for cells with no link");
        assert_eq!(grid.link_url(id), Some("https://example.com"));
    }

    #[test]
    fn test_a_link_never_comes_to_point_at_another_url() {
        // Ids wrapping or being reused while a cell still carries them would
        // make Ctrl+click open a URL other than the one shown.
        let mut grid = Grid::new(10, 2);
        grid.set_active_link(Some("https://first.example"));
        grid.print('f');
        grid.set_active_link(None);
        for n in 0..70_000 {
            let id = grid.intern_link(&format!("https://x.example/{n}"));
            assert_eq!(grid.cell_link(0, 0), Some("https://first.example"), "after {n}");
            if id != 0 {
                assert_eq!(grid.link_url(id), Some(format!("https://x.example/{n}").as_str()));
            }
        }
    }
}
