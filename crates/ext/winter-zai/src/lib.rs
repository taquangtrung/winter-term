//! Z.ai's usage quota on Winter's status bar, as an extension shipped with
//! it: how much of the five-hour token window and of the weekly one is
//! used, and when each starts over, under Z.ai's logo.
//!
//! It asks Z.ai's quota endpoint with the user's API key, which
//! `Z.ai: Set API Key` asks for once. Winter keeps the key in the system's
//! secret store and puts it in the request itself; this extension never
//! sees it.
//!
//! - It asks as its segment comes into sight, if what it has is older than
//!   [`REFRESH`], then every [`REFRESH`] while the segment stays in sight.
//! - A click on the segment, or `Z.ai: Refresh Quota`, asks again now.
//! - With no key, or one Z.ai refuses, the segment shows nothing; a failed
//!   request keeps the last figures, dimmed.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, FixedOffset, Utc};
use serde_json::Value;
use winter_ext_api::{
    Event, Extension, FetchError, Fetched, Host, Icon, Registry, Request, Status,
};

// ========================================================================
// Constants
// ========================================================================

/// The extension's id.
const ID: &str = "zai";

/// The segment's name among the extension's: `zai.quota` on the bar.
const SEGMENT: &str = "quota";

/// The logo drawn before the segment.
const LOGO: &str = include_str!("../assets/logo.svg");

/// How often the quota is asked for: it moves with use, and a request a
/// minute would only spend the endpoint's rate limit.
const REFRESH: Duration = Duration::from_secs(5 * 60);

/// The timer that asks again.
const REFRESH_TIMER: &str = "refresh";

/// The tag the usage request goes under.
const USAGE_TAG: u64 = 1;

/// Where the quota is asked for.
const QUOTA_URL: &str = "https://api.z.ai/api/monitor/usage/quota/limit";

/// The secret the API key is kept as, and what asking for it says.
const API_KEY: &str = "api-key";
const API_KEY_LABEL: &str = "Z.ai API key (blank to forget it)";

/// The status meaning the key is no good.
const REFUSED: u16 = 401;

/// The kind of limit the token windows are.
const TOKENS_LIMIT: &str = "TOKENS_LIMIT";

/// How the endpoint spells each token window, as `(unit, number)`: five
/// hours, and one week.
const FIVE_HOURS: (i64, i64) = (3, 5);
const ONE_WEEK: (i64, i64) = (6, 1);

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
pub struct Zai;

#[cfg(feature = "wasm")]
winter_ext_guest::export!(Zai);

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

impl Extension for Zai {
    fn id(&self) -> &'static str {
        ID
    }

    fn activate(&self, registry: &mut Registry) {
        registry.status_segment_with_icon(SEGMENT, Icon::Svg(LOGO.to_string()));
        registry.command("quota_refresh", "Z.ai: Refresh Quota", ask);
        registry.command("set_api_key", "Z.ai: Set API Key", |host| {
            host.ask_secret(API_KEY, API_KEY_LABEL)
        });
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
        Event::Clicked(_) | Event::SecretChanged(_) | Event::Timer(_) => ask(host),
        Event::Fetched(fetched) if fetched.tag == USAGE_TAG => take(host, fetched),
        _ => {}
    }
}

/// Ask for the quota, Winter putting the API key in.
fn ask(host: &mut dyn Host) {
    state().asked_at = Some(now());
    let request = Request::new(QUOTA_URL)
        .with_header("Accept", "application/json")
        .with_secret_header("Authorization", "Bearer ", API_KEY);
    host.fetch(USAGE_TAG, request);
}

/// Take what the usage request came to, show it, and ask again in
/// [`REFRESH`].
fn take(host: &mut dyn Host, fetched: &Fetched) {
    host.set_timer(REFRESH_TIMER, REFRESH);
    let mut state = state();
    match &fetched.outcome {
        Ok(response) if response.status == REFUSED => *state = State::default(),
        Err(FetchError::MissingSecret(_)) => *state = State::default(),
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
        true => format!("Z.ai usage · refresh failed, last heard {heard}"),
        false => format!("Z.ai usage · updated {heard}"),
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

/// The token windows in the endpoint's answer, the five-hour one first:
/// `None` when the answer is a refusal or has no five-hour window. A token
/// window spelled some other way stands in for the five-hour one when that
/// is missing, as the endpoint's older answers had it.
fn parse(body: &str) -> Option<Vec<Window>> {
    let answer: Value = serde_json::from_str(body).ok()?;
    if answer["success"].as_bool() != Some(true) {
        return None;
    }
    let mut short = None;
    let mut weekly = None;
    let mut other = None;
    for limit in answer["data"]["limits"].as_array()? {
        if limit["type"].as_str() != Some(TOKENS_LIMIT) {
            continue;
        }
        let spelled = (
            limit["unit"].as_i64().unwrap_or_default(),
            limit["number"].as_i64().unwrap_or_default(),
        );
        let slot = match spelled {
            FIVE_HOURS => &mut short,
            ONE_WEEK => &mut weekly,
            _ => &mut other,
        };
        slot.get_or_insert(limit);
    }
    let window = |limit: &Value, weekly: bool| {
        Some(Window {
            resets_at: limit["nextResetTime"]
                .as_i64()
                .and_then(DateTime::from_timestamp_millis),
            used_percent: limit["percentage"].as_f64()?,
            weekly,
        })
    };
    let mut windows = vec![window(short.or(other)?, false)?];
    windows.extend(weekly.and_then(|limit| window(limit, true)));
    Some(windows)
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
    fn test_the_token_windows_are_told_apart_by_how_they_are_spelled() {
        let body = r#"{"success": true, "data": {"limits": [
            {"type": "TIME_LIMIT", "percentage": 90, "unit": 5, "number": 1},
            {"type": "TOKENS_LIMIT", "percentage": 12, "unit": 6, "number": 1,
             "nextResetTime": 1792000000000},
            {"type": "TOKENS_LIMIT", "percentage": 34, "unit": 3, "number": 5}
        ]}}"#;
        let windows = parse(body).expect("windows");
        assert_eq!(
            windows
                .iter()
                .map(|w| (w.used_percent, w.weekly))
                .collect::<Vec<_>>(),
            [(34.0, false), (12.0, true)],
            "the short window first, the time limit left out"
        );
        assert!(windows[0].resets_at.is_none() && windows[1].resets_at.is_some());
    }

    #[test]
    fn test_a_refusal_shows_nothing() {
        assert_eq!(parse(r#"{"success": false, "msg": "bad key"}"#), None);
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
            Some("Z.ai usage · refresh failed, last heard 00:00")
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
