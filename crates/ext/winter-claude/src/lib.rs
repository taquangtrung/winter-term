//! Claude Code's usage quota on Winter's status bar, as an extension shipped
//! with it: how much of the five-hour window and of the weekly window is
//! used, and when each starts over, under Claude's logo.
//!
//! It asks Anthropic's usage endpoint the way Claude Code does, with the
//! OAuth token Claude Code signed in with, read from `.credentials.json` in
//! `~/.claude`. Nothing needs setting up beyond signing in to Claude Code.
//!
//! - It asks as its segment comes into sight, if what it has is older than
//!   [`REFRESH`], then every [`REFRESH`] while the segment stays in sight.
//! - A click on the segment, or `Claude: Refresh Quota`, asks again now.
//! - Signed out, or the token run out, the segment shows nothing; a failed
//!   request keeps the last figures, dimmed.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, FixedOffset, Utc};
use serde_json::Value;
use winter_ext_api::{Event, Extension, Fetched, Host, Icon, Registry, Request, Status};

// ========================================================================
// Constants
// ========================================================================

/// The extension's id.
const ID: &str = "claude";

/// The segment's name among the extension's: `claude.quota` on the bar.
const SEGMENT: &str = "quota";

/// The logo drawn before the segment, one Winter bundles.
const LOGO: &str = "apps/claude";

/// How often the quota is asked for: it moves with use, and a request a
/// minute would only spend the endpoint's rate limit.
const REFRESH: Duration = Duration::from_secs(5 * 60);

/// The timer that asks again.
const REFRESH_TIMER: &str = "refresh";

/// The tag the usage request goes under.
const USAGE_TAG: u64 = 1;

/// Where the usage is asked for.
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// The beta the OAuth endpoints are behind.
const BETA_HEADER: (&str, &str) = ("anthropic-beta", "oauth-2025-04-20");

/// The variable holding the user's home directory, inside the sandbox too.
const HOME: &str = "HOME";

/// Where Claude Code keeps its sign-in, under the home directory.
const CREDENTIALS: [&str; 2] = [".claude", ".credentials.json"];

/// The statuses meaning the token is no good: run out, or revoked. Claude
/// Code renews it the next time it runs; until then there is nothing to show.
const SIGNED_OUT: [u16; 2] = [401, 403];

/// The windows the endpoint reports, the short one first, and whether each
/// is weekly.
const WINDOWS: [(&str, bool); 2] = [("five_hour", false), ("seven_day", true)];

/// What stands between two windows, and between a window's use and its
/// reset: thin spaces, so the quota reads as one tight group.
const WINDOW_SEPARATOR: &str = "\u{2009}|\u{2009}";
const THIN_SPACE: &str = "\u{2009}";

/// Shown for a window whose reset time is not known.
const UNKNOWN_RESET: &str = "--";

/// A short window's reset, hours away: the time alone. A weekly window's,
/// days away: the day, then the time.
const SHORT_RESET_FORMAT: &str = "%H:%M";
const WEEKLY_RESET_FORMAT: &str = "%a\u{2009}%H:%M";

// ========================================================================
// Data Structures
// ========================================================================

/// The extension.
pub struct Claude;

#[cfg(feature = "wasm")]
winter_ext_guest::export!(Claude);

/// What the extension last heard.
#[derive(Debug, Default)]
struct State {
    /// When the last request was made, seconds since 1970.
    asked_at: Option<i64>,
    /// Whether the last request failed, so the figures shown are old.
    failing: bool,
    /// When figures were last heard, seconds since 1970.
    heard_at: Option<i64>,
    windows: Vec<Window>,
}

/// One usage window: how much of it is used, and when it starts over.
#[derive(Clone, Debug, PartialEq)]
struct Window {
    resets_at: Option<DateTime<Utc>>,
    /// From 0 to 100.
    used_percent: f64,
    /// Whether the reset is days away, so its day is shown with the time.
    weekly: bool,
}

// ========================================================================
// Statics
// ========================================================================

static STATE: Mutex<State> = Mutex::new(State {
    asked_at: None,
    failing: false,
    heard_at: None,
    windows: Vec::new(),
});

// ========================================================================
// Extension
// ========================================================================

