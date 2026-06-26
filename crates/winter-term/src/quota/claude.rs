//! Claude Code's quota: Anthropic's usage endpoint, asked with the OAuth
//! token Claude Code signed in with. The token is read where Claude Code
//! keeps it: the macOS keychain, else `.credentials.json` in its directory.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::model::path::home_dir;
#[cfg(target_os = "macos")]
use crate::process;

use super::http::{self, HttpError, Request};
use super::{parse_time, Fetched, Window};

// ========================================================================
// Constants
// ========================================================================

/// Where the usage is asked for.
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// The beta the OAuth endpoints are behind.
const BETA_HEADER: &str = "anthropic-beta: oauth-2025-04-20";

/// How long the usage request may take.
const TIMEOUT: Duration = Duration::from_secs(15);

/// Where Claude Code is told to keep its files instead of `~/.claude`.
const CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";

/// Claude Code's directory under the home directory.
const DEFAULT_DIR: &str = ".claude";

/// The file holding the sign-in, in Claude Code's directory.
const CREDENTIALS_FILE: &str = ".credentials.json";

/// The macOS keychain item Claude Code keeps the sign-in in.
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// The windows the endpoint reports, the short one first, and whether each
/// is weekly.
const WINDOWS: [(&str, bool); 2] = [("five_hour", false), ("seven_day", true)];

// ========================================================================
// Functions
// ========================================================================

/// Ask for Claude Code's windows.
pub(super) fn fetch() -> Fetched {
    let Some(token) = access_token() else {
        return Fetched::Missing;
    };
    let headers = [
        "Accept: application/json".to_string(),
        BETA_HEADER.to_string(),
        format!("Authorization: Bearer {token}"),
    ];
    let request = Request {
        body: None,
        headers: &headers,
        insecure: false,
        timeout: TIMEOUT,
        url: USAGE_URL,
    };
    match http::send(&request) {
        Ok(body) => parse(&body).map_or(Fetched::Failed, Fetched::Usage),
        // A token that has run out is renewed by Claude Code the next time
        // it runs; until then there is nothing to show.
        Err(HttpError::Status(401 | 403)) => Fetched::Missing,
        Err(_) => Fetched::Failed,
    }
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
                resets_at: window["resets_at"].as_str().and_then(parse_time),
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
    let credentials = keychain_credentials().or_else(|| {
        let path = claude_dir()?.join(CREDENTIALS_FILE);
        std::fs::read_to_string(path).ok()
    })?;
    token_in(&credentials)
}

/// The token in a credentials document.
fn token_in(credentials: &str) -> Option<String> {
    let credentials: Value = serde_json::from_str(credentials.trim()).ok()?;
    let token = credentials["claudeAiOauth"]["accessToken"].as_str()?;
    (!token.is_empty()).then(|| token.to_string())
}

/// Claude Code's directory: where it was told to keep its files, else
/// `~/.claude`.
fn claude_dir() -> Option<PathBuf> {
    match std::env::var_os(CONFIG_DIR_ENV) {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => Some(home_dir()?.join(DEFAULT_DIR)),
    }
}

/// The credentials document in the macOS keychain, where Claude Code keeps
/// it there.
#[cfg(target_os = "macos")]
fn keychain_credentials() -> Option<String> {
    let output = process::command("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Only macOS keeps it in a keychain.
#[cfg(not(target_os = "macos"))]
fn keychain_credentials() -> Option<String> {
    None
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
}
