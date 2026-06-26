//! Window title composition and the highlight a yank leaves.

use std::time::Instant;

use crate::config::{expand_window_title_template, WindowTitleVars};
use crate::model::input::PendingPrefix;

use super::strip::StripHit;
use super::tabbar;
use super::App;
use super::YANK_FLASH_DURATION;

// ========================================================================
// App: title and yank flash
// ========================================================================

impl App {
    /// Render the `window-title-template` config against the focused tab: the
    /// app running in it feeds the `{{ title }}`,
    /// `{{ app_name }}`, `{{ pane_title }}`, and `{{ cwd }}` placeholders.
    pub(crate) fn render_window_title(&self) -> String {
        let focused = self.layout().focused();
        let terminal = self.open_tabs.terminal(&focused);
        expand_window_title_template(
            &self.config.window_title_template,
            &WindowTitleVars {
                title: self.tab_title(focused),
                app_name: terminal
                    .and_then(|p| p.foreground_process_name())
                    .or_else(|| {
                        self.open_tabs
                            .tool(&focused)
                            .map(|slot| slot.kind().title().to_string())
                    })
                    .unwrap_or_default(),
                pane_title: self
                    .tabs
                    .program_title(focused)
                    .map(tabbar::clean_title)
                    .unwrap_or_default(),
                cwd: terminal
                    .and_then(|p| p.cwd().map(|c| tabbar::clean_cwd(&c)))
                    .or_else(|| {
                        self.open_tabs
                            .tool(&focused)
                            .and_then(|slot| slot.tool.cwd())
                            .map(|p| tabbar::clean_cwd(&p.to_string_lossy()))
                    })
                    .unwrap_or_default(),
            },
        )
    }
    pub(crate) fn update_window_title(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };

        let title = if self.settings_page().is_some() {
            "Winter - Settings".to_string()
        } else if let Some(palette) = self.palette() {
            let selected = palette.selected_action().unwrap_or("");
            format!("Winter - palette: > {} [{}]", palette.query, selected)
        } else if self.search.query.is_some() || self.pending == PendingPrefix::SearchInput {
            let query = self.search.query.as_deref().unwrap_or("");
            let prefix = if self.search.reverse { '?' } else { '/' };
            format!("Winter - search: {prefix}{query}")
        } else if self.quick_select.is_some() {
            "Winter - quick select".to_string()
        } else if let StripHit::Tab(pane) = self.tabs.strip_hover {
            let t = self.tab_title(pane);
            format!("Winter - {t}")
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
    /// Leave the span the last yank took lit, so what landed in the register
    /// is visible rather than only reported. The span is already the
    /// selection: what this adds is the deadline it goes away at.
    pub(crate) fn flash_yank(&mut self) {
        let Some(span) = self.selection.span.clone() else {
            return;
        };
        self.yank_flash = Some((span, Instant::now() + YANK_FLASH_DURATION));
        self.dirty = true;
    }

    /// Put out a yank flash that has burned out, and report when the live one
    /// expires so the event loop can wake to clear it.
    ///
    /// Only a selection still showing exactly what the yank took is cleared: a
    /// drag or a Visual selection made since owns the span now, and taking it
    /// away under the user would be worse than a highlight that overstays.
    pub(crate) fn expire_yank_flash(&mut self) -> Option<Instant> {
        let (span, deadline) = self.yank_flash.as_ref()?;
        if Instant::now() < *deadline {
            return Some(*deadline);
        }
        if self.selection.span.as_ref() == Some(span) {
            self.selection.span = None;
        }
        self.yank_flash = None;
        self.dirty = true;
        None
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_window_title_expands_pane_placeholders() {
        let mut app = App::new();
        let focused = app.layout().focused();
        // An OSC 0/2 title set by the running app (e.g. butterfly naming its
        // open file) resolves into both `pane_title` and the tab `title`.
        app.tabs
            .set_program_title(focused, "butterfly — Winter Term.md".into());

        app.config.window_title_template = "{{ app_name }} — {{ pane_title }} [{{ title }}]".into();
        // No PTY is attached in this fixture, so app_name stays empty.
        assert_eq!(
            app.render_window_title(),
            " — butterfly — Winter Term.md [butterfly — Winter Term.md]"
        );

        app.config.window_title_template = "Winter - {{ title }}".into();
        assert_eq!(
            app.render_window_title(),
            "Winter - butterfly — Winter Term.md"
        );
    }
}
