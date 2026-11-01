//! Token inspection popup composition and formatting.
//!
//! Renders the token / scope inspector card over the focused cursor,
//! extracting data from custom editor implementations, rasterized editor grids,
//! or terminal cell grids.

use winter_emulator::{Cell, Color, UnderlineStyle};
use winter_render::{Theme, TokenInspectView};

use crate::window::editor::buffers::PageMut;
use crate::window::editor::cursor::EditorCursor;
use crate::window::layout::open_tabs::OpenTabs;
use crate::window::terminal::navigation::vim::line_chars;
use crate::window::App;
use winter_model::editor::TokenInspectData;
use winter_model::layout::{Rect, TabId};
use winter_model::mode::Mode;

// ========================================================================
// Functions
// ========================================================================

/// The token inspection popup over the cursor when inspection is enabled on
/// the focused editor or terminal.
pub(in crate::window) fn token_inspect_popup(
    open_tabs: &mut OpenTabs,
    editor_cursor: Option<&EditorCursor>,
    token_inspect: Option<TabId>,
    rects: &[(TabId, Rect)],
    (cw, ch): (f32, f32),
    focused: TabId,
    theme: &Theme,
) -> Option<TokenInspectView> {
    let is_app_inspecting = token_inspect == Some(focused);
    let (_, rect) = rects.iter().find(|(id, _)| *id == focused)?;
    // Hung from the cursor's cell where the renderer draws it.
    let pane = App::layout_rect_to_pane(*rect);

    // Case 1: An editor tab
    if let Some(mut slot) = open_tabs.page_mut(&focused) {
        let (data, (row, col)) =
            inspect_editor_slot(&mut slot, editor_cursor, focused, is_app_inspecting, theme)?;
        let anchor = pane.cell_box((row, col), (cw, ch));
        return Some(into_token_inspect_view(anchor, data));
    }

    // Case 2: A terminal tab
    if !is_app_inspecting {
        return None;
    }

    let (data, (row, col)) = inspect_terminal(open_tabs, focused, theme)?;
    let anchor = pane.cell_box((row, col), (cw, ch));
    Some(into_token_inspect_view(anchor, data))
}

/// Convert [`TokenInspectData`] and screen pixel anchor into [`TokenInspectView`].
fn into_token_inspect_view(anchor: (f32, f32, f32), data: TokenInspectData) -> TokenInspectView {
    TokenInspectView {
        anchor,
        character: data.character,
        color_rule: data.color_rule,
        foreground_hex: data.foreground_hex,
        language: data.language,
        position: data.position,
        scopes: data.scopes,
        style: data.style,
    }
}

/// Inspect an editor slot, returning inspect data and its grid `(row, col)`.
fn inspect_editor_slot(
    slot: &mut PageMut<'_>,
    editor_cursor: Option<&EditorCursor>,
    focused: TabId,
    is_app_inspecting: bool,
    theme: &Theme,
) -> Option<(TokenInspectData, (usize, usize))> {
    let (row, col) = if let Some(cursor) = editor_cursor.filter(|c| c.tab == focused) {
        (cursor.row, cursor.col)
    } else {
        let row = slot.view.cursor_line.unwrap_or(0);
        let col = slot.page.caret().map_or_else(
            || {
                slot.view
                    .painted
                    .as_ref()
                    .map_or(0, |g| slot.view.column.col_on(&line_chars(g, row)))
            },
            |c| c.col,
        );
        (row, col)
    };

    let data_opt = slot.page.token_inspect_at(row, col);
    if data_opt.is_none() && !is_app_inspecting {
        return None;
    }

    let cell = slot.view.painted.as_ref().and_then(|g| g.cell(row, col));
    let (rule, hex, style) = cell_theme_style(cell, theme);

    if let Some(mut data) = data_opt {
        if slot.page.caret().is_none() {
            if let Some(cell) = cell {
                let tail = cell.tail.as_deref();
                data.character = TokenInspectData::format_char_with_tail(cell.ch, tail);
                data.position = TokenInspectData::format_position(row, col);
            }
            if data.foreground_hex.is_none() {
                data.foreground_hex = hex;
            }
            if data.color_rule.is_none() {
                data.color_rule = rule;
            }
            if data.style.is_none() {
                data.style = style;
            }
        } else if data.foreground_hex.is_none() {
            data.foreground_hex = hex;
        }
        return Some((data, (row, col)));
    }

    // Fallback for editors without custom token_inspect
    let ch = cell.map(|c| c.ch).unwrap_or(' ');
    let tail = cell.and_then(|c| c.tail.as_deref());
    let character = TokenInspectData::format_char_with_tail(ch, tail);
    let position = TokenInspectData::format_position(row, col);
    let language = slot.kind().title().to_string();
    let scopes = vec![format!("winter.{}", slot.kind().name())];

    let data = TokenInspectData {
        character,
        color_rule: rule,
        foreground_hex: hex,
        language,
        position,
        scopes,
        style,
    };
    Some((data, (row, col)))
}

