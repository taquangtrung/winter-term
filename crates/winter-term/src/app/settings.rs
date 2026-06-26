//! The settings page: building it, editing fields, applying the result.

use winit::keyboard::{Key, NamedKey};

use crate::config::{PromptEditBindings, TitleBarStyle, DEFAULT_WINDOW_TITLE_TEMPLATE};
use crate::model::settings_page::{ChoiceOption, SettingsField, SettingsPage};
use winter_render::{ControlsSide, CursorShape, MenuStyle};

use super::App;
use super::{
    settings_theme_options, FONT_SIZE_STEP, MAX_FONT_SIZE, MAX_OPACITY, MAX_PANE_BORDER,
    MAX_SCROLLBACK, MIN_FONT_SIZE, MIN_OPACITY, MIN_PANE_BORDER, MIN_SCROLLBACK, OPACITY_STEP,
    PANE_BORDER_STEP, SCROLLBACK_STEP,
};

// ========================================================================
// App: settings page
// ========================================================================

impl App {
    /// Open the full-window settings page, dismissing any open menu or palette
    /// first. A no-op if it is already open.
    pub(crate) fn open_settings(&mut self) {
        if self.settings_page.is_some() {
            return;
        }
        self.close_menu();
        self.palette = None;
        self.settings_page = Some(self.build_settings_page());
        // The page covers the window; hide block tiles so they don't show over it.
        self.webview_mgr.hide_all();
        self.dirty = true;
    }
    /// Side effects of leaving the settings page: re-show block tiles and redraw.
    pub(crate) fn on_settings_closed(&mut self) {
        // The overlay is gone; force block tiles to re-show and re-position.
        self.last_tile_layout = None;
        self.dirty = true;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    /// Build the settings page from the live config: one row per editable setting,
    /// pre-filled with the current value.
    pub(crate) fn build_settings_page(&self) -> SettingsPage {
        let theme_options = settings_theme_options();
        let theme_value = self.config.theme.as_value();
        let theme_index = theme_options
            .iter()
            .position(|o| o.value == theme_value)
            .unwrap_or(0);

        let menu_options = vec![
            ChoiceOption {
                label: "Modern".into(),
                value: "modern".into(),
            },
            ChoiceOption {
                label: "Classic".into(),
                value: "classic".into(),
            },
        ];
        let menu_index = match self.config.menu_style {
            MenuStyle::Modern => 0,
            MenuStyle::Classic => 1,
        };

        let status = &self.config.status_bar;
        let fields = vec![
            SettingsField::choice("theme", "Theme", theme_options, theme_index)
                .in_section("Appearance")
                .with_note("Color palette for the terminal and chrome. To add a custom one, run \"Theme: Create New...\" from the command palette"),
            SettingsField::choice("menu_style", "Menu style", menu_options, menu_index)
                .with_note("Modern hamburger menu or a classic menubar"),
            SettingsField::toggle("status.enabled", "Show status bar", status.enabled)
                .in_section("Status bar"),
            SettingsField::toggle("status.show_mode", "Show mode indicator", status.show_mode),
            SettingsField::text(
                "font_family",
                "Font family",
                self.config.font_family.clone().unwrap_or_default(),
            )
            .in_section("Text")
            .with_note("Applied on restart"),
            SettingsField::number(
                "font_size",
                "Font size",
                self.config.font_size,
                MIN_FONT_SIZE,
                MAX_FONT_SIZE,
                FONT_SIZE_STEP,
                0,
            )
            .with_note("Applied on restart"),
            SettingsField::text(
                "font_weight",
                "Font weight",
                self.config.font_weight.clone().unwrap_or_default(),
            )
            .in_section("Text")
            .with_note("e.g. 300, light, normal. Applied on restart"),
            SettingsField::text(
                "font_weight_bold",
                "Bold font weight",
                self.config.font_weight_bold.clone().unwrap_or_default(),
            )
            .in_section("Text")
            .with_note("e.g. 500, bold, medium. Applied on restart"),
            SettingsField::toggle("ligatures", "Ligatures", self.config.ligatures)
                .in_section("Text")
                .with_note("Render -> and => as arrows. Off by default, so every glyph stays at its own cell"),
            SettingsField::number(
                "opacity",
                "Opacity",
                self.config.opacity,
                MIN_OPACITY,
                MAX_OPACITY,
                OPACITY_STEP,
                2,
            )
            .with_note("Applied on restart"),
            {
                let cursor_options = vec![
                    ChoiceOption { label: "Block".into(), value: "block".into() },
                    ChoiceOption { label: "Bar".into(), value: "bar".into() },
                    ChoiceOption { label: "Underline".into(), value: "underline".into() },
                ];
                let idx = ["block", "bar", "underline"]
                    .iter()
                    .position(|&v| v == self.config.cursor.insert.as_value())
                    .unwrap_or(1);
                SettingsField::choice("cursor.insert", "Cursor (insert)", cursor_options, idx)
                    .in_section("Cursor")
            },
            {
                let cursor_options = vec![
                    ChoiceOption { label: "Block".into(), value: "block".into() },
                    ChoiceOption { label: "Bar".into(), value: "bar".into() },
                    ChoiceOption { label: "Underline".into(), value: "underline".into() },
                ];
                let idx = ["block", "bar", "underline"]
                    .iter()
                    .position(|&v| v == self.config.cursor.normal.as_value())
                    .unwrap_or(0);
                SettingsField::choice("cursor.normal", "Cursor (normal)", cursor_options, idx)
            },
            {
                let cursor_options = vec![
                    ChoiceOption { label: "Block".into(), value: "block".into() },
                    ChoiceOption { label: "Bar".into(), value: "bar".into() },
                    ChoiceOption { label: "Underline".into(), value: "underline".into() },
                ];
                let idx = ["block", "bar", "underline"]
                    .iter()
                    .position(|&v| v == self.config.cursor.visual.as_value())
                    .unwrap_or(0);
                SettingsField::choice("cursor.visual", "Cursor (visual)", cursor_options, idx)
            },
            {
                let cursor_options = vec![
                    ChoiceOption { label: "Block".into(), value: "block".into() },
                    ChoiceOption { label: "Bar".into(), value: "bar".into() },
                    ChoiceOption { label: "Underline".into(), value: "underline".into() },
                ];
                let idx = ["block", "bar", "underline"]
                    .iter()
                    .position(|&v| v == self.config.cursor.block_focus.as_value())
                    .unwrap_or(1);
                SettingsField::choice("cursor.block_focus", "Cursor (block focus)", cursor_options, idx)
                    .in_section("Cursor")
            },
            SettingsField::toggle("cursor.blink", "Blink the cursor", self.config.cursor.blink)
                .in_section("Cursor"),
            SettingsField::toggle(
                "cursor.hide_in_inactive",
                "Hide cursor in inactive panes",
                self.config.cursor.hide_in_inactive,
            )
            .in_section("Cursor"),
            SettingsField::text(
                "shell",
                "Shell",
                self.config.active_shell().unwrap_or_default().to_string(),
            )
            .in_section("Terminal")
            .with_note("Shell for this OS. Saves to the per-OS key (shell-linux, shell-macos, shell-windows) in settings.kdl"),
            SettingsField::number(
                "scrollback_lines",
                "Scrollback lines",
                self.config.scrollback_lines.unwrap_or(winter_render::MAX_SCROLLBACK) as f32,
                MIN_SCROLLBACK,
                MAX_SCROLLBACK,
                SCROLLBACK_STEP,
                0,
            )
            .with_note("Applied to new panes"),
            SettingsField::toggle("rainbow_parens", "Rainbow Parentheses", self.config.rainbow_parens)
                .in_section("Terminal")
                .with_note("Depth-color matching bracket pairs and highlight unmatched closers"),
            SettingsField::toggle("sentence_highlight", "Sentence Highlight", self.config.sentence_highlight)
                .in_section("Terminal")
                .with_note("Alternating background tint per sentence for reading transcripts"),
            SettingsField::toggle("url_underline", "Underline URLs", self.config.url_underline)
                .in_section("Terminal")
                .with_note("Underline auto-detected and OSC 8 hyperlink URLs"),
            SettingsField::toggle("wrap_indent", "Hanging Indent", self.config.wrap_indent)
                .in_section("Terminal")
                .with_note("Indent soft-wrapped continuation lines to match the logical line's indent"),
            {
                let binding_options = vec![
                    ChoiceOption { label: "Emacs".into(), value: "emacs".into() },
                    ChoiceOption { label: "None".into(), value: "none".into() },
                ];
                let idx = match self.config.prompt_edit_bindings {
                    PromptEditBindings::Emacs => 0,
                    PromptEditBindings::None => 1,
                };
                SettingsField::choice(
                    "prompt_edit_bindings",
                    "Prompt-line bindings",
                    binding_options,
                    idx,
                )
                .in_section("Terminal")
                .with_note("Which line-editor chords Vim operators send to the shell. Choose None if your shell is in vi mode")
            },
            SettingsField::toggle(
                "palette_match_underline",
                "Palette match underline",
                self.config.palette_match_underline,
            )
            .in_section("Palette")
            .with_note("Underline fuzzy-matched characters in palette results"),
            {
                let side_options = vec![
                    ChoiceOption { label: "Left".into(), value: "left".into() },
                    ChoiceOption { label: "Right".into(), value: "right".into() },
                ];
                let idx = if self.config.window_controls_side == ControlsSide::Left { 0 } else { 1 };
                SettingsField::choice("window_controls_side", "Window controls", side_options, idx)
                    .in_section("Window")
                    .with_note("Side for minimize/maximize/close buttons")
            },
            {
                let style_options = vec![
                    ChoiceOption { label: "Modern".into(), value: "modern".into() },
                    ChoiceOption { label: "System".into(), value: "system".into() },
                ];
                let idx = if self.config.title_bar_style == TitleBarStyle::Modern { 0 } else { 1 };
                SettingsField::choice("title_bar_style", "Title bar style", style_options, idx)
                    .with_note("Applied on restart")
            },
            SettingsField::text(
                "window_title_template",
                "Window title",
                self.config.window_title_template.clone(),
            )
            .in_section("Window")
            .with_note("Placeholders: {{ title }}, {{ app_name }}, {{ pane_title }}, {{ cwd }}. Empty resets"),
            SettingsField::toggle(
                "paste_on_right_click",
                "Paste on right-click",
                self.config.paste_on_right_click,
            )
            .in_section("Window")
            .with_note("Right-click pastes clipboard instead of opening the context menu"),
            SettingsField::toggle("dim_inactive", "Dim inactive panes", self.config.dim_inactive)
                .in_section("Window")
                .with_note("Blend unfocused panes toward the background"),
            SettingsField::number(
                "pane_border_width",
                "Pane divider width",
                self.config.pane_border_width,
                MIN_PANE_BORDER,
                MAX_PANE_BORDER,
                PANE_BORDER_STEP,
                0,
            )
            .in_section("Window")
            .with_note("Thickness of the line between split panes, in logical pixels"),
            SettingsField::toggle(
                "restore_session",
                "Restore session on launch",
                self.config.restore_session,
            )
            .in_section("Window")
            .with_note("Reopen the previous tab and pane layout"),
            SettingsField::toggle(
                "clipboard_read",
                "Allow clipboard reads (OSC 52)",
                self.config.clipboard_read,
            )
            .in_section("Security")
            .with_note("Off by default: the query is silent, so any program in the pane, including one behind ssh, could read what you last copied"),
            {
                let tier_options = vec![
                    ChoiceOption { label: "Isolated".into(), value: "isolated".into() },
                    ChoiceOption { label: "Restricted".into(), value: "restricted".into() },
                    ChoiceOption { label: "Trusted".into(), value: "trusted".into() },
                ];
                let idx = ["isolated", "restricted", "trusted"]
                    .iter()
                    .position(|&v| v == self.config.security.block_max_trust.as_str())
                    .unwrap_or(1);
                SettingsField::choice(
                    "security.block_max_trust",
                    "Block trust ceiling",
                    tier_options,
                    idx,
                )
                .in_section("Security")
                .with_note("Ceiling on the tier a block asks for. Trusted grants scripting to any stream that reaches a pane, not only to the tools you had in mind")
            },
            SettingsField::toggle(
                "security.block_remote_assets",
                "Let blocks load remote assets",
                self.config.security.block_remote_assets,
            )
            .in_section("Security")
            .with_note("Required for live Vega charts, whose runtime comes from a CDN. Off by default so drawing a block makes no request you did not initiate"),
        ];
        SettingsPage::new(fields)
    }
    /// Route one key to the open settings page. Returns whether the page should
    /// stay open (`false` on Enter/Escape). Each value change is applied and
    /// persisted immediately, mirroring the WebView's live preview.
    pub(crate) fn handle_settings_input(&mut self, page: &mut SettingsPage, key: &Key) -> bool {
        match key {
            Key::Named(NamedKey::Escape) | Key::Named(NamedKey::Enter) => return false,
            Key::Named(NamedKey::ArrowUp) => page.move_up(),
            Key::Named(NamedKey::ArrowDown) => page.move_down(),
            Key::Named(NamedKey::ArrowLeft) => {
                if let Some((k, v)) = page.adjust(false) {
                    self.apply_settings_edit(&k, &v);
                }
            }
            Key::Named(NamedKey::ArrowRight) => {
                if let Some((k, v)) = page.adjust(true) {
                    self.apply_settings_edit(&k, &v);
                }
            }
            Key::Named(NamedKey::Backspace) => {
                if let Some((k, v)) = page.pop_char() {
                    self.apply_settings_edit(&k, &v);
                }
            }
            // winit reports the space bar as a named key, not a character. On a
            // text row it inserts a space (font names have them); elsewhere it
            // flips the toggle or steps the control.
            Key::Named(NamedKey::Space) => {
                let edit = if page.selected_is_text() {
                    page.push_char(' ')
                } else {
                    page.adjust(true)
                };
                if let Some((k, v)) = edit {
                    self.apply_settings_edit(&k, &v);
                }
            }
            Key::Character(chars) => {
                for ch in chars.chars() {
                    if let Some((k, v)) = page.push_char(ch) {
                        self.apply_settings_edit(&k, &v);
                    }
                }
            }
            _ => {}
        }
        true
    }
    /// Apply one settings edit to the live config and persist it.
    pub(crate) fn apply_settings_edit(&mut self, key: &str, value: &str) {
        if self.apply_setting(key, value) {
            if let Err(e) = self.config.save() {
                eprintln!("winter: could not save settings: {e}");
            }
        }
        self.dirty = true;
    }
    /// Apply one settings edit to the live config and perform any renderer or
    /// layout refresh it implies. Returns whether the config changed (and so
    /// should be persisted); an unparseable value leaves the config untouched.
    pub(crate) fn apply_setting(&mut self, key: &str, value: &str) -> bool {
        use crate::config::ThemeSetting;
        match key {
            "theme" => {
                self.config.theme = ThemeSetting::from_value(value);
                self.rebuild_theme();
            }
            "menu_style" => {
                self.config.menu_style = match value {
                    "classic" => MenuStyle::Classic,
                    _ => MenuStyle::Modern,
                };
                self.relayout_tabbar();
            }
            "font_family" => {
                let trimmed = value.trim();
                self.config.font_family = (!trimmed.is_empty()).then(|| trimmed.to_string());
            }
            "font_weight" => {
                let trimmed = value.trim();
                self.config.font_weight = (!trimmed.is_empty()).then(|| trimmed.to_string());
            }
            "font_weight_bold" => {
                let trimmed = value.trim();
                self.config.font_weight_bold = (!trimmed.is_empty()).then(|| trimmed.to_string());
            }
            "font_size" => match value.parse::<f32>() {
                Ok(size) => self.config.font_size = size,
                Err(_) => return false,
            },
            "opacity" => match value.parse::<f32>() {
                Ok(opacity) => self.config.opacity = opacity.clamp(0.1, 1.0),
                Err(_) => return false,
            },
            "status.enabled" => {
                self.config.status_bar.enabled = value == "true";
                self.relayout_tabbar();
            }
            "status.show_mode" => {
                self.config.status_bar.show_mode = value == "true";
                self.dirty = true;
            }
            "cursor.insert" => {
                self.config.cursor.insert = CursorShape::from_value(value);
                self.dirty = true;
            }
            "cursor.normal" => {
                self.config.cursor.normal = CursorShape::from_value(value);
                self.dirty = true;
            }
            "cursor.visual" => {
                self.config.cursor.visual = CursorShape::from_value(value);
                self.dirty = true;
            }
            "shell" => {
                let trimmed = value.trim();
                let val = (!trimmed.is_empty()).then(|| trimmed.to_string());
                // Clear the generic `shell` so `to_kdl` doesn't emit both the
                // generic and the OS-specific key (which would be redundant).
                self.config.shell = None;
                #[cfg(target_os = "windows")]
                {
                    self.config.shell_windows = val;
                }
                #[cfg(target_os = "macos")]
                {
                    self.config.shell_macos = val;
                }
                #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
                {
                    self.config.shell_linux = val;
                }
            }
            "scrollback_lines" => match value.parse::<f32>() {
                Ok(n) if n >= 1.0 => {
                    self.config.scrollback_lines = Some(n as usize);
                }
                _ => return false,
            },
            "cursor.block_focus" => {
                self.config.cursor.block_focus = CursorShape::from_value(value);
                self.dirty = true;
            }
            "cursor.blink" => {
                self.config.cursor.blink = value == "true";
                if !self.config.cursor.blink {
                    self.blink_phase = true;
                }
                self.dirty = true;
            }
            "ligatures" => {
                self.config.ligatures = value == "true";
                if let Some(r) = &mut self.renderer {
                    r.set_ligatures(self.config.ligatures);
                }
                self.dirty = true;
            }
            "palette_match_underline" => {
                self.config.palette_match_underline = value == "true";
                self.dirty = true;
            }
            "rainbow_parens" => {
                self.config.rainbow_parens = value == "true";
                self.dirty = true;
            }
            "sentence_highlight" => {
                self.config.sentence_highlight = value == "true";
                self.dirty = true;
            }
            "url_underline" => {
                self.config.url_underline = value == "true";
                self.dirty = true;
            }
            "wrap_indent" => {
                let enabled = value == "true";
                self.config.wrap_indent = enabled;
                for pane in self.panes.values_mut() {
                    pane.grid_mut().set_wrap_indent(enabled);
                }
                self.dirty = true;
            }
            "window_controls_side" => {
                self.config.window_controls_side = match value {
                    "left" => ControlsSide::Left,
                    _ => ControlsSide::Right,
                };
                self.relayout_tabbar();
            }
            "title_bar_style" => {
                self.config.title_bar_style = TitleBarStyle::from_value(value);
            }
            "window_title_template" => {
                let trimmed = value.trim();
                self.config.window_title_template = if trimmed.is_empty() {
                    DEFAULT_WINDOW_TITLE_TEMPLATE.to_string()
                } else {
                    trimmed.to_string()
                };
                self.update_window_title();
            }
            "paste_on_right_click" => {
                self.config.paste_on_right_click = value == "true";
            }
            "dim_inactive" => {
                self.config.dim_inactive = value == "true";
                self.dirty = true;
            }
            "pane_border_width" => match value.parse::<f32>() {
                Ok(width) => {
                    self.config.pane_border_width = width.clamp(MIN_PANE_BORDER, MAX_PANE_BORDER);
                    self.dirty = true;
                }
                Err(_) => return false,
            },
            "restore_session" => {
                self.config.restore_session = value == "true";
            }
            "cursor.hide_in_inactive" => {
                self.config.cursor.hide_in_inactive = value == "true";
                self.dirty = true;
            }
            "prompt_edit_bindings" => {
                self.config.prompt_edit_bindings = PromptEditBindings::from_value(value);
            }
            "clipboard_read" => {
                self.config.clipboard_read = value == "true";
            }
            // A tier typed here is still a ceiling, not a grant: what a block
            // asks for on the wire is clamped against it either way.
            "security.block_max_trust" => match value.parse() {
                Ok(tier) => self.config.security.block_max_trust = tier,
                Err(_) => return false,
            },
            "security.block_remote_assets" => {
                self.config.security.block_remote_assets = value == "true";
            }
            _ => return false,
        }
        true
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::settings_page::Control;

    /// Settings no single row can stand for, and why.
    const NOT_ROWS: [&str; 8] = [
        // A whole color table needs an editor of its own, not a row.
        "colors",
        // Chords live in their own file, and the keys page lists them.
        "keybindings",
        // Written through the one `shell` row, which saves the key for this OS.
        "shell_linux",
        "shell_macos",
        "shell_windows",
        // Nested blocks, checked field by field below.
        "cursor",
        "security",
        "status_bar",
    ];

    /// Rows whose key differs from the setting they write.
    const RENAMED: [(&str, &str); 1] = [("font", "font_family")];

    /// The fields of one schema struct, read from the schema's own source so the
    /// list cannot fall behind it.
    fn schema_fields(name: &str) -> Vec<String> {
        let schema = include_str!("../config/schema.rs");
        let start = schema
            .find(&format!("struct {name} {{"))
            .unwrap_or_else(|| panic!("the schema declares {name}"));
        schema[start..]
            .lines()
            .skip(1)
            .take_while(|line| !line.starts_with('}'))
            .filter_map(|line| line.trim().strip_prefix("pub(crate) "))
            .filter_map(|field| field.split(':').next())
            .map(str::to_string)
            .collect()
    }

    /// Every key the settings page can write.
    fn row_keys() -> Vec<String> {
        App::new()
            .build_settings_page()
            .fields
            .into_iter()
            .map(|field| field.key)
            .collect()
    }

    #[test]
    fn test_every_setting_has_a_row_on_the_settings_page() {
        // A setting the file accepts and the page omits is one only a user who
        // reads the sample config ever finds. The keys come from the schema
        // rather than a list here, because a list is what fell behind before.
        let keys = row_keys();
        let missing: Vec<String> = schema_fields("KdlConfig")
            .into_iter()
            .filter(|field| !NOT_ROWS.contains(&field.as_str()))
            .map(|field| {
                RENAMED
                    .iter()
                    .find(|(setting, _)| *setting == field)
                    .map_or(field.clone(), |(_, row)| (*row).to_string())
            })
            .filter(|row| !keys.contains(row))
            .collect();
        assert!(missing.is_empty(), "no settings row writes: {missing:?}");
    }

    #[test]
    fn test_every_setting_in_a_nested_block_has_a_row_too() {
        // Cursor and security rows are named for their fields. The status bar's
        // are named `status.enabled` and `status.show_mode`, and its mode glyphs
        // are Nerd Font characters nobody types into a text field, so that block
        // is checked by name instead of read from the schema.
        let keys = row_keys();
        for (block, prefix) in [("KdlCursor", "cursor."), ("KdlSecurity", "security.")] {
            for field in schema_fields(block) {
                let row = format!("{prefix}{field}");
                assert!(keys.contains(&row), "no settings row writes: {row}");
            }
        }
        for row in ["status.enabled", "status.show_mode"] {
            assert!(
                keys.contains(&row.to_string()),
                "no settings row writes: {row}"
            );
        }
    }

    /// What a row currently holds, in the form the config file writes.
    fn row_value(field: &SettingsField) -> String {
        match &field.control {
            Control::Choice(choice) => choice
                .options
                .get(choice.index)
                .map(|option| option.value.clone())
                .unwrap_or_default(),
            Control::Number(number) => format!("{:.*}", number.decimals, number.value),
            Control::Text(text) => text.value.clone(),
            Control::Toggle(toggle) => toggle.on.to_string(),
        }
    }

    #[test]
    fn test_each_section_heading_is_declared_once() {
        // The page opens a heading whenever a row's section differs from the row
        // above it, so a row filed under a section its neighbours have left
        // behind prints that heading a second time.
        let mut seen: Vec<String> = Vec::new();
        let mut current = String::new();
        for field in App::new().build_settings_page().fields {
            let Some(section) = field.section else {
                continue;
            };
            if section == current {
                continue;
            }
            assert!(
                !seen.contains(&section),
                "the {section} heading is printed more than once"
            );
            seen.push(section.clone());
            current = section;
        }
    }

    #[test]
    fn test_every_row_on_the_page_is_applied_somewhere() {
        // A row whose key no arm answers to is a control that looks editable and
        // changes nothing when you edit it. Each row is handed back the value it
        // already holds, so the check is an edit that changes nothing.
        let mut app = App::new();
        let rows = app.build_settings_page().fields;
        let unhandled: Vec<String> = rows
            .iter()
            .filter(|field| !app.apply_setting(&field.key, &row_value(field)))
            .map(|field| field.key.clone())
            .collect();
        assert!(unhandled.is_empty(), "nothing applies: {unhandled:?}");
    }
}
