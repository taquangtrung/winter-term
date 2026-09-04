//! Interactive terminal pane: a cell grid fed by a [`CombinedPerformer`]
//! (unified `vte` performer that drives both the visual grid and the block
//! parser), with bytes arriving either from a locally spawned PTY child
//! or from a named session on an external mux server.

mod performer;
mod shell;

use performer::{ApcDecision, CombinedPerformer};
pub(crate) use shell::osc52_read_response;
use shell::{parse_foreground_process, resolve_shell};

use std::io::Write;
use std::sync::mpsc;
use std::thread;

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

use winter_core::Scrollback;
use winter_render::Grid;
#[cfg(test)]
use winter_render::MAX_SCROLLBACK;

use super::block_queue::BlockQueue;
use crate::mux::protocol::ServerMessage;
use crate::mux::resilience::ResilientClient;

// ========================================================================
// Constants
// ========================================================================

const READ_CHUNK: usize = 4096;
const BELL: u8 = 0x07;
const LINE_FEED: u8 = b'\n';
const CARRIAGE_RETURN: u8 = b'\r';
const BACKSPACE: u8 = 0x08;
const HORIZONTAL_TAB: u8 = b'\t';
/// Final byte of the RIS escape sequence (`ESC c`, "Reset to Initial State").
/// `reset` sends it; a full reset must restore legacy xterm keyboard encoding.
const RIS: u8 = b'c';

/// Default grid rows reserved for a content block whose displayed height is not
/// known at emit time (markdown, SVG, HTML). Reserved in-sequence (at the
/// escape) so the shell's subsequent output flows below the block instead of
/// under it, without desyncing the shell's cursor.
pub(crate) const BLOCK_RESERVE_ROWS: usize = 12;

/// Prefix marking a pane label as a mux session name rather than a shell
/// or command path; session restore re-attaches these panes instead of
/// spawning a local shell.
pub(crate) const MUX_COMMAND_PREFIX: &str = "mux:";

/// Prefix marking a pane label as `host|session` for a session attached
/// over SSH; session restore re-attaches these to the remote host instead
/// of the local mux server.
pub(crate) const MUX_REMOTE_COMMAND_PREFIX: &str = "mux-remote:";

/// Upper bound on rows an image block may reserve, so a tall image cannot eat
/// the whole screen. Raster images reserve exactly the rows they occupy, capped
/// here; the app scales them to fit the same cap.
pub(crate) const MAX_IMAGE_ROWS: usize = 24;

// ========================================================================
// Data Structures
// ========================================================================

// ========================================================================
// CombinedPerformer
// ========================================================================

// ========================================================================
// Pane
// ========================================================================

/// One interactive pane: a terminal cell grid whose bytes arrive from a
/// local PTY or a mux session. Local PTY reads happen on a background
/// thread; the main thread drains pending output via [`Pane::drain_output`].
pub struct Pane {
    block_queue: BlockQueue,
    /// The shell or command path used to spawn this pane, or the mux
    /// session label for a pane attached to a local or SSH-remote session.
    command: String,
    combined: CombinedPerformer,
    parser: vte::Parser,
    transport: PaneTransport,
}

/// Where a pane's bytes come from and go to: a locally spawned PTY
/// child, or a named session owned by an external mux server.
enum PaneTransport {
    Local {
        child: Box<dyn portable_pty::Child + Send>,
        master: Box<dyn portable_pty::MasterPty + Send>,
        rx: mpsc::Receiver<Vec<u8>>,
        writer: Box<dyn Write + Send>,
        _read_thread: Option<thread::JoinHandle<()>>,
    },
    Mux {
        client: ResilientClient,
        /// Set once the server reports the session's process exited.
        exited: bool,
        session: String,
    },
}

impl Pane {
    /// Spawn the default shell under a PTY with the given grid dimensions.
    pub fn new(
        cols: usize,
        rows: usize,
        configured_shell: Option<&str>,
        max_scrollback: usize,
    ) -> anyhow::Result<Self> {
        Self::new_with_cwd(cols, rows, configured_shell, max_scrollback, None)
    }

