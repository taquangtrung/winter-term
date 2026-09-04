//! Runtime appearance changes: font size, themes, and display toggles.

use crate::config::Config;
use crate::model::input::WindowKeymap;
use winter_render::Theme;

use super::App;

// ========================================================================
// App: appearance
// ========================================================================

impl App {
    /// Drain pending config-directory filesystem events and, if any arrived,
    /// reload and apply the new settings, including re-resolving the theme's
    /// colors. A single save can raise several events (e.g. an editor's
    /// write-temp-then-rename), so every pending one is drained first and
    /// applied as one reload rather than one per event. Returns true when a
    /// reload occurred.
    pub(crate) fn reload_config_if_changed(&mut self) -> bool {
        let Some(rx) = &self.config_watch_rx else {
            return false;
        };
        let mut changed = false;
        while rx.try_recv().is_ok() {
            changed = true;
        }
        if !changed {
            return false;
        }
        let (config, config_error) = Config::load_checked();
        self.config = config;
        if let Some(message) = config_error {
            self.set_error(message);
        }
        self.window_keymap = WindowKeymap::from_config(
            self.config.keybindings.get("window"),
            self.config.keybindings.get("editing"),
        );
        let ligatures = self.config.ligatures;
        let pane_border_width = self.config.pane_border_width;
        if let Some(r) = &mut self.renderer {
            r.set_ligatures(ligatures);
            r.set_divider_width(pane_border_width);
        }
        if !self.config.cursor.blink {
            self.blink_phase = true;
        }
        // An edited `window-title-template` in settings.kdl takes effect
        // without restart.
        self.update_window_title();
        self.rebuild_theme();
        self.resize_all_panes();
        true
    }
    /// Adjust font size by `delta` points; returns true when actually changed.
    pub(crate) fn change_font_size(&mut self, delta: f32) -> bool {
        let new_size = (self.config.font_size + delta).clamp(6.0, 72.0);
        self.change_font_size_to(new_size)
    }
    /// Set font size to `logical_size` points; returns true when actually changed.
    pub(crate) fn change_font_size_to(&mut self, logical_size: f32) -> bool {
        let Some(renderer) = &mut self.renderer else {
            return false;
        };
        if renderer.set_font_size(logical_size).is_none() {
            return false;
        }
        self.config.font_size = logical_size;
        self.resize_all_panes();
        self.dirty = true;
        true
    }
    pub(crate) fn toggle_rainbow_parens(&mut self) {
        self.config.rainbow_parens = !self.config.rainbow_parens;
        let state = if self.config.rainbow_parens {
            "enabled"
        } else {
            "disabled"
        };
        self.set_notice(format!("rainbow parens {state}"));
        self.dirty = true;
    }
    pub(crate) fn toggle_sentence_highlight(&mut self) {
        self.config.sentence_highlight = !self.config.sentence_highlight;
        let state = if self.config.sentence_highlight {
            "enabled"
        } else {
            "disabled"
        };
        self.set_notice(format!("sentence highlight {state}"));
        self.dirty = true;
    }
    /// Rebuild the renderer theme from the current `config.theme` selection plus
    /// any color overrides, and request a redraw. Shared by the theme menu
    /// commands and the live settings page.
    pub(crate) fn rebuild_theme(&mut self) {
        use crate::config::ThemeSetting;
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let mut theme = match &self.config.theme {
            ThemeSetting::Dark => Theme::dark(),
            ThemeSetting::Light => Theme::light(),
            ThemeSetting::Auto => self
                .window
                .as_ref()
                .and_then(|w| w.theme())
                .map(|t| match t {
                    winit::window::Theme::Dark => Theme::dark(),
                    winit::window::Theme::Light => Theme::light(),
                })
                .unwrap_or_default(),
            // A user theme file; fall back to the dark preset if it is missing.
            ThemeSetting::Named(name) => {
                crate::config::load_named_theme(name).unwrap_or_else(Theme::dark)
            }
        };
        self.config.colors.apply(&mut theme);
        renderer.set_theme(theme);
        self.dirty = true;
    }
    /// Validate `name`, save the currently active (resolved) theme's colors as
    /// `themes/<name>.kdl`, and switch to it. Reports success or failure as a
    /// status notice. Like the `theme_dark`/`theme_light`/`theme_auto` quick
    /// commands, this only previews the switch in `config.theme`; it does not
    /// persist the selection to `settings.kdl`.
    pub(crate) fn create_named_theme(&mut self, name: &str) {
        let name = name.trim();
        if !crate::config::is_valid_theme_name(name) {
            self.set_error("Theme name must use only letters, numbers, - or _");
            return;
        }
        let theme = self
            .renderer
            .as_ref()
            .map_or_else(Theme::default, |r| r.theme().clone());
        match crate::config::save_named_theme(name, &theme) {
            Ok(()) => {
                self.config.theme = crate::config::ThemeSetting::Named(name.to_string());
                self.rebuild_theme();
                self.set_notice(format!("Created theme \"{name}\""));
            }
            Err(e) => self.set_error(e.to_string()),
        }
    }
}