impl Extension for Claude {
    fn id(&self) -> &'static str {
        ID
    }

    fn activate(&self, registry: &mut Registry) {
        registry.status_segment_with_icon(SEGMENT, Icon::Named(LOGO.to_string()));
        registry.command("quota_refresh", "Claude: Refresh Quota", ask);
        registry.on_event(hear);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What the extension does on hearing `event`.
fn hear(host: &mut dyn Host, event: &Event) {
    match event {
        Event::Shown if is_due(&state(), now()) => ask(host),
        Event::Clicked(_) | Event::Timer(_) => ask(host),
        Event::Fetched(fetched) if fetched.tag == USAGE_TAG => take(host, fetched),
        _ => {}
    }
}

/// Ask for the usage, with the token Claude Code signed in with; signed
/// out, show nothing.
fn ask(host: &mut dyn Host) {
    let Some(token) = access_token() else {
        *state() = State::default();
        host.set_status(SEGMENT, None);
        return;
    };
    state().asked_at = Some(now());
    let request = Request::new(USAGE_URL)
        .with_header("Accept", "application/json")
        .with_header(BETA_HEADER.0, BETA_HEADER.1)
        .with_header("Authorization", &format!("Bearer {token}"));
    host.fetch(USAGE_TAG, request);
}

/// Take what the usage request came to, show it, and ask again in
/// [`REFRESH`].
fn take(host: &mut dyn Host, fetched: &Fetched) {
    host.set_timer(REFRESH_TIMER, REFRESH);
    let mut state = state();
    match &fetched.outcome {
        Ok(response) if SIGNED_OUT.contains(&response.status) => *state = State::default(),
        Ok(response) => match response
            .is_success()
            .then(|| parse(&response.body))
            .flatten()
        {
            Some(windows) => {
                state.windows = windows;
                state.heard_at = Some(now());
                state.failing = false;
            }
            None => state.failing = true,
        },
        Err(_) => state.failing = true,
    }
    let status = status_of(&state, |at| host.utc_offset(at));
    drop(state);
    host.set_status(SEGMENT, status);
}

/// Whether it is time to ask again, as of `now`: never asked, or not for
/// [`REFRESH`].
fn is_due(state: &State, now: i64) -> bool {
    state
        .asked_at
        .is_none_or(|at| now - at >= REFRESH.as_secs() as i64)
}

/// What the segment shows for `state`, with `offset` giving local time's
/// lead on UTC at a moment: nothing until figures are heard.
fn status_of(state: &State, offset: impl Fn(i64) -> i32) -> Option<Status> {
    let heard_at = state.heard_at?;
    let text = describe(&state.windows, &offset)?;
    let short = describe_compact(&state.windows)?;
    let heard = clock(heard_at, offset(heard_at), SHORT_RESET_FORMAT);
    let tooltip = match state.failing {
        true => format!("Claude Code usage · refresh failed, last heard {heard}"),
        false => format!("Claude Code usage · updated {heard}"),
    };
    Some(
        Status::new(text)
            .with_short(short)
            .with_tooltip(tooltip)
            .with_stale(state.failing),
    )
}

/// Each window's use and reset, in local time: `42% 14:05 | 10% Mon 09:00`.
fn describe(windows: &[Window], offset: &impl Fn(i64) -> i32) -> Option<String> {
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
                let at = at.timestamp();
                clock(at, offset(at), format)
            });
            format!("{:.0}%{THIN_SPACE}{reset}", window.used_percent)
        })
        .collect();
    Some(parts.join(WINDOW_SEPARATOR))
}

/// Each window's use alone, for a bar short of room: `42% 10%`.
fn describe_compact(windows: &[Window]) -> Option<String> {
    if windows.is_empty() {
        return None;
    }
    let parts: Vec<String> = windows
        .iter()
        .map(|window| format!("{:.0}%", window.used_percent))
        .collect();
    Some(parts.join(THIN_SPACE))
}

/// `at`, seconds since 1970, as a clock `offset` seconds ahead of UTC reads
/// it, in `format`.
fn clock(at: i64, offset: i32, format: &str) -> String {
    let zone = FixedOffset::east_opt(offset).unwrap_or(FixedOffset::east_opt(0).expect("UTC"));
    DateTime::from_timestamp(at, 0)
        .map(|at| at.with_timezone(&zone).format(format).to_string())
        .unwrap_or_else(|| UNKNOWN_RESET.to_string())
}

/// The windows in the endpoint's answer: `None` when it has no short window,
/// which every answer that means anything carries.
fn parse(body: &str) -> Option<Vec<Window>> {
    let usage: Value = serde_json::from_str(body).ok()?;
    let windows: Vec<Window> = WINDOWS
        .iter()
        .filter_map(|&(key, weekly)| {
            let window = usage.get(key)?;
            Some(Window {
                resets_at: window["resets_at"]
                    .as_str()
                    .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
                    .map(|at| at.with_timezone(&Utc)),
                used_percent: window["utilization"].as_f64()?,
                weekly,
            })
        })
        .collect();
    windows
        .first()
        .is_some_and(|first| !first.weekly)
        .then_some(windows)
}

