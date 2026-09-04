//! OSC 8 hyperlinks and the URL scanner that finds bare links.

use super::*;

// ========================================================================
// Items
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
    /// existing slot when the same URL has been seen before.
    pub(super) fn intern_link(&mut self, url: &str) -> u16 {
        if let Some(i) = self.link_table.iter().position(|s| s == url) {
            return i as u16;
        }
        let id = self.link_table.len() as u16;
        self.link_table.push(url.to_string());
        id
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
        self.link_table
            .iter()
            .position(|s| s == url)
            .map(|i| i as u16)
            .unwrap_or(0)
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
/// small bounded delay is imperceptible — but skipping the O(rows*cols) rescan
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
}
