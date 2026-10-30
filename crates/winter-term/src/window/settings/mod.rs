//! The settings page: building it, editing fields, applying the result.

pub(crate) mod grid;
mod workspace;

use winit::keyboard::{Key, NamedKey};

use crate::config::schema::{cursor_shape_as_value, cursor_shape_from_value};
use crate::config::{Config, PromptEditBindings, TitleBarStyle, MAX_SCROLL_MARGIN};
use crate::model::input;
use crate::model::paint::{GutterSide, MarkupMarkers, WrapIndent};
use crate::model::settings_page::{ChoiceOption, Scope, SettingsField, SettingsPage};
use winter_render::{ControlsSide, MenuStyle};

use super::{
    App, FONT_SIZE_STEP, MAX_FONT_SIZE, MAX_OPACITY, MAX_PANE_BORDER, MAX_SCROLLBACK,
    MIN_FONT_SIZE, MIN_OPACITY, MIN_PANE_BORDER, MIN_SCROLLBACK, OPACITY_STEP, PANE_BORDER_STEP,
    SCROLLBACK_STEP,
};
use crate::window::input::overlay::Overlay;

// ========================================================================
// Constants
// ========================================================================

/// The fewest and most letters typed before completion opens by itself.
const MIN_COMPLETION_CHARS: f32 = 1.0;
const MAX_COMPLETION_CHARS: f32 = 10.0;

/// The range and step the completion delay is set over, in milliseconds.
const MAX_COMPLETION_DELAY_MS: f32 = 2000.0;
const COMPLETION_DELAY_STEP_MS: f32 = 50.0;

/// How much one step of the popup input height's control moves it, in rows.
const POPUP_INPUT_STEP: f32 = 0.1;

/// Why a settings-page edit was applied but not saved.
const SETTINGS_NOT_SAVED: &str =
    "settings.kdl has errors: this change applies now, but is not saved until the file is fixed";

// ========================================================================
// App: settings page
// ========================================================================