/// The OAuth token Claude Code signed in with.
fn access_token() -> Option<String> {
    let home = PathBuf::from(std::env::var_os(HOME)?);
    let path = CREDENTIALS.iter().fold(home, |path, part| path.join(part));
    token_in(&std::fs::read_to_string(path).ok()?)
}

/// The token in a credentials document.
fn token_in(credentials: &str) -> Option<String> {
    let credentials: Value = serde_json::from_str(credentials.trim()).ok()?;
    let token = credentials["claudeAiOauth"]["accessToken"].as_str()?;
    (!token.is_empty()).then(|| token.to_string())
}

/// Now, in seconds since 1970.
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_usage_answer_gives_the_short_window_then_the_weekly() {
        let body = r#"{
            "five_hour": {"utilization": 42.0, "resets_at": "2026-10-22T14:05:00+00:00"},
            "seven_day": {"utilization": 10.0, "resets_at": null},
            "seven_day_opus": {"utilization": 0.0, "resets_at": null}
        }"#;
        let windows = parse(body).expect("windows");
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].used_percent, 42.0);
        assert!(!windows[0].weekly && windows[0].resets_at.is_some());
        assert!(windows[1].weekly && windows[1].resets_at.is_none());
    }

    #[test]
    fn test_an_answer_without_a_short_window_means_nothing() {
        assert_eq!(parse(r#"{"seven_day": {"utilization": 1.0}}"#), None);
        assert_eq!(parse(r#"{"error": "nope"}"#), None);
    }

    #[test]
    fn test_the_token_is_read_from_the_claude_ai_sign_in() {
        let credentials = r#"{"claudeAiOauth": {"accessToken": "sk-ant-oat", "expiresAt": 1}}"#;
        assert_eq!(token_in(credentials).as_deref(), Some("sk-ant-oat"));
        assert_eq!(token_in(r#"{"claudeAiOauth": {"accessToken": ""}}"#), None);
        assert_eq!(token_in("not json"), None);
    }

    #[test]
    fn test_a_weekly_reset_names_its_day_in_local_time_and_a_short_one_does_not() {
        let at = |text| {
            DateTime::parse_from_rfc3339(text)
                .ok()
                .map(|at| at.with_timezone(&Utc))
        };
        let windows = [
            Window {
                resets_at: at("2026-10-22T14:05:00Z"),
                used_percent: 41.6,
                weekly: false,
            },
            Window {
                resets_at: at("2026-10-26T09:00:00Z"),
                used_percent: 10.0,
                weekly: true,
            },
            Window {
                resets_at: None,
                used_percent: 3.0,
                weekly: false,
            },
        ];
        let two_hours_ahead = |_| 2 * 3600;
        assert_eq!(
            describe(&windows, &two_hours_ahead).as_deref(),
            Some(
                "42%\u{2009}16:05\u{2009}|\u{2009}10%\u{2009}Mon\u{2009}11:00\u{2009}|\u{2009}3%\u{2009}--"
            )
        );
        assert_eq!(
            describe_compact(&windows).as_deref(),
            Some("42%\u{2009}10%\u{2009}3%")
        );
    }

    #[test]
    fn test_figures_once_heard_stay_shown_dimmed_when_asking_again_fails() {
        let mut state = State::default();
        assert_eq!(status_of(&state, |_| 0), None, "nothing heard yet");
        state.windows = vec![Window {
            resets_at: None,
            used_percent: 42.0,
            weekly: false,
        }];
        state.heard_at = Some(0);
        state.failing = true;
        let status = status_of(&state, |_| 0).expect("shown");
        assert!(status.stale, "dimmed");
        assert_eq!(
            status.tooltip.as_deref(),
            Some("Claude Code usage · refresh failed, last heard 00:00")
        );
    }

    #[test]
    fn test_coming_into_sight_asks_again_only_once_the_figures_are_old() {
        let mut state = State::default();
        assert!(is_due(&state, 0), "never asked");
        state.asked_at = Some(1000);
        assert!(!is_due(&state, 1000 + 60), "asked a minute ago");
        assert!(is_due(&state, 1000 + REFRESH.as_secs() as i64));
    }
}
