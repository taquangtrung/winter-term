//! The window's side of notifications: posting them, expiring and dismissing
//! toasts, the bell, the history in the palette, and what the renderer is
//! handed to draw. What a notification is and how long it lasts lives in
//! [`winter_model::notify`].

use std::time::{Instant, SystemTime};

use winter_render::{NoticeKind, StatusBell, StatusNotice, ToastCard, ToastHit};

use crate::editors::notifications::NotificationsEditor;
use winter_model::input::Key;
use winter_model::layout::TabId;
use winter_model::notify::{Notice, Notification, Severity};

use crate::window::editor::EditorKind;
use crate::window::input::overlay::{ErrorChoice, ErrorDialog, Overlay};
use crate::window::App;
use crate::window::Chrome;

// ========================================================================
// Constants
// ========================================================================

/// The bell while nothing is unread: the codicon VS Code's status bar uses.
const BELL_ICON: char = '\u{eaa2}';

/// The bell with a dot, while an error or a warning is unread.
const BELL_UNREAD_ICON: char = '\u{eb9a}';

/// What the dialog reporting an error is titled, after who reports it.
const ERROR_TITLE: &str = "Error";

/// What copying an error's details says.
const COPIED_DETAILS: &str = "copied the details";

/// What a copy with no clipboard to copy to says.
const CLIPBOARD_UNAVAILABLE: &str = "clipboard unavailable";

// ========================================================================
// Free functions
// ========================================================================

/// Now, in seconds since the Unix epoch.
pub(in crate::window) fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// How the renderer colors a severity.
fn notice_kind(severity: Severity) -> NoticeKind {
    match severity {
        Severity::Error => NoticeKind::Error,
        Severity::Info => NoticeKind::Info,
        Severity::Refusal | Severity::Warning => NoticeKind::Warning,
    }
}

/// `text`, said by `source` when one is named.
fn attributed(source: Option<&str>, text: &str) -> String {
    match source {
        Some(source) => format!("{source}: {text}"),
        None => text.to_string(),
    }
}

