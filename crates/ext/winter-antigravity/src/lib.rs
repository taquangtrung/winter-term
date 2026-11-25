//! Antigravity's usage quota on Winter's status bar, as an extension shipped
//! with it: how much of Gemini's five-hour window and of its weekly window
//! is used, and when each starts over, under Antigravity's logo.
//!
//! It reads the statusline file the `agy` CLI writes, and watches it, so the
//! figures change as the CLI writes new ones and nothing is read in
//! between. A window whose reset has passed has started over, unused since
//! or the file would say so: it shows `0%`, its next reset not yet known.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, FixedOffset, Utc};
use serde_json::Value;
use winter_ext_api::{Event, Extension, Host, Icon, Registry, Status};

// ========================================================================
// Constants
// ========================================================================

/// The extension's id.
const ID: &str = "antigravity";

/// The segment's name among the extension's: `antigravity.quota` on the
/// bar.
const SEGMENT: &str = "quota";

/// The logo drawn before the segment, one Winter bundles.
const LOGO: &str = "apps/antigravity";

/// The variable holding the user's home directory, inside the sandbox too.
const HOME: &str = "HOME";

/// Where the CLI writes its statusline, under the home directory.
const STATUSLINE: [&str; 3] = [".gemini", "antigravity-cli", ".statusline-last.json"];

/// The quota group whose windows are shown: Gemini's is the one every plan
/// has.
const SHOWN_GROUP: &str = "gemini";

/// How the windows are named in the file, after the group.
const SHORT_WINDOW: &str = "5h";
const WEEKLY_WINDOW: &str = "weekly";

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
pub struct Antigravity;

#[cfg(feature = "wasm")]
winter_ext_guest::export!(Antigravity);

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
// Extension
// ========================================================================

impl Extension for Antigravity {
    fn id(&self) -> &'static str {
        ID
    }

    fn activate(&self, registry: &mut Registry) {
        registry.status_segment_with_icon(SEGMENT, Icon::Named(LOGO.to_string()));
        registry.on_event(hear);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What the extension does on hearing `event`: shown, it watches the
/// statusline file and reads it; the file changed, or the segment clicked,
/// it reads it again.
fn hear(host: &mut dyn Host, event: &Event) {
    let Some(path) = statusline_path() else {
        return;
    };
    match event {
        Event::Shown => {
            host.watch_file(&path);
            show(host, &path);
        }
        Event::Clicked(_) | Event::FileChanged(_) => show(host, &path),
        _ => {}
    }
}

/// Show what the statusline file at `path` says, or nothing.
fn show(host: &mut dyn Host, path: &std::path::Path) {
    let text = std::fs::read_to_string(path).ok();
    let written = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
        .map(|since| since.as_secs() as i64);
    let windows = text
        .as_deref()
        .and_then(parse_statusline)
        .map(|windows| current(windows, now()))
        .unwrap_or_default();
    let status = written.and_then(|written| status_of(&windows, written, |at| host.utc_offset(at)));
    host.set_status(SEGMENT, status);
}

/// `windows` as they stand at `now`: one whose reset has passed has started
/// over, unused since, as the CLI would have written otherwise, its next
/// reset not yet known.
fn current(windows: Vec<Window>, now: i64) -> Vec<Window> {
    windows
        .into_iter()
        .map(|window| match window.resets_at {
            Some(at) if at.timestamp() <= now => Window {
                resets_at: None,
                used_percent: 0.0,
                weekly: window.weekly,
            },
            _ => window,
        })
        .collect()
}

/// What the segment shows for `windows`, read from a file `written` then,
/// with `offset` giving local time's lead on UTC at a moment.
fn status_of(windows: &[Window], written: i64, offset: impl Fn(i64) -> i32) -> Option<Status> {
    let text = describe(windows, &offset)?;
    let short = describe_compact(windows)?;
    let written = clock(written, offset(written), SHORT_RESET_FORMAT);
    Some(
        Status::new(text)
            .with_short(short)
            .with_tooltip(format!("Antigravity usage · updated {written}")),
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

/// The shown group's windows in a statusline file's text, whose quota is
/// keyed `<group>-<window>`, e.g. `gemini-5h`. A window without a reset
/// time is left out.
fn parse_statusline(text: &str) -> Option<Vec<Window>> {
    let file: Value = serde_json::from_str(text).ok()?;
    let quota = file.get("quota")?;
    let window = |name: &str, weekly: bool| {
        let entry = quota.get(format!("{SHOWN_GROUP}-{name}"))?;
        let reset = DateTime::parse_from_rfc3339(entry["reset_time"].as_str()?).ok()?;
        Some(Window {
            resets_at: Some(reset.with_timezone(&Utc)),
            used_percent: used_percent(entry["remaining_fraction"].as_f64()),
            weekly,
        })
    };
    let windows: Vec<Window> = [window(SHORT_WINDOW, false), window(WEEKLY_WINDOW, true)]
        .into_iter()
        .flatten()
        .collect();
    (!windows.is_empty()).then_some(windows)
}

/// How much is used, from how much is left: an unreported fraction is a
/// window untouched.
fn used_percent(remaining_fraction: Option<f64>) -> f64 {
    (1.0 - remaining_fraction.unwrap_or(1.0)) * 100.0
}

/// Where the CLI writes its statusline.
fn statusline_path() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os(HOME)?);
    Some(STATUSLINE.iter().fold(home, |path, part| path.join(part)))
}

/// Now, in seconds since 1970.
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_statusline_file_is_keyed_by_group_and_window() {
        let text = r#"{"plan_tier": "pro", "quota": {
            "gemini-5h": {"remaining_fraction": 0.6, "reset_time": "2026-10-22T14:05:00Z"},
            "3p-5h": {"remaining_fraction": 0.0, "reset_time": "2026-10-22T14:05:00Z"},
            "gemini-weekly": {"remaining_fraction": 0.9}
        }}"#;
        let windows = parse_statusline(text).expect("windows");
        assert_eq!(windows.len(), 1, "a window without a reset is left out");
        assert!((windows[0].used_percent - 40.0).abs() < 1e-9);
    }

    #[test]
    fn test_a_window_whose_reset_has_passed_shows_it_started_over() {
        // Left out, a CLI unused since the five-hour reset showed the weekly
        // window alone; kept as read, it showed the use of a window long over.
        let text = r#"{"quota": {
            "gemini-5h": {"remaining_fraction": 0.6, "reset_time": "2026-10-22T14:05:00Z"},
            "gemini-weekly": {"remaining_fraction": 0.5, "reset_time": "2026-10-26T09:00:00Z"}
        }}"#;
        let windows = parse_statusline(text).expect("windows");
        let between = DateTime::parse_from_rfc3339("2026-10-23T00:00:00Z")
            .expect("a time")
            .timestamp();
        let now = current(windows.clone(), between);
        assert_eq!(now.len(), 2, "both windows shown");
        assert_eq!(
            (now[0].used_percent, now[0].resets_at),
            (0.0, None),
            "the five-hour window started over"
        );
        assert!(
            (now[1].used_percent - 50.0).abs() < 1e-9,
            "the weekly window still runs"
        );
        assert_eq!(
            describe(&now, &|_| 0).as_deref(),
            Some("0%\u{2009}--\u{2009}|\u{2009}50%\u{2009}Mon\u{2009}09:00")
        );
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
}
