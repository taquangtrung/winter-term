//! Scrollback buffer export in plain text, ANSI escape code, and HTML formats.
//!
//! Provides pure extraction functions over [`Grid`] and [`Theme`], capturing
//! the active pane's scrollback and visible viewport for copying or external
//! viewing.

use std::fmt::Write;

use crate::model::layout::TabId;
use winter_core::{Color, Grid, Style};
use winter_render::Theme;

use crate::window::App;

// ========================================================================
// Plain text export
// ========================================================================

/// Extract the full scrollback and visible rows as a plain text string.
/// Trailing whitespace per row is trimmed, and trailing empty rows at the
/// bottom of the buffer are stripped.
pub(in crate::window) fn export_scrollback_plain(grid: &Grid) -> String {
    let total_rows = grid.scrollback_len() + grid.rows();
    let mut lines = Vec::with_capacity(total_rows);

    for abs_row in 0..total_rows {
        let mut line = String::with_capacity(grid.cols());
        for col in 0..grid.cols() {
            if let Some(cell) = grid.absolute_cell(abs_row, col) {
                if cell.ch != '\0' {
                    line.push(cell.ch);
                }
            }
        }
        while line.ends_with(|c: char| c.is_whitespace()) {
            line.pop();
        }
        lines.push(line);
    }

    // Trim trailing empty rows at the bottom of the grid
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }

    lines.join("\n")
}

// ========================================================================
// ANSI export
// ========================================================================

/// Extract the full scrollback and visible rows formatted with standard ANSI
/// SGR escape sequences for colors and styles (bold, italic, underline, reverse).
pub(in crate::window) fn export_scrollback_ansi(grid: &Grid, _theme: &Theme) -> String {
    let total_rows = grid.scrollback_len() + grid.rows();
    let mut out = String::new();
    let mut last_non_empty_len = 0;

    for abs_row in 0..total_rows {
        let mut row_has_text = false;
        // The pen the row's text has been written in so far, links aside:
        // they are not pen attributes and the export does not carry them.
        let mut pen = Style::default();

        let mut row_str = String::with_capacity(grid.cols());

        for col in 0..grid.cols() {
            let Some(cell) = grid.absolute_cell(abs_row, col) else {
                continue;
            };
            if cell.ch == '\0' {
                continue;
            }
            if !cell.ch.is_whitespace() {
                row_has_text = true;
            }

            let style = Style {
                link: 0,
                ..cell.style
            };
            if style != pen {
                let _ = write!(row_str, "\x1b[{}m", style.sgr_params());
                pen = style;
            }

            row_str.push(cell.ch);
        }

        if pen != Style::default() {
            row_str.push_str("\x1b[0m");
        }

        out.push_str(&row_str);
        out.push('\n');

        if row_has_text {
            last_non_empty_len = out.len();
        }
    }

    out.truncate(last_non_empty_len);
    out
}

// ========================================================================
// HTML export
// ========================================================================

/// Escape text for HTML content.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Extract the full scrollback and visible rows as a styled standalone HTML document.
pub(in crate::window) fn export_scrollback_html(grid: &Grid, theme: &Theme) -> String {
    let bg_hex = theme.background.to_hex();
    let fg_hex = theme.foreground.to_hex();
    let total_rows = grid.scrollback_len() + grid.rows();

    let mut body = String::new();
    let mut last_non_empty_len = 0;

    for abs_row in 0..total_rows {
        let mut row_has_text = false;
        let mut line = String::new();

        for col in 0..grid.cols() {
            let Some(cell) = grid.absolute_cell(abs_row, col) else {
                continue;
            };
            if cell.ch == '\0' {
                continue;
            }
            if !cell.ch.is_whitespace() {
                row_has_text = true;
            }

            let mut style_parts = Vec::new();
            if cell.style.bold {
                style_parts.push("font-weight:bold;".to_string());
            }
            if cell.style.italic {
                style_parts.push("font-style:italic;".to_string());
            }
            if cell.style.faint {
                style_parts.push("opacity:0.5;".to_string());
            }
            match (cell.style.underline.is_drawn(), cell.style.strikethrough) {
                (true, true) => {
                    style_parts.push("text-decoration:underline line-through;".to_string())
                }
                (true, false) => style_parts.push("text-decoration:underline;".to_string()),
                (false, true) => style_parts.push("text-decoration:line-through;".to_string()),
                (false, false) => {}
            }

            match cell.style.foreground {
                Color::Rgb(rgb) => {
                    style_parts.push(format!("color:#{:02x}{:02x}{:02x};", rgb.r, rgb.g, rgb.b));
                }
                Color::Indexed(idx) => {
                    let rgb = theme
                        .ansi
                        .get(idx as usize % theme.ansi.len())
                        .copied()
                        .unwrap_or(theme.foreground);
                    style_parts.push(format!("color:{};", rgb.to_hex()));
                }
                Color::Default => {}
            }

            match cell.style.background {
                Color::Rgb(rgb) => {
                    style_parts.push(format!(
                        "background-color:#{:02x}{:02x}{:02x};",
                        rgb.r, rgb.g, rgb.b
                    ));
                }
                Color::Indexed(idx) => {
                    let rgb = theme
                        .ansi
                        .get(idx as usize % theme.ansi.len())
                        .copied()
                        .unwrap_or(theme.background);
                    style_parts.push(format!("background-color:{};", rgb.to_hex()));
                }
                Color::Default => {}
            }

            let text = html_escape(&cell.ch.to_string());
            if style_parts.is_empty() {
                line.push_str(&text);
            } else {
                let _ = write!(
                    line,
                    "<span style=\"{}\">{}</span>",
                    style_parts.join(""),
                    text
                );
            }
        }

        body.push_str(&line);
        body.push('\n');

        if row_has_text {
            last_non_empty_len = body.len();
        }
    }

    body.truncate(last_non_empty_len);

    format!(
        r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<title>Winter Terminal Scrollback Export</title>
<style>
body {{
  background-color: {bg_hex};
  color: {fg_hex};
  font-family: monospace;
  margin: 16px;
  line-height: 1.2;
}}
pre {{
  margin: 0;
  white-space: pre-wrap;
  word-break: break-all;
}}
</style>
</head>
<body>
<pre>{body}</pre>
</body>
</html>
"#
    )
}