    /// Spawn the default shell under a PTY, starting the child in `cwd` when
    /// given. Used so a split/new tab opens in the same working directory as the
    /// pane it was spawned from instead of the process default (usually `$HOME`).
    pub fn new_with_cwd(
        cols: usize,
        rows: usize,
        configured_shell: Option<&str>,
        max_scrollback: usize,
        cwd: Option<&str>,
    ) -> anyhow::Result<Self> {
        let shell = configured_shell
            .map(|s| s.to_string())
            .or_else(|| std::env::var("WINTER_SHELL").ok())
            .or_else(|| {
                #[cfg(target_os = "windows")]
                {
                    std::env::var("COMSPEC").ok()
                }
                #[cfg(not(target_os = "windows"))]
                {
                    std::env::var("SHELL").ok()
                }
            })
            .unwrap_or_else(|| {
                #[cfg(target_os = "windows")]
                {
                    "powershell.exe".to_string()
                }
                #[cfg(target_os = "macos")]
                {
                    "/bin/zsh".to_string()
                }
                #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
                {
                    "/bin/bash".to_string()
                }
            });

        let mut command = CommandBuilder::new(resolve_shell(&shell));
        if let Some(dir) = cwd {
            command.cwd(dir);
        }
        Self::with_command(cols, rows, command, max_scrollback)
    }

    /// Spawn `command` under a PTY with the given grid dimensions.
    pub fn with_command(
        cols: usize,
        rows: usize,
        command: CommandBuilder,
        max_scrollback: usize,
    ) -> anyhow::Result<Self> {
        let command_str = command
            .get_argv()
            .first()
            .and_then(|a| a.to_str())
            .unwrap_or("sh")
            .to_string();
        Self::with_command_labeled(cols, rows, command, max_scrollback, command_str)
    }