impl App {
    /// Open the full-window settings page, dismissing any open menu or palette
    /// first. A no-op if it is already open.
    pub(in crate::window) fn open_settings(&mut self) {
        if self.chrome.settings_page().is_some() {
            return;
        }
        self.close_menu();
        let page = self.build_settings_page();
        self.open_overlay(Overlay::Settings(Box::new(page)));
        self.dirty = true;
    }
    /// Side effects of leaving the settings page: redraw what it covered.
    pub(in crate::window) fn on_settings_closed(&mut self) {
        self.dirty = true;
    }
    /// Build the settings page from the live config: one row per editable setting,
    /// pre-filled with the current value.
    pub(in crate::window) fn build_settings_page(&self) -> SettingsPage {
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
            {
                let side_options = vec![
                    ChoiceOption { label: "Left".into(), value: "left".into() },
                    ChoiceOption { label: "Right".into(), value: "right".into() },
                ];
                let idx = match self.config.gutter_side {
                    GutterSide::Left => 0,
                    GutterSide::Right => 1,
                };
                SettingsField::choice("gutter_side", "Gutter", side_options, idx)
                    .with_note("Edge of a listing's git states and marks")
            },
            {
                let indent_options = vec![
                    ChoiceOption { label: "None".into(), value: "none".into() },
                    ChoiceOption { label: "Same as the line".into(), value: "same".into() },
                    ChoiceOption { label: "To the text (lists, headings, comments)".into(), value: "semantic".into() },
                ];
                let idx = match self.config.editor_wrap_indent {
                    WrapIndent::None => 0,
                    WrapIndent::Same => 1,
                    WrapIndent::Semantic => 2,
                };
                SettingsField::choice("editor_wrap_indent", "Wrapped line indent", indent_options, idx)
                    .with_note("Where the text editor starts the rows a long line wraps onto")
            },
            {
                let marker_options = vec![
                    ChoiceOption { label: "Always hidden".into(), value: "always-hidden".into() },
                    ChoiceOption { label: "Shown where the cursor is".into(), value: "show-on-cursor".into() },
                    ChoiceOption { label: "Visible".into(), value: "visible".into() },
                ];
                let idx = match self.config.editor_markup_markers {
                    MarkupMarkers::AlwaysHidden => 0,
                    MarkupMarkers::ShowOnCursor => 1,
                    MarkupMarkers::Visible => 2,
                };
                SettingsField::choice("editor_markup_markers", "Markup markers", marker_options, idx)
                    .with_note("The ** of **bold**, LaTeX's \\textbf{ and }, and their kin, hidden as magic-vscode hides them")
            },
            SettingsField::number(
                "scroll_margin",
                "Scroll margin",
                self.config.scroll_margin as f32,
                0.0,
                MAX_SCROLL_MARGIN as f32,
                1.0,
                0,
            )
            .with_note("Rows an editor keeps above and below the cursor as it scrolls, held under half the pane"),
            SettingsField::toggle("status.enabled", "Show status bar", status.enabled)
                .in_section("Status bar"),
            SettingsField::toggle("status.show_mode", "Show mode indicator", status.show_mode),
            SettingsField::toggle(
                "notifications.toasts",
                "Show warnings and errors as toasts",
                self.config.notifications.toasts,
            )
            .with_note("Cards in the top-right corner; off says them in the status bar"),
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
                    .position(|&v| v == cursor_shape_as_value(self.config.cursor.insert))
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
                    .position(|&v| v == cursor_shape_as_value(self.config.cursor.normal))
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
                    .position(|&v| v == cursor_shape_as_value(self.config.cursor.visual))
                    .unwrap_or(0);
                SettingsField::choice("cursor.visual", "Cursor (visual)", cursor_options, idx)
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
                self.config.scrollback_lines.unwrap_or(winter_core::MAX_SCROLLBACK) as f32,
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
                .with_note("Indent wrapped scrollback lines to match the line's indent, after a width change"),
            SettingsField::toggle("wrap_words", "Wrap Words", self.config.wrap_words)
                .in_section("Terminal")
                .with_note("Rewrap scrollback at word boundaries after a width change"),
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
            SettingsField::number(
                "popup_input_height",
                "Popup input height",
                self.config.popup_input_height,
                winter_render::POPUP_INPUT_HEIGHT_RANGE.0,
                winter_render::POPUP_INPUT_HEIGHT_RANGE.1,
                POPUP_INPUT_STEP,
                1,
            )
            .in_section("Window")
            .with_note("How tall the field typed into is in every popup, in rows of the font"),
            SettingsField::toggle(
                "restore_session",
                "Restore session on launch",
                self.config.restore_session,
            )
            .in_section("Window")
            .with_note("Reopen the previous tab and pane layout"),
            SettingsField::toggle(
                "compile.dock",
                "Show compilation in dock",
                self.config.compile.dock,
            )
            .in_section("Compile")
            .with_note("Open the Compile tab in the dock along the bottom of the window"),
            SettingsField::toggle(
                "compile.jump_to_error",
                "Open a failed build's first error",
                self.config.compile.jump_to_error,
            )
            .in_section("Compile")
            .with_note("In the file you are in, the way Emacs' auto-jump does"),
            SettingsField::text(
                "compile.latex_command",
                "LaTeX build command",
                self.config.compile.latex_command.clone(),
            )
            .in_section("Compile")
            .with_note("The document's root file name is added after it"),
            SettingsField::toggle(
                "compile.latex_on_save",
                "Build LaTeX on save",
                self.config.compile.latex_on_save,
            )
            .in_section("Compile")
            .with_note("Saving a .tex file rebuilds its document in the Compile tab"),
            SettingsField::toggle(
                "completion.auto",
                "Open completion while typing",
                self.config.completion.auto,
            )
            .in_section("Completion")
            .with_note("Without it, only C-SPC opens the popup"),
            SettingsField::number(
                "completion.delay_ms",
                "Pause before it opens (ms)",
                self.config.completion.delay.as_millis() as f32,
                0.0,
                MAX_COMPLETION_DELAY_MS,
                COMPLETION_DELAY_STEP_MS,
                0,
            )
            .in_section("Completion")
            .with_note("C-SPC opens it at once whatever this is"),
            SettingsField::number(
                "completion.min_chars",
                "Letters before it opens",
                self.config.completion.min_chars as f32,
                MIN_COMPLETION_CHARS,
                MAX_COMPLETION_CHARS,
                1.0,
                0,
            )
            .in_section("Completion")
            .with_note("Which back-ends each language asks is set in settings.kdl"),
            SettingsField::toggle("copilot.enable", "Copilot", self.config.copilot.enable)
                .in_section("Copilot")
                .with_note("GitHub Copilot's suggestions, through copilot-language-server and the sign-in on this machine"),
            SettingsField::toggle("copilot.completions", "Ghost text as you type", self.config.copilot.completions)
                .with_note("Tab takes it, M-f a word, C-. a line, C-g hides it"),
            SettingsField::toggle("copilot.next_edit", "Next edit suggestions", self.config.copilot.next_edit)
                .with_note("After a change, the edit Copilot expects next, for Tab to make"),
            SettingsField::text("copilot.server_path", "Server path", self.config.copilot.server_path.clone())
                .with_note("copilot-language-server to run; empty to look for one"),
            SettingsField::text(
                "todo_keywords",
                "Todo keywords",
                self.config.todo_keywords.join(" "),
            )
            .in_section("Todo")
            .with_note("Separated by spaces; each found as a whole word in its own case"),
            SettingsField::toggle(
                "clipboard_read",
                "Allow clipboard reads (OSC 52)",
                self.config.clipboard_read,
            )
            .in_section("Security")
            .with_note("Off by default: the query is silent, so any program in the pane, including one behind ssh, could read what you last copied"),
        ];
        let page = SettingsPage::new(fields);
        match &self.workspace.active {
            Some(name) => {
                let overridden = self.workspace.settings.keys().cloned().collect();
                page.with_workspace(name.clone(), overridden)
            }
            None => page,
        }
    }
    /// Route one key to the open settings page. Returns whether the page should
    /// stay open (`false` on Enter/Escape). Each value change is applied and
    /// persisted immediately, so the window previews it live.
    pub(in crate::window) fn handle_settings_input(
        &mut self,
        page: &mut SettingsPage,
        key: &Key,
        typed: &input::Key,
    ) -> bool {
        let closes = matches!(key, Key::Named(NamedKey::Escape | NamedKey::Enter));
        let typed = typed.typed();
        if typed.chord() == (input::Mods::CTRL, input::KeyCode::Char('g')) {
            return false;
        }
        // The scope is chosen and a workspace's setting dropped ahead of a
        // text row, which would take both keys as typing.
        if matches!(key, Key::Named(NamedKey::Tab)) {
            page.toggle_scope();
            return true;
        }
        if typed.chord() == (input::Mods::SHIFT, input::KeyCode::Delete)
            && page.scope == Scope::Workspace
        {
            if let Some(row) = page.selected_key().map(str::to_string) {
                self.reset_workspace_setting(&row);
                self.refresh_settings_page(page);
            }
            return true;
        }
        // A text row is a field, and takes a field's Emacs keys: everything
        // but the keys that close the page and move between rows.
        let moves = matches!(key, Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown));
        if page.selected_is_text() && !closes && !moves {
            if let Some((k, v)) = page.edit_text(&typed) {
                self.apply_page_edit(page, &k, &v);
            }
            return true;
        }
        match key {
            Key::Named(NamedKey::Escape) | Key::Named(NamedKey::Enter) => return false,
            Key::Named(NamedKey::ArrowUp) => page.move_up(),
            Key::Named(NamedKey::ArrowDown) => page.move_down(),
            Key::Named(NamedKey::ArrowLeft) => {
                if let Some((k, v)) = page.adjust(false) {
                    self.apply_page_edit(page, &k, &v);
                }
            }
            Key::Named(NamedKey::ArrowRight) => {
                if let Some((k, v)) = page.adjust(true) {
                    self.apply_page_edit(page, &k, &v);
                }
            }
            // winit reports the space bar as a named key, not a character:
            // off a text row it flips the toggle or steps the control.
            Key::Named(NamedKey::Space) => {
                if let Some((k, v)) = page.adjust(true) {
                    self.apply_page_edit(page, &k, &v);
                }
            }
            _ => {}
        }
        true
    }
    /// Turn hiding markup's markers on and off (`always` unset: between
    /// visible and shown where the cursor is), or always hiding them on and
    /// off (`always`: between always and shown where the cursor is), as
    /// magic-vscode's two toggles do, and keep the choice.
    pub(in crate::window) fn toggle_markup_markers(&mut self, always: bool) {
        let now = self.config.editor_markup_markers;
        let next = match (always, now) {
            (false, MarkupMarkers::Visible) | (true, MarkupMarkers::AlwaysHidden) => MarkupMarkers::ShowOnCursor,
            (false, _) => MarkupMarkers::Visible,
            (true, _) => MarkupMarkers::AlwaysHidden,
        };
        self.apply_settings_edit("editor_markup_markers", next.as_value());
        let said = match next {
            MarkupMarkers::Visible => "visible",
            MarkupMarkers::ShowOnCursor => "shown where the cursor is",
            MarkupMarkers::AlwaysHidden => "always hidden",
        };
        self.set_notice(format!("Markup markers: {said}"));
    }

    /// Apply one settings edit to the live config and persist it.
    pub(in crate::window) fn apply_settings_edit(&mut self, key: &str, value: &str) {
        if self.apply_setting(key, value) {
            // While settings.kdl does not parse, what is in force is not what
            // the file says, and saving would write that over the user's own
            // file: the edit applies, the file is left for them to fix.
            if self.runtime.config_watch.settings_rejected {
                self.set_error(SETTINGS_NOT_SAVED);
            } else if let Err(e) = self.config.save() {
                self.set_error(format!("could not save settings.kdl: {e}"));
            }
        }
        self.dirty = true;
    }
    /// Apply one settings edit to the live config and perform any renderer or
    /// layout refresh it implies. Returns whether the config changed (and so
    /// should be persisted); an unparseable value leaves the config untouched.
    pub(in crate::window) fn apply_setting(&mut self, key: &str, value: &str) -> bool {
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
            // Held to what the renderer can draw before it is saved: a zero
            // or negative size written to settings.kdl would stop every launch.
            "font_size" => match value.parse::<f32>() {
                Ok(size) => {
                    // A size chosen here is the setting, and ends any zoom.
                    self.config.font_size = winter_render::clamp_font_size(size);
                    self.reset_font_size();
                }
                Err(_) => return false,
            },
            "opacity" => match value.parse::<f32>() {
                Ok(opacity) => {
                    self.config.opacity = opacity.clamp(0.1, 1.0);
                    self.sync_translucency();
                    self.dirty = true;
                }
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
            "compile.latex_command" => {
                let trimmed = value.trim();
                self.config.compile.latex_command = match trimmed.is_empty() {
                    true => crate::config::CompileConfig::default().latex_command,
                    false => trimmed.to_string(),
                };
            }
            "todo_keywords" => {
                self.config.todo_keywords =
                    crate::config::todo_keywords_from_names(vec![value.to_string()]);
            }
            "compile.dock" => {
                self.config.compile.dock = value == "true";
            }
            "compile.jump_to_error" => {
                self.config.compile.jump_to_error = value == "true";
            }
            "completion.auto" => {
                self.config.completion.auto = value == "true";
            }
            "copilot.enable" => self.config.copilot.enable = value == "true",
            "copilot.completions" => self.config.copilot.completions = value == "true",
            "copilot.next_edit" => self.config.copilot.next_edit = value == "true",
            "copilot.server_path" => self.config.copilot.server_path = value.trim().to_string(),
            "completion.delay_ms" => match value.parse::<f32>() {
                Ok(ms) => {
                    let ms = ms.clamp(0.0, MAX_COMPLETION_DELAY_MS) as u64;
                    self.config.completion.delay = std::time::Duration::from_millis(ms);
                }
                Err(_) => return false,
            },
            "completion.min_chars" => match value.parse::<f32>() {
                Ok(chars) => {
                    self.config.completion.min_chars =
                        chars.clamp(MIN_COMPLETION_CHARS, MAX_COMPLETION_CHARS) as usize;
                }
                Err(_) => return false,
            },
            "compile.latex_on_save" => {
                self.config.compile.latex_on_save = value == "true";
            }
            "notifications.toasts" => {
                self.config.notifications.toasts = value == "true";
                self.dirty = true;
            }
            "cursor.insert" => {
                self.config.cursor.insert = cursor_shape_from_value(value);
                self.dirty = true;
            }
            "cursor.normal" => {
                self.config.cursor.normal = cursor_shape_from_value(value);
                self.dirty = true;
            }
            "cursor.visual" => {
                self.config.cursor.visual = cursor_shape_from_value(value);
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
            "scroll_margin" => match value.parse::<f32>() {
                Ok(n) if n >= 0.0 => {
                    self.config.scroll_margin = (n as usize).min(MAX_SCROLL_MARGIN);
                    self.dirty = true;
                }
                _ => return false,
            },
            "scrollback_lines" => match value.parse::<f32>() {
                Ok(n) if n >= 1.0 => {
                    self.config.scrollback_lines = Some(n as usize);
                }
                _ => return false,
            },
            "cursor.blink" => {
                self.config.cursor.blink = value == "true";
                if !self.config.cursor.blink {
                    self.blink.phase = true;
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
                self.overrides.rainbow_parens = None;
                self.dirty = true;
            }
            "sentence_highlight" => {
                self.config.sentence_highlight = value == "true";
                self.overrides.sentence_highlight = None;
                self.dirty = true;
            }
            "url_underline" => {
                self.config.url_underline = value == "true";
                self.dirty = true;
            }
            // Every pane picks these up on the next drain (see `drain_all_panes`).
            "wrap_indent" => {
                self.config.wrap_indent = value == "true";
                self.dirty = true;
            }
            "wrap_words" => {
                self.config.wrap_words = value == "true";
                self.dirty = true;
            }
            "gutter_side" => {
                self.config.gutter_side = GutterSide::from_value(value);
                self.dirty = true;
            }
            "editor_markup_markers" => {
                self.config.editor_markup_markers = MarkupMarkers::from_value(value);
            }
            "editor_wrap_indent" => {
                self.config.editor_wrap_indent = WrapIndent::from_value(value);
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
                    Config::default().window_title_template
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
            "popup_input_height" => match value.parse::<f32>() {
                Ok(rows) => {
                    let (least, most) = winter_render::POPUP_INPUT_HEIGHT_RANGE;
                    self.config.popup_input_height = rows.clamp(least, most);
                    if let Some(renderer) = &mut self.renderer {
                        renderer.set_popup_input_height(self.config.popup_input_height);
                    }
                    self.dirty = true;
                }
                Err(_) => return false,
            },
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
            _ => return false,
        }
        true
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The theme dropdown choices for the settings page: the three built-ins, then
/// each `themes/*.kdl` file by name. Mirrors the values [`ThemeSetting`](crate::config::ThemeSetting) accepts.
fn settings_theme_options() -> Vec<ChoiceOption> {
    let mut options = vec![
        ChoiceOption {
            label: "Dark".into(),
            value: "dark".into(),
        },
        ChoiceOption {
            label: "Light".into(),
            value: "light".into(),
        },
        ChoiceOption {
            label: "Auto (follow system)".into(),
            value: "auto".into(),
        },
    ];
    for name in crate::config::available_themes() {
        options.push(ChoiceOption {
            label: name.clone(),
            value: name,
        });
    }
    options
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::settings_page::Control;

    /// Settings no single row can stand for, and why.
    const NOT_ROWS: [&str; 12] = [
        // A whole color table needs an editor of its own, not a row.
        "colors",
        // Chords live in their own file, and the keys editor lists them.
        "keybindings",
        // Written through the one `shell` row, which saves the key for this OS.
        "shell_linux",
        "shell_macos",
        "shell_windows",
        // Nested blocks, checked field by field below.
        "compile",
        "completion",
        "copilot",
        // Lists of languages and commands, which no row edits.
        "lsp",
        "cursor",
        "notifications",
        "status_bar",
    ];

    /// Rows whose key differs from the setting they write.
    const RENAMED: [(&str, &str); 1] = [("font", "font_family")];

    /// The fields of one schema struct, read from the schema's own source so the
    /// list cannot fall behind it.
    fn schema_fields(name: &str) -> Vec<String> {
        let schema = include_str!("../../config/schema.rs");
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
        // Cursor and notification rows are named for their fields. The status bar's
        // are named `status.enabled` and `status.show_mode`, so that block is
        // checked by name instead of read from the schema.
        let keys = row_keys();
        for (block, prefix) in [
            ("KdlCursor", "cursor."),
            ("KdlNotifications", "notifications."),
            ("KdlCompile", "compile."),
            ("KdlCopilot", "copilot."),
        ] {
            for field in schema_fields(block) {
                let row = format!("{prefix}{field}");
                assert!(keys.contains(&row), "no settings row writes: {row}");
            }
        }
        // A language's back-end list has no row: it is set in the file.
        for row in ["status.enabled", "status.show_mode", "completion.auto", "completion.min_chars"] {
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

    #[test]
    fn test_an_edit_applies_but_is_not_saved_over_a_settings_file_that_does_not_parse() {
        // Saving here would write the in-force settings (defaults, after the
        // file failed) over the user's own file, typo and all.
        let mut app = App::new();
        app.runtime.config_watch.settings_rejected = true;
        app.apply_settings_edit("font_size", "20");
        assert_eq!(app.config.font_size, 20.0, "applied");
        let notice = app.chrome.notice_text();
        assert_eq!(notice, Some(SETTINGS_NOT_SAVED), "and said so, not saved");
    }
}
