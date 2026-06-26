//! Scrollback buffer export in plain text, ANSI escape code, and HTML formats.
//!
//! Provides pure extraction functions over [`Grid`] and [`Theme`], capturing
//! the active pane's scrollback and visible viewport for copying or external
//! viewing.

use std::fmt::Write;

use winter_render::{Color, Grid, Theme};

// ========================================================================
// Plain text export
// ========================================================================

/// Extract the full scrollback and visible rows as a plain text string.
/// Trailing whitespace per row is trimmed, and trailing empty rows at the
/// bottom of the buffer are stripped.
pub(crate) fn export_scrollback_plain(grid: &Grid) -> String {
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
pub(crate) fn export_scrollback_ansi(grid: &Grid, _theme: &Theme) -> String {
    let total_rows = grid.scrollback_len() + grid.rows();
    let mut out = String::new();
    let mut last_non_empty_len = 0;

    for abs_row in 0..total_rows {
        let mut row_has_text = false;
        let mut last_fg = Color::Default;
        let mut last_bg = Color::Default;
        let mut last_bold = false;
        let mut last_italic = false;
        let mut last_underline = false;
        let mut last_reversed = false;

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

            let style_changed = cell.style.foreground != last_fg
                || cell.style.background != last_bg
                || cell.style.bold != last_bold
                || cell.style.italic != last_italic
                || cell.style.underline != last_underline
                || cell.style.reversed != last_reversed;

            if style_changed {
                row_str.push_str("\x1b[0m");
                if cell.style.bold {
                    row_str.push_str("\x1b[1m");
                }
                if cell.style.italic {
                    row_str.push_str("\x1b[3m");
                }
                if cell.style.underline {
                    row_str.push_str("\x1b[4m");
                }
                if cell.style.reversed {
                    row_str.push_str("\x1b[7m");
                }

                match cell.style.foreground {
                    Color::Rgb(rgb) => {
                        let _ = write!(row_str, "\x1b[38;2;{};{};{}m", rgb.r, rgb.g, rgb.b);
                    }
                    Color::Indexed(idx) => {
                        let _ = write!(row_str, "\x1b[38;5;{}m", idx);
                    }
                    Color::Default => {}
                }

                match cell.style.background {
                    Color::Rgb(rgb) => {
                        let _ = write!(row_str, "\x1b[48;2;{};{};{}m", rgb.r, rgb.g, rgb.b);
                    }
                    Color::Indexed(idx) => {
                        let _ = write!(row_str, "\x1b[48;5;{}m", idx);
                    }
                    Color::Default => {}
                }

                last_fg = cell.style.foreground;
                last_bg = cell.style.background;
                last_bold = cell.style.bold;
                last_italic = cell.style.italic;
                last_underline = cell.style.underline;
                last_reversed = cell.style.reversed;
            }

            row_str.push(cell.ch);
        }

        if last_bold
            || last_italic
            || last_underline
            || last_reversed
            || last_fg != Color::Default
            || last_bg != Color::Default
        {
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
pub(crate) fn export_scrollback_html(grid: &Grid, theme: &Theme) -> String {
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
            if cell.style.underline {
                style_parts.push("text-decoration:underline;".to_string());
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
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use winter_render::Style;

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
        assert!(ansi.contains("\x1b[1m"), "contains bold escape code");
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