    fn with_command_labeled(
        cols: usize,
        rows: usize,
        mut command: CommandBuilder,
        max_scrollback: usize,
        command_str: String,
    ) -> anyhow::Result<Self> {
        // Advertise Winter to the child so capability-detecting tools emit rich
        // blocks instead of the plain-text fallback.
        command.env("TERM_PROGRAM", "winter");
        command.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
        command.env("WINTER", "1");
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");

        let pty_system = NativePtySystem::default();
        let pair = pty_system.openpty(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let child = pair.slave.spawn_command(command)?;
        drop(pair.slave);

        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        let (tx, rx) = mpsc::channel::<Vec<u8>>();

        let read_thread = thread::Builder::new()
            .name("winter pty read".into())
            .spawn(move || {
                let mut buf = [0u8; READ_CHUNK];
                let mut reader = reader;
                loop {
                    match std::io::Read::read(&mut reader, &mut buf) {
                        Ok(0) => break,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                        Ok(count) => {
                            if tx.send(buf[..count].to_vec()).is_err() {
                                break;
                            }
                        }
                    }
                }
            })?;

        Ok(Self {
            block_queue: BlockQueue::new(),
            command: command_str,
            combined: CombinedPerformer::new(cols, rows, max_scrollback),
            parser: vte::Parser::new(),
            transport: PaneTransport::Local {
                child,
                master: pair.master,
                rx,
                writer,
                _read_thread: Some(read_thread),
            },
        })
    }

    /// Attach a new pane to a named session on the default mux server.
    pub fn new_mux(
        cols: usize,
        rows: usize,
        session: &str,
        max_scrollback: usize,
    ) -> anyhow::Result<Self> {
        Self::new_mux_at(
            &crate::mux::server::default_socket_path(),
            cols,
            rows,
            session,
            max_scrollback,
        )
    }

    /// Attach a new pane to a named session on the mux server at `path`.
    /// The session's buffered output replays into the grid as it arrives.
    pub fn new_mux_at(
        path: &str,
        cols: usize,
        rows: usize,
        session: &str,
        max_scrollback: usize,
    ) -> anyhow::Result<Self> {
        let client = ResilientClient::new(path, session);
        if !client.is_connected() {
            anyhow::bail!("could not connect to the mux server at {path}");
        }
        let mut pane = Self {
            block_queue: BlockQueue::new(),
            command: format!("{MUX_COMMAND_PREFIX}{session}"),
            combined: CombinedPerformer::new(cols.max(1), rows.max(1), max_scrollback),
            parser: vte::Parser::new(),
            transport: PaneTransport::Mux {
                client,
                exited: false,
                session: session.to_string(),
            },
        };
        // The server sizes a fresh session at its default geometry; tell
        // it this pane's real size so the remote PTY matches the grid.
        pane.transport_resize(cols.max(1), rows.max(1));
        Ok(pane)
    }

    /// Attach a new pane to a named session on a mux server reached over
    /// SSH at `host`. The session's buffered output replays into the grid
    /// as it arrives.
    pub fn new_mux_remote(
        host: &str,
        cols: usize,
        rows: usize,
        session: &str,
        max_scrollback: usize,
    ) -> anyhow::Result<Self> {
        let client = ResilientClient::new_remote(host, session);
        if !client.is_connected() {
            anyhow::bail!("could not reach '{host}' over ssh");
        }
        let mut pane = Self {
            block_queue: BlockQueue::new(),
            command: format!("{MUX_REMOTE_COMMAND_PREFIX}{host}|{session}"),
            combined: CombinedPerformer::new(cols.max(1), rows.max(1), max_scrollback),
            parser: vte::Parser::new(),
            transport: PaneTransport::Mux {
                client,
                exited: false,
                session: session.to_string(),
            },
        };
        pane.transport_resize(cols.max(1), rows.max(1));
        Ok(pane)
    }

    /// Drain all pending PTY output into the cell grid and block parser.
    /// Returns `true` if any output was processed.
    pub fn drain_output(&mut self) -> bool {
        let mut chunks = Vec::new();
        // Session geometry reported by the mux server (attach confirmation
        // or arbitration): applied after the transport borrow ends.
        let mut session_geometry: Option<(usize, usize)> = None;
        match &mut self.transport {
            PaneTransport::Local { rx, .. } => {
                while let Ok(chunk) = rx.try_recv() {
                    chunks.push(chunk);
                }
            }
            PaneTransport::Mux { client, exited, .. } => {
                while let Some(msg) = client.recv() {
                    match msg {
                        ServerMessage::Output { bytes, .. }
                        | ServerMessage::Scrollback { bytes, .. } => chunks.push(bytes),
                        ServerMessage::Exit { .. } => *exited = true,
                        // The server owns the session's real geometry — the
                        // smallest among attached clients. Snap the grid to
                        // it (letterboxing within the layout) so a stream
                        // wrapped for a smaller session doesn't double-wrap.
                        ServerMessage::Attached { cols, rows, .. }
                        | ServerMessage::Resized { cols, rows, .. } => {
                            session_geometry = Some((cols as usize, rows as usize));
                        }
                        _ => {}
                    }
                }
            }
        }
        let got_any = !chunks.is_empty() || session_geometry.is_some();
        for chunk in &chunks {
            for &byte in chunk {
                match self.combined.apc_filter(byte) {
                    ApcDecision::Drop => {}
                    ApcDecision::Pass => {
                        self.parser.advance(&mut self.combined, byte);
                    }
                    ApcDecision::ReplayEscThenByte(b) => {
                        self.parser.advance(&mut self.combined, b'\x1b');
                        self.parser.advance(&mut self.combined, b);
                    }
                }
            }
        }
        if got_any {
            let (row, _) = self.combined.grid().cursor();
            let anchors = self.combined.take_block_anchors();
            self.block_queue
                .update(self.combined.scrollback(), row, &anchors);
        }
        if let Some((cols, rows)) = session_geometry {
            self.apply_grid_resize(cols, rows);
        }
        let responses = self.combined.take_pending_responses();
        if !responses.is_empty() {
            self.write(&responses);
        }
        got_any
    }

    /// Current Kitty keyboard protocol flags active in this pane (0 = legacy).
    pub fn kitty_flags(&self) -> u32 {
        self.combined.kitty_flags()
    }

    /// xterm modifyOtherKeys mode: `None` = disabled, `Some(1)` or `Some(2)`.
    pub fn modify_other_keys(&self) -> Option<i64> {
        self.combined.modify_other_keys()
    }

    /// Write bytes to the PTY (keyboard input).
    pub fn write(&mut self, bytes: &[u8]) {
        match &mut self.transport {
            PaneTransport::Local { writer, .. } => {
                let _ = writer.write_all(bytes);
                let _ = writer.flush();
            }
            PaneTransport::Mux { client, .. } => {
                let _ = client.send_input(bytes);
            }
        }
    }

    /// Resize the PTY and the cell grid. Signals the child process via
    /// `SIGWINCH` so the shell knows about the new dimensions.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        // Skip everything when the size is unchanged: a redundant SIGWINCH at
        // the same size leaves the shell's prompt/cursor untouched, and the grid
        // reflow would replace the shell's exact cursor with an approximation
        // (off by one on prompts with trailing markup), so the insert cursor
        // would land before the first typeable column.
        if cols == self.combined.grid().cols() && rows == self.combined.grid().rows() {
            return;
        }
        self.transport_resize(cols, rows);
        self.apply_grid_resize(cols, rows);
    }

