//! `Winter` — a web-native terminal.
//!
//! **Window mode** (no args): opens a GPU-accelerated terminal window (winit +
//! wgpu + cosmic-text) with an interactive shell.
//!
//! **Headless demo** (with args): runs the given command under a PTY and prints
//! the live cell grid and parsed block list.

#![forbid(unsafe_code)]
// No-op on non-Windows targets. On Windows it links the GUI subsystem instead
// of the default console subsystem, so launching `winter.exe` without an
// inherited console (e.g. via ShellExecute, as RightKeys does) no longer
// spawns a visible conhost.exe window alongside the actual terminal window.
#![windows_subsystem = "windows"]

use std::env;
use std::process::ExitCode;

use portable_pty::CommandBuilder;
use winter_core::run_to_completion;
use winter_render::Screen;

// ============================================================================
// Constants
// ============================================================================

/// Name printed by `--help` and `--version`. The crate is `winter` and the
/// distro package is `winter-term`; the user-facing name is neither.
const PROGRAM_NAME: &str = "winter";

// ============================================================================
// Entry point
// ============================================================================

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.is_empty() {
        return run_window();
    }
    // `--help`/`--version` are matched before the headless fallthrough, which
    // would otherwise hand them to a PTY and try to execute `--version` as a
    // command. Packagers and `command -v`-style probes expect both.
    match args[0].as_str() {
        "-h" | "--help" | "help" => {
            println!("{}", help_text());
            ExitCode::SUCCESS
        }
        "-V" | "--version" | "version" => {
            println!("{} {}", PROGRAM_NAME, env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        "mux" => run_mux(&args[1..]),
        "--reload" => run_reload(),
        _ => run_headless(&args),
    }
}

/// Usage text for `winter --help`.
///
/// Written out rather than derived from an argument parser: the CLI is four
/// verbs wide and pulling in a parser crate to print one page would be the
/// larger cost.
fn help_text() -> String {
    format!(
        "\
{PROGRAM_NAME} {version}: a web-native terminal

USAGE:
    winter                        Open a terminal window
    winter <command> [args...]    Run a command headless and dump the
                                  parsed grid and block list
    winter mux <subcommand>       Manage multiplexer sessions
    winter --reload               Reload config in a running instance

OPTIONS:
    -h, --help                    Print this help
    -V, --version                 Print the version

MUX SUBCOMMANDS:
    serve                         Run the session server
    new <name> [-w dir] [-c cmd]  Create a session
    attach [name] [--host H]      Attach to a local or remote session
    list                          List sessions with geometry and uptime
    kill <name>                   Terminate a session
    proxy <name>                  Bridge stdio to the socket (used over ssh)

ENVIRONMENT:
    WINTER_CWD                    Directory for the initial pane
    WINTER_SIDECHANNEL_DIR        Directory TBP file-referenced payloads
                                  are read from

Config lives in ~/.config/winter-term/ (settings.kdl, keybindings.kdl).
Docs: {repository}",
        version = env!("CARGO_PKG_VERSION"),
        repository = env!("CARGO_PKG_REPOSITORY"),
    )
}

// ============================================================================
// Window mode
// ============================================================================

fn run_window() -> ExitCode {
    use winit::event_loop::EventLoop;

    let event_loop = match EventLoop::new() {
        Ok(el) => el,
        Err(error) => {
            eprintln!("winter: {error}");
            return ExitCode::FAILURE;
        }
    };

    let mut app = winter_app::app::App::new();
    match event_loop.run_app(&mut app) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("winter: {error}");
            ExitCode::FAILURE
        }
    }
}

// ============================================================================
// Reload a running instance
// ============================================================================

// ============================================================================
// Mux session creation
// ============================================================================

