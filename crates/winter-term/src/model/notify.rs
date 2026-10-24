//! Notifications: what the app and its editors report, how long each is shown,
//! and what is kept afterwards.
//!
//! One [`Notifier`] owns the whole lifecycle. A [`Notice`] posted to it goes
//! two ways: onto the [`ToastStack`], the cards shown for a few seconds, and
//! (unless it says otherwise) into the [`NotificationLog`], the session's
//! history the bell counts and the palette lists. Both halves are plain data
//! driven by a clock the caller passes in, so nothing here reads the time or
//! draws anything.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

// ========================================================================
// Constants
// ========================================================================

/// How many notifications the log keeps before the oldest go.
const LOG_LIMIT: usize = 200;

/// How many toasts are shown at once; a new one pushes out the oldest.
const TOAST_LIMIT: usize = 3;

/// How long a confirmation is shown.
const INFO_DURATION: Duration = Duration::from_secs(3);

/// How long a warning is shown: long enough to be read in full.
const WARNING_DURATION: Duration = Duration::from_secs(6);

/// How long an error is shown: the longest, since it is the one that matters.
const ERROR_DURATION: Duration = Duration::from_secs(8);

// ========================================================================
// Data Structures
// ========================================================================

/// How much a notice matters, which decides its color, how long it is shown,
/// and whether it counts on the bell until it is looked at.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Severity {
    /// A confirmation of something done: `pushed`, `copied`.
    Info,
    /// Something worth knowing that is not a failure.
    Warning,
    /// Something that failed.
    Error,
}

/// Something to tell the user, as the app or an editor posts it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Notice {
    /// Whether it is kept in the log once its toast is gone. Off for what the
    /// log says about itself, which would only fill it.
    pub logged: bool,
    /// How much it matters.
    pub severity: Severity,
    /// Who is saying it (`Git`, `Dir`), shown above the text; `None` for the
    /// window itself.
    pub source: Option<String>,
    /// What is said.
    pub text: String,
}

/// A notification as the log keeps it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Notification {
    /// When it last came, in seconds since the Unix epoch.
    pub at: i64,
    /// How many times in a row it came, so a repeat is counted, not listed.
    pub repeats: usize,
    /// How much it matters.
    pub severity: Severity,
    /// Who said it, as the notice named it.
    pub source: Option<String>,
    /// What was said.
    pub text: String,
    /// Whether it still counts on the bell: an error or a warning not yet
    /// looked at. A confirmation is never unread.
    unread: bool,
}

/// Every notification of the session, newest first.
#[derive(Debug, Default)]
pub struct NotificationLog {
    entries: VecDeque<Notification>,
}

/// A notice on screen as a toast.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Toast {
    /// When it goes away on its own.
    pub expires: Instant,
    /// Names it for the whole time it is shown, so a click on a card dismisses
    /// that card even if others came or went since it was drawn.
    pub id: u64,
    /// How many times in a row it came.
    pub repeats: usize,
    /// How much it matters, which colors the card.
    pub severity: Severity,
    /// Who said it, shown above the text.
    pub source: Option<String>,
    /// What was said.
    pub text: String,
}

/// The toasts on screen, newest first.
#[derive(Debug, Default)]
pub struct ToastStack {
    /// The toast the pointer rests on, which does not expire while it does.
    held: Option<u64>,
    next_id: u64,
    toasts: Vec<Toast>,
}

/// The single owner of what is shown and what is kept.
#[derive(Debug, Default)]
pub struct Notifier {
    /// What is kept for the session.
    pub log: NotificationLog,
    /// What is on screen now.
    pub toasts: ToastStack,
}

// ========================================================================
// Severity
// ========================================================================

impl Severity {
    /// How long a toast of this severity is shown.
    pub fn duration(self) -> Duration {
        match self {
            Severity::Error => ERROR_DURATION,
            Severity::Info => INFO_DURATION,
            Severity::Warning => WARNING_DURATION,
        }
    }

    /// What the severity is called in the history.
    pub fn name(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Info => "info",
            Severity::Warning => "warning",
        }
    }

    /// Whether a notification of this severity counts on the bell until it
    /// is looked at.
    fn needs_attention(self) -> bool {
        self >= Severity::Warning
    }
}

// ========================================================================
// Notice
// ========================================================================