    /// Size change sent to the PTY or mux server without the same-size
    /// guard above; used at attach time, when the remote PTY starts at the
    /// server's default geometry regardless of this grid's size.
    fn transport_resize(&mut self, cols: usize, rows: usize) {
        match &mut self.transport {
            PaneTransport::Local { master, .. } => {
                let _ = master.resize(PtySize {
                    rows: rows as u16,
                    cols: cols as u16,
                    pixel_width: 0,
                    pixel_height: 0,
                });
            }
            PaneTransport::Mux { client, .. } => {
                let _ = client.resize(cols as u16, rows as u16);
            }
        }
    }

    /// Apply a grid size change: a no-op at the same size, otherwise a
    /// reflow that preserves any scrolling region the child set (restored
    /// via `CSI r` so full-screen apps keep their bounds). Used both for
    /// layout-driven resizes ([`Pane::resize`]) and for mux
    /// session-geometry sync, where the PTY lives server-side and only the
    /// grid must follow.
    /// Apply a grid size change: a no-op at the same size, otherwise a
    /// reflow that preserves any scrolling region the child set (restored
    /// via `CSI r` so full-screen apps keep their bounds). Used both for
    /// layout-driven resizes ([`Pane::resize`]) and for mux
    /// session-geometry sync, where the PTY lives server-side and only the
    /// grid must follow.
    fn apply_grid_resize(&mut self, cols: usize, rows: usize) {
        if cols == self.combined.grid().cols() && rows == self.combined.grid().rows() {
            return;
        }
        let had_region = {
            let g = self.combined.grid();
            g.scroll_top() != 0 || g.scroll_bottom() != g.rows().saturating_sub(1)
        };
        self.combined.resize(cols, rows);
        if had_region {
            self.write(b"\x1b[r");
        }
    }

    /// The terminal cell grid (read-only for rendering).
    pub fn grid(&self) -> &Grid {
        self.combined.grid()
    }

    /// Set the pixel cell size so image blocks reserve the exact rows they
    /// occupy. Called once the renderer's metrics are known.
    pub fn set_cell_size(&mut self, width: f32, height: f32) {
        self.combined.set_cell_size(width, height);
    }

    /// The terminal cell grid (mutable, for scrollback navigation).
    pub fn grid_mut(&mut self) -> &mut Grid {
        self.combined.grid_mut()
    }

    /// The scrollback parsed so far.
    pub fn scrollback(&self) -> &Scrollback {
        self.combined.scrollback()
    }

    /// True when no full-screen process is running. Uses the alternate screen as
    /// a proxy: full-screen apps (vim, fzf, less) enter it; the shell prompt does not.
    pub fn is_at_prompt(&self) -> bool {
        !self.combined.grid().is_alt_screen()
    }

    /// True while a foreground process other than the shell itself owns the
    /// pane: a full-screen program (via [`Self::is_at_prompt`]) or, on Linux,
    /// any other foreground process group leader (via
    /// [`Self::foreground_process_name`]). A bare Escape in Insert mode is
    /// forwarded to it instead of switching to Normal mode.
    pub fn has_foreground_process(&self) -> bool {
        !self.is_at_prompt() || self.foreground_process_name().is_some()
    }