// ========================================================================
// App: scrollback export
// ========================================================================

impl App {
    /// Copy the focused pane's full scrollback and visible text to clipboard.
    pub(in crate::window) fn copy_scrollback(&mut self, focused: TabId) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let text = export_scrollback_plain(terminal.grid());
        let copied = self
            .input.clipboard()
            .and_then(|cb| cb.set_text(text).ok())
            .is_some();
        if copied {
            self.set_notice("copied scrollback to clipboard");
        } else {
            self.set_error("could not copy to clipboard");
        }
    }

    /// Export scrollback with ANSI color escape codes to a file and open it.
    pub(in crate::window) fn export_scrollback_ansi(&mut self, focused: TabId) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let theme = self
            .renderer
            .as_ref()
            .map(|r| r.theme())
            .cloned()
            .unwrap_or_default();
        let ansi = export_scrollback_ansi(terminal.grid(), &theme);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("winter-scrollback-ansi-{timestamp}.txt"));
        if let Err(e) = std::fs::write(&path, ansi) {
            self.set_error(format!("could not write ANSI scrollback file: {e}"));
            return;
        }
        self.open_file_in_new_tab(path, None);
        self.set_notice("opened ANSI scrollback in editor");
    }

    /// Export scrollback and open in a new tab running $VISUAL / $EDITOR.
    pub(in crate::window) fn export_scrollback_editor(&mut self, focused: TabId) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let text = export_scrollback_plain(terminal.grid());
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("winter-scrollback-{timestamp}.txt"));
        if let Err(e) = std::fs::write(&path, text) {
            self.set_error(format!("could not write scrollback file: {e}"));
            return;
        }
        self.open_file_in_new_tab(path, None);
        self.set_notice("opened scrollback in editor");
    }

    /// Export scrollback as HTML and open it in the default web browser.
    pub(in crate::window) fn export_scrollback_html(&mut self, focused: TabId) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let theme = self
            .renderer
            .as_ref()
            .map(|r| r.theme())
            .cloned()
            .unwrap_or_default();
        let html = export_scrollback_html(terminal.grid(), &theme);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("winter-scrollback-{timestamp}.html"));
        if let Err(e) = std::fs::write(&path, html) {
            self.set_error(format!("could not write scrollback HTML: {e}"));
            return;
        }
        match ::open::that_detached(&path) {
            Ok(()) => self.set_notice(format!("opened {}", path.display())),
            Err(e) => self.set_error(format!("could not open HTML file: {e}")),
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_export_scrollback_plain_trims_whitespace_and_empty_rows() {
        let mut grid = Grid::new(20, 4);
        grid.move_to(0, 0);
        for ch in "hello world   ".chars() {
            grid.print(ch);
        }
        grid.move_to(1, 0);
        for ch in "line 2".chars() {
            grid.print(ch);
        }
        let plain = export_scrollback_plain(&grid);
        assert_eq!(plain, "hello world\nline 2");
    }

    #[test]
    fn test_export_scrollback_ansi_includes_colors_and_styles() {
        let mut grid = Grid::new(20, 2);
        grid.move_to(0, 0);
        let style = Style {
            bold: true,
            ..Default::default()
        };
        grid.set_style(style);
        for ch in "bold".chars() {
            grid.print(ch);
        }
        let theme = Theme::default();
        let ansi = export_scrollback_ansi(&grid, &theme);
        assert!(ansi.contains("\x1b[0;1m"), "contains bold escape code");
        assert!(ansi.contains("bold"), "contains text");
    }

    #[test]
    fn test_export_scrollback_html_escapes_entities_and_wraps_in_document() {
        let mut grid = Grid::new(20, 2);
        grid.move_to(0, 0);
        for ch in "<span> & 'test'".chars() {
            grid.print(ch);
        }
        let theme = Theme::default();
        let html = export_scrollback_html(&grid, &theme);
        assert!(html.contains("&lt;span&gt;"), "escapes angle brackets");
        assert!(html.contains("&amp;"), "escapes ampersand");
        assert!(
            html.contains("<!DOCTYPE html>"),
            "is complete html document"
        );
    }
}