impl Notice {
    fn new(severity: Severity, text: impl Into<String>) -> Self {
        Self {
            logged: true,
            severity,
            source: None,
            text: text.into(),
        }
    }

    /// Something that failed.
    pub fn error(text: impl Into<String>) -> Self {
        Self::new(Severity::Error, text)
    }

    /// A confirmation of something done.
    pub fn info(text: impl Into<String>) -> Self {
        Self::new(Severity::Info, text)
    }

    /// Something worth knowing that is not a failure.
    pub fn warning(text: impl Into<String>) -> Self {
        Self::new(Severity::Warning, text)
    }

    /// The same notice, said by `source`. A source already given is kept:
    /// the one closest to the notice knows best who is speaking.
    pub fn from_source(mut self, source: impl Into<String>) -> Self {
        self.source.get_or_insert_with(|| source.into());
        self
    }

    /// The same notice, shown but not kept in the log.
    pub fn unlogged(mut self) -> Self {
        self.logged = false;
        self
    }

    /// Whether `self` says what an earlier notice said, so it counts as a
    /// repeat of it rather than as news.
    fn repeats(&self, severity: Severity, source: &Option<String>, text: &str) -> bool {
        self.severity == severity && &self.source == source && self.text == text
    }
}

// ========================================================================
// NotificationLog
// ========================================================================

impl Notification {
    /// What was said, as a list shows it: who said it ahead of it, and how
    /// many times when it came more than once.
    pub fn message(&self) -> String {
        let text = match &self.source {
            Some(source) => format!("{source}: {}", self.text),
            None => self.text.clone(),
        };
        match self.repeats {
            1 => text,
            repeats => format!("{text} ({repeats}x)"),
        }
    }
}

impl NotificationLog {
    /// Keep `notice`, which came at `at`. The same notification again only
    /// bumps the newest one's count, and makes it news again.
    pub(crate) fn record(&mut self, notice: &Notice, at: i64) {
        let unread = notice.severity.needs_attention();
        if let Some(newest) = self.entries.front_mut() {
            if notice.repeats(newest.severity, &newest.source, &newest.text) {
                newest.at = at;
                newest.repeats += 1;
                newest.unread |= unread;
                return;
            }
        }
        self.entries.push_front(Notification {
            at,
            repeats: 1,
            severity: notice.severity,
            source: notice.source.clone(),
            text: notice.text.clone(),
            unread,
        });
        self.entries.truncate(LOG_LIMIT);
    }

    /// How many errors and warnings are unread.
    pub fn unread(&self) -> usize {
        self.entries.iter().filter(|entry| entry.unread).count()
    }

    /// The most pressing severity among the unread.
    pub fn most_pressing_unread(&self) -> Option<Severity> {
        self.entries
            .iter()
            .filter(|entry| entry.unread)
            .map(|entry| entry.severity)
            .max()
    }

    /// Count every notification as looked at.
    pub fn mark_read(&mut self) {
        self.entries
            .iter_mut()
            .for_each(|entry| entry.unread = false);
    }

    /// Forget every notification.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// The notification `index` places from the newest.
    pub fn get(&self, index: usize) -> Option<&Notification> {
        self.entries.get(index)
    }

    /// Every notification, newest first.
    pub fn iter(&self) -> impl Iterator<Item = &Notification> {
        self.entries.iter()
    }
}

// ========================================================================
// ToastStack
// ========================================================================

impl ToastStack {
    /// Show `notice` from `now`. A repeat of the newest toast refreshes it
    /// in place rather than stacking a copy. Past [`TOAST_LIMIT`] the oldest
    /// confirmation goes first, then the oldest of the rest, so a burst of
    /// confirmations never pushes a problem off screen.
    fn show(&mut self, notice: &Notice, now: Instant) {
        let expires = now + notice.severity.duration();
        if let Some(newest) = self.toasts.first_mut() {
            if notice.repeats(newest.severity, &newest.source, &newest.text) {
                newest.expires = expires;
                newest.repeats += 1;
                return;
            }
        }
        self.next_id += 1;
        self.toasts.insert(
            0,
            Toast {
                expires,
                id: self.next_id,
                repeats: 1,
                severity: notice.severity,
                source: notice.source.clone(),
                text: notice.text.clone(),
            },
        );
        while self.toasts.len() > TOAST_LIMIT {
            let oldest_info = self
                .toasts
                .iter()
                .rposition(|toast| toast.severity == Severity::Info);
            let evicted = oldest_info.unwrap_or(self.toasts.len() - 1);
            self.toasts.remove(evicted);
        }
    }

