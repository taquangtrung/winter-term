//! Starting the commands that reach a remote file: `sudo`, `docker exec`, or
//! `ssh`, each running one script from [`super::scripts`], under a deadline
//! so a host that stops answering cannot hold the window for ever.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::paths;
use crate::process::command;

use super::scripts::EXIT_MEANINGS;
use super::{Method, Remote, RemoteError};

// ========================================================================
// Constants
// ========================================================================

/// How long a listing, a check, or a stat may take.
pub(super) const QUICK: Duration = Duration::from_secs(20);

/// How long reading or writing a whole file may take.
pub(super) const TRANSFER: Duration = Duration::from_secs(120);

/// How often a running command is looked in on while it has time left.
const POLL: Duration = Duration::from_millis(5);

/// How long, once a command has ended, what it printed is waited for. All
/// of it is in the pipe by then; the wait is bounded because a process it
/// left behind (a shared SSH connection, a child of a root shell this user
/// cannot kill) may hold the pipe open long after.
const DRAIN_GRACE: Duration = Duration::from_millis(500);

/// How much of a pipe is read at a time.
const CHUNK: usize = 64 * 1024;

const SUDO: &str = "sudo";
const DOCKER: &str = "docker";
const SSH: &str = "ssh";

/// Asks sudo to fail rather than ask for a password: it could only ask on a
/// terminal, and there is none.
const SUDO_NON_INTERACTIVE: &str = "-n";

/// Has sudo read the password from standard input, with no prompt of its
/// own, and only refresh its timestamp.
const SUDO_VALIDATE: [&str; 4] = ["-S", "-p", "", "-v"];

/// The command sudo runs to show whether it would act without a password.
const SUDO_PROBE: &str = "true";

/// The locale every far side answers in, so its errors read the same
/// everywhere.
const LOCALE: (&str, &str) = ("LC_ALL", "C");

/// How a script is run on the far side: `sh -c SCRIPT sh ARGS...`, the
/// second `sh` being `$0`.
const SHELL: &str = "sh";
const SHELL_SCRIPT: &str = "-c";

/// What SSH is always given: never a prompt (there is nobody to answer it),
/// and not long to wait for a host that is not there.
const SSH_OPTIONS: [&str; 4] = ["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"];

/// One connection kept open per host for ten minutes after its last use,
/// so a listing after the first costs a round trip, not a handshake.
const SSH_SHARING: [&str; 4] = ["-o", "ControlMaster=auto", "-o", "ControlPersist=10m"];

/// Where the shared connections' sockets live, under the private directory.
const SSH_SOCKETS: &str = "ssh";

/// What the far side prints before the script's own output, so whatever a
/// login shell says first (a banner, a message of the day) can be dropped.
const SSH_MARKER: &str = "@winter-output@";

/// Characters a word may hold and still reach a POSIX shell unquoted.
const SHELL_SAFE: &str = "_@%+=:,./-";

/// What SSH says when the host wants something BatchMode will not give.
const SSH_AUTH_FAILURES: [&str; 2] = ["Permission denied", "Host key verification failed"];

/// Said beside an SSH refusal, since BatchMode refuses what a terminal
/// would have asked.
const SSH_AUTH_HINT: &str =
    "Winter connects without prompting: use a key (ssh-agent) and accept the host key first";

// ========================================================================
// Data Structures
// ========================================================================

/// What has come out of one of a command's pipes so far, and word of when
/// it closed.
struct Drain {
    bytes: Arc<Mutex<Vec<u8>>>,
    closed: Receiver<()>,
}

/// How a command that ran to its end ended.
#[derive(Debug)]
struct Finished {
    /// Its exit code, `None` when a signal ended it.
    code: Option<i32>,
    stderr: String,
    stdout: Vec<u8>,
    success: bool,
}

// ========================================================================
// Functions
// ========================================================================

/// Run `script` against `remote` with `args`, feeding it `input`, and give
/// back what it printed.
pub(super) fn script(
    remote: &Remote,
    script: &str,
    args: &[&str],
    input: Option<&[u8]>,
    deadline: Duration,
) -> Result<Vec<u8>, RemoteError> {
    if remote.method == Method::Sudo && !super::sudo_available() {
        return Err(RemoteError::Failed(super::SUDO_UNAVAILABLE.to_string()));
    }
    let mut command = command_for(&remote.method, script, args);
    let finished = run(&mut command, input, deadline)?;
    if finished.success {
        return Ok(match remote.method {
            Method::Ssh(_) => after_marker(finished.stdout),
            Method::Sudo | Method::Docker(_) => finished.stdout,
        });
    }
    if remote.method == Method::Sudo && !sudo_ready() {
        return Err(RemoteError::NeedsPassword);
    }
    Err(RemoteError::Failed(reason(&remote.method, &finished)))
}

