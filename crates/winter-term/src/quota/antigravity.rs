//! Antigravity's quota, asked of the language server the Antigravity editor
//! runs on this machine, or read from the file the `agy` CLI writes its
//! statusline to.
//!
//! The language server is found among the running processes by the flags it
//! is started with: the port it serves on and the token it wants with every
//! request. It answers on that port or on another it listens on, so each is
//! tried until one answers; the one that did is remembered for next time.
//! The CLI's server hands its token only to its own child shells, so for the
//! CLI the statusline file is all there is.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;

use crate::paths::home_dir;
use crate::process;

use super::http::{self, Request};
use super::{parse_time, Fetched, Window};

// ========================================================================
// Constants
// ========================================================================

/// What the quota is asked for at, on the language server.
const SUMMARY_PATH: &str = "/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";

/// How long the language server may take to answer one port.
const TIMEOUT: Duration = Duration::from_secs(5);

/// The flags the language server is started with: the port it serves the
/// editor on, the token it wants back, and the directory naming the editor.
const PORT_FLAG: &str = "--extension_server_port";
const TOKEN_FLAG: &str = "--csrf_token";
const APP_DIR_FLAG: &str = "--app_data_dir";

/// What the app directory of Antigravity's own server holds, which tells it
/// from the same server run by another editor.
const APP_NAME: &str = "antigravity";

/// Part of the language server's program name on every platform.
const SERVER_PROGRAM: &str = "language_server";

/// The CLI's program name.
const CLI_PROGRAM: &str = "agy";

/// Programs a script may be run through, whose first argument is then the
/// program the process is.
const INTERPRETERS: [&str; 5] = ["bun", "deno", "node", "python", "python3"];

/// Where the CLI writes its statusline, under the home directory.
const STATUSLINE_PATH: [&str; 3] = [".gemini", "antigravity-cli", ".statusline-last.json"];

/// The quota group whose windows are shown, matched without case: Gemini's
/// is the one every plan has.
const SHOWN_GROUP: &str = "gemini";

/// How the windows are named, in the server's answer and in the file.
const SHORT_WINDOW: &str = "5h";
const WEEKLY_WINDOW: &str = "weekly";

/// The language server's program name on Windows, which lists processes
/// by name rather than by command line.
#[cfg(windows)]
const WINDOWS_SERVER: &str = "language_server_windows_x64.exe";

// ========================================================================
// Data Structures
// ========================================================================

/// Where a language server answers, and the token it wants.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Server {
    port: u16,
    token: String,
}

/// A language server process, as found among the running ones.
#[derive(Debug, Eq, PartialEq)]
struct Candidate {
    pid: u32,
    /// The port its flags name, tried before the others it listens on.
    port: u16,
    token: String,
}

// ========================================================================
// Statics
// ========================================================================

/// The server that answered last, asked first next time.
static LAST_SERVER: Mutex<Option<Server>> = Mutex::new(None);

// ========================================================================
// Functions
// ========================================================================

/// Ask for Antigravity's windows.
pub(super) fn fetch() -> Fetched {
    let last = LAST_SERVER.lock().ok().and_then(|last| last.clone());
    if let Some(windows) = last.as_ref().and_then(ask) {
        return Fetched::Usage(windows);
    }
    let processes = list_processes();
    if let Some((server, windows)) = find_server(&processes) {
        if let Ok(mut last) = LAST_SERVER.lock() {
            *last = Some(server);
        }
        return Fetched::Usage(windows);
    }
    let cli_running = processes.iter().any(|(_, args)| is_cli(args));
    match cli_running.then(read_statusline).flatten() {
        Some(windows) => Fetched::Usage(windows),
        None => Fetched::Missing,
    }
}

