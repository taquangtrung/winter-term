//! Runtime appearance changes: font size, themes, and display toggles.

use winter_render::renderer::GpuRenderer;
use winter_render::Theme;

use crate::config::Config;
use crate::terminal::session::{Presentation, ThemeColors};
use crate::window::input::sequence::keymap_from_blocks;

use super::App;

// ========================================================================
// Constants
// ========================================================================

/// How long the config directory must be quiet after a change before it is
/// read: long enough for an editor's save to finish, short enough to feel
/// immediate.
const CONFIG_SETTLE: std::time::Duration = std::time::Duration::from_millis(150);

// ========================================================================
// Data Structures
// ========================================================================

/// What this session has changed over the config without it being a setting:
/// a zoom, a toggle from the palette. Kept apart from [`Config`], so saving
/// the settings page never writes them to settings.kdl and a reload of the
/// file never takes them back. `None` follows the config.
#[derive(Clone, Copy, Debug, Default)]
pub(in crate::window) struct SessionOverrides {
    pub(in crate::window) font_size: Option<f32>,
    pub(in crate::window) rainbow_parens: Option<bool>,
    pub(in crate::window) sentence_highlight: Option<bool>,
}

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
    pub(in crate::window) fn reload_config_if_changed(&mut self) -> bool {
        let Some(rx) = &self.runtime.config_watch.rx else {
            return false;
        };
        // Each event pushes the reload back: an editor that saves by
        // truncating and then writing raises one for the empty file first,
        // and an empty file parses, as all defaults. Reading once the
        // directory has been quiet for a moment reads the finished save.
        let now = std::time::Instant::now();
        while rx.try_recv().is_ok() {
            self.runtime.config_watch.reload_due = Some(now + CONFIG_SETTLE);
        }
        match self.runtime.config_watch.reload_due {
            Some(due) if now >= due => self.runtime.config_watch.reload_due = None,
            Some(_) | None => return false,
        }
        let (config, config_error) = Config::load_checked();
        if let Some(message) = config_error {
            // A settings.kdl that does not parse is reported, not applied: the
            // settings in force stay, rather than every one of them falling
            // back to its default over one typo, and the settings page leaves
            // the file alone until it parses (see `apply_settings_edit`).
            self.runtime.config_watch.settings_rejected = true;
            self.set_error(message);
            return true;
        }
        self.runtime.config_watch.settings_rejected = false;
        self.adopt_config(config);
        // The window's workspace sets its own over what the file says.
        self.apply_workspace_settings();
        true
    }

    /// Put a freshly read `config` in force. The session's own overrides (a
    /// zoom, a toggle) stay on top of it, as they were made over whatever
    /// the file said.
    pub(in crate::window) fn adopt_config(&mut self, config: Config) {
        self.config = config;
        if let Some(message) = self.config.keybindings_error.clone() {
            // A keybindings.kdl that does not read is reported, and the keys
            // in force stay: the defaults it would fall back to are not what
            // the user bound, and a half-typed file must not unbind them.
            self.set_error(message);
        } else {
            let (keymap, keymap_error) = keymap_from_blocks(&self.config.keybindings);
            self.keymap = keymap;
            if let Some(message) = keymap_error {
                self.set_error(message);
            }
        }
        if let Some(message) = self.config.settings_warning.clone() {
            self.set_error(message);
        }
        // A size edited in the file takes effect now, unless the session has
        // zoomed away from it.
        self.apply_font_size(self.font_size());
        let ligatures = self.config.ligatures;
        let pane_border_width = self.config.pane_border_width;
        let popup_input_height = self.config.popup_input_height;
        if let Some(r) = &mut self.renderer {
            r.set_ligatures(ligatures);
            r.set_divider_width(pane_border_width);
            r.set_popup_input_height(popup_input_height);
        }
        if !self.config.cursor.blink {
            self.blink.phase = true;
        }
        // An edited `window-title-template` in settings.kdl takes effect
        // without restart.
        self.update_window_title();
        self.rebuild_theme();
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
    }

    // --------------------------------------------------------------------
    // Session overrides
    // --------------------------------------------------------------------

    /// The font size in force: the session's zoom, else the configured one.
    pub(in crate::window) fn font_size(&self) -> f32 {
        self.overrides.font_size.unwrap_or(self.config.font_size)
    }

    /// Whether brackets are colored by depth, as toggled or configured.
    pub(in crate::window) fn rainbow_parens(&self) -> bool {
        self.overrides
            .rainbow_parens
            .unwrap_or(self.config.rainbow_parens)
    }

    /// Whether the sentence under the cursor is lit, as toggled or configured.
    pub(in crate::window) fn sentence_highlight(&self) -> bool {
        self.overrides
            .sentence_highlight
            .unwrap_or(self.config.sentence_highlight)
    }

    /// Adjust font size by `delta` points; returns true when actually changed.
    pub(in crate::window) fn change_font_size(&mut self, delta: f32) -> bool {
        let new_size = winter_render::clamp_font_size(self.font_size() + delta);
        self.change_font_size_to(new_size)
    }

    /// Zoom to `logical_size` points for this session; returns true when
    /// actually changed. The configured size is left as it is: a zoom is
    /// not a setting, and is never written to settings.kdl.
    pub(in crate::window) fn change_font_size_to(&mut self, logical_size: f32) -> bool {
        if !self.apply_font_size(logical_size) {
            return false;
        }
        self.overrides.font_size = (logical_size != self.config.font_size).then_some(logical_size);
        true
    }

    /// Go back to the configured font size, dropping the session's zoom.
    pub(in crate::window) fn reset_font_size(&mut self) {
        self.overrides.font_size = None;
        self.apply_font_size(self.config.font_size);
    }

    /// Draw at `logical_size` points. Without a window yet there is nothing
    /// to redraw, and the size is taken as it stands; with one, `false` when
    /// the renderer is already at that size.
    fn apply_font_size(&mut self, logical_size: f32) -> bool {
        if let Some(renderer) = &mut self.renderer {
            if renderer.set_font_size(logical_size).is_none() {
                return false;
            }
            self.resize_all_panes();
        }
        self.dirty = true;
        true
    }

    pub(in crate::window) fn toggle_rainbow_parens(&mut self) {
        let on = !self.rainbow_parens();
        self.overrides.rainbow_parens = Some(on);
        self.set_notice(format!("rainbow parens {}", toggle_word(on)));
        self.dirty = true;
    }

    pub(in crate::window) fn toggle_sentence_highlight(&mut self) {
        let on = !self.sentence_highlight();
        self.overrides.sentence_highlight = Some(on);
        self.set_notice(format!("sentence highlight {}", toggle_word(on)));
        self.dirty = true;
    }
    /// Rebuild the renderer theme from the current `config.theme` selection plus
    /// any color overrides, and request a redraw. Shared by the theme menu
    /// commands and the live settings page.
    pub(in crate::window) fn rebuild_theme(&mut self) {
        use crate::config::ThemeSetting;
        if self.renderer.is_none() {
            return;
        }
        let mut problem = None;
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
            // A user theme file; the dark preset, and a word saying why, when
            // it cannot be read.
            ThemeSetting::Named(name) => match crate::config::load_named_theme(name) {
                Ok(theme) => theme,
                Err(why) => {
                    problem = Some(why);
                    Theme::dark()
                }
            },
        };
        self.config.colors.apply(&mut theme);
        if let Some(renderer) = &mut self.renderer {
            renderer.set_theme(theme);
        }
        if let Some(why) = problem {
            self.set_error(why);
        }
        self.sync_translucency();
        self.dirty = true;
    }
    /// Hand the configured opacity to the renderer and make the window
    /// transparent exactly when the background is see-through, since a
    /// transparent window is recomposited by many compositors even when
    /// nothing shows through it.
    pub(in crate::window) fn sync_translucency(&mut self) {
        let (Some(renderer), Some(window)) = (&mut self.renderer, &self.window) else {
            return;
        };
        renderer.set_window_opacity(self.config.opacity);
        window.set_transparent(renderer.is_translucent());
    }
    /// Validate `name`, save the currently active (resolved) theme's colors as
    /// `themes/<name>.kdl`, and switch to it. Reports success or failure as a
    /// status notice. Like the `theme_dark`/`theme_light`/`theme_auto` quick
    /// commands, this only previews the switch in `config.theme`; it does not
    /// persist the selection to `settings.kdl`.
    pub(in crate::window) fn create_named_theme(&mut self, name: &str) {
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

// ========================================================================
// Free functions
// ========================================================================

/// How a notice names a toggle's new state.
fn toggle_word(on: bool) -> &'static str {
    match on {
        true => "enabled",
        false => "disabled",
    }
}