/// `winter mux new <name> [-w <dir>] [-c <command>]`: create a session on
/// the server running `command` (or the default shell) in `dir`. The
/// command runs through the user's shell, so it can carry arguments,
/// pipes, and environment expansions.
fn run_mux_new(args: &[String]) -> ExitCode {
    let mut name: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut command: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-w" => match args.get(index + 1) {
                Some(dir) => {
                    cwd = Some(dir.clone());
                    index += 2;
                }
                None => {
                    eprintln!("winter mux new: -w requires a directory");
                    return ExitCode::FAILURE;
                }
            },
            "-c" => match args.get(index + 1) {
                Some(cmd) => {
                    command = Some(cmd.clone());
                    index += 2;
                }
                None => {
                    eprintln!("winter mux new: -c requires a command");
                    return ExitCode::FAILURE;
                }
            },
            other if name.is_none() => {
                name = Some(other.to_string());
                index += 1;
            }
            other => {
                eprintln!("winter mux new: unexpected argument: {other}");
                return ExitCode::FAILURE;
            }
        }
    }
    let name = name.unwrap_or_else(|| "default".to_string());

    let path = winter_app::mux::server::default_socket_path();
    let mut client = match winter_app::mux::client::MuxClient::connect(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("winter mux: cannot connect to server: {e}");
            eprintln!("winter mux: start one with 'winter mux serve'");
            return ExitCode::FAILURE;
        }
    };
    // The server answers on its own poll cycle; wait for the confirmation
    // with a deadline rather than a single nonblocking read.
    match client.spawn_confirmed(
        &name,
        80,
        24,
        cwd.as_deref(),
        command.as_deref(),
        ATTACH_REPLAY_TIMEOUT,
    ) {
        Ok((cols, rows)) => {
            let command = command.as_deref().unwrap_or("(default shell)").to_string();
            println!("started session '{name}' ({cols}x{rows}): {command}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("winter mux: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Ask an already-running windowed instance (over the control socket) to
/// save its session, relaunch itself, and exit. Used by install scripts that
/// need to replace the running binary out from under an open window.
fn run_reload() -> ExitCode {
    let path = winter_app::control::socket_path();
    match winter_app::control::request_reload(&path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("winter: no running instance to reload: {error}");
            ExitCode::FAILURE
        }
    }
}

// ============================================================================
// Headless demo
// ============================================================================

fn run_headless(args: &[String]) -> ExitCode {
    let mut command = CommandBuilder::new(&args[0]);
    for arg in &args[1..] {
        command.arg(arg);
    }

    let terminal = match run_to_completion(command) {
        Ok(t) => t,
        Err(error) => {
            eprintln!("winter: {error}");
            return ExitCode::FAILURE;
        }
    };

    println!("=== Blocks ===");
    for (index, block) in terminal.scrollback().blocks().iter().enumerate() {
        println!("--- block {index} ---");
        println!("{block:#?}");
    }

    println!("\n=== Screen Grid ===");
    let mut screen = Screen::new(80, 24);
    screen.feed(terminal.scrollback().plain_text().as_bytes());
    print_grid(screen.grid());

    ExitCode::SUCCESS
}

fn print_grid(grid: &winter_render::Grid) {
    for row in 0..grid.rows() {
        let mut line = String::with_capacity(grid.cols());
        for col in 0..grid.cols() {
            let ch = grid.cell(row, col).map(|c| c.ch).unwrap_or(' ');
            line.push(ch);
        }
        println!("{}", line.trim_end());
    }
}

// ============================================================================
// Mux subcommands
// ============================================================================

/// How long an attaching client waits for the confirmation and replayed
/// scrollback before assuming the server has nothing buffered.
const ATTACH_REPLAY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

/// Idle poll step while waiting for the first buffered message.
const MUX_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

fn run_mux(args: &[String]) -> ExitCode {
    if args.is_empty() {
        eprintln!("usage: winter mux <serve|new|attach [--host H]|list|kill|proxy> [args]");
        return ExitCode::FAILURE;
    }

    match args[0].as_str() {
        "serve" => {
            let path = winter_app::mux::server::default_socket_path();
            eprintln!("winter mux: listening on {path}");
            let server = winter_app::mux::server::MuxServer::new(&path);
            match server.run() {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("winter mux: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        "list" => {
            let path = winter_app::mux::server::default_socket_path();
            let mut client = match winter_app::mux::client::MuxClient::connect(&path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("winter mux: cannot connect to server: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let _ = client.list_sessions();
            std::thread::sleep(std::time::Duration::from_millis(100));
            while let Some(msg) = client.recv().unwrap_or(None) {
                if let winter_app::mux::protocol::ServerMessage::SessionList { sessions } = msg {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    for s in &sessions {
                        let uptime = winter_app::mux::protocol::format_uptime(s.created, now);
                        println!(
                            "{} ({}x{}, up {uptime}, {} attached) - {}",
                            s.name, s.cols, s.rows, s.attach_count, s.command
                        );
                    }
                    if sessions.is_empty() {
                        println!("(no sessions)");
                    }
                    return ExitCode::SUCCESS;
                }
            }
            eprintln!("winter mux: no response from server");
            ExitCode::FAILURE
        }
        "new" => run_mux_new(&args[1..]),
        "kill" => {
            let session = args.get(1).map(|s| s.as_str()).unwrap_or("default");
            let path = winter_app::mux::server::default_socket_path();
            let mut client = match winter_app::mux::client::MuxClient::connect(&path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("winter mux: cannot connect: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let _ = client.kill(session);
            println!("killed session: {session}");
            ExitCode::SUCCESS
        }
        "attach" => run_mux_attach_args(&args[1..]),
        "proxy" => {
            let session = args.get(1).map(|s| s.as_str()).unwrap_or("default");
            match winter_app::mux::proxy::run(session) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("winter mux proxy: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!("usage: winter mux <serve|new|attach [--host H]|list|kill|proxy> [args]");
            ExitCode::FAILURE
        }
    }
}

/// `winter mux attach [--host <host>] [session]`: attach to a local
/// session (default), or one on a remote mux server reached over ssh at
/// `--host`.
fn run_mux_attach_args(args: &[String]) -> ExitCode {
    let mut host: Option<String> = None;
    let mut session: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-h" | "--host" => match args.get(index + 1) {
                Some(h) => {
                    host = Some(h.clone());
                    index += 2;
                }
                None => {
                    eprintln!("winter mux attach: --host requires a hostname");
                    return ExitCode::FAILURE;
                }
            },
            other if session.is_none() => {
                session = Some(other.to_string());
                index += 1;
            }
            other => {
                eprintln!("winter mux attach: unexpected argument: {other}");
                return ExitCode::FAILURE;
            }
        }
    }
    let session = session.unwrap_or_else(|| "default".to_string());
    match host {
        Some(host) => run_mux_attach_remote(&host, &session),
        None => run_mux_attach(&session),
    }
}

fn run_mux_attach(session: &str) -> ExitCode {
    let path = winter_app::mux::server::default_socket_path();
    let mut client = match winter_app::mux::client::MuxClient::connect(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("winter mux: cannot connect: {e}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = client.attach(session) {
        eprintln!("winter mux: attach failed: {e}");
        return ExitCode::FAILURE;
    }

    eprintln!("attached to session: {session}");

    use std::io::{self, BufRead};
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    // Print the scrollback the server replays on attach before waiting on
    // stdin, or it stays invisible until the first keystroke. The reply
    // arrives asynchronously, so the drain waits for it rather than
    // returning on the first idle non-blocking read.
    if drain_mux_messages(&mut client, &mut stdout, ATTACH_REPLAY_TIMEOUT) {
        return ExitCode::SUCCESS;
    }

    for line in stdin.lock().lines() {
        match line {
            Ok(line) => {
                let mut bytes = line.into_bytes();
                bytes.push(b'\n');
                let _ = client.send_input(session, &bytes);
                std::thread::sleep(std::time::Duration::from_millis(50));
                if drain_mux_messages(&mut client, &mut stdout, std::time::Duration::ZERO) {
                    return ExitCode::SUCCESS;
                }
            }
            Err(_) => break,
        }
    }

    let _ = client.detach();
    ExitCode::SUCCESS
}

/// Attach to `session` on a mux server reached over ssh at `host`. The
/// remote proxy process attaches implicitly, so there is no separate
/// attach step.
fn run_mux_attach_remote(host: &str, session: &str) -> ExitCode {
    let mut client = match winter_app::mux::remote::RemoteClient::connect(host, Some(session)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("winter mux: could not reach '{host}' over ssh: {e}");
            return ExitCode::FAILURE;
        }
    };

    eprintln!("attached to '{host}:{session}'");

    use std::io::{self, BufRead};
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    if drain_mux_messages_remote(&mut client, &mut stdout, ATTACH_REPLAY_TIMEOUT) {
        return ExitCode::SUCCESS;
    }

    for line in stdin.lock().lines() {
        match line {
            Ok(line) => {
                let mut bytes = line.into_bytes();
                bytes.push(b'\n');
                let _ = client.send_input(session, &bytes);
                std::thread::sleep(std::time::Duration::from_millis(50));
                if drain_mux_messages_remote(&mut client, &mut stdout, std::time::Duration::ZERO) {
                    return ExitCode::SUCCESS;
                }
            }
            Err(_) => break,
        }
    }

    ExitCode::SUCCESS
}

/// Write every buffered server message to stdout — live output and replayed
/// scrollback alike — until the connection goes idle. When nothing has
/// arrived yet, keeps polling up to `first_wait` so an attach's reply is
/// not missed. Returns `true` when the session has ended and the caller
/// should stop.
fn drain_mux_messages(
    client: &mut winter_app::mux::client::MuxClient,
    stdout: &mut std::io::Stdout,
    first_wait: std::time::Duration,
) -> bool {
    use std::io::Write;
    use winter_app::mux::protocol::ServerMessage;

    let deadline = std::time::Instant::now() + first_wait;
    loop {
        let mut seen_any = false;
        while let Some(msg) = client.recv().unwrap_or(None) {
            seen_any = true;
            match msg {
                ServerMessage::Output { bytes, .. } | ServerMessage::Scrollback { bytes, .. } => {
                    let _ = stdout.write_all(&bytes);
                    let _ = stdout.flush();
                }
                ServerMessage::Exit { code, .. } => {
                    eprintln!("session exited (code: {code:?})");
                    return true;
                }
                _ => {}
            }
        }
        if seen_any || std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(MUX_POLL_INTERVAL);
    }
}

/// Write every buffered server message from the ssh bridge to stdout until
/// the connection goes idle or the bridge itself reports EOF (dead
/// tunnel). Returns `true` when the session has ended and the caller
/// should stop.
fn drain_mux_messages_remote(
    client: &mut winter_app::mux::remote::RemoteClient,
    stdout: &mut std::io::Stdout,
    first_wait: std::time::Duration,
) -> bool {
    use std::io::Write;
    use winter_app::mux::protocol::ServerMessage;

    let deadline = std::time::Instant::now() + first_wait;
    loop {
        let mut seen_any = false;
        while let Some(msg) = client.recv().unwrap_or(None) {
            seen_any = true;
            match msg {
                ServerMessage::Output { bytes, .. } | ServerMessage::Scrollback { bytes, .. } => {
                    let _ = stdout.write_all(&bytes);
                    let _ = stdout.flush();
                }
                ServerMessage::Exit { code, .. } => {
                    eprintln!("session exited (code: {code:?})");
                    return true;
                }
                _ => {}
            }
        }
        if client.eof() {
            eprintln!("winter mux: ssh bridge closed");
            return true;
        }
        if seen_any || std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(MUX_POLL_INTERVAL);
    }
}
