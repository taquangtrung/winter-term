//! Window title composition.

use crate::config::{expand_window_title_template, WindowTitleVars};
use crate::model::input::PendingPrefix;

use super::{clean_cwd, clean_title, home_relative};
use crate::window::strip::StripHit;
use crate::window::App;

// ========================================================================
// App: window title
// ========================================================================

impl App {
    /// Render the `window-title-template` config against the focused tab: the
    /// app running in it feeds the `{{ title }}`,
    /// `{{ app_name }}`, `{{ pane_title }}`, and `{{ cwd }}` placeholders.
    pub(in crate::window) fn render_window_title(&self) -> String {
        let focused = self.layout().focused();
        let facts = self.open_tabs.terminal(&focused).map(|terminal| terminal.title_facts());
        let (name, location, path) = self.title_place();
        expand_window_title_template(
            &self.config.window_title_template,
            &WindowTitleVars {
                title: self.title_for_window(focused),
                app_name: facts
                    .as_ref()
                    .and_then(|facts| facts.program.clone())
                    .or_else(|| {
                        self.open_tabs
                            .editor(&focused)
                            .map(|slot| slot.kind().title().to_string())
                    })
                    .unwrap_or_default(),
                pane_title: self
                    .tabs
                    .program_title(focused)
                    .map(|title| clean_title(title, facts.as_ref().and_then(|facts| facts.host.remote_name())))
                    .unwrap_or_default(),
                cwd: facts
                    .as_ref()
                    .and_then(|facts| facts.cwd.as_deref().map(clean_cwd))
                    .or_else(|| {
                        self.open_tabs
                            .editor(&focused)
                            .and_then(|slot| slot.editor.cwd())
                            .map(|p| clean_cwd(&p.to_string_lossy()))
                    })
                    .unwrap_or_default(),
                location,
                name,
                path,
            },
        )
    }

    /// What the focused tab shows, for the title: by name, the directory
    /// it is in, and the whole place, home written `~`. A terminal shows
    /// its program over its working directory; an editor showing no place
    /// on disk is named by its kind.
    fn title_place(&self) -> (String, String, String) {
        let focused = self.layout().focused();
        if let Some(slot) = self.open_tabs.editor(&focused) {
            let Some(path) = slot.editor.address().filter(|path| !path.is_empty()) else {
                return (slot.kind().title().to_string(), String::new(), String::new());
            };
            let place = std::path::Path::new(&path);
            let name = place
                .file_name()
                .map_or_else(|| path.clone(), |name| name.to_string_lossy().to_string());
            let location = place
                .parent()
                .map(|parent| parent.to_string_lossy().to_string())
                .unwrap_or_default();
            return (name, location, path);
        }
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return (String::new(), String::new(), String::new());
        };
        let facts = terminal.title_facts();
        let name = facts.program.unwrap_or_else(|| self.title_for_window(focused));
        let location = facts.cwd.map(|cwd| home_relative(&cwd)).unwrap_or_default();
        (name, location.clone(), location)
    }
    pub(in crate::window) fn update_window_title(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };

        let title = if self.chrome.settings_page().is_some() {
            "Settings — Winter Term".to_string()
        } else if let Some(palette) = self.chrome.picker() {
            let selected = palette.selected_action().unwrap_or("");
            format!("palette: > {} [{}] — Winter Term", palette.query, selected)
        } else if self.search.query.is_some() || self.input.keys.prefix == PendingPrefix::SearchInput {
            let query = self.search.query.as_deref().unwrap_or("");
            let prefix = if self.search.reverse { '?' } else { '/' };
            format!("search: {prefix}{query} — Winter Term")
        } else if self.chrome.labels.quick.is_some() {
            "quick select — Winter Term".to_string()
        } else if let StripHit::Tab(pane) = self.tabs.strip_hover {
            let t = self.title_for_window(pane);
            format!("{t} — Winter Term")
        } else {
            self.render_window_title()
        };

        // The window manager call is an IPC round-trip; only make it when the
        // title actually changes, not on every keystroke or PTY poll.
        if title != self.window_title {
            window.set_title(&title);
            self.window_title = title;
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
    fn test_the_default_title_names_what_the_tab_shows_then_winter_term() {
        let dir = std::env::temp_dir().join(format!("winter-title-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("main.rs");
        std::fs::write(&file, "fn main() {}\n").expect("temp file");
        let mut app = App::new();
        app.config.window_title_template = crate::config::Config::default().window_title_template;
        app.open_text_editor(crate::model::editor::OpenTarget::file(file));
        assert_eq!(app.render_window_title(), "main.rs — Winter Term");
        // A tab showing no place on disk is named by its kind alone.
        app.open_notifications();
        assert_eq!(app.render_window_title(), "Notifications — Winter Term");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