/// Whether `sudo` would act now without asking for a password.
pub(super) fn sudo_ready() -> bool {
    let mut command = command(SUDO);
    command.args([SUDO_NON_INTERACTIVE, SUDO_PROBE]);
    run(&mut command, None, QUICK).is_ok_and(|finished| finished.success)
}

/// Hand `password` to `sudo -v`, which checks it and refreshes the
/// timestamp later `sudo -n` calls act on.
pub(super) fn sudo_authenticate(password: &str) -> Result<(), RemoteError> {
    let mut command = command(SUDO);
    command.args(SUDO_VALIDATE);
    let line = format!("{password}\n");
    let finished = run(&mut command, Some(line.as_bytes()), QUICK)?;
    if !finished.success {
        return Err(RemoteError::Failed(reason(&Method::Sudo, &finished)));
    }
    // A sudo keeping no timestamp (`timestamp_timeout=0`) took the password
    // and will ask again at once, which would ask for ever.
    match sudo_ready() {
        true => Ok(()),
        false => Err(RemoteError::Failed(
            "sudo keeps no timestamp here (timestamp_timeout=0), so every call would ask"
                .to_string(),
        )),
    }
}

/// The command that runs `script` with `args` where `method` reaches.
fn command_for(method: &Method, script: &str, args: &[&str]) -> Command {
    match method {
        Method::Sudo => {
            let mut command = command(SUDO);
            command
                .env(LOCALE.0, LOCALE.1)
                .args([
                    SUDO_NON_INTERACTIVE,
                    "--",
                    SHELL,
                    SHELL_SCRIPT,
                    script,
                    SHELL,
                ])
                .args(args);
            command
        }
        Method::Docker(container) => {
            let mut command = command(DOCKER);
            command.args(["exec", "-i", "-e", &format!("{}={}", LOCALE.0, LOCALE.1)]);
            if let Some(user) = &container.user {
                command.args(["-u", user]);
            }
            command
                .arg(&container.name)
                .args([SHELL, SHELL_SCRIPT, script, SHELL])
                .args(args);
            command
        }
        Method::Ssh(host) => {
            let mut command = command(SSH);
            command.args(SSH_OPTIONS);
            if let Some(sockets) = sockets_dir() {
                let mut path = OsString::from("ControlPath=");
                path.push(sockets.join("%C"));
                command.args(SSH_SHARING).arg("-o").arg(path);
            }
            if let Some(port) = host.port {
                command.arg("-p").arg(port.to_string());
            }
            if let Some(user) = &host.user {
                command.args(["-l", user]);
            }
            command
                .args(["-T", "--", &host.host])
                .arg(remote_command(script, args));
            command
        }
    }
}

/// The one line an SSH host's login shell is given: the marker, then the
/// script under `sh` in the C locale, every word quoted for that shell.
fn remote_command(script: &str, args: &[&str]) -> String {
    let words: Vec<String> = ["exec", "env", &format!("{}={}", LOCALE.0, LOCALE.1)]
        .into_iter()
        .chain([SHELL, SHELL_SCRIPT, script, SHELL])
        .chain(args.iter().copied())
        .map(sh_quote)
        .collect();
    format!("printf '%s\\n' {SSH_MARKER}; {}", words.join(" "))
}

/// `word` as a POSIX shell reads it back unchanged: as it is where nothing
/// in it is special, else single-quoted with its own quotes spelled `'\''`.
fn sh_quote(word: &str) -> String {
    let safe = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || SHELL_SAFE.contains(c));
    match safe {
        true => word.to_string(),
        false => format!("'{}'", word.replace('\'', "'\\''")),
    }
}

/// The directory for SSH's shared-connection sockets, private to this user.
/// `None` where there is none, or the platform's SSH cannot share.
fn sockets_dir() -> Option<PathBuf> {
    match cfg!(unix) {
        true => paths::private_subdir(SSH_SOCKETS),
        false => None,
    }
}