/// `text` with its first letter a capital, which sets a notice apart from what
/// the status bar always shows.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
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
    /// Tell the user `notice`: an error in a dialog, anything else as a toast
    /// (or in the status bar), and, unless it says otherwise, kept in the
    /// history.
    pub(in crate::window) fn notify(&mut self, notice: Notice) {
        self.notify_from(None, notice);
    }

    /// Tell the user `notice` as [`Self::notify`] does, posted by tab `tab`'s
    /// editor where one did: an action chosen on its dialog goes back there.
    pub(in crate::window) fn notify_from(&mut self, tab: Option<TabId>, notice: Notice) {
        match notice.severity {
            Severity::Error => {
                if notice.logged {
                    self.chrome.notifier.log.record(&notice, now_seconds());
                }
                self.report_error(tab, &notice);
            }
            Severity::Info | Severity::Refusal | Severity::Warning => {
                self.chrome
                    .notifier
                    .post(&notice, Instant::now(), now_seconds());
            }
        }
        if notice.logged {
            self.sync_notification_tabs();
        }
        self.dirty = true;
    }

    /// Open the dialog reporting the error `notice` from tab `tab`, which
    /// waits its turn behind any dialog already up.
    fn report_error(&mut self, tab: Option<TabId>, notice: &Notice) {
        let title = match notice.source.as_deref() {
            Some(source) => format!("{source} {ERROR_TITLE}"),
            None => ERROR_TITLE.to_string(),
        };
        self.open_overlay(Overlay::Error(ErrorDialog::new(tab, notice, title)));
    }

    /// Route one key into the error dialog: a button's key carries it out.
    pub(in crate::window) fn handle_error_dialog_key(&mut self, key: &Key) {
        let Some(Overlay::Error(dialog)) = &mut self.chrome.overlay else {
            return;
        };
        let choice = dialog.buttons.on_key(key);
        self.dirty = true;
        if let Some(choice) = choice {
            self.choose_on_error_dialog(choice);
        }
    }

    /// Put the error dialog away, carrying out `choice`: the poster's
    /// action handed back to its editor, or the details copied.
    pub(in crate::window) fn choose_on_error_dialog(&mut self, choice: ErrorChoice) {
        let Some(Overlay::Error(dialog)) = self
            .chrome
            .overlay
            .take_if(|overlay| matches!(overlay, Overlay::Error(_)))
        else {
            return;
        };
        self.dirty = true;
        match choice {
            ErrorChoice::Dismiss => {}
            ErrorChoice::Copy => {
                let copied = self
                    .input
                    .clipboard()
                    .and_then(|clipboard| clipboard.set_text(dialog.message()).ok())
                    .is_some();
                match copied {
                    true => self.set_unlogged_notice(COPIED_DETAILS),
                    false => self.set_error(CLIPBOARD_UNAVAILABLE),
                }
            }
            ErrorChoice::Action(id) => {
                let Some(tab) = dialog.tab else {
                    return;
                };
                let Some(slot) = self.open_tabs.page_mut(&tab) else {
                    return;
                };
                let outcome = slot.page.on_notice_action(id);
                self.act_on_editor_outcome(tab, outcome);
            }
        }
    }

    /// Report a failure, in a dialog.
    pub(in crate::window) fn set_error(&mut self, message: impl Into<String>) {
        self.notify(Notice::error(message));
    }

    /// Say why a command did nothing, in passing (see [`Notice::refusal`]).
    pub(in crate::window) fn refuse(&mut self, message: impl Into<String>) {
        self.notify(Notice::refusal(message));
    }

    /// Report an event or confirmation to the status bar and keep it in the log.
    pub(in crate::window) fn set_notice(&mut self, message: impl Into<String>) {
        self.notify(Notice::info(message));
    }

    /// Confirm a transient action (such as copying) in the status bar without
    /// keeping it in the notification log.
    pub(in crate::window) fn set_unlogged_notice(&mut self, message: impl Into<String>) {
        self.notify(Notice::info(message).unlogged());
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
        self.chrome
            .notifier
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
            .chrome
            .notifier
            .toasts
            .iter()
            .find(|toast| !self.is_carded(toast.severity));
        match newest {
            Some(toast) => Some(StatusNotice {
                kind: notice_kind(toast.severity),
                text: counted(
                    attributed(toast.source.as_deref(), &capitalized(&toast.text)),
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
        if close_hovered != self.input.pointer.toast_close_hovered {
            self.input.pointer.toast_close_hovered = close_hovered;
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
        self.open_page(EditorKind::Notifications, |_| {
            Box::new(NotificationsEditor::new(unread.clone(), now))
        });
        for page in self.open_tabs.own_pages_mut() {
            page.with_kind_mut(|tab: &mut NotificationsEditor| tab.set_notifications(&unread, now));
        }
        self.dirty = true;
    }

    /// Hand the history to every Notifications tab, after it changed.
    pub(in crate::window) fn sync_notification_tabs(&mut self) {
        let entries = self.chrome.notification_entries();
        let now = now_seconds();
        for page in self.open_tabs.own_pages_mut() {
            page.with_kind_mut(|tab: &mut NotificationsEditor| {
                tab.set_notifications(&entries, now)
            });
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
    /// The text of the newest notice still on screen, the error dialog's
    /// ahead of any toast, for tests that check what was said.
    #[cfg(test)]
    pub(in crate::window) fn notice_text(&self) -> Option<&str> {
        self.error_dialog()
            .map(|dialog| dialog.summary.as_str())
            .or_else(|| {
                self.notifier
                    .toasts
                    .newest()
                    .map(|toast| toast.text.as_str())
            })
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
    pub(in crate::window) fn notification_entries(&self) -> Vec<Notification> {
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
    fn test_an_error_is_reported_in_a_dialog_until_acknowledged() {
        let mut app = App::new();
        app.notify(Notice::error("failed: cannot rebase").from_source("Git"));
        let dialog = app.chrome.error_dialog().expect("a dialog, not a toast");
        assert_eq!(dialog.title, "Git Error");
        assert_eq!(dialog.message(), "failed: cannot rebase");
        assert!(app.chrome.notifier.toasts.newest().is_none());
        assert_eq!(
            app.chrome.notifier.log.iter().count(),
            1,
            "kept in the history"
        );

        app.handle_error_dialog_key(&Key::plain(winter_model::input::KeyCode::Escape));
        assert!(app.chrome.error_dialog().is_none(), "Esc presses its OK");
    }

    #[test]
    fn test_an_error_s_dialog_shows_its_details_under_it_and_offers_its_actions() {
        let mut app = App::new();
        let notice = Notice::error("git rebase main failed")
            .with_detail("CONFLICT (content): Merge conflict in a.rs")
            .with_action("abort_rebase", "Abort Rebase")
            .from_source("Git");
        app.notify(notice);
        let dialog = app.chrome.error_dialog().expect("a dialog");
        assert_eq!(
            dialog.message(),
            "git rebase main failed\n\nCONFLICT (content): Merge conflict in a.rs"
        );
        let labels: Vec<&str> = dialog
            .buttons
            .buttons()
            .iter()
            .map(|b| b.label.as_str())
            .collect();
        assert_eq!(labels, ["OK", "Abort Rebase", "Copy Details"]);
        assert_eq!(
            app.chrome
                .notifier
                .log
                .iter()
                .next()
                .and_then(|n| n.detail.as_deref()),
            Some("CONFLICT (content): Merge conflict in a.rs"),
            "the history keeps the details"
        );
    }

    #[test]
    fn test_errors_wait_their_turn_and_a_repeat_is_counted() {
        let mut app = App::new();
        app.set_error("first");
        app.set_error("second");
        app.set_error("second");
        assert_eq!(
            app.chrome.error_dialog().map(|d| d.summary.as_str()),
            Some("first")
        );
        app.handle_error_dialog_key(&Key::plain(winter_model::input::KeyCode::Enter));
        app.open_held_dialog();
        let next = app.chrome.error_dialog().expect("the next one, not lost");
        assert_eq!((next.summary.as_str(), next.repeats), ("second", 2));
        assert_eq!(next.shown_title(), "Error (2x)");
    }

    #[test]
    fn test_a_refusal_is_said_in_passing_and_never_kept() {
        let mut app = App::new();
        app.refuse("nothing to undo");
        assert!(
            app.chrome.error_dialog().is_none(),
            "no dialog for a key that did nothing"
        );
        assert_eq!(app.chrome.notice_text(), Some("nothing to undo"));
        assert_eq!(app.chrome.notifier.log.iter().count(), 0, "not kept");
        assert_eq!(app.chrome.status_bell().text, BELL_ICON.to_string());
    }

    #[test]
    fn test_an_error_rings_the_bell_until_the_history_is_opened() {
        let mut app = App::new();
        app.set_error("push failed");
        assert_eq!(
            app.chrome.status_bell().text,
            BELL_UNREAD_ICON.to_string(),
            "dotted, with no count"
        );
        app.handle_error_dialog_key(&Key::plain(winter_model::input::KeyCode::Enter));

        app.open_notifications();
        assert_eq!(app.chrome.status_bell().text, BELL_ICON.to_string());
        assert!(
            app.chrome.notice_text().is_none(),
            "the toasts went into the list"
        );
        let tab = app.layout().focused();
        let slot = app
            .open_tabs
            .page(&tab)
            .expect("the history is open in a tab");
        assert_eq!(slot.kind(), EditorKind::Notifications);
        app.open_notifications();
        let tabs = app
            .open_tabs
            .pages()
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
            Some("Copied".into()),
            "capitalized, to stand apart in the bar"
        );

        app.config.notifications.toasts = false;
        assert!(app.toast_cards().is_empty(), "cards off: all in the bar");
        assert_eq!(
            app.status_notice().map(|notice| notice.text),
            Some("Copied".into())
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

    #[test]
    fn test_unlogged_notice_shows_in_status_bar_without_logging() {
        let mut app = App::new();
        app.set_unlogged_notice("Copied 1 line, 5 characters");
        assert_eq!(
            app.status_notice().map(|notice| notice.text),
            Some("Copied 1 line, 5 characters".into())
        );
        assert_eq!(app.chrome.notifier.log.iter().count(), 0);
    }
}