/// The first language server among `processes` that answers, and what it
/// answered.
fn find_server(processes: &[(u32, String)]) -> Option<(Server, Vec<Window>)> {
    processes
        .iter()
        .filter_map(|(pid, args)| candidate(*pid, args))
        .find_map(|candidate| {
            let others = listening_ports(candidate.pid);
            let ports = std::iter::once(candidate.port)
                .chain(others.into_iter().filter(|&port| port != candidate.port));
            ports
                .map(|port| Server {
                    port,
                    token: candidate.token.clone(),
                })
                .find_map(|server| ask(&server).map(|windows| (server, windows)))
        })
}

/// Ask `server` for the quota summary.
fn ask(server: &Server) -> Option<Vec<Window>> {
    let headers = [
        "Content-Type: application/json".to_string(),
        "Connect-Protocol-Version: 1".to_string(),
        format!("X-Codeium-Csrf-Token: {}", server.token),
    ];
    let url = format!("https://127.0.0.1:{}{SUMMARY_PATH}", server.port);
    let request = Request {
        body: Some("{}"),
        headers: &headers,
        insecure: true,
        timeout: TIMEOUT,
        url: &url,
    };
    parse_summary(&http::send(&request).ok()?)
}

/// The shown group's windows in the server's quota summary.
fn parse_summary(body: &str) -> Option<Vec<Window>> {
    let summary: Value = serde_json::from_str(body).ok()?;
    let group = summary["response"]["groups"]
        .as_array()?
        .iter()
        .find(|group| {
            group["displayName"]
                .as_str()
                .is_some_and(|name| name.to_lowercase().contains(SHOWN_GROUP))
        })?;
    let buckets = group["buckets"].as_array()?;
    let window = |name: &str, weekly: bool| {
        let bucket = buckets
            .iter()
            .find(|bucket| bucket["window"].as_str() == Some(name))?;
        Some(Window {
            resets_at: bucket["resetTime"].as_str().and_then(parse_time),
            used_percent: used_percent(bucket["remainingFraction"].as_f64()),
            weekly,
        })
    };
    let windows: Vec<Window> = [window(SHORT_WINDOW, false), window(WEEKLY_WINDOW, true)]
        .into_iter()
        .flatten()
        .collect();
    (!windows.is_empty()).then_some(windows)
}

/// The shown group's windows in the CLI's statusline file.
fn read_statusline() -> Option<Vec<Window>> {
    let path = STATUSLINE_PATH
        .iter()
        .fold(home_dir()?, |path, part| path.join(part));
    parse_statusline(&std::fs::read_to_string(path).ok()?)
}

