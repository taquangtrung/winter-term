//! Window title composition and the transient status-bar notices.

use std::time::Instant;

use crate::config::{expand_window_title_template, WindowTitleVars};
use crate::model::input::PendingPrefix;
use winter_render::NoticeKind;

use super::tabbar;
use super::App;
use super::NOTICE_DURATION;

// ========================================================================
// App: title and notices
// ========================================================================

impl App {
    /// Render the `window-title-template` config against the active tab: the
    /// app running in the focused pane feeds the `{{ title }}`,
    /// `{{ app_name }}`, `{{ pane_title }}`, and `{{ cwd }}` placeholders.
    pub(crate) fn render_window_title(&self) -> String {
        let idx = self.active_tab;
        let focused = self.tabs[idx].focused();
        let pane = self.panes.get(&focused);
        expand_window_title_template(
            &self.config.window_title_template,
            &WindowTitleVars {
                title: self.tab_title(idx),
                app_name: pane
                    .and_then(|p| p.foreground_process_name())
                    .unwrap_or_default(),
                pane_title: self
                    .pane_titles
                    .get(&focused)
                    .map(|t| tabbar::clean_title(t))
                    .unwrap_or_default(),
                cwd: pane
                    .and_then(|p| p.cwd().map(|c| tabbar::clean_cwd(&c)))
                    .unwrap_or_default(),
            },
        )
    }
    pub(crate) fn update_window_title(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };

        let title = if self.settings_page.is_some() {
            "Winter - Settings".to_string()
        } else if let Some(palette) = &self.palette {
            let selected = palette.selected_action().unwrap_or("");
            format!("Winter - palette: > {} [{}]", palette.query, selected)
        } else if self.search_query.is_some() || self.pending == PendingPrefix::SearchInput {
            let query = self.search_query.as_deref().unwrap_or("");
            let prefix = if self.search_reverse { '?' } else { '/' };
            format!("Winter - search: {prefix}{query}")
        } else if self.quick_select.is_some() {
            "Winter - quick select".to_string()
        } else if let winter_render::TabbarHit::Tab(idx) = self.tabbar_hover {
            let t = self.tab_title(idx);
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
    /// Show `message` as a transient error notice (red) in the status bar.
    pub(crate) fn set_error(&mut self, message: impl Into<String>) {
        self.show_notice(message, NoticeKind::Error);
    }
    /// Show `message` as a transient info notice (green) in the status bar, used
    /// to confirm an action such as copying text to the clipboard.
    pub(crate) fn set_notice(&mut self, message: impl Into<String>) {
        self.show_notice(message, NoticeKind::Info);
    }
    /// Store a transient notice of `kind` and force a redraw so it paints now
    /// and clears once it expires.
    pub(crate) fn show_notice(&mut self, message: impl Into<String>, kind: NoticeKind) {
        self.notice = Some((message.into(), kind, Instant::now() + NOTICE_DURATION));
        self.dirty = true;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    /// The current notice text and kind, if one is set and has not yet expired.
    pub(crate) fn active_notice(&self) -> Option<(&str, NoticeKind)> {
        self.notice
            .as_ref()
            .filter(|(_, _, expiry)| Instant::now() < *expiry)
            .map(|(text, kind, _)| (text.as_str(), *kind))
    }
}
