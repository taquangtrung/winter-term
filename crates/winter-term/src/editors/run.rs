//! Running a program for the text it prints: what the monitors' collectors
//! use to ask the system things it only answers through a command.

use std::fmt;
use std::io::{ErrorKind, Read};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

// ========================================================================
// Constants
// ========================================================================

/// Commands like `ps` print numbers with the locale's decimal separator,
/// which the parsers cannot read; the C locale's is a dot.
const LOCALE: (&str, &str) = ("LC_ALL", "C");

// ========================================================================
// State
// ========================================================================

/// The programs given up on that have still not ended: one stuck in the
/// kernel (`df` on a dead network mount, `nvidia-smi` on a hung driver)
/// ignores even a kill, and starting it again on every refresh would pile
/// up stuck processes behind it. Kept by program, so one stuck does not
/// stop another.
static STUCK: Mutex<Vec<String>> = Mutex::new(Vec::new());

// ========================================================================
// Data Structures
// ========================================================================

/// Why a program run for its text gave none back.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunError {
    /// It ran and failed, did not answer in time, or an earlier run of it
    /// has still not ended; said how.
    Failed(String),
    /// It is not installed.
    Missing,
}

// ========================================================================
// RunError
// ========================================================================

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Failed(why) => f.write_str(why),
            RunError::Missing => f.write_str("not installed"),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A program's standard output, or the first line of why it did not run.
pub fn run_text(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .env(LOCALE.0, LOCALE.1)
        .output()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr.lines().next().unwrap_or("failed");
        return Err(format!("{program}: {reason}"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// [`run_text`], given up on after `patience`: the program is killed, and
/// while one given up on has not ended, no other is started.
pub fn run_text_within(program: &str, args: &[&str], patience: Duration) -> Result<String, RunError> {
    if is_stuck(program) {
        return Err(RunError::Failed(format!("{program}: an earlier run has not ended")));
    }
    let mut child = Command::new(program)
        .args(args)
        .env(LOCALE.0, LOCALE.1)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => RunError::Missing,
            _ => RunError::Failed(format!("cannot run {program}: {e}")),
        })?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| RunError::Failed(format!("{program}: no output")))?;
    // Read on a thread of its own so the wait below can give up.
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        let _ = tx.send(bytes);
    });
    let Ok(bytes) = rx.recv_timeout(patience) else {
        let _ = child.kill();
        // Reaped off this thread: a process stuck in the kernel outlives the
        // kill, and waiting for it here would hang whoever asked.
        set_stuck(program, true);
        let name = program.to_string();
        thread::spawn(move || {
            let _ = child.wait();
            set_stuck(&name, false);
        });
        return Err(RunError::Failed(format!("{program}: no answer in time")));
    };
    match child.wait() {
        Ok(status) if status.success() => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        _ => Err(RunError::Failed(format!("{program}: failed"))),
    }
}

/// Whether a run of `program` given up on has still not ended.
fn is_stuck(program: &str) -> bool {
    STUCK.lock().unwrap_or_else(PoisonError::into_inner).iter().any(|stuck| stuck == program)
}

/// Note that a run of `program` given up on has not ended (`stuck`), or
/// that it has.
fn set_stuck(program: &str, stuck: bool) {
    let mut programs = STUCK.lock().unwrap_or_else(PoisonError::into_inner);
    programs.retain(|name| name != program);
    if stuck {
        programs.push(program.to_string());
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn test_a_program_that_never_answers_is_given_up_on() {
        // `df` on a dead network mount never returned, and each refresh
        // started another, until every job slot was taken by one.
        let started = std::time::Instant::now();
        let answer = run_text_within("sleep", &["30"], Duration::from_millis(100));
        assert!(answer.is_err());
        assert!(started.elapsed() < Duration::from_secs(5), "it did not wait the program out");
    }

    #[cfg(unix)]
    #[test]
    fn test_one_program_stuck_does_not_stop_another_and_a_missing_one_says_so() {
        // `nvidia-smi` hung on a driver used to keep `df` from running too,
        // and the monitor needs to know a program is absent to stop asking.
        set_stuck("winter-test-stuck", true);
        assert!(run_text_within("true", &[], Duration::from_secs(5)).is_ok());
        assert!(run_text_within("winter-test-stuck", &[], Duration::from_secs(5)).is_err());
        set_stuck("winter-test-stuck", false);
        assert_eq!(
            run_text_within("winter-no-such-program", &[], Duration::from_secs(5)),
            Err(RunError::Missing)
        );
    }
}
