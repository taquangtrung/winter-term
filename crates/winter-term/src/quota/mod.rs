//! The AI providers' usage quotas the status bar shows: Antigravity's,
//! Claude Code's, and Z.ai's, each as how much of its short window and its
//! weekly window is used and when each starts over.
//!
//! Each provider is asked the way its own editors ask, so nothing here needs
//! setting up beyond what those editors already did: Claude Code's sign-in, a
//! running Antigravity, and for Z.ai an API key kept in the system's secret
//! store (see [`secret`]).
//!
//! - [`antigravity`]: the local language server, or the CLI's statusline file.
//! - [`claude`]: Anthropic's usage endpoint, with Claude Code's own token.
//! - [`http`]: requests, made through `curl`.
//! - [`monitor`]: the thread that asks each provider on its own schedule.
//! - [`secret`]: the Z.ai key in the system's secret store.
//! - [`zai`]: Z.ai's quota endpoint.

mod antigravity;
mod claude;
mod http;
mod monitor;
pub(crate) mod secret;
mod zai;

use std::fmt::Display;
use std::time::Duration;

use chrono::{DateTime, Local, TimeZone, Utc};
use winter_render::THIN_SPACE;

pub(crate) use monitor::Monitor;

// ========================================================================
// Constants
// ========================================================================

/// How often Claude Code's and Z.ai's quotas are asked for: they move with
/// use, and a request a minute would only spend their rate limits.
const SLOW_REFRESH: Duration = Duration::from_secs(5 * 60);

/// How often Antigravity's is: it is asked on this machine, so asking is
/// cheap.
const FAST_REFRESH: Duration = Duration::from_secs(60);

/// What stands between two windows of one provider. Every gap in a quota's
/// text is a [`THIN_SPACE`], half a cell, so the quota reads as one tight
/// group beside the whole-cell gaps between segments.
const WINDOW_SEPARATOR: &str = "\u{2009}|\u{2009}";

/// Shown for a window whose reset time is not known.
const UNKNOWN_RESET: &str = "--";

/// A short window's reset, which is hours away: the time alone.
const SHORT_RESET_FORMAT: &str = "%H:%M";

/// A weekly window's reset, which is days away: the day, then the time.
const WEEKLY_RESET_FORMAT: &str = "%a\u{2009}%H:%M";

// ========================================================================
// Data Structures
// ========================================================================

/// A provider whose quota is shown, in the order they stand on the bar.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum Provider {
    Antigravity,
    Claude,
    Zai,
}

/// One usage window: how much of it is used, and when it starts over.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Window {
    pub(crate) resets_at: Option<DateTime<Utc>>,
    /// From 0 to 100.
    pub(crate) used_percent: f64,
    /// Whether the reset is days away, so its day is shown with the time.
    pub(crate) weekly: bool,
}

/// What asking a provider came to.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Fetched {
    /// The asking failed on the way (no network, a server error): what was
    /// shown last stays.
    Failed,
    /// There is nothing to ask with (no sign-in, no key, nothing running),
    /// so nothing is shown.
    Missing,
    /// The provider's windows, the short one first.
    Usage(Vec<Window>),
}

// ========================================================================
// Provider Implementations
// ========================================================================

impl Provider {
    /// The packed icon drawn before the provider's windows.
    pub(crate) fn icon_name(self) -> &'static str {
        match self {
            Self::Antigravity => "brand/antigravity",
            Self::Claude => "brand/claude",
            Self::Zai => "brand/zai",
        }
    }

    /// How long to wait between two askings.
    fn refresh_every(self) -> Duration {
        match self {
            Self::Antigravity => FAST_REFRESH,
            Self::Claude | Self::Zai => SLOW_REFRESH,
        }
    }

    /// Ask the provider for its windows. Blocks on the network or on a
    /// helper program, so only the monitor's thread calls this.
    fn fetch(self) -> Fetched {
        match self {
            Self::Antigravity => antigravity::fetch(),
            Self::Claude => claude::fetch(),
            Self::Zai => zai::fetch(),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The status bar's text for `windows`, with reset times in local time, e.g.
/// `42% 14:05 | 10% Mon 09:00`. `None` when there are no windows.
pub(crate) fn describe(windows: &[Window]) -> Option<String> {
    describe_in(windows, &Local)
}

/// [`describe`], with reset times in `zone`.
fn describe_in<Tz>(windows: &[Window], zone: &Tz) -> Option<String>
where
    Tz: TimeZone,
    Tz::Offset: Display,
{
    if windows.is_empty() {
        return None;
    }
    let parts: Vec<String> = windows
        .iter()
        .map(|window| {
            let format = match window.weekly {
                true => WEEKLY_RESET_FORMAT,
                false => SHORT_RESET_FORMAT,
            };
            let reset = window.resets_at.map_or(UNKNOWN_RESET.to_string(), |at| {
                at.with_timezone(zone).format(format).to_string()
            });
            format!("{:.0}%{THIN_SPACE}{reset}", window.used_percent)
        })
        .collect();
    Some(parts.join(WINDOW_SEPARATOR))
}

/// The status bar's text for `windows` when it is short of room: how much
/// of each is used, without the reset times, e.g. `42% 10%`. `None` when
/// there are no windows.
pub(crate) fn describe_compact(windows: &[Window]) -> Option<String> {
    if windows.is_empty() {
        return None;
    }
    let parts: Vec<String> = windows
        .iter()
        .map(|window| format!("{:.0}%", window.used_percent))
        .collect();
    Some(parts.join(&THIN_SPACE.to_string()))
}

/// When an RFC 3339 timestamp (`2026-10-22T14:05:00Z`) is, if it is one.
fn parse_time(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_weekly_reset_names_its_day_and_a_short_one_does_not() {
        let windows = [
            Window {
                resets_at: parse_time("2026-10-22T14:05:00Z"),
                used_percent: 41.6,
                weekly: false,
            },
            Window {
                resets_at: parse_time("2026-10-26T09:00:00Z"),
                used_percent: 10.0,
                weekly: true,
            },
        ];
        assert_eq!(
            describe_in(&windows, &Utc).as_deref(),
            Some("42%\u{2009}14:05\u{2009}|\u{2009}10%\u{2009}Mon\u{2009}09:00")
        );
    }

    #[test]
    fn test_a_window_without_a_reset_time_says_so() {
        let windows = [Window {
            resets_at: None,
            used_percent: 3.0,
            weekly: false,
        }];
        assert_eq!(describe_in(&windows, &Utc).as_deref(), Some("3%\u{2009}--"));
        assert_eq!(describe_in(&[], &Utc), None, "nothing to show");
    }

    #[test]
    fn test_a_compact_quota_keeps_each_window_s_use_and_drops_its_reset() {
        let windows = [
            Window {
                resets_at: parse_time("2026-10-22T14:05:00Z"),
                used_percent: 16.6,
                weekly: false,
            },
            Window {
                resets_at: parse_time("2026-10-26T09:00:00Z"),
                used_percent: 30.0,
                weekly: true,
            },
        ];
        assert_eq!(
            describe_compact(&windows).as_deref(),
            Some("17%\u{2009}30%")
        );
        assert_eq!(describe_compact(&[]), None);
    }
}
