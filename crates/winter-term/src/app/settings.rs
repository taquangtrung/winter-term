//! The settings page: building it, editing fields, applying the result.

use winit::keyboard::{Key, NamedKey};

use crate::config::{TitleBarStyle, DEFAULT_WINDOW_TITLE_TEMPLATE};
use crate::model::settings_page::{ChoiceOption, SettingsField, SettingsPage};
use winter_render::{ControlsSide, CursorShape, MenuStyle};

use super::App;
use super::{
    settings_theme_options, FONT_SIZE_STEP, MAX_FONT_SIZE, MAX_OPACITY, MAX_SCROLLBACK,
    MIN_FONT_SIZE, MIN_OPACITY, MIN_SCROLLBACK, OPACITY_STEP, SCROLLBACK_STEP,
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
            _ => return false,
        }
        true
    }
}