/// Inspect terminal contents at current cursor position.
fn inspect_terminal(
    open_tabs: &OpenTabs,
    focused: TabId,
    theme: &Theme,
) -> Option<(TokenInspectData, (usize, usize))> {
    let terminal = open_tabs.terminal(&focused)?;
    let pane_mode = open_tabs.mode(&focused).unwrap_or_default();
    let (row, col) = if matches!(pane_mode, Mode::Normal | Mode::Visual) {
        open_tabs
            .nav_cursor(&focused)
            .unwrap_or_else(|| terminal.grid().cursor())
    } else {
        terminal.grid().cursor()
    };

    let cell = terminal.grid().visible_cell(row, col);
    let ch = cell.map(|c| c.ch).unwrap_or(' ');
    let tail = cell.and_then(|c| c.tail.as_deref());
    let character = TokenInspectData::format_char_with_tail(ch, tail);
    let position = TokenInspectData::format_position(row, col);
    let language = "Terminal".to_string();
    let (color_rule, foreground_hex, style) = cell_theme_style(cell, theme);
    let mut scopes = vec!["terminal".to_string()];
    if let Some(cell) = cell {
        if cell.style.link > 0 {
            if let Some(url) = terminal.grid().cell_link(row, col) {
                scopes.push(format!("link.url ({url})"));
            }
        }
    }

    let data = TokenInspectData {
        character,
        color_rule,
        foreground_hex,
        language,
        position,
        scopes,
        style,
    };
    Some((data, (row, col)))
}

/// Resolve color rule, foreground hex, and font style from a terminal/grid cell.
pub(in crate::window) fn cell_theme_style(
    cell: Option<&Cell>,
    theme: &Theme,
) -> (Option<String>, Option<String>, Option<String>) {
    let Some(cell) = cell else {
        return (None, None, None);
    };
    let fg = cell.style.foreground;
    let (rule, hex) = match fg {
        Color::Default => (
            Some("default".to_string()),
            Some(format!(
                "#{:02x}{:02x}{:02x}",
                theme.foreground.r, theme.foreground.g, theme.foreground.b
            )),
        ),
        Color::Indexed(idx) => {
            let (r, g, b) = theme.palette_color(idx);
            let name = ansi_color_name(idx);
            (Some(name), Some(format!("#{:02x}{:02x}{:02x}", r, g, b)))
        }
        Color::Rgb(rgb) => (
            Some("rgb".to_string()),
            Some(format!("#{:02x}{:02x}{:02x}", rgb.r, rgb.g, rgb.b)),
        ),
    };
    let mut style_parts = Vec::new();
    if cell.style.bold {
        style_parts.push("bold");
    }
    if cell.style.italic {
        style_parts.push("italic");
    }
    if cell.style.faint {
        style_parts.push("faint");
    }
    if cell.style.underline != UnderlineStyle::None {
        style_parts.push("underline");
    }
    if cell.style.strikethrough {
        style_parts.push("strikethrough");
    }
    let style = if style_parts.is_empty() {
        None
    } else {
        Some(style_parts.join(" "))
    };
    (rule, hex, style)
}

