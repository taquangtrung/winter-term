//! Drives the shipped shell-integration scripts through a real PTY and checks
//! that the marks they emit are the ones the block parser consumes.
//!
//! The scripts and the parser are two halves of one contract with nothing in
//! the type system tying them together: a mark renamed on either side leaves
//! blocks silently not forming, which is what the whole feature is. So this
//! runs the real shell, sources the real script, and asserts on the parsed
//! block list rather than on the bytes.

#![cfg(unix)]

use std::io::Read;
use std::process::Command;

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use winter_core::Terminal;

// ========================================================================
// Constants
// ========================================================================

/// Marker command whose output identifies the block under test.
const PROBE_COMMAND: &str = "echo winter-probe-output";

/// Bounded so a shell that never reaches a prompt fails the test rather than
/// hanging the suite.
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Pause between submitted lines, letting the shell finish drawing its prompt
/// and emitting its marks before the next line's echo arrives.
const SHELL_SETTLE: std::time::Duration = std::time::Duration::from_millis(700);

// ========================================================================
// Harness
// ========================================================================

/// Whether `shell` is installed, so the test skips rather than fails on a
/// machine (or CI image) that does not ship it.
fn shell_available(shell: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {shell}"))
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Run `shell` interactively under a PTY with the integration sourced, submit
/// a probe command, and return the parsed scrollback.
fn run_probe(shell: &str, source_line: &str) -> Terminal {
    let pty = NativePtySystem::default()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open pty");

    let mut cmd = CommandBuilder::new(shell);
    // Start without user config. This test is about the shipped script, so it
    // must not pass or fail based on whose dotfiles are on the machine; a
    // developer's prompt framework would otherwise make it non-reproducible
    // between a laptop and CI.
    let no_rc: &[&str] = match shell {
        "bash" => &["--norc", "--noprofile", "-i"],
        "zsh" => &["-f", "-i"],
        "fish" => &["--no-config", "-i"],
        other => panic!("no no-config flags known for {other}"),
    };
    cmd.args(no_rc);
    // WINTER is what the scripts gate on, exactly as the real app sets it.
    cmd.env("WINTER", "1");
    cmd.env("PS1", "probe$ ");
    cmd.env("TERM", "xterm-256color");
    let mut child = pty.slave.spawn_command(cmd).expect("spawn shell");

    let mut writer = pty.master.take_writer().expect("pty writer");
    let mut reader = pty.master.try_clone_reader().expect("pty reader");
    drop(pty.slave);

    let lines = [
        source_line.to_string(),
        PROBE_COMMAND.to_string(),
        "exit".to_string(),
    ];
    std::thread::spawn(move || {
        use std::io::Write;
        // One line at a time, with a beat between. The tty echoes input the
        // moment it is written, so writing the whole script at once interleaves
        // the echo of later lines with the marks for earlier ones and the
        // phases come out scrambled. Real typing is paced; this imitates that.
        std::thread::sleep(SHELL_SETTLE);
        for line in lines {
            let _ = writer.write_all(format!("{line}\n").as_bytes());
            let _ = writer.flush();
            std::thread::sleep(SHELL_SETTLE);
        }
    });

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        while let Ok(n) = reader.read(&mut chunk) {
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        let _ = tx.send(buf);
    });

    let bytes = rx
        .recv_timeout(READ_TIMEOUT)
        .expect("shell produced output");
    let _ = child.wait();

    let mut term = Terminal::new();
    term.feed(&bytes);
    term
}

/// Assert that the probe produced a real command block: its own output, its
/// command line, and a zero exit code.
fn assert_blocks_formed(shell: &str, term: &Terminal) {
    let blocks = term.scrollback().blocks();
    assert!(
        blocks.len() > 1,
        "{shell}: no block boundaries were marked, the session stayed one rolling block"
    );

    let probe = blocks
        .iter()
        .find(|b| b.command.contains("winter-probe-output"))
        .unwrap_or_else(|| {
            panic!(
                "{shell}: no block captured the probe command; commands seen: {:?}",
                blocks.iter().map(|b| &b.command).collect::<Vec<_>>()
            )
        });

    assert!(
        probe.plain_text().contains("winter-probe-output"),
        "{shell}: the probe's output did not land in its own block. block = {probe:#?}"
    );
    assert_eq!(
        probe.exit_code,
        Some(0),
        "{shell}: OSC 133;D did not record the exit code"
    );
    assert!(
        probe.cwd.is_some(),
        "{shell}: OSC 7 did not report a working directory"
    );
}

// ========================================================================
// Tests
// ========================================================================

#[test]
fn test_bash_integration_marks_command_blocks() {
    if !shell_available("bash") {
        eprintln!("skipping: bash not installed");
        return;
    }
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../clients/shell-integration/winter.bash"
    );
    let term = run_probe("bash", &format!(". {script}"));
    assert_blocks_formed("bash", &term);
}

#[test]
fn test_zsh_integration_marks_command_blocks() {
    if !shell_available("zsh") {
        eprintln!("skipping: zsh not installed");
        return;
    }
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../clients/shell-integration/winter.zsh"
    );
    let term = run_probe("zsh", &format!(". {script}"));
    assert_blocks_formed("zsh", &term);
}

#[test]
fn test_fish_integration_marks_command_blocks() {
    if !shell_available("fish") {
        eprintln!("skipping: fish not installed");
        return;
    }
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../clients/shell-integration/winter.fish"
    );
    let term = run_probe("fish", &format!("source {script}"));
    assert_blocks_formed("fish", &term);
}