    /// Whether bracketed paste mode (CSI ?2004h) is active.
    pub fn bracketed_paste(&self) -> bool {
        self.combined.grid().bracketed_paste()
    }

    /// Whether any mouse tracking mode is active.
    pub fn mouse_tracking(&self) -> bool {
        self.combined.grid().mouse_tracking()
    }

    /// Whether drag tracking specifically is active.
    pub fn mouse_drag_tracking(&self) -> bool {
        self.combined.grid().mouse_drag_tracking()
    }

    /// Whether focus event mode (CSI ?1004h) is active.
    pub fn focus_event(&self) -> bool {
        self.combined.grid().focus_event()
    }

    /// Whether SGR extended mouse mode is active.
    pub fn mouse_sgr(&self) -> bool {
        self.combined.grid().mouse_sgr()
    }

    /// Take the pending window title set by OSC 0/2, if any.
    pub fn take_title(&mut self) -> Option<String> {
        self.combined.take_title()
    }

    /// Take the clipboard text from a pending `OSC 52` write, if any.
    pub fn take_clipboard_write(&mut self) -> Option<String> {
        self.combined.take_clipboard_write()
    }

    /// Take the flag raised by an `OSC 52 ; c ; ?` clipboard read query —
    /// the app layer answers it from the OS clipboard.
    pub fn take_clipboard_read(&mut self) -> bool {
        self.combined.take_clipboard_read()
    }

    /// Whether a bell character was received since the last check.
    pub fn take_bell(&mut self) -> bool {
        self.combined.take_bell()
    }

    pub fn block_queue(&self) -> &BlockQueue {
        &self.block_queue
    }

    pub fn block_queue_mut(&mut self) -> &mut BlockQueue {
        &mut self.block_queue
    }

    pub fn drain_live_patches(&mut self) -> Vec<usize> {
        let blocks = self.combined.scrollback().blocks().to_vec();
        self.block_queue.drain_patched_live(&blocks)
    }

    /// Grow a reserved band by inserting `extra` blank rows at screen row
    /// `row` (the first row past the band): the rows, cursor, and every block
    /// anchor at or below `row` shift down, so the content beneath a patched
    /// block is overdrawn by neither the block nor the shell.
    pub fn insert_band_rows(&mut self, row: usize, extra: usize) {
        self.combined.grid_mut().insert_rows_at(row, extra);
        self.combined.shift_block_anchors(row, extra);
        self.block_queue.shift_rows_at_or_below(row, extra);
    }

    /// Whether the child process has exited.
    pub fn is_alive(&mut self) -> bool {
        match &mut self.transport {
            PaneTransport::Local { child, .. } => match child.try_wait() {
                Ok(Some(_)) => false,
                Ok(None) => true,
                Err(_) => false,
            },
            PaneTransport::Mux { exited, .. } => !*exited,
        }
    }

    /// The shell or command path used to spawn this pane.
    pub fn shell_command(&self) -> &str {
        &self.command
    }

    /// The mux session this pane is attached to, if any.
    pub fn mux_session(&self) -> Option<&str> {
        match &self.transport {
            PaneTransport::Mux { session, .. } => Some(session),
            PaneTransport::Local { .. } => None,
        }
    }