/// Human-readable ANSI color name for standard palette indices.
pub(in crate::window) fn ansi_color_name(idx: u8) -> String {
    match idx {
        0 => "ansi.black".to_string(),
        1 => "ansi.red".to_string(),
        2 => "ansi.green".to_string(),
        3 => "ansi.yellow".to_string(),
        4 => "ansi.blue".to_string(),
        5 => "ansi.magenta".to_string(),
        6 => "ansi.cyan".to_string(),
        7 => "ansi.white".to_string(),
        8 => "ansi.bright_black".to_string(),
        9 => "ansi.bright_red".to_string(),
        10 => "ansi.bright_green".to_string(),
        11 => "ansi.bright_yellow".to_string(),
        12 => "ansi.bright_blue".to_string(),
        13 => "ansi.bright_magenta".to_string(),
        14 => "ansi.bright_cyan".to_string(),
        15 => "ansi.bright_white".to_string(),
        _ => format!("ansi[{idx}]"),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use winter_emulator::{Cell, Color, Style};
    use winter_terminal::session::git_editor_command;

    #[test]
    fn test_ansi_color_name_resolves_standard_and_bright() {
        assert_eq!(ansi_color_name(0), "ansi.black");
        assert_eq!(ansi_color_name(1), "ansi.red");
        assert_eq!(ansi_color_name(7), "ansi.white");
        assert_eq!(ansi_color_name(8), "ansi.bright_black");
        assert_eq!(ansi_color_name(15), "ansi.bright_white");
        assert_eq!(ansi_color_name(200), "ansi[200]");
    }

    #[test]
    fn test_cell_theme_style_resolves_color_and_attributes() {
        let theme = Theme::default();
        let cell = Cell {
            ch: 'x',
            style: Style {
                foreground: Color::Indexed(1),
                bold: true,
                italic: true,
                ..Style::default()
            },
            ..Cell::default()
        };

        let (rule, hex, style) = cell_theme_style(Some(&cell), &theme);
        assert_eq!(rule.as_deref(), Some("ansi.red"));
        assert!(hex.is_some());
        assert_eq!(style.as_deref(), Some("bold italic"));
    }

    #[test]
    fn test_the_token_card_follows_the_cursor_and_sits_on_its_cell() {
        let dir = std::env::temp_dir().join(format!("winter-inspect-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("main.rs");
        std::fs::write(&file, "fn main() {}\n").expect("file");
        let mut app = App::new();
        let editor = winter_text::TextEditor::new(file, None).expect("opens");
        let tab = app.show_editor(crate::window::editor::EditorKind::Text, Box::new(editor));
        app.toggle_token_inspect(tab);
        let rect = Rect {
            height: 190.0,
            width: 400.0,
            x: 0.0,
            y: 30.0,
        };
        let cell = (9.0, 20.0);
        let card = |app: &mut App| {
            app.open_tabs
                .page_mut(&tab)
                .expect("editor")
                .page
                .content(9, 40, false);
            token_inspect_popup(
                &mut app.open_tabs,
                None,
                app.host.token_inspect,
                &[(tab, rect)],
                cell,
                tab,
                &Theme::dark(),
            )
            .expect("the card is open")
        };
        let first = card(&mut app);
        assert!(first.character.starts_with("'f'"), "{}", first.character);
        let pane = App::layout_rect_to_pane(rect);
        assert_eq!(
            first.anchor.1,
            pane.grid_area(cell.1).y,
            "on the cursor's row, where it is drawn"
        );
        app.open_tabs.page_mut(&tab).expect("editor").page.on_key(
            &winter_model::input::Key::plain(winter_model::input::KeyCode::Char('w')),
        );
        let moved = card(&mut app);
        assert!(
            moved.character.starts_with("'m'"),
            "the character under the cursor now: {}",
            moved.character
        );
        assert!(moved.anchor.0 > first.anchor.0, "and hung from its cell");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_token_card_follows_cursor_in_git_editor_and_updates_character() {
        use crate::window::editor::EditorKind;
        use std::path::PathBuf;
        use winter_git::GitEditor;

        let mut app = App::new();
        let editor = GitEditor::new(PathBuf::from("/tmp"), git_editor_command());
        let tab = app.show_editor(EditorKind::Git, Box::new(editor));
        let rect = Rect {
            height: 190.0,
            width: 400.0,
            x: 0.0,
            y: 30.0,
        };
        let cell = (9.0, 20.0);

        let mut grid = winter_emulator::Grid::new(40, 2);
        grid.move_to(0, 0);
        "  M src/main.rs".chars().for_each(|ch| grid.print(ch));
        if let Some(slot) = app.open_tabs.page_mut(&tab) {
            slot.view.painted = Some(grid);
            slot.view.cursor_line = Some(0);
        }

        app.toggle_token_inspect(tab);

        let card = |app: &mut App| {
            token_inspect_popup(
                &mut app.open_tabs,
                None,
                app.host.token_inspect,
                &[(tab, rect)],
                cell,
                tab,
                &Theme::dark(),
            )
            .expect("the card is open")
        };

        // First non-blank is at col 2 ('M')
        let first = card(&mut app);
        assert!(first.character.starts_with("'M'"), "{}", first.character);
        assert_eq!(first.position, "ln 1, col 3");

        // Move column to 's' in "src/main.rs" (col 4)
        app.place_editor_column(tab, 4);
        let moved = card(&mut app);
        assert!(moved.character.starts_with("'s'"), "{}", moved.character);
        assert_eq!(moved.position, "ln 1, col 5");
        assert!(
            moved.anchor.0 > first.anchor.0,
            "hung from the new column cell"
        );

        // Move column to 'r' (col 5)
        app.place_editor_column(tab, 5);
        let moved2 = card(&mut app);
        assert!(moved2.character.starts_with("'r'"), "{}", moved2.character);
        assert_eq!(moved2.position, "ln 1, col 6");
    }

    #[test]
    fn test_token_card_fallback_for_editor_without_caret_updates_character_on_cursor_move() {
        use crate::editors::notifications::NotificationsEditor;
        use crate::window::editor::EditorKind;

        let mut app = App::new();
        let editor = NotificationsEditor::new(Vec::new(), 0);
        let tab = app.show_page(EditorKind::Notifications, Box::new(editor));
        let rect = Rect {
            height: 190.0,
            width: 400.0,
            x: 0.0,
            y: 30.0,
        };
        let cell = (9.0, 20.0);

        let mut grid = winter_emulator::Grid::new(40, 2);
        grid.move_to(0, 0);
        "   alpha beta".chars().for_each(|ch| grid.print(ch));
        if let Some(slot) = app.open_tabs.page_mut(&tab) {
            slot.view.painted = Some(grid);
            slot.view.cursor_line = Some(0);
        }

        app.toggle_token_inspect(tab);

        let card = |app: &mut App| {
            token_inspect_popup(
                &mut app.open_tabs,
                None,
                app.host.token_inspect,
                &[(tab, rect)],
                cell,
                tab,
                &Theme::dark(),
            )
            .expect("the card is open")
        };

        // First non-blank is at col 3 ('a')
        let first = card(&mut app);
        assert!(first.character.starts_with("'a'"), "{}", first.character);
        assert_eq!(first.position, "ln 1, col 4");

        // Move to 'b' in "beta" (col 9)
        app.place_editor_column(tab, 9);
        let moved = card(&mut app);
        assert!(moved.character.starts_with("'b'"), "{}", moved.character);
        assert_eq!(moved.position, "ln 1, col 10");
        assert!(moved.anchor.0 > first.anchor.0);
    }
}
