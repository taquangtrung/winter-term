//! The commands that act on a process: signal it, or change its priority. Each
//! is a program run on the monitor's behalf, so no unsafe code is needed to
//! reach the system call behind it.

use std::ops::RangeInclusive;
use std::path::PathBuf;

use crate::model::page::CommandRequest;

// ========================================================================
// Constants
// ========================================================================

/// The tag on a request that signals processes.
pub const TAG_SIGNAL: &str = "proc-signal";

/// The tag on a request that changes a process's priority.
pub const TAG_RENICE: &str = "proc-renice";

/// Lowest and highest nice value Unix accepts: the first is the greediest.
const NICE_RANGE: RangeInclusive<i32> = -20..=19;

/// The priority classes Windows accepts, lowest first.
const WINDOWS_PRIORITIES: [&str; 6] = [
    "Idle",
    "BelowNormal",
    "Normal",
    "AboveNormal",
    "High",
    "RealTime",
];

/// Said when a Windows process is asked to stop or continue, which it has no
/// signal for.
const NO_SUSPEND_ON_WINDOWS: &str = "suspend and resume are not supported on Windows";

// ========================================================================
// Data Structures
// ========================================================================

/// What to do to a process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Signal {
    /// Let a stopped process run again.
    Continue,
    /// End it with no chance to clean up.
    Kill,
    /// Freeze it where it is.
    Stop,
    /// Ask it to end, which it may decline.
    Terminate,
}

// ========================================================================
// Signal
// ========================================================================