/// How `renderer` shows a terminal, for the terminal to answer programs'
/// size and color queries with (see `Terminal::set_presentation`).
pub(in crate::window) fn terminal_presentation(renderer: &GpuRenderer) -> Presentation {
    let (cell_width_px, cell_height_px) = renderer.cell_size();
    Presentation {
        cell_height_px,
        cell_width_px,
        colors: ThemeColors::from_theme(renderer.theme()),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::mpsc;
    use std::time::Instant;

    #[test]
    fn test_a_zoom_is_the_sessions_and_never_the_settings_files() {
        // Regression: a zoom changed the config itself, so the next save from
        // the settings page wrote it to settings.kdl for good.
        let mut app = App::new();
        let configured = app.config.font_size;
        assert!(app.change_font_size(2.0));
        assert_eq!(app.font_size(), configured + 2.0);
        assert_eq!(app.config.font_size, configured, "the setting is untouched");
        assert!(
            app.config
                .to_kdl()
                .contains(&format!("font-size {configured}\n")),
            "and a save writes the setting, not the zoom"
        );

        app.reset_font_size();
        assert_eq!(app.font_size(), configured);
    }

    #[test]
    fn test_a_reload_of_the_settings_file_keeps_the_sessions_overrides() {
        // Regression: a reload put the file's font size back in the config
        // while the window stayed zoomed, and the next step jumped from it.
        let mut app = App::new();
        app.change_font_size(3.0);
        app.toggle_rainbow_parens();
        let zoomed = app.font_size();
        let toggled = app.rainbow_parens();

        app.adopt_config(Config::default());
        assert_eq!(app.font_size(), zoomed);
        assert_eq!(app.rainbow_parens(), toggled);
    }

    #[test]
    fn test_a_keybindings_file_that_does_not_read_leaves_the_keys_in_force() {
        let mut window = HashMap::new();
        window.insert("M-z".to_string(), "new_tab".to_string());
        let mut config = Config::default();
        config.keybindings.insert("window".to_string(), window);
        config.keybindings_error = Some("keybindings.kdl: line 1: unexpected token".to_string());

        let mut app = App::new();
        let before = app.keymap.clone();
        app.adopt_config(config.clone());
        assert_eq!(app.keymap, before, "nothing rebound over a broken file");
        let notice = app.chrome.notice_text();
        assert_eq!(notice, Some("keybindings.kdl: line 1: unexpected token"));

        config.keybindings_error = None;
        app.adopt_config(config);
        assert_ne!(app.keymap, before, "a file that reads is applied");
    }

    #[test]
    fn test_a_config_change_is_read_once_the_directory_has_gone_quiet() {
        // An editor saving by truncate-then-write raises an event for the
        // empty file first; reading at once applied an empty settings.kdl.
        let mut app = App::new();
        let (tx, rx) = mpsc::channel();
        app.runtime.config_watch.rx = Some(rx);
        tx.send(()).expect("event");
        assert!(
            !app.reload_config_if_changed(),
            "not while the save may be going on"
        );
        assert!(app.runtime.config_watch.reload_due.is_some());
        app.runtime.config_watch.reload_due = Some(Instant::now());
        assert!(app.reload_config_if_changed(), "read once quiet");
        assert!(app.runtime.config_watch.reload_due.is_none());
    }
}