    /// The toasts on screen, newest first.
    pub fn iter(&self) -> impl Iterator<Item = &Toast> {
        self.toasts.iter()
    }

    /// The toast the pointer rests on, if any.
    pub fn held(&self) -> Option<u64> {
        self.held
    }

    /// The newest toast, if any is on screen.
    pub fn newest(&self) -> Option<&Toast> {
        self.toasts.first()
    }

    /// Whether no toast is on screen.
    pub fn is_empty(&self) -> bool {
        self.toasts.is_empty()
    }

    /// Hold the toast `id` on screen while the pointer rests on it, or none.
    /// Letting go gives the toast its full time again from `now`, so it is
    /// not snatched away the moment the pointer leaves.
    pub fn hold(&mut self, id: Option<u64>, now: Instant) -> bool {
        if self.held == id {
            return false;
        }
        if let Some(released) = self.held.and_then(|held| self.find_mut(held)) {
            released.expires = now + released.severity.duration();
        }
        self.held = id;
        true
    }

    /// Take the toast `id` off screen at once. Whether it was there.
    pub fn dismiss(&mut self, id: u64) -> bool {
        let before = self.toasts.len();
        self.toasts.retain(|toast| toast.id != id);
        self.toasts.len() != before
    }

    /// Take every toast off screen.
    pub fn dismiss_all(&mut self) {
        self.toasts.clear();
    }

    /// Drop the toasts whose time is up at `now`, all but the held one.
    /// Whether any went.
    pub fn expire(&mut self, now: Instant) -> bool {
        let before = self.toasts.len();
        let held = self.held;
        self.toasts
            .retain(|toast| Some(toast.id) == held || toast.expires > now);
        self.toasts.len() != before
    }

    /// When the next toast is due to go, for the event loop to wake at.
    pub fn next_expiry(&self) -> Option<Instant> {
        self.toasts
            .iter()
            .filter(|toast| Some(toast.id) != self.held)
            .map(|toast| toast.expires)
            .min()
    }

    fn find_mut(&mut self, id: u64) -> Option<&mut Toast> {
        self.toasts.iter_mut().find(|toast| toast.id == id)
    }
}

// ========================================================================
// Notifier
// ========================================================================

