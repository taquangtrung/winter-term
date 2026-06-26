//! Opening what the Normal-mode cursor sits on (`gx`) and reading the pane's
//! working directory back out (the palette's "Copy: Working Directory").
//!
//! Resolution mirrors what a mouse click would act on, so the keyboard and the
//! pointer can't disagree about what counts as a link: an OSC 8 hyperlink under
//! the cursor first (its target only exists in the escape sequence), then the
//! first plain URL whose span the cursor rests on, then a file reference
//! (`src/foo.rs:42:10`, `notes.txt`) resolved against the pane's OSC 7 cwd,
//! and finally a quoted string taken whole — the only way a path containing
//! spaces resolves at all.

use std::path::PathBuf;

use winter_render::Grid;

use crate::model::layout::PaneId;
use crate::terminal::pane::Pane;

use super::vim::line_chars;
use super::App;

/// The URL pattern, deliberately narrower than a full URL grammar: it only has
/// to find what a terminal actually prints. Trailing sentence punctuation is
/// trimmed separately (see [`strip_trailing_url_punct`]).
const URL_REGEX: &str = r#"\bhttps?://[^\s<>"'`|(){}\[\]]+"#;

// ========================================================================
// Data Structures
// ========================================================================

/// What `gx` resolved to under the cursor, and how to open it: URLs go to the
/// system opener, files to a new tab running the user's editor.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum OpenTarget {
    File {
        /// The 1-based line a `path:line[:col]` suffix carried, when present.
        line: Option<usize>,
        path: PathBuf,
    },
    Url(String),
}

// ========================================================================
// Resolution
// ========================================================================

/// What `gx` should open for the cursor sitting at viewport `(row, col)`:
/// the cell's own OSC 8 hyperlink target, else the first URL / file reference
/// on the cursor's logical (possibly soft-wrapped) line whose span the cursor
/// rests on — counting a wrapping quote or bracket as part of the span, since
/// a word motion lands on exactly those. `None` when nothing under the cursor
/// resolves to something openable.
pub(crate) fn resolve_open_target(
    grid: &Grid,
    row: usize,
    col: usize,
    cwd: Option<&str>,
) -> Option<OpenTarget> {
    if let Some(url) = grid.cell_link(row, col) {
        return Some(OpenTarget::Url(url.to_string()));
    }

    // The cursor's logical line: all rows of its soft-wrap span joined, with
    // the cursor's index translated into that joined line.
    let (first, last) = grid.wrapped_row_span(row);
    let mut line: Vec<char> = Vec::new();
    for r in first..=last {
        line.extend(line_chars(grid, r));
    }
    let idx = (first..row)
        .map(|r| line_chars(grid, r).len())
        .sum::<usize>()
        + col;

    for (start, len, url) in find_urls(&line) {
        if span_covers(&line, start, len, idx) {
            return Some(OpenTarget::Url(url));
        }
    }

    for (start, len) in token_spans(&line) {
        if !span_covers(&line, start, len, idx) {
            continue;
        }
        let token: String = line[start..start + len].iter().collect();
        let (path_str, line_no) = parse_file_ref(&token);
        if let Some(path) = resolve_existing_path(&path_str, cwd) {
            return Some(OpenTarget::File {
                line: line_no,
                path,
            });
        }
    }

    // Last resort: a quoted string taken whole, so `"my notes.txt"` resolves
    // despite the space a whitespace-split token can't carry.
    if let Some((start, end)) = enclosed_quote_span(&line, idx) {
        let inner: String = line[start..end].iter().collect();
        if let Some(path) = resolve_existing_path(&inner, cwd) {
            return Some(OpenTarget::File { line: None, path });
        }
    }

    None
}

/// Trim the sentence punctuation that surrounds a printed URL far more often
/// than it belongs to one (`https://x.com/a,` — the comma is the sentence's).
fn strip_trailing_url_punct(url: &str) -> &str {
    url.trim_end_matches(['.', ',', ';', ':', '!', '?'])
}