    /// Working directory of the foreground process running in this pane.
    /// On Linux this reads `/proc/{pid}/cwd`; returns `None` on other
    /// platforms, for a mux pane (the process runs on the server's
    /// machine), or when the PID is not available.
    pub fn cwd(&self) -> Option<String> {
        #[cfg(target_os = "linux")]
        {
            let PaneTransport::Local { child, .. } = &self.transport else {
                return None;
            };
            let pid = child.process_id()?;
            std::fs::read_link(format!("/proc/{pid}/cwd"))
                .ok()
                .and_then(|p| p.into_os_string().into_string().ok())
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }

    /// Name of the foreground process running in this pane, if any. A mux
    /// pane has no local foreground process, so it reports none.
    pub fn foreground_process_name(&self) -> Option<String> {
        #[cfg(target_os = "linux")]
        {
            let PaneTransport::Local { child, .. } = &self.transport else {
                return None;
            };
            let shell_pid = child.process_id()?;
            let stat = std::fs::read_to_string(format!("/proc/{shell_pid}/stat")).ok()?;
            let tpgid = parse_foreground_process(&stat)?;
            let comm = std::fs::read_to_string(format!("/proc/{tpgid}/comm")).ok()?;
            let name = comm.trim().to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
        None
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        if let PaneTransport::Local { child, writer, .. } = &mut self.transport {
            writer.flush().ok();
            let _ = child.kill();
            let _ = child.wait();
        }
        // A mux pane detaches implicitly: dropping the client closes the
        // connection, and the session keeps running server-side.
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pane_echo() {
        let mut pane = Pane::with_command(40, 10, CommandBuilder::new("bash"), MAX_SCROLLBACK)
            .expect("test pane spawn");
        pane.write(b"echo hello\n");
        thread::sleep(std::time::Duration::from_millis(100));
        pane.drain_output();
        let text = pane.grid().to_text();
        assert!(
            text.contains("hello"),
            "expected 'hello' in output, got: {text}"
        );
    }

    #[test]
    fn test_with_command_returns_an_error_instead_of_panicking_on_a_bad_command() {
        // Regression: PTY spawn failures used to be `.expect()`-ed, crashing
        // the whole app on an ordinary misconfiguration (a bad shell path,
        // fd exhaustion, ...). A nonexistent executable must surface as an
        // `Err`, not a panic, so callers can fall back or show an error.
        let result = Pane::with_command(
            20,
            5,
            CommandBuilder::new("/nonexistent/winter-test-executable-xyz"),
            MAX_SCROLLBACK,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_pane_resize_signals_pty() {
        let mut pane = Pane::with_command(20, 5, CommandBuilder::new("bash"), MAX_SCROLLBACK)
            .expect("test pane spawn");
        pane.resize(40, 10);
        thread::sleep(std::time::Duration::from_millis(50));
        assert!(pane.is_alive());
    }

    #[test]
    fn test_mux_pane_replays_scrollback_and_sends_input() {
        use crate::mux::protocol::{self, ClientMessage};
        use std::io::{Read, Write};
        #[cfg(unix)]
        use std::os::unix::net::UnixListener;
        #[cfg(windows)]
        use uds_windows::UnixListener;

        fn read_frame(conn: &mut impl Read) -> Vec<u8> {
            let mut len_bytes = [0u8; 4];
            conn.read_exact(&mut len_bytes).unwrap();
            let mut framed = len_bytes.to_vec();
            let mut body = vec![0u8; u32::from_be_bytes(len_bytes) as usize];
            conn.read_exact(&mut body).unwrap();
            framed.extend(body);
            framed
        }

        let path =
            std::env::temp_dir().join(format!("winter-mux-pane-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let socket_path = path.to_string_lossy().to_string();

        let (input_tx, input_rx) = mpsc::channel();
        let (resize_tx, resize_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let _attach = read_frame(&mut conn); // client's Attach
            let session = "work";
            for frame in [
                protocol::encode(&ServerMessage::Attached {
                    session: session.into(),
                    cols: 80,
                    rows: 24,
                }),
                protocol::encode(&ServerMessage::Scrollback {
                    session: session.into(),
                    bytes: b"replayed\n".to_vec(),
                }),
                protocol::encode(&ServerMessage::Output {
                    session: session.into(),
                    bytes: b"live\n".to_vec(),
                }),
            ] {
                conn.write_all(&frame).unwrap();
            }
            // The pane sends a Resize for its real geometry right after
            // attach, then whatever the user types; wait for the Input.
            let mut saw_matching_resize = false;
            loop {
                let frame = read_frame(&mut conn);
                let Some(msg) = protocol::decode::<ClientMessage>(&frame) else {
                    continue;
                };
                match msg {
                    ClientMessage::Input { bytes, .. } => {
                        let _ = input_tx.send(bytes);
                        break;
                    }
                    ClientMessage::Resize {
                        cols: 40, rows: 10, ..
                    } => saw_matching_resize = true,
                    _ => {}
                }
            }
            let _ = resize_tx.send(saw_matching_resize);
        });

        let mut pane = Pane::new_mux_at(&socket_path, 40, 10, "work", MAX_SCROLLBACK).unwrap();
        assert_eq!(pane.mux_session(), Some("work"));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut text = String::new();
        while std::time::Instant::now() < deadline {
            pane.drain_output();
            text = pane.grid().to_text();
            if text.contains("replayed") && text.contains("live") {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            text.contains("replayed"),
            "scrollback must replay, got: {text}"
        );
        assert!(
            text.contains("live"),
            "live output must render, got: {text}"
        );
        assert!(pane.is_alive());

        pane.write(b"echo hi\n");
        let sent = input_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(sent, b"echo hi\n");
        assert!(
            resize_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            "attach must tell the server the pane's real geometry"
        );
        let _ = server.join();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_mux_session_geometry_snaps_the_grid() {
        // The server owns the session's real geometry (the smallest among
        // attached clients). A pane whose layout is larger must follow it —
        // letterboxing within the layout — instead of rendering a stream
        // wrapped for a width it doesn't have.
        use crate::mux::protocol;
        use std::io::{Read, Write};
        #[cfg(unix)]
        use std::os::unix::net::UnixListener;
        #[cfg(windows)]
        use uds_windows::UnixListener;

        fn read_frame(conn: &mut impl Read) -> Vec<u8> {
            let mut len_bytes = [0u8; 4];
            conn.read_exact(&mut len_bytes).unwrap();
            let mut framed = len_bytes.to_vec();
            let mut body = vec![0u8; u32::from_be_bytes(len_bytes) as usize];
            conn.read_exact(&mut body).unwrap();
            framed.extend(body);
            framed
        }

        let path =
            std::env::temp_dir().join(format!("winter-mux-geo-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let socket_path = path.to_string_lossy().to_string();

        let server = thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let _attach = read_frame(&mut conn); // the pane's Attach
            for frame in [
                protocol::encode(&ServerMessage::Attached {
                    session: "s".into(),
                    cols: 20,
                    rows: 5,
                }),
                protocol::encode(&ServerMessage::Output {
                    session: "s".into(),
                    bytes: b"hello\n".to_vec(),
                }),
            ] {
                conn.write_all(&frame).unwrap();
            }
            let _resize = read_frame(&mut conn); // the attach-time Resize
                                                 // Then the arbitration result: another client's smaller
                                                 // geometry won.
            conn.write_all(&protocol::encode(&ServerMessage::Resized {
                session: "s".into(),
                cols: 10,
                rows: 3,
            }))
            .unwrap();
            // Hold the connection open until the pane has drained.
            thread::sleep(std::time::Duration::from_millis(300));
        });

        let mut pane = Pane::new_mux_at(&socket_path, 30, 10, "s", MAX_SCROLLBACK).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            pane.drain_output();
            if (pane.grid().cols(), pane.grid().rows()) == (10, 3) {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            (pane.grid().cols(), pane.grid().rows()),
            (10, 3),
            "the grid must follow the server-arbitrated session geometry"
        );
        assert!(pane.grid().to_text().contains("hello"));

        let _ = server.join();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_new_mux_reports_missing_server() {
        assert!(Pane::new_mux_at(
            "/tmp/winter-mux-pane-missing-test.sock",
            20,
            5,
            "s",
            MAX_SCROLLBACK
        )
        .is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_has_foreground_process_detects_a_running_foreground_command() {
        // Regression: Escape in Insert mode should only be forwarded to a
        // running foreground command instead of switching straight to Normal
        // mode, so `has_foreground_process` must actually track a command
        // that isn't a full-screen (alt-screen) app.
        let mut pane = Pane::with_command(40, 10, CommandBuilder::new("bash"), MAX_SCROLLBACK)
            .expect("test pane spawn");
        for _ in 0..100 {
            pane.drain_output();
            if !pane.has_foreground_process() {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            !pane.has_foreground_process(),
            "an idle shell prompt has no foreground process"
        );

        pane.write(b"sleep 2\n");
        let mut detected = false;
        for _ in 0..100 {
            pane.drain_output();
            if pane.has_foreground_process() {
                detected = true;
                break;
            }
            thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            detected,
            "a running `sleep` should count as a foreground process"
        );
    }
}
