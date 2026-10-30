//! Painting the settings page into a cell grid covering the window.

use crate::model::settings_page::{Control, Scope, SettingsField, SettingsPage};
use winter_core::{Color, Grid, Style};
use winter_render::Theme;

use crate::window::frame::frame_grid::{mix_rgb, put, theme_rgb};

// ========================================================================
// Constants
// ========================================================================

/// Settings-editor layout, in cells: the label indent, the column dim notes start
/// at, the right margin values align to, and the body's first row (below the
/// header band and its divider).
const SETTINGS_LEFT_PAD: usize = 4;

const SETTINGS_NOTE_COL: usize = 28;

const SETTINGS_RIGHT_PAD: usize = 4;

/// What stands at the caret of a text row being edited.
const TEXT_CARET: &str = "\u{258f}";

const SETTINGS_FIRST_ROW: usize = 3;

/// Footer hint shown along the bottom of the settings page.
const SETTINGS_HINT: &str = "↑/↓ Move     ←/→ Change     Space Toggle     Enter/Esc Close";

/// The same, in a window with a workspace, whose scope `Tab` chooses.
const SETTINGS_HINT_SCOPED: &str =
    "↑/↓ Move     ←/→ Change     Space Toggle     Tab Scope     Enter/Esc Close";

/// The same, in the Workspace scope, where a row's own value can be dropped.
const SETTINGS_HINT_WORKSPACE: &str =
    "↑/↓ Move     ←/→ Change     Tab Scope     S-Del Use User's     Enter/Esc Close";

/// The page's title, before the scope tabs.
const SETTINGS_TITLE: &str = "Settings";

/// The columns between the title and the scope tabs, and between the tabs.
const SCOPE_GAP: usize = 4;

/// What marks a row the workspace sets over `settings.kdl`.
const OVERRIDDEN_MARK: &str = "●";

/// The column the mark stands in, left of the label.
const OVERRIDDEN_COL: usize = 2;

// ========================================================================
// Data Structures
// ========================================================================

/// The resolved theme colors the settings page paints with, bundled so the
/// row/value helpers can share one palette.
struct SettingsPalette {
    accent: Color,
    accent_fg: Color,
    fg: Color,
    muted: Color,
    selected_bg: Color,
}

// ========================================================================
// Functions
// ========================================================================

/// Paint the settings page into a fresh `cols` x `rows` grid: an elevated header
/// band, then titled sections of rows. Each row shows a label, a dim note, and a
/// right-aligned control; the selected row gets an accent bar and a highlight. A
/// key-hint footer sits along the bottom.
pub(in crate::window) fn build_settings_grid(
    editor: &SettingsPage,
    theme: &Theme,
    cols: usize,
    rows: usize,
) -> Grid {
    let mut grid = Grid::new(cols, rows);
    let pal = SettingsPalette {
        accent: theme_rgb(theme.cursor_bg),
        accent_fg: theme_rgb(theme.cursor_fg),
        fg: theme_rgb(theme.foreground),
        muted: mix_rgb(theme.foreground, theme.background, 0.45),
        selected_bg: theme_rgb(theme.menu_hover_bg),
    };
    let header_bg = theme_rgb(theme.menu_bg);
    let divider = theme_rgb(theme.divider);

    // Header band: a "Settings" title on an elevated strip, underlined by a rule.
    let band = Style {
        background: header_bg,
        ..Style::default()
    };
    put(&mut grid, 0, 0, &" ".repeat(cols), band);
    put(
        &mut grid,
        0,
        SETTINGS_LEFT_PAD,
        SETTINGS_TITLE,
        Style {
            background: header_bg,
            bold: true,
            foreground: pal.fg,
            ..Style::default()
        },
    );
    if let Some(name) = editor.workspace.as_deref() {
        draw_scope_tabs(&mut grid, editor.scope, name, header_bg, &pal);
    }
    put(
        &mut grid,
        1,
        0,
        &"─".repeat(cols),
        Style {
            foreground: divider,
            ..Style::default()
        },
    );

    // Body: sections of field rows. Stop before the footer's divider and hint.
    let body_end = rows.saturating_sub(2);
    let mut row = SETTINGS_FIRST_ROW;
    let mut section: Option<&str> = None;
    for (i, field) in editor.fields.iter().enumerate() {
        if let Some(name) = field.section.as_deref() {
            if section != Some(name) {
                section = Some(name);
                row += 1; // spacer above the section header
                if row >= body_end {
                    break;
                }
                put(
                    &mut grid,
                    row,
                    SETTINGS_LEFT_PAD,
                    &name.to_uppercase(),
                    Style {
                        bold: true,
                        foreground: pal.accent,
                        ..Style::default()
                    },
                );
                row += 1;
            }
        }
        if row >= body_end {
            break;
        }
        let selected = i == editor.selected;
        let caret = selected.then(|| editor.text_caret()).flatten();
        draw_field_row(&mut grid, row, cols, field, selected, caret, &pal);
        if editor.overridden.contains(&field.key) {
            let background = match selected {
                true => pal.selected_bg,
                false => Color::Default,
            };
            put(
                &mut grid,
                row,
                OVERRIDDEN_COL,
                OVERRIDDEN_MARK,
                Style {
                    background,
                    foreground: pal.accent,
                    ..Style::default()
                },
            );
        }
        row += 1;
    }

    put(
        &mut grid,
        rows.saturating_sub(2),
        0,
        &"─".repeat(cols),
        Style {
            foreground: divider,
            ..Style::default()
        },
    );
    let hint = match (&editor.workspace, editor.scope) {
        (None, _) => SETTINGS_HINT,
        (Some(_), Scope::User) => SETTINGS_HINT_SCOPED,
        (Some(_), Scope::Workspace) => SETTINGS_HINT_WORKSPACE,
    };
    put(
        &mut grid,
        rows.saturating_sub(1),
        center_col(cols, hint.chars().count()),
        hint,
        Style {
            foreground: pal.muted,
            ..Style::default()
        },
    );
    grid
}

