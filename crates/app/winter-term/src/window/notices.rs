//! The window's side of notifications: posting them, expiring and dismissing
//! toasts, the bell, the history in the palette, and what the renderer is
//! handed to draw. What a notification is and how long it lasts lives in
//! [`winter_model::notify`].

use std::time::{Instant, SystemTime};

use crate::render::{NoticeKind, StatusBell, StatusNotice};

use crate::editors::notifications::NotificationsEditor;
use misty::{Routing, ToastsResponse, Widget};
use winter_model::input::Key;
use winter_model::layout::TabId;
use winter_model::notify::{LoggedNotice, Notice, Severity};

use crate::window::App;
use crate::window::Chrome;
use crate::window::editor::EditorKind;
use crate::window::input::overlay::{ErrorChoice, ErrorDialog, Overlay};
use crate::window::notice::WindowNotice;
use crate::window::terminal::notice::TerminalNotice;

// ========================================================================
// Constants
// ========================================================================

/// The bell while nothing is unread: the codicon VS Code's status bar uses,
/// drawn from the bundled SVG rather than a font, since the bar's
/// proportional font has no codicons and another font's glyph at the same
/// private-use codepoint would show in its place.
const BELL_ICON: &str = "symbols/bell";

/// The bell with a dot, while an error or a warning is unread.
const BELL_UNREAD_ICON: &str = "symbols/bell-dot";

/// What the dialog reporting an error is titled, after who reports it.
const ERROR_TITLE: &str = "Error";

// ========================================================================
// Free functions
// ========================================================================

