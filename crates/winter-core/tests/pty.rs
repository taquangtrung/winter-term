//! End-to-end check that the PTY runtime drives the parser over a real child.

use portable_pty::CommandBuilder;
use winter_core::run_to_completion;

#[test]
fn test_run_echo_captures_child_output() {
    let mut command = CommandBuilder::new("echo");
    command.arg("hello-winter");

    let terminal = run_to_completion(command).expect("echo runs under a PTY");
    assert!(
        terminal.scrollback().plain_text().contains("hello-winter"),
        "captured: {:?}",
        terminal.scrollback().plain_text()
    );
}
