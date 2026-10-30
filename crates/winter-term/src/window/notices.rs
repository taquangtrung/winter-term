//! The window's side of notifications: posting them, expiring and dismissing
//! toasts, the bell, the history in the palette, and what the renderer is
//! handed to draw. What a notification is and how long it lasts lives in
//! [`crate::model::notify`].

use std::time::{Instant, SystemTime};

use winter_render::{NoticeKind, StatusBell, StatusNotice, ToastCard, ToastHit};

use crate::editors::notifications::NotificationsEditor;
use crate::model::notify::{Notice, Notification, Severity};

use crate::window::editor::EditorKind;
use crate::window::App;
use crate::window::Chrome;

// ========================================================================
// Constants
// ========================================================================

/// The bell while nothing is unread: the codicon VS Code's status bar uses.
const BELL_ICON: char = '\u{eaa2}';

/// The bell with a dot, while an error or a warning is unread.
const BELL_UNREAD_ICON: char = '\u{eb9a}';

// ========================================================================
// Free functions
// ========================================================================

/// Now, in seconds since the Unix epoch.
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// How the renderer colors a severity.
fn notice_kind(severity: Severity) -> NoticeKind {
    match severity {
        Severity::Error => NoticeKind::Error,
        Severity::Info => NoticeKind::Info,
        Severity::Warning => NoticeKind::Warning,
    }
}

/// `text`, said by `source` when one is named.
fn attributed(source: Option<&str>, text: &str) -> String {
    match source {
        Some(source) => format!("{source}: {text}"),
        None => text.to_string(),
    }
}

/// `text`, counted when it came more than once.
fn counted(text: String, repeats: usize) -> String {
    match repeats {
        1 => text,
        repeats => format!("{text} ({repeats}x)"),
    }
}

// ========================================================================
// App: posting
// ========================================================================

impl App {
    /// Tell the user `notice`: shown as a toast (or in the status bar) and,
    /// unless it says otherwise, kept in the history.
    pub(in crate::window) fn notify(&mut self, notice: Notice) {
        self.chrome.notifier.post(&notice, Instant::now(), now_seconds());
        if notice.logged {
            self.sync_notification_tabs();
        }
        self.dirty = true;
    }

    /// Report a failure.
    pub(in crate::window) fn set_error(&mut self, message: impl Into<String>) {
        self.notify(Notice::error(message));
    }

    /// Confirm an action, such as copying text to the clipboard.
    pub(in crate::window) fn set_notice(&mut self, message: impl Into<String>) {
        self.notify(Notice::info(message));
    }

    /// Take the toasts whose time is up off screen, and return when the next
    /// one is due, for the event loop to wake at.
    pub(in crate::window) fn expire_toasts(&mut self, now: Instant) -> Option<Instant> {
        if self.chrome.notifier.toasts.expire(now) {
            self.dirty = true;
        }
        self.chrome.notifier.toasts.next_expiry()
    }

}

// ========================================================================
// App: what is drawn
// ========================================================================

impl App {
    /// Whether a notice of `severity` is drawn as a toast card rather than in
    /// the status bar: a warning or an error when cards are on, and any
    /// notice while the status bar is hidden, having nowhere else to go.
    fn is_carded(&self, severity: Severity) -> bool {
        !self.status_bar_visible()
            || (self.config.notifications.toasts && severity >= Severity::Warning)
    }

    /// The cards for the top-right corner, newest first.
    pub(in crate::window) fn toast_cards(&self) -> Vec<ToastCard> {
        self.chrome.notifier
            .toasts
            .iter()
            .filter(|toast| self.is_carded(toast.severity))
            .map(|toast| ToastCard {
                hovered: self.chrome.notifier.toasts.held() == Some(toast.id),
                id: toast.id,
                kind: notice_kind(toast.severity),
                source: toast.source.clone(),
                text: counted(toast.text.clone(), toast.repeats),
            })
            .collect()
    }

    /// What the status bar says after its mode: the newest notice not drawn
    /// as a card, else the progress of a program still running.
    pub(in crate::window) fn status_notice(&self) -> Option<StatusNotice> {
        let newest = self
            .chrome.notifier
            .toasts
            .iter()
            .find(|toast| !self.is_carded(toast.severity));
        match newest {
            Some(toast) => Some(StatusNotice {
                kind: notice_kind(toast.severity),
                text: counted(
                    attributed(toast.source.as_deref(), &toast.text),
                    toast.repeats,
                ),
            }),
            None => self.job_progress.clone().map(|text| StatusNotice {
                kind: NoticeKind::Progress,
                text,
            }),
        }
    }

}

// ========================================================================
// App: the pointer
// ========================================================================

impl App {
    /// The toast card under the pointer at `(x, y)`, as last drawn.
    pub(in crate::window) fn toast_at(&self, x: f32, y: f32) -> Option<ToastHit> {
        self.renderer.as_ref()?.hits().toast_at(x, y)
    }

    /// Hold the toast under the pointer on screen while it rests there.
    pub(in crate::window) fn hover_toasts(&mut self, x: f32, y: f32) {
        let hit = self.toast_at(x, y);
        let held = hit.map(|h| h.id);
        let close_hovered = hit.is_some_and(|h| h.close);
        let mut dirty = self.chrome.notifier.toasts.hold(held, Instant::now());
        if close_hovered != self.pointer.toast_close_hovered {
            self.pointer.toast_close_hovered = close_hovered;
            dirty = true;
        }
        self.dirty |= dirty;
    }