impl Notifier {
    /// Show `notice` from `now` and, unless it is unlogged, keep it in the
    /// log as having come at `at` (seconds since the Unix epoch).
    pub fn post(&mut self, notice: &Notice, now: Instant, at: i64) {
        self.toasts.show(notice, now);
        if notice.logged {
            self.log.record(notice, at);
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn post(notifier: &mut Notifier, notice: Notice, now: Instant) {
        notifier.post(&notice, now, 0);
    }

    #[test]
    fn test_only_errors_and_warnings_count_until_read() {
        let mut notifier = Notifier::default();
        let now = Instant::now();
        post(&mut notifier, Notice::info("copied"), now);
        assert_eq!(notifier.log.unread(), 0);
        post(&mut notifier, Notice::warning("slow fetch"), now);
        assert_eq!(notifier.log.most_pressing_unread(), Some(Severity::Warning));
        post(&mut notifier, Notice::error("push failed"), now);
        post(&mut notifier, Notice::warning("slow again"), now);
        assert_eq!(notifier.log.unread(), 3);
        assert_eq!(notifier.log.most_pressing_unread(), Some(Severity::Error));

        notifier.log.mark_read();
        assert_eq!(notifier.log.most_pressing_unread(), None);
        assert_eq!(notifier.log.iter().count(), 4, "read is not forgotten");
    }

    #[test]
    fn test_a_repeat_merges_into_the_newest_in_both_halves() {
        let mut notifier = Notifier::default();
        let now = Instant::now();
        post(&mut notifier, Notice::error("push failed"), now);
        notifier.log.mark_read();
        let later = now + Duration::from_secs(5);
        post(&mut notifier, Notice::error("push failed"), later);
        assert_eq!(notifier.log.iter().count(), 1);
        assert_eq!(notifier.log.get(0).map(|entry| entry.repeats), Some(2));
        assert_eq!(notifier.log.unread(), 1, "the repeat is news again");
        let toast = notifier.toasts.newest().expect("the toast");
        assert_eq!((toast.repeats, toast.expires), (2, later + ERROR_DURATION));

        post(
            &mut notifier,
            Notice::error("push failed").from_source("Git"),
            later,
        );
        assert_eq!(notifier.log.iter().count(), 2, "another source is news");
    }

    #[test]
    fn test_an_unlogged_notice_is_shown_but_not_kept() {
        let mut notifier = Notifier::default();
        post(
            &mut notifier,
            Notice::info("cleared").unlogged(),
            Instant::now(),
        );
        assert!(!notifier.toasts.is_empty());
        assert_eq!(notifier.log.iter().count(), 0);
    }

    #[test]
    fn test_the_log_and_the_stack_keep_only_the_newest() {
        let mut notifier = Notifier::default();
        let now = Instant::now();
        for at in 0..LOG_LIMIT + 5 {
            notifier.post(&Notice::info(at.to_string()), now, at as i64);
        }
        assert_eq!(notifier.log.iter().count(), LOG_LIMIT);
        assert_eq!(
            notifier.log.get(0).map(|entry| entry.at),
            Some(LOG_LIMIT as i64 + 4)
        );
        let shown: Vec<_> = notifier
            .toasts
            .iter()
            .map(|toast| toast.text.clone())
            .collect();
        assert_eq!(shown.len(), TOAST_LIMIT);
        assert_eq!(shown[0], (LOG_LIMIT + 4).to_string(), "newest first");
    }

    #[test]
    fn test_confirmations_give_way_before_problems() {
        let mut notifier = Notifier::default();
        let now = Instant::now();
        post(&mut notifier, Notice::error("push failed"), now);
        for text in ["a", "b", "c", "d"] {
            post(&mut notifier, Notice::info(text), now);
        }
        let shown: Vec<_> = notifier
            .toasts
            .iter()
            .map(|toast| toast.text.as_str())
            .collect();
        assert_eq!(shown, ["d", "c", "push failed"]);
    }

    #[test]
    fn test_toasts_expire_by_severity_except_the_held_one() {
        let mut notifier = Notifier::default();
        let now = Instant::now();
        post(&mut notifier, Notice::info("copied"), now);
        post(&mut notifier, Notice::error("push failed"), now);
        assert_eq!(notifier.toasts.next_expiry(), Some(now + INFO_DURATION));

        assert!(notifier.toasts.expire(now + INFO_DURATION));
        let left: Vec<_> = notifier.toasts.iter().map(|toast| toast.severity).collect();
        assert_eq!(left, [Severity::Error], "an error outlasts a confirmation");

        let error = notifier.toasts.newest().map(|toast| toast.id);
        notifier.toasts.hold(error, now);
        assert_eq!(notifier.toasts.next_expiry(), None, "a held toast waits");
        assert!(!notifier.toasts.expire(now + ERROR_DURATION * 2));

        let released = now + ERROR_DURATION * 2;
        notifier.toasts.hold(None, released);
        assert_eq!(
            notifier.toasts.next_expiry(),
            Some(released + ERROR_DURATION),
            "let go, it gets its full time again"
        );
    }

    #[test]
    fn test_dismissing_takes_only_the_named_toast() {
        let mut notifier = Notifier::default();
        let now = Instant::now();
        post(&mut notifier, Notice::info("a"), now);
        post(&mut notifier, Notice::info("b"), now);
        let first = notifier
            .toasts
            .iter()
            .last()
            .map(|toast| toast.id)
            .expect("a");
        assert!(notifier.toasts.dismiss(first));
        assert!(!notifier.toasts.dismiss(first), "already gone");
        let left: Vec<_> = notifier
            .toasts
            .iter()
            .map(|toast| toast.text.as_str())
            .collect();
        assert_eq!(left, ["b"]);
    }

    #[test]
    fn test_the_nearest_source_wins() {
        let notice = Notice::error("x").from_source("Git").from_source("window");
        assert_eq!(notice.source.as_deref(), Some("Git"));
    }
}