/// Paint the scope tabs after the title: `User`, then `Workspace: name`, the
/// one edits go to bold in the accent color, the other dim.
fn draw_scope_tabs(
    grid: &mut Grid,
    scope: Scope,
    name: &str,
    background: Color,
    pal: &SettingsPalette,
) {
    let style = |chosen: bool| Style {
        background,
        bold: chosen,
        foreground: match chosen {
            true => pal.accent,
            false => pal.muted,
        },
        ..Style::default()
    };
    let user_col = SETTINGS_LEFT_PAD + SETTINGS_TITLE.chars().count() + SCOPE_GAP;
    let user = "User";
    put(grid, 0, user_col, user, style(scope == Scope::User));
    let workspace_col = user_col + user.chars().count() + SCOPE_GAP;
    let workspace = format!("Workspace: {name}");
    put(
        grid,
        0,
        workspace_col,
        &workspace,
        style(scope == Scope::Workspace),
    );
}

/// Paint one field row: an optional accent bar and highlight when selected, the
/// label, the right-aligned control, and the dim note between them. `caret` is
/// where the caret sits in a selected text row.
fn draw_field_row(
    grid: &mut Grid,
    row: usize,
    cols: usize,
    field: &SettingsField,
    selected: bool,
    caret: Option<usize>,
    pal: &SettingsPalette,
) {
    let row_bg = if selected {
        pal.selected_bg
    } else {
        Color::Default
    };
    if selected {
        put(
            grid,
            row,
            0,
            &" ".repeat(cols),
            Style {
                background: pal.selected_bg,
                ..Style::default()
            },
        );
        // A left accent bar marks the focused row, like VSCode's focused setting.
        put(
            grid,
            row,
            0,
            "▌",
            Style {
                background: pal.selected_bg,
                foreground: pal.accent,
                ..Style::default()
            },
        );
    }
    put(
        grid,
        row,
        SETTINGS_LEFT_PAD,
        &field.label,
        Style {
            background: row_bg,
            foreground: pal.fg,
            ..Style::default()
        },
    );

    let value_col = draw_value(grid, row, cols, &field.control, caret, row_bg, pal);
    if let Some(note) = field.note.as_deref() {
        if SETTINGS_NOTE_COL + 1 < value_col {
            let budget = value_col - SETTINGS_NOTE_COL - 1;
            let text: String = note.chars().take(budget).collect();
            put(
                grid,
                row,
                SETTINGS_NOTE_COL,
                &text,
                Style {
                    background: row_bg,
                    foreground: pal.muted,
                    ..Style::default()
                },
            );
        }
    }
}

/// Draw a field's control, right-aligned to the margin, and return the column it
/// starts at so the caller can keep the note clear of it. Toggles render as an
/// `ON` pill or dim `OFF`; choices and numbers as `‹ value ›`; text inline with a
/// caret at `caret` when focused.
fn draw_value(
    grid: &mut Grid,
    row: usize,
    cols: usize,
    control: &Control,
    caret: Option<usize>,
    row_bg: Color,
    pal: &SettingsPalette,
) -> usize {
    let on_value = Style {
        background: pal.accent,
        bold: true,
        foreground: pal.accent_fg,
        ..Style::default()
    };
    let muted = Style {
        background: row_bg,
        foreground: pal.muted,
        ..Style::default()
    };
    let accent = Style {
        background: row_bg,
        foreground: pal.accent,
        ..Style::default()
    };
    let segments: Vec<(String, Style)> = match control {
        Control::Toggle(t) if t.on => vec![(" ON ".to_string(), on_value)],
        Control::Toggle(_) => vec![(" OFF ".to_string(), muted)],
        Control::Choice(c) => {
            let label = c
                .options
                .get(c.index)
                .map(|o| o.label.as_str())
                .unwrap_or("");
            bracketed(label, muted, accent)
        }
        Control::Number(n) => {
            let value = format!("{:.*}", n.decimals, n.value);
            bracketed(&value, muted, accent)
        }
        Control::Text(t) if t.value.is_empty() => {
            let mut segments = vec![("default".to_string(), muted)];
            if caret.is_some() {
                segments.push((TEXT_CARET.to_string(), accent));
            }
            segments
        }
        Control::Text(t) => {
            let style = Style {
                background: row_bg,
                foreground: pal.fg,
                ..Style::default()
            };
            match caret {
                Some(at) => {
                    let before: String = t.value.chars().take(at).collect();
                    let after: String = t.value.chars().skip(at).collect();
                    vec![
                        (before, style),
                        (TEXT_CARET.to_string(), accent),
                        (after, style),
                    ]
                }
                None => vec![(t.value.clone(), style)],
            }
        }
    };

    let width: usize = segments.iter().map(|(s, _)| s.chars().count()).sum();
    let start = cols.saturating_sub(SETTINGS_RIGHT_PAD + width);
    let mut col = start;
    for (text, style) in &segments {
        put(grid, row, col, text, *style);
        col += text.chars().count();
    }
    start
}

/// The `‹ value ›` segments for a choice or number, value in `accent` and the
/// guillemets in `muted`.
fn bracketed(value: &str, muted: Style, accent: Style) -> Vec<(String, Style)> {
    vec![
        ("‹ ".to_string(), muted),
        (value.to_string(), accent),
        (" ›".to_string(), muted),
    ]
}

/// The starting column that centers `len` cells within `cols`.
fn center_col(cols: usize, len: usize) -> usize {
    cols.saturating_sub(len) / 2
}
