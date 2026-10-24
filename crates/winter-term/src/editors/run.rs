//! Running a program for the text it prints: what the monitors' collectors
//! use to ask the system things it only answers through a command.

use std::process::Command;

// ========================================================================
// Constants
// ========================================================================

/// Commands like `ps` print numbers with the locale's decimal separator,
/// which the parsers cannot read; the C locale's is a dot.
const LOCALE: (&str, &str) = ("LC_ALL", "C");

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