/// The shown group's windows in a statusline file's text, whose quota is
/// keyed `<group>-<window>`, e.g. `gemini-5h`.
fn parse_statusline(text: &str) -> Option<Vec<Window>> {
    let file: Value = serde_json::from_str(text).ok()?;
    let quota = file.get("quota")?;
    let window = |name: &str, weekly: bool| {
        let entry = quota.get(format!("{SHOWN_GROUP}-{name}"))?;
        Some(Window {
            resets_at: Some(parse_time(entry["reset_time"].as_str()?)?),
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

/// The language server a process with `args` is, if it is Antigravity's.
fn candidate(pid: u32, args: &str) -> Option<Candidate> {
    if !args.contains(SERVER_PROGRAM) {
        return None;
    }
    let app_dir = flag_value(args, APP_DIR_FLAG)?;
    if !app_dir.to_lowercase().contains(APP_NAME) {
        return None;
    }
    Some(Candidate {
        pid,
        port: flag_value(args, PORT_FLAG)?.parse().ok()?,
        token: flag_value(args, TOKEN_FLAG)?.to_string(),
    })
}

/// The value given to `flag` in a command line, as `--flag value` or
/// `--flag=value`, quotes taken off.
fn flag_value<'a>(args: &'a str, flag: &str) -> Option<&'a str> {
    let mut words = args.split_whitespace();
    while let Some(word) = words.next() {
        let value = match word.strip_prefix(flag) {
            Some("") => words.next(),
            Some(rest) => rest.strip_prefix('='),
            None => None,
        };
        if let Some(value) = value {
            return Some(value.trim_matches(['"', '\'']));
        }
    }
    None
}

/// Whether a command line is the CLI's: its program is, or the script an
/// interpreter is running is.
fn is_cli(args: &str) -> bool {
    let name = |word: &str| {
        PathBuf::from(word)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    };
    let mut words = args.split_whitespace();
    let Some(program) = words.next().and_then(name) else {
        return false;
    };
    match INTERPRETERS.contains(&program.as_str()) {
        true => words
            .next()
            .and_then(name)
            .is_some_and(|script| script == CLI_PROGRAM),
        false => program == CLI_PROGRAM,
    }
}

/// Every running process, as its id and command line.
#[cfg(not(windows))]
fn list_processes() -> Vec<(u32, String)> {
    let Some(listing) = output_of("ps", &["-eo", "pid=,args="]) else {
        return Vec::new();
    };
    listing
        .lines()
        .filter_map(|line| {
            let (pid, args) = line.trim_start().split_once(char::is_whitespace)?;
            Some((pid.parse().ok()?, args.trim().to_string()))
        })
        .collect()
}

/// The language server processes: Windows has no `ps`, and asking it for
/// every command line is slow, so only the server's are asked for.
#[cfg(windows)]
fn list_processes() -> Vec<(u32, String)> {
    let script = format!(
        "ConvertTo-Json -Compress -InputObject @(Get-CimInstance Win32_Process \
         -Filter \"Name='{WINDOWS_SERVER}'\" | Select-Object ProcessId, CommandLine)"
    );
    let args = ["-NoProfile", "-NonInteractive", "-Command", script.as_str()];
    let Some(listing) = output_of("powershell.exe", &args) else {
        return Vec::new();
    };
    let Ok(Value::Array(rows)) = serde_json::from_str(listing.trim()) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let pid = u32::try_from(row["ProcessId"].as_u64()?).ok()?;
            Some((pid, row["CommandLine"].as_str()?.to_string()))
        })
        .collect()
}

/// The TCP ports process `pid` listens on.
#[cfg(target_os = "linux")]
fn listening_ports(pid: u32) -> Vec<u16> {
    output_of("ss", &["-tlnpH"]).map_or_else(Vec::new, |listing| ss_ports(&listing, pid))
}

/// The TCP ports process `pid` listens on.
#[cfg(target_os = "macos")]
fn listening_ports(pid: u32) -> Vec<u16> {
    let pid = pid.to_string();
    let args = ["-Pan", "-p", pid.as_str(), "-iTCP", "-sTCP:LISTEN"];
    output_of("lsof", &args).map_or_else(Vec::new, |listing| lsof_ports(&listing))
}

/// The TCP ports process `pid` listens on.
#[cfg(windows)]
fn listening_ports(pid: u32) -> Vec<u16> {
    output_of("netstat", &["-ano"]).map_or_else(Vec::new, |listing| netstat_ports(&listing, pid))
}

/// No way to list them here: only the port the flags name is tried.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn listening_ports(_pid: u32) -> Vec<u16> {
    Vec::new()
}

/// The listening ports `ss -tlnpH` gives process `pid`: the local address's
/// port, on the lines naming the process.
#[cfg(any(target_os = "linux", test))]
fn ss_ports(listing: &str, pid: u32) -> Vec<u16> {
    let owner = format!("pid={pid},");
    let mut ports: Vec<u16> = listing
        .lines()
        .filter(|line| line.contains(&owner))
        .filter_map(|line| {
            let local = line.split_whitespace().nth(3)?;
            local.rsplit_once(':')?.1.parse().ok()
        })
        .collect();
    ports.dedup();
    ports
}

/// The listening ports in `lsof -iTCP -sTCP:LISTEN` output: the address
/// before `(LISTEN)`.
#[cfg(any(target_os = "macos", test))]
fn lsof_ports(listing: &str) -> Vec<u16> {
    let mut ports: Vec<u16> = listing
        .lines()
        .filter_map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            let at = words.iter().position(|word| *word == "(LISTEN)")?;
            words
                .get(at.checked_sub(1)?)?
                .rsplit_once(':')?
                .1
                .parse()
                .ok()
        })
        .collect();
    ports.dedup();
    ports
}