/// Every URL in `line` as a `(start, len, url)` char-index span, trailing
/// punctuation trimmed from both the span and the returned URL.
fn find_urls(line: &[char]) -> Vec<(usize, usize, String)> {
    let text: String = line.iter().collect();
    let Ok(re) = regex::Regex::new(URL_REGEX) else {
        return vec![];
    };
    re.find_iter(&text)
        .filter_map(|m| {
            let url = strip_trailing_url_punct(m.as_str());
            if url.len() < m.as_str().len() {
                // The trimmed tail was punctuation of the sentence, not the
                // URL; only return a hit when something printable remains.
                if url.is_empty() {
                    return None;
                }
            }
            let start = text[..m.start()].chars().count();
            let len = url.chars().count();
            Some((start, len, url.to_string()))
        })
        .collect()
}

/// Whether the cursor at `idx` counts as resting on the span `[start,
/// start+len)`: inside it, or one character out on a wrapping quote or
/// bracket — a word motion lands on exactly those, and a click there is
/// surely meant for the span.
fn span_covers(line: &[char], start: usize, len: usize, idx: usize) -> bool {
    let end = start + len;
    if idx >= start && idx < end {
        return true;
    }
    let is_wrap = |c: char| {
        matches!(
            c,
            '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>'
        )
    };
    (idx == start.saturating_sub(1) && idx < line.len() && is_wrap(line[idx]))
        || (idx == end && idx < line.len() && is_wrap(line[idx]))
}