impl Signal {
    /// The verb for a one-line report: "terminated", "killed".
    pub fn past_tense(self) -> &'static str {
        match self {
            Signal::Continue => "resumed",
            Signal::Kill => "killed",
            Signal::Stop => "suspended",
            Signal::Terminate => "terminated",
        }
    }

    /// The verb for a question: "terminate", "kill".
    pub fn verb(self) -> &'static str {
        match self {
            Signal::Continue => "resume",
            Signal::Kill => "kill",
            Signal::Stop => "suspend",
            Signal::Terminate => "terminate",
        }
    }

    /// The signal's name as `kill` spells it.
    fn unix_name(self) -> &'static str {
        match self {
            Signal::Continue => "-CONT",
            Signal::Kill => "-KILL",
            Signal::Stop => "-STOP",
            Signal::Terminate => "-TERM",
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The command that sends `signal` to `pid`, and with `tree` to everything
/// under it as well. `descendants` lists those, children before their
/// parents. Unix signals each one in a single `kill`; Windows hands the whole
/// tree to `taskkill`, which finds it itself.
pub fn signal_request(
    signal: Signal,
    pid: u32,
    descendants: &[u32],
    tree: bool,
    windows: bool,
) -> Result<CommandRequest, String> {
    if windows {
        return windows_signal(signal, pid, tree);
    }
    let targets = if tree { descendants } else { &[] };
    let args = std::iter::once(signal.unix_name().to_string())
        .chain(targets.iter().chain([&pid]).map(u32::to_string))
        .collect();
    Ok(request("kill", args, TAG_SIGNAL))
}

/// The command that sets `pid`'s priority to `value`: a nice value on Unix, a
/// priority class on Windows. Nothing else is ever put on a command line.
pub fn renice_request(pid: u32, value: &str, windows: bool) -> Result<CommandRequest, String> {
    let value = value.trim();
    if windows {
        let class = WINDOWS_PRIORITIES
            .iter()
            .find(|class| class.eq_ignore_ascii_case(value))
            .ok_or_else(|| format!("not a priority class: {value}"))?;
        let script = format!("(Get-Process -Id {pid}).PriorityClass = '{class}'");
        let args = ["-NoProfile", "-NonInteractive", "-Command", &script]
            .map(str::to_string)
            .to_vec();
        return Ok(request("powershell.exe", args, TAG_RENICE));
    }
    let nice: i32 = value
        .parse()
        .ok()
        .filter(|nice| NICE_RANGE.contains(nice))
        .ok_or_else(|| {
            format!(
                "not a nice value: {value} (from {} to {})",
                NICE_RANGE.start(),
                NICE_RANGE.end()
            )
        })?;
    // The value comes first, on its own: util-linux, BSD and BusyBox renice
    // all read that as the absolute priority, where `-n` is an increment on
    // the last two.
    let args = vec![nice.to_string(), "-p".to_string(), pid.to_string()];
    Ok(request("renice", args, TAG_RENICE))
}

/// `taskkill` for a Windows process. Without `/F` it only asks a process's
/// windows to close, which a console or background process refuses.
fn windows_signal(signal: Signal, pid: u32, tree: bool) -> Result<CommandRequest, String> {
    let force = match signal {
        Signal::Kill => true,
        Signal::Terminate => false,
        Signal::Continue | Signal::Stop => return Err(NO_SUSPEND_ON_WINDOWS.to_string()),
    };
    let mut args = vec!["/PID".to_string(), pid.to_string()];
    if tree {
        args.push("/T".to_string());
    }
    if force {
        args.push("/F".to_string());
    }
    Ok(request("taskkill", args, TAG_SIGNAL))
}

/// A request to run `program`, from wherever the app is.
fn request(program: &str, args: Vec<String>, tag: &'static str) -> CommandRequest {
    CommandRequest {
        args,
        cwd: PathBuf::from("."),
        program: program.to_string(),
        stdin: None,
        tag,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_tree_kill_signals_the_leaves_before_the_root() {
        let command = signal_request(Signal::Terminate, 1, &[100, 11, 10], true, false).unwrap();
        assert_eq!(command.program, "kill");
        // Signalling the root first would let it respawn what was just killed.
        assert_eq!(command.args, ["-TERM", "100", "11", "10", "1"]);
    }

    #[test]
    fn test_a_single_kill_leaves_the_children_alone() {
        let command = signal_request(Signal::Kill, 10, &[100], false, false).unwrap();
        assert_eq!(command.args, ["-KILL", "10"]);
    }

    #[test]
    fn test_windows_gentle_kill_has_no_force_flag_and_hard_kill_has() {
        let gentle = signal_request(Signal::Terminate, 8, &[], true, true).unwrap();
        assert_eq!(gentle.program, "taskkill");
        assert_eq!(gentle.args, ["/PID", "8", "/T"]);
        let hard = signal_request(Signal::Kill, 8, &[], false, true).unwrap();
        assert_eq!(hard.args, ["/PID", "8", "/F"]);
    }

    #[test]
    fn test_windows_cannot_suspend() {
        assert!(signal_request(Signal::Stop, 8, &[], false, true).is_err());
        assert!(signal_request(Signal::Continue, 8, &[], false, true).is_err());
    }

    #[test]
    fn test_a_nice_value_goes_first_and_alone() {
        let command = renice_request(42, " +5 ", false).unwrap();
        assert_eq!(command.program, "renice");
        assert_eq!(command.args, ["5", "-p", "42"]);
        let negative = renice_request(42, "-10", false).unwrap();
        assert_eq!(negative.args, ["-10", "-p", "42"]);
    }

    #[test]
    fn test_a_nice_value_outside_the_range_or_not_a_number_is_refused() {
        assert!(renice_request(1, "20", false).is_err());
        assert!(renice_request(1, "-21", false).is_err());
        assert!(renice_request(1, "5; rm -rf /", false).is_err());
        assert!(renice_request(1, "", false).is_err());
    }

    #[test]
    fn test_a_windows_priority_is_matched_against_the_known_classes() {
        let command = renice_request(42, "belownormal", true).unwrap();
        assert_eq!(command.program, "powershell.exe");
        // The spelling that reaches the script is the class's own, never what
        // was typed.
        assert!(command.args[3].contains("= 'BelowNormal'"));
        assert!(renice_request(42, "Normal'; calc; '", true).is_err());
    }
}