/// The ports `netstat -ano` shows process `pid` listening on.
#[cfg(any(windows, test))]
fn netstat_ports(listing: &str, pid: u32) -> Vec<u16> {
    let pid = pid.to_string();
    let mut ports: Vec<u16> = listing
        .lines()
        .filter_map(|line| {
            // Proto, Local Address, Foreign Address, State, PID.
            let columns: Vec<&str> = line.split_whitespace().collect();
            if columns.len() < 5 || columns[3] != "LISTENING" || columns[4] != pid {
                return None;
            }
            columns[1].rsplit_once(':')?.1.parse().ok()
        })
        .collect();
    ports.dedup();
    ports
}

/// What `program` prints, when it runs and succeeds.
fn output_of(program: &str, args: &[&str]) -> Option<String> {
    let output = process::command(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_antigravity_s_language_server_is_found_by_its_flags() {
        let args = "/opt/antigravity/language_server_linux_x64 --extension_server_port 42100 \
                    --csrf_token=1f2e-ab --app_data_dir \"antigravity\"";
        assert_eq!(
            candidate(7, args),
            Some(Candidate {
                pid: 7,
                port: 42100,
                token: "1f2e-ab".to_string(),
            })
        );
        let other_editor = args.replace("\"antigravity\"", "windsurf");
        assert_eq!(candidate(7, &other_editor), None, "another editor's server");
        assert_eq!(candidate(7, "vim language_server.md"), None);
    }

    #[test]
    fn test_the_cli_is_told_by_its_program_name_alone() {
        assert!(is_cli("/home/me/.local/bin/agy --resume"));
        assert!(is_cli("node /usr/lib/agy/bin/agy chat"));
        assert!(!is_cli("grep -w agy"), "a word, not the program");
        assert!(!is_cli("/usr/bin/agyle"));
    }

    #[test]
    fn test_the_summary_gives_the_gemini_group_s_windows() {
        let body = r#"{"response": {"groups": [
            {"displayName": "Third-party models", "buckets": [
                {"window": "5h", "remainingFraction": 0.1}]},
            {"displayName": "Gemini", "buckets": [
                {"window": "weekly", "remainingFraction": 0.75,
                 "resetTime": "2026-10-26T09:00:00Z"},
                {"window": "5h", "resetTime": "2026-10-22T14:05:00Z"}]}
        ]}}"#;
        let windows = parse_summary(body).expect("windows");
        assert_eq!(
            windows
                .iter()
                .map(|w| (w.used_percent, w.weekly))
                .collect::<Vec<_>>(),
            [(0.0, false), (25.0, true)],
            "short first; a missing fraction is a window untouched"
        );
    }

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
    fn test_listening_ports_are_read_from_each_platform_s_listing() {
        let ss = "LISTEN 0 4096 127.0.0.1:42100 0.0.0.0:* users:((\"language_server\",pid=7,fd=9))\n\
                  LISTEN 0 4096 127.0.0.1:42101 0.0.0.0:* users:((\"language_server\",pid=70,fd=9))\n\
                  LISTEN 0 4096 [::1]:42102 [::]:* users:((\"language_server\",pid=7,fd=10))";
        assert_eq!(ss_ports(ss, 7), [42100, 42102], "not pid 70's");

        let lsof = "COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME\n\
                    language_ 7 me 9u IPv4 0x1 0t0 TCP 127.0.0.1:42100 (LISTEN)";
        assert_eq!(lsof_ports(lsof), [42100]);

        let netstat = "  TCP    127.0.0.1:42100   0.0.0.0:0   LISTENING   7\n\
                       \x20 TCP    127.0.0.1:42101   0.0.0.0:0   LISTENING   77\n\
                       \x20 TCP    127.0.0.1:42102   127.0.0.1:5 ESTABLISHED 7";
        assert_eq!(netstat_ports(netstat, 7), [42100]);
    }
}