/// Maximal whitespace-free runs of `line`, trimmed of the quotes, brackets,
/// and trailing sentence punctuation that wrap them in prose, as char-index
/// spans `(start, len)`.
fn token_spans(line: &[char]) -> Vec<(usize, usize)> {
    let is_space = |c: char| c.is_whitespace();
    let leading = |c: char| matches!(c, '"' | '\'' | '`' | '(' | '[' | '{' | '<');
    let trailing = |c: char| {
        matches!(
            c,
            '"' | '\'' | '`' | ')' | ']' | '}' | '>' | '.' | ',' | ';' | ':' | '!' | '?'
        )
    };
    let mut spans = Vec::new();
    let mut i = 0;
    while i < line.len() {
        if is_space(line[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < line.len() && !is_space(line[i]) {
            i += 1;
        }
        let mut s = start;
        let mut e = i;
        while s < e && leading(line[s]) {
            s += 1;
        }
        while e > s && trailing(line[e - 1]) {
            e -= 1;
        }
        if e > s {
            spans.push((s, e - s));
        }
    }
    spans
}

/// Split a `path:line` or `path:line:col` suffix (ripgrep, gcc, cargo output)
/// off `token`, returning the path and its 1-based line. A token with no
/// numeric suffix is returned whole with `None`.
fn parse_file_ref(token: &str) -> (String, Option<usize>) {
    let Ok(re) = regex::Regex::new(r"^(.+?):(\d+)(?::\d+)?$") else {
        return (token.to_string(), None);
    };
    match re.captures(token) {
        Some(caps) => {
            let path = caps.get(1).expect("capture group 1").as_str();
            let line = caps
                .get(2)
                .expect("capture group 2")
                .as_str()
                .parse()
                .unwrap_or(1);
            (path.to_string(), Some(line))
        }
        None => (token.to_string(), None),
    }
}

/// Resolve `raw` to an existing path: absolute as-is, else joined onto `cwd`.
/// `None` when the result doesn't exist — a guess that resolves to nothing is
/// not openable.
fn resolve_existing_path(raw: &str, cwd: Option<&str>) -> Option<PathBuf> {
    let path = PathBuf::from(raw);
    let resolved = match cwd {
        Some(cwd) if !path.is_absolute() => PathBuf::from(cwd).join(path),
        _ => path,
    };
    resolved.exists().then_some(resolved)
}

/// The innermost `[quote, quote)` span containing `idx`, as an inclusive
/// char-index pair `(inner_start, inner_end)`. `None` when the cursor isn't
/// between a pair of matching quotes on the line.
fn enclosed_quote_span(line: &[char], idx: usize) -> Option<(usize, usize)> {
    let quotes = ['"', '\'', '`'];
    let mut best: Option<(usize, usize)> = None;
    for &q in &quotes {
        let positions: Vec<usize> = line
            .iter()
            .enumerate()
            .filter(|&(_, &c)| c == q)
            .map(|(i, _)| i)
            .collect();
        for pair in positions.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if idx > a && idx < b {
                let inner = (a + 1, b);
                let narrower = |x: (usize, usize), y: (usize, usize)| (x.1 - x.0) < (y.1 - y.0);
                if best.is_none_or(|cur| narrower(inner, cur)) {
                    best = Some(inner);
                }
            }
        }
    }
    best
}

// ========================================================================
// App — open under cursor, copy cwd
// ========================================================================

impl App {
    /// `gx`: open what the Normal-mode cursor sits on (see
    /// [`resolve_open_target`]). A URL goes to the system opener; a file
    /// opens in a new tab running the user's editor at the referenced line.
    pub(crate) fn open_under_cursor(&mut self, focused: PaneId) {
        let Some((row, col)) = self.nav_cursor(focused) else {
            return;
        };
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let cwd = pane.cwd();
        let target = resolve_open_target(pane.grid(), row, col, cwd.as_deref());
        match target {
            Some(OpenTarget::Url(url)) => match ::open::that(&url) {
                Ok(()) => self.set_notice(format!("opened {url}")),
                Err(e) => self.set_error(format!("could not open {url}: {e}")),
            },
            Some(OpenTarget::File { path, line }) => self.open_file_in_new_tab(path, line),
            None => self.set_notice("nothing to open under the cursor"),
        }
    }

    /// Open `path` in a new tab running `$VISUAL` / `$EDITOR` (falling back
    /// to `vi`), positioned at `line` when the reference carried one. Winter
    /// owns the PTY, so the editor gets a real terminal — no detached spawn.
    pub(crate) fn open_file_in_new_tab(&mut self, path: PathBuf, line: Option<usize>) {
        let editor = std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .unwrap_or_else(|_| "vi".to_string());
        let mut command = portable_pty::CommandBuilder::new(&editor);
        if let Some(line) = line {
            command.arg(format!("+{line}"));
        }
        command.arg(&path);
        if let Some(dir) = path.parent() {
            command.cwd(dir);
        }

        let id = self.alloc_pane_id();
        let (cols, rows) = self.renderer.as_ref().map(|r| r.grid_size()).unwrap_or((
            crate::app::DEFAULT_COLS as usize,
            crate::app::DEFAULT_ROWS as usize,
        ));
        let scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_render::MAX_SCROLLBACK);
        match Pane::with_command(cols.max(1), rows.max(1), command, scrollback) {
            Ok(pane) => self.push_new_tab(id, pane),
            Err(e) => self.set_error(format!("could not start {editor}: {e}")),
        }
    }

    /// The palette's "Copy: Working Directory": copy the focused pane's OSC 7
    /// cwd to the system clipboard, confirming in the status bar.
    pub(crate) fn copy_pane_cwd(&mut self, focused: PaneId) {
        let Some(cwd) = self.panes.get(&focused).and_then(|p| p.cwd()) else {
            self.set_notice("no working directory reported yet");
            return;
        };
        let copied = self
            .clipboard()
            .and_then(|cb| cb.set_text(&cwd).ok())
            .is_some();
        if copied {
            self.set_notice(format!("copied {cwd}"));
        } else {
            self.set_error("clipboard unavailable");
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid whose viewport holds `lines`, one per row from the top, written
    /// straight into the grid (no shell, no timing).
    fn grid_from_lines(lines: &[&str]) -> Grid {
        let cols = lines
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(20)
            .max(20)
            + 5;
        let mut grid = Grid::new(cols, lines.len().max(4));
        for (row, line) in lines.iter().enumerate() {
            grid.move_to(row, 0);
            for ch in line.chars() {
                grid.print(ch);
            }
        }
        grid.move_to(0, 0);
        grid
    }

    /// A temporary directory that removes itself on drop, so file-existence
    /// tests never leak state between runs.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("winter-open-test-{tag}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }

        fn touch(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, "x").expect("temp file");
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn test_resolves_url_under_cursor() {
        let grid = grid_from_lines(&["see https://example.com/x for more"]);
        let idx = "see ".chars().count();
        assert_eq!(
            resolve_open_target(&grid, 0, idx + 2, None),
            Some(OpenTarget::Url("https://example.com/x".to_string()))
        );
    }

    #[test]
    fn test_url_resolution_trims_trailing_punct_and_covers_wrapping_quote() {
        // The comma is the sentence's, not the URL's; and the cursor resting
        // on the closing quote still counts as being on the link — a word
        // motion lands on exactly that quote.
        let grid = grid_from_lines(&["see \"https://example.com/a,\","]);
        let line = "see \"https://example.com/a,\",";
        let quote_pos = line.chars().position(|c| c == '"').expect("quote");
        assert_eq!(
            resolve_open_target(&grid, 0, quote_pos, None),
            Some(OpenTarget::Url("https://example.com/a".to_string()))
        );
    }

    #[test]
    fn test_resolves_file_reference_with_line_col_against_cwd() {
        // `src/foo.rs:42:10` resolves against the pane's cwd (the tmp root),
        // so the nested relative path and the `:line:col` suffix both count.
        let tmp = TempDir::new("fileref");
        let src = tmp.0.join("src");
        std::fs::create_dir_all(&src).expect("src dir");
        std::fs::write(src.join("foo.rs"), "x").expect("file");
        let grid = grid_from_lines(&["error at src/foo.rs:42:10 here"]);
        let idx = "error at ".chars().count();

        assert_eq!(
            resolve_open_target(&grid, 0, idx + 3, tmp.0.to_str()),
            Some(OpenTarget::File {
                line: Some(42),
                path: src.join("foo.rs"),
            })
        );
    }

    #[test]
    fn test_resolves_plain_relative_path_and_skips_nonexistent_tokens() {
        // `missing.txt` doesn't resolve so the cursor on it yields nothing;
        // `notes.txt` does. Both sit on the same line to prove the existence
        // check filters rather than the whole line being judged at once.
        let tmp = TempDir::new("plainpath");
        tmp.touch("notes.txt");
        let grid = grid_from_lines(&["read missing.txt or notes.txt now"]);

        let missing_idx = "read ".chars().count() + 2;
        assert_eq!(
            resolve_open_target(&grid, 0, missing_idx, tmp.0.to_str()),
            None,
            "a token that resolves to nothing is not openable"
        );

        let notes_idx = "read missing.txt or ".chars().count() + 3;
        assert_eq!(
            resolve_open_target(&grid, 0, notes_idx, tmp.0.to_str()),
            Some(OpenTarget::File {
                line: None,
                path: tmp.0.join("notes.txt"),
            })
        );
    }

    #[test]
    fn test_resolves_quoted_path_with_spaces() {
        // The space inside the quotes defeats whitespace-split tokens; the
        // quoted-string fallback is the only way this resolves.
        let tmp = TempDir::new("quoted");
        tmp.touch("my notes.txt");
        let grid = grid_from_lines(&["open \"my notes.txt\" please"]);
        let idx = "open \"".chars().count() + 2;

        assert_eq!(
            resolve_open_target(&grid, 0, idx, tmp.0.to_str()),
            Some(OpenTarget::File {
                line: None,
                path: tmp.0.join("my notes.txt"),
            })
        );
    }

    #[test]
    fn test_plain_word_resolves_to_nothing() {
        let grid = grid_from_lines(&["just some prose here"]);
        assert_eq!(resolve_open_target(&grid, 0, 6, None), None);
    }

    #[test]
    fn test_parse_file_ref_variants() {
        assert_eq!(
            parse_file_ref("foo.rs:42:10"),
            ("foo.rs".to_string(), Some(42))
        );
        assert_eq!(parse_file_ref("foo.rs:7"), ("foo.rs".to_string(), Some(7)));
        assert_eq!(parse_file_ref("foo.rs"), ("foo.rs".to_string(), None));
        // A version-ish token must not lose its colon as a line number.
        assert_eq!(parse_file_ref("v1.2.3"), ("v1.2.3".to_string(), None));
    }

    #[test]
    fn test_span_covers_inside_and_wrapping_delimiters_only() {
        let line: Vec<char> = "\"abc\"".chars().collect();
        assert!(span_covers(&line, 1, 3, 2), "inside the span");
        assert!(span_covers(&line, 1, 3, 1), "span start");
        // One past the end lands on the closing quote, which counts by design —
        // the plain-letter case below is the one that must not.
        let plain: Vec<char> = "zabcz".chars().collect();
        assert!(
            !span_covers(&plain, 1, 3, 4),
            "one past the end on a plain letter does not count"
        );
        let bracketed: Vec<char> = "[ab]".chars().collect();
        assert!(
            span_covers(&bracketed, 1, 2, 3),
            "resting on the wrapping bracket counts"
        );
        let prose: Vec<char> = "x abc y".chars().collect();
        assert!(
            !span_covers(&prose, 2, 3, 1),
            "a plain letter one out does not count"
        );
    }
}