/// What follows the marker line in `stdout`, or all of it where the marker
/// never came.
fn after_marker(stdout: Vec<u8>) -> Vec<u8> {
    let marker = format!("{SSH_MARKER}\n");
    match stdout
        .windows(marker.len())
        .position(|window| window == marker.as_bytes())
    {
        Some(at) => stdout[at + marker.len()..].to_vec(),
        None => stdout,
    }
}

/// Why `finished` failed, in a line: the scripts' own exit codes first, then
/// the last thing the command said.
fn reason(method: &Method, finished: &Finished) -> String {
    let said = finished
        .stderr
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .map(str::trim);
    let meaning = EXIT_MEANINGS
        .iter()
        .find(|(code, _)| Some(*code) == finished.code)
        .map(|(_, meaning)| *meaning);
    let reason = match (meaning, said) {
        (Some(meaning), _) => meaning.to_string(),
        (None, Some(said)) => said.to_string(),
        (None, None) => format!("failed with {:?}", finished.code),
    };
    let refused = SSH_AUTH_FAILURES
        .iter()
        .any(|failure| reason.contains(failure));
    match (method, refused) {
        (Method::Ssh(_), true) => format!("{}. {SSH_AUTH_HINT}", reason.trim_end_matches('.')),
        _ => reason,
    }
}

/// Run `command` to its end or `deadline`, whichever is first, feeding it
/// `input` and collecting what it prints.
fn run(
    command: &mut Command,
    input: Option<&[u8]>,
    deadline: Duration,
) -> Result<Finished, RemoteError> {
    let program = command.get_program().to_string_lossy().to_string();
    let mut child = command
        .stdin(match input {
            Some(_) => Stdio::piped(),
            None => Stdio::null(),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| RemoteError::Failed(format!("could not run {program}: {e}")))?;
    // Fed and drained on threads of their own, so a command that prints a
    // lot before it has read all it is given cannot deadlock against us.
    let feeder = input.map(|bytes| {
        let bytes = bytes.to_vec();
        let stdin = child.stdin.take();
        thread::spawn(move || {
            if let Some(mut stdin) = stdin {
                // One that stops reading early has said why on stderr.
                let _ = stdin.write_all(&bytes);
            }
        })
    });
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    // The feeder and the drains are left to end on their own: waiting on
    // them would wait on whatever still holds the pipes.
    drop(feeder);
    let Some(code) = wait_until(&mut child, Instant::now() + deadline) else {
        return Err(RemoteError::Failed(format!(
            "{program} did not answer within {}s",
            deadline.as_secs()
        )));
    };
    let stdout = stdout.take();
    let stderr = stderr.take();
    Ok(Finished {
        code,
        stderr: String::from_utf8_lossy(&stderr).to_string(),
        stdout,
        success: code == Some(0),
    })
}

/// Read all of `pipe` on a thread of its own.
fn drain(pipe: Option<impl Read + Send + 'static>) -> Drain {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let (done, closed) = mpsc::channel();
    let into = Arc::clone(&bytes);
    thread::spawn(move || {
        let mut chunk = vec![0; CHUNK];
        if let Some(mut pipe) = pipe {
            while let Ok(read @ 1..) = pipe.read(&mut chunk) {
                if let Ok(mut bytes) = into.lock() {
                    bytes.extend_from_slice(&chunk[..read]);
                }
            }
        }
        let _ = done.send(());
    });
    Drain { bytes, closed }
}

/// The exit code `child` ended with, `Some(None)` for one killed by a
/// signal, or `None` when it was still running at `until` and was killed.
fn wait_until(child: &mut Child, until: Instant) -> Option<Option<i32>> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.code()),
            Ok(None) if Instant::now() < until => thread::sleep(POLL),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

// ========================================================================
// Drain
// ========================================================================

