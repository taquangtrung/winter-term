//! Z.ai's quota: its usage endpoint, asked with the API key kept in the
//! system's secret store (see [`super::secret`]).

use std::time::Duration;

use chrono::DateTime;
use serde_json::Value;

use super::http::{self, HttpError, Request};
use super::{secret, Fetched, Window};

// ========================================================================
// Constants
// ========================================================================

/// Where the quota is asked for.
const QUOTA_URL: &str = "https://api.z.ai/api/monitor/usage/quota/limit";

/// How long the quota request may take.
const TIMEOUT: Duration = Duration::from_secs(10);

/// The kind of limit the token windows are.
const TOKENS_LIMIT: &str = "TOKENS_LIMIT";

/// How the endpoint spells each token window, as `(unit, number)`: five
/// hours, and one week.
const FIVE_HOURS: (i64, i64) = (3, 5);
const ONE_WEEK: (i64, i64) = (6, 1);

// ========================================================================
// Functions
// ========================================================================

/// Ask for Z.ai's windows.
pub(super) fn fetch() -> Fetched {
    let Some(key) = secret::zai_key() else {
        return Fetched::Missing;
    };
    let headers = [
        "Accept: application/json".to_string(),
        format!("Authorization: Bearer {key}"),
    ];
    let request = Request {
        body: None,
        headers: &headers,
        insecure: false,
        timeout: TIMEOUT,
        url: QUOTA_URL,
    };
    match http::send(&request) {
        Ok(body) => parse(&body).map_or(Fetched::Failed, Fetched::Usage),
        Err(HttpError::Status(401)) => Fetched::Missing,
        Err(_) => Fetched::Failed,
    }
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
}