/// Now, in seconds since the Unix epoch.
pub(in crate::window) fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// The icons the status bar names: the two bells, tinted in the bar's text
/// color as its glyph was.
pub(in crate::window) fn status_bar_icons() -> misty::IconSet {
    let mut icons = misty::IconSet::new();
    for name in [BELL_ICON, BELL_UNREAD_ICON] {
        let svg = crate::icons::named_svg(name).and_then(|bytes| misty::Svg::parse(&bytes).ok());
        if let Some(svg) = svg {
            icons.insert(name, svg);
        }
    }
    icons
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
    pub(in crate::window) fn notify(&mut self, notice: impl Into<Notice>) {
        self.notify_from(None, notice.into());
    }

    /// Tell the user `notice` as [`Self::notify`] does, posted by tab `tab`'s
    /// editor where one did: an action chosen on its dialog goes back there.
    pub(in crate::window) fn notify_from(&mut self, tab: Option<TabId>, notice: Notice) {
        match notice.severity {
            Severity::Error => {
                if notice.logged {
                    self.chrome
                        .notifier
                        .log_mut()
                        .record(&notice, now_seconds());
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
                    true => self.notify(WindowNotice::DetailsCopied),
                    false => self.notify(TerminalNotice::ClipboardFailed),
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

    /// Take the toasts whose time is up off screen, and return when the next
    /// one is due, for the event loop to wake at.
    pub(in crate::window) fn expire_toasts(&mut self, now: Instant) -> Option<Instant> {
        if self.chrome.notifier.toasts_mut().expire(now) {
            self.dirty = true;
        }
        self.chrome.notifier.toasts().next_expiry()
    }
}

// ========================================================================
// App: what is drawn
// ========================================================================

impl App {
    /// Tell the notifier where each notice shows: a warning or an error as
    /// a card when cards are on, and every notice while the status bar is
    /// hidden, having nowhere else to go.
    pub(in crate::window) fn sync_notice_routing(&mut self) {
        let routing = Routing {
            cards_from: self
                .config
                .notifications
                .toasts
                .then_some(Severity::Warning),
            status_bar: self.status_bar_visible(),
        };
        self.chrome.notifier.set_routing(routing);
    }

    /// What the status bar says of the oldest task under way: who runs it,
    /// what it is doing and how far it has got, how many more run, and how
    /// to stop it when it may be stopped.
    fn progress_line(&self) -> Option<String> {
        let tasks = self.chrome.notifier.tasks();
        let task = tasks.first()?;
        let mut line = attributed(task.source(), task.title());
        if let Some(step) = task.step() {
            line.push_str(&format!(" \u{00b7} {step}"));
        }
        if let Some(fraction) = task.fraction() {
            line.push_str(&format!(" {:.0}%", fraction * 100.0));
        }
        if tasks.len() > 1 {
            line.push_str(&format!(" +{}", tasks.len() - 1));
        }
        if task.is_cancellable() {
            let stop =
                winter_model::input::format_key(&crate::window::runtime::jobs::STOP_TASKS_KEY);
            line.push_str(&format!(" ({stop} to stop)"));
        }
        Some(line)
    }

    /// What the status bar says after its mode: the newest notice not drawn
    /// as a card, else the progress of a program still running.
    pub(in crate::window) fn status_notice(&self) -> Option<StatusNotice> {
        let newest = self.chrome.notifier.status_message();
        match newest {
            Some(toast) => Some(StatusNotice {
                kind: notice_kind(toast.severity()),
                text: counted(
                    attributed(toast.source(), &capitalized(toast.text())),
                    toast.repeats(),
                ),
            }),
            None => self.progress_line().map(|text| StatusNotice {
                kind: NoticeKind::Progress,
                text,
            }),
        }
    }
}

// ========================================================================
// App: the pointer
// ========================================================================

impl App {}

// ========================================================================
// App: the cards misty draws
// ========================================================================

impl App {
    /// Draw the notifier's cards over `frame`, in the top-right corner below
    /// the tabbar.
    pub(in crate::window) fn draw_cards(&mut self, frame: &mut misty::Frame) {
        let theme = crate::window::appearance::ui_theme(self.renderer.as_ref());
        let bounds = frame.bounds();
        let top = self.viewport_rect().y;
        let area = misty::Rect::new(bounds.x, top, bounds.width, (bounds.height - top).max(0.0));
        self.chrome
            .cards
            .lend(&mut self.chrome.notifier)
            .draw(frame, area, theme);
    }

    /// Hand pointer `input` to the cards; whether they took it. A click on
    /// a card opens the history; Cancel on a task stops its work.
    pub(in crate::window) fn point_at_cards(
        &mut self,
        input: &misty::Input,
        cx: &mut misty::EventCx<'_>,
    ) -> bool {
        let answer = self
            .chrome
            .cards
            .lend(&mut self.chrome.notifier)
            .handle(input, cx);
        match answer {
            ToastsResponse::Cancelled(task) => self.cancel_task(task),
            ToastsResponse::Closed(_) => self.dirty = true,
            ToastsResponse::Handled => {}
            ToastsResponse::Ignored => return false,
            ToastsResponse::Opened(_) => self.open_notifications(),
            // A response misty adds later is none the cards act on.
            _ => return false,
        }
        true
    }

    /// When the cards next need a frame: a toast's time up, or a bar of a
    /// task of unknown length moving.
    pub(in crate::window) fn cards_deadline(
        &mut self,
        cx: &mut misty::EventCx<'_>,
    ) -> Option<Instant> {
        self.chrome
            .cards
            .lend(&mut self.chrome.notifier)
            .about_to_wait(cx)
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
        self.chrome.notifier.log_mut().mark_read();
        self.chrome.notifier.toasts_mut().dismiss_all();
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
        self.chrome.notifier.log_mut().clear();
        self.chrome.notifier.toasts_mut().dismiss_all();
        self.sync_notification_tabs();
        self.notify(WindowNotice::NotificationsCleared);
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
            .or_else(|| self.notifier.toasts().newest().map(|toast| toast.text()))
    }

    /// The bell as the status bar draws it: dotted while a notification is
    /// unread, plain otherwise. The dot says enough; how many is the list's
    /// to say.
    pub(in crate::window) fn status_bell(&self) -> StatusBell {
        let icon = match self.notifier.log().unread() {
            0 => BELL_ICON,
            _ => BELL_UNREAD_ICON,
        };
        StatusBell { icon }
    }

    /// The history, newest first, as a Notifications tab lists it.
    pub(in crate::window) fn notification_entries(&self) -> Vec<LoggedNotice> {
        self.notifier.log().iter().cloned().collect()
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
        assert!(app.chrome.notifier.toasts().newest().is_none());
        assert_eq!(
            app.chrome.notifier.log().iter().count(),
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
                .log()
                .iter()
                .next()
                .and_then(|n| n.detail()),
            Some("CONFLICT (content): Merge conflict in a.rs"),
            "the history keeps the details"
        );
    }

    #[test]
    fn test_errors_wait_their_turn_and_a_repeat_is_counted() {
        let mut app = App::new();
        app.notify(Notice::error("first"));
        app.notify(Notice::error("second"));
        app.notify(Notice::error("second"));
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
        app.notify(Notice::refusal("nothing to undo"));
        assert!(
            app.chrome.error_dialog().is_none(),
            "no dialog for a key that did nothing"
        );
        assert_eq!(app.chrome.notice_text(), Some("nothing to undo"));
        assert_eq!(app.chrome.notifier.log().iter().count(), 0, "not kept");
        assert_eq!(app.chrome.status_bell().icon, BELL_ICON);
    }

    #[test]
    fn test_an_error_rings_the_bell_until_the_history_is_opened() {
        let mut app = App::new();
        app.notify(Notice::error("push failed"));
        assert_eq!(
            app.chrome.status_bell().icon,
            BELL_UNREAD_ICON,
            "dotted, with no count"
        );
        app.handle_error_dialog_key(&Key::plain(winter_model::input::KeyCode::Enter));

        app.open_notifications();
        assert_eq!(app.chrome.status_bell().icon, BELL_ICON);
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
            app.chrome.notifier.log().iter().count(),
            0,
            "clearing says so unlogged"
        );
    }

    #[test]
    fn test_problems_go_to_cards_and_confirmations_to_the_status_bar() {
        let mut app = App::new();
        app.notify(Notice::warning("slow").from_source("Git"));
        app.notify(Notice::info("copied"));
        app.sync_notice_routing();
        let cards: Vec<_> = app.chrome.notifier.cards().collect();
        assert_eq!(cards.len(), 1, "only the warning is a card");
        assert_eq!(cards[0].source(), Some("Git"));
        assert_eq!(
            app.status_notice().map(|notice| notice.text),
            Some("Copied".into()),
            "capitalized, to stand apart in the bar"
        );

        app.config.notifications.toasts = false;
        app.sync_notice_routing();
        assert_eq!(
            app.chrome.notifier.cards().count(),
            0,
            "cards off: all in the bar"
        );
        assert_eq!(
            app.status_notice().map(|notice| notice.text),
            Some("Copied".into())
        );

        app.config.status_bar.enabled = false;
        app.sync_notice_routing();
        assert_eq!(
            app.chrome.notifier.cards().count(),
            2,
            "no bar: everything is a card"
        );
        assert!(app.status_notice().is_none());
    }

    #[test]
    fn test_with_no_notice_the_status_bar_shows_running_work() {
        let mut app = App::new();
        app.config.notifications.toasts = false;
        let progress = misty::Progress::new("cargo build").cancellable();
        let build = app.chrome.notifier.start(progress, Instant::now());
        app.chrome.notifier.set_step(build, Some("3/10".into()));
        let notice = app.status_notice().expect("the progress");
        assert_eq!(notice.kind, NoticeKind::Progress);
        assert!(
            notice.text.starts_with("cargo build \u{00b7} 3/10 ("),
            "{}",
            notice.text
        );
    }

    #[test]
    fn test_unlogged_notice_shows_in_status_bar_without_logging() {
        let mut app = App::new();
        app.notify(Notice::info("Copied 1 line, 5 characters").unlogged());
        assert_eq!(
            app.status_notice().map(|notice| notice.text),
            Some("Copied 1 line, 5 characters".into())
        );
        assert_eq!(app.chrome.notifier.log().iter().count(), 0);
    }
}