impl Drain {
    /// What the pipe held, once it closes or [`DRAIN_GRACE`] passes.
    fn take(self) -> Vec<u8> {
        let _ = self.closed.recv_timeout(DRAIN_GRACE);
        self.bytes
            .lock()
            .map(|mut bytes| std::mem::take(&mut *bytes))
            .unwrap_or_default()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_word_is_quoted_only_where_a_shell_would_read_it_otherwise() {
        assert_eq!(sh_quote("/etc/hosts"), "/etc/hosts");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
        assert_eq!(sh_quote(""), "''");
        assert_eq!(sh_quote("$(rm -rf ~)"), "'$(rm -rf ~)'");
    }

    // Run by a local `sh`, which Windows does not have.
    #[cfg(unix)]
    #[test]
    fn test_a_quoted_command_reaches_sh_as_the_words_it_was_made_of() {
        // What an SSH host's shell does with the line, done by a local one.
        let path = "/tmp/it's a $file";
        let line = remote_command("printf '%s|' \"$0\" \"$1\"", &[path]);
        let output = Command::new(SHELL)
            .args([SHELL_SCRIPT, &line])
            .output()
            .expect("sh runs");
        let printed = String::from_utf8_lossy(&output.stdout);
        assert_eq!(printed, format!("{SSH_MARKER}\nsh|{path}|"));
    }

    #[test]
    fn test_whatever_a_login_shell_prints_first_is_dropped() {
        let stdout = format!("Welcome!\n{SSH_MARKER}\nthe output").into_bytes();
        assert_eq!(after_marker(stdout), b"the output");
        assert_eq!(after_marker(b"no marker".to_vec()), b"no marker");
    }

    // Run by a local `sh`, which Windows does not have.
    #[cfg(unix)]
    #[test]
    fn test_a_command_past_its_deadline_is_killed_and_said_so() {
        let mut command = Command::new(SHELL);
        command.args([SHELL_SCRIPT, "sleep 5"]);
        let started = Instant::now();
        let outcome = run(&mut command, None, Duration::from_millis(50));
        assert!(
            matches!(outcome, Err(RemoteError::Failed(_))),
            "{outcome:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "it did not wait the sleep out"
        );
    }

    // Run by a local `sh`, which Windows does not have.
    #[cfg(unix)]
    #[test]
    fn test_a_pipe_held_open_after_the_command_ends_does_not_hold_the_answer() {
        // The shape of a shared SSH connection left running in the background.
        let mut command = Command::new(SHELL);
        command.args([SHELL_SCRIPT, "echo done; sleep 5 & exit 0"]);
        let started = Instant::now();
        let finished = run(&mut command, None, QUICK).expect("runs");
        assert_eq!(finished.stdout, b"done\n");
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "it did not wait for the sleep"
        );
    }

    // Run by a local `sh`, which Windows does not have.
    #[cfg(unix)]
    #[test]
    fn test_the_write_script_replaces_the_contents_and_keeps_the_file() {
        // The script itself, run here by a local `sh` as a far side would.
        let dir = std::env::temp_dir().join(format!("winter-remote-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("f.txt");
        std::fs::write(&file, "old contents\n").expect("seed");
        let path = file.to_string_lossy().to_string();
        let new = b"new\n";
        let run_write = |len: &str| {
            let mut command = Command::new(SHELL);
            command.args([
                SHELL_SCRIPT,
                super::super::scripts::WRITE,
                SHELL,
                &path,
                len,
            ]);
            run(&mut command, Some(new), QUICK).expect("runs")
        };

        let short = run_write("99");
        assert_eq!(
            short.code,
            Some(8),
            "a length that does not match is refused"
        );
        assert_eq!(
            std::fs::read_to_string(&file).expect("reads"),
            "old contents\n"
        );

        assert!(run_write(&new.len().to_string()).success);
        assert_eq!(std::fs::read(&file).expect("reads"), new);
        let left: Vec<_> = std::fs::read_dir(&dir).expect("lists").collect();
        assert_eq!(left.len(), 1, "no temporary file is left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Run by a local `sh`, which Windows does not have.
    #[cfg(unix)]
    #[test]
    fn test_the_list_script_reads_a_directory_through_a_local_sh() {
        let dir = std::env::temp_dir().join(format!("winter-remote-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub dir")).expect("dir");
        std::fs::write(dir.join(".hidden"), "12345").expect("file");
        let mut command = Command::new(SHELL);
        let path = dir.to_string_lossy().to_string();
        command.args([SHELL_SCRIPT, super::super::scripts::LIST, SHELL, &path]);
        let finished = run(&mut command, None, QUICK).expect("runs");
        let mut entries =
            super::super::scripts::parse_listing(&String::from_utf8_lossy(&finished.stdout));
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let named: Vec<(&str, bool)> = entries
            .iter()
            .map(|e| (e.name.as_str(), e.is_dir))
            .collect();
        assert_eq!(named, [(".hidden", false), ("sub dir", true)]);
        assert_eq!(entries[0].size, Some(5));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