    /// Act on a press at `(x, y)` if it lands on a toast or on the bell: the
    /// close button dismisses its card, the rest of a card or the bell opens
    /// the history. Whether the press was taken.
    pub(in crate::window) fn press_notifications(&mut self, x: f32, y: f32) -> bool {
        if let Some(hit) = self.toast_at(x, y) {
            match hit.close {
                true => {
                    self.chrome.notifier.toasts.dismiss(hit.id);
                    self.dirty = true;
                }
                false => self.open_notifications(),
            }
            return true;
        }
        let on_bell = self
            .renderer
            .as_ref()
            .is_some_and(|renderer| renderer.hits().status_bell_at(x, y));
        if on_bell && self.status_bar_visible() {
            self.open_notifications();
        }
        on_bell
    }
}

// ========================================================================
// App: the history
// ========================================================================

impl App {
    /// Show the history in a Notifications tab beside the focused one, or
    /// go to the one the pane already has, count it as read, and take the
    /// toasts down: they are all in the list.
    pub(in crate::window) fn open_notifications(&mut self) {
        // The page is handed the history as it was before it was read, so
        // it can still mark which it was that had not been.
        let unread = self.chrome.notification_entries();
        self.chrome.notifier.log.mark_read();
        self.chrome.notifier.toasts.dismiss_all();
        let now = now_seconds();
        self.open_single(EditorKind::Notifications, |_| {
            Box::new(NotificationsEditor::new(unread.clone(), now))
        });
        for slot in self.open_tabs.buffers_mut() {
            slot.editor.with_kind_mut(|tab: &mut NotificationsEditor| tab.set_notifications(&unread, now));
        }
        self.dirty = true;
    }

    /// Hand the history to every Notifications tab, after it changed.
    pub(in crate::window) fn sync_notification_tabs(&mut self) {
        let entries = self.chrome.notification_entries();
        let now = now_seconds();
        for slot in self.open_tabs.buffers_mut() {
            slot.editor.with_kind_mut(|tab: &mut NotificationsEditor| tab.set_notifications(&entries, now));
        }
    }

    /// Forget the history and take the toasts down.
    pub(in crate::window) fn clear_notifications(&mut self) {
        self.chrome.notifier.log.clear();
        self.chrome.notifier.toasts.dismiss_all();
        self.sync_notification_tabs();
        self.notify(Notice::info("notifications cleared").unlogged());
    }
}

// ========================================================================
// Chrome
// ========================================================================

impl Chrome {
    /// The text of the newest notice still on screen, for tests that check
    /// what was said.
    #[cfg(test)]
    pub(in crate::window) fn notice_text(&self) -> Option<&str> {
        self.notifier
            .toasts
            .newest()
            .map(|toast| toast.text.as_str())
    }

    /// The bell as the status bar draws it: dotted while a notification is
    /// unread, plain otherwise. The dot says enough; how many is the list's
    /// to say.
    pub(in crate::window) fn status_bell(&self) -> StatusBell {
        let icon = match self.notifier.log.unread() {
            0 => BELL_ICON,
            _ => BELL_UNREAD_ICON,
        };
        StatusBell {
            text: icon.to_string(),
        }
    }

    /// The history, newest first, as a Notifications tab lists it.
    fn notification_entries(&self) -> Vec<Notification> {
        self.notifier.log.iter().cloned().collect()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_an_error_rings_the_bell_until_the_history_is_opened() {
        let mut app = App::new();
        app.set_error("push failed");
        assert_eq!(app.chrome.status_bell().text, BELL_UNREAD_ICON.to_string(), "dotted, with no count");

        app.open_notifications();
        assert_eq!(app.chrome.status_bell().text, BELL_ICON.to_string());
        assert!(app.chrome.notice_text().is_none(), "the toasts went into the list");
        let tab = app.layout().focused();
        let slot = app
            .open_tabs
            .editor(&tab)
            .expect("the history is open in a tab");
        assert_eq!(slot.kind(), EditorKind::Notifications);
        app.open_notifications();
        let tabs = app
            .open_tabs
            .editors()
            .filter(|(_, slot)| slot.kind() == EditorKind::Notifications)
            .count();
        assert_eq!(tabs, 1, "opening it again goes to the tab it has");

        app.clear_notifications();
        assert_eq!(
            app.chrome.notifier.log.iter().count(),
            0,
            "clearing says so unlogged"
        );
    }

    #[test]
    fn test_problems_go_to_cards_and_confirmations_to_the_status_bar() {
        let mut app = App::new();
        app.notify(Notice::warning("slow").from_source("Git"));
        app.set_notice("copied");
        let cards = app.toast_cards();
        assert_eq!(cards.len(), 1, "only the warning is a card");
        assert_eq!(cards[0].source.as_deref(), Some("Git"));
        assert_eq!(
            app.status_notice().map(|notice| notice.text),
            Some("copied".into())
        );

        app.config.notifications.toasts = false;
        assert!(app.toast_cards().is_empty(), "cards off: all in the bar");
        assert_eq!(
            app.status_notice().map(|notice| notice.text),
            Some("copied".into())
        );

        app.config.status_bar.enabled = false;
        assert_eq!(app.toast_cards().len(), 2, "no bar: everything is a card");
        assert!(app.status_notice().is_none());
    }

    #[test]
    fn test_with_no_notice_the_status_bar_shows_running_work() {
        let mut app = App::new();
        app.config.notifications.toasts = false;
        app.job_progress = Some("C-g to stop".to_string());
        let notice = app.status_notice().expect("the progress");
        assert_eq!(notice.kind, NoticeKind::Progress);
    }
}
