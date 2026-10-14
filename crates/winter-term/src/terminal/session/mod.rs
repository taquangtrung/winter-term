//! Interactive terminal pane: a cell grid fed by a [`CombinedPerformer`]
//! (unified `vte` performer that drives both the visual grid and the block
//! parser), with bytes arriving either from a locally spawned PTY child
//! or from a named session on an external mux server.

mod performer;
mod shell;
#[cfg(target_os = "windows")]
mod win_process;

use performer::{ApcDecision, CombinedPerformer};
pub(crate) use shell::osc52_read_response;
#[cfg(target_os = "linux")]
use shell::parse_foreground_process;
use shell::resolve_shell;

use std::io::Write;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

use winter_core::Scrollback;
use winter_render::Grid;
#[cfg(test)]
use winter_render::MAX_SCROLLBACK;

use super::block_queue::{BlockBand, BlockQueue};
use crate::mux::protocol::ServerMessage;
use crate::mux::resilience::ResilientClient;

// ========================================================================
// Constants
// ========================================================================

const READ_CHUNK: usize = 4096;

/// DEC private mode for synchronized output: `CSI ? 2026 h` begins a frame the
/// terminal holds back, `CSI ? 2026 l` ends it.
const SYNC_OUTPUT_MODE: u16 = 2026;

/// How a synchronized frame ends in the byte stream, which is what releases the
/// bytes held back while it was drawn.
const SYNC_OUTPUT_END: &[u8] = b"\x1b[?2026l";

/// The longest a frame is held for before it is shown regardless: a program
/// that began one and never ended it must not freeze the pane.
const SYNC_OUTPUT_TIMEOUT: Duration = Duration::from_millis(35);

/// The most bytes held for one frame, for the same reason.
const SYNC_OUTPUT_MAX_BYTES: usize = 8 * 1024 * 1024;
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
pub struct Terminal {
    block_queue: BlockQueue,
    /// The shell or command path used to spawn this pane, or the mux
    /// session label for a pane attached to a local or SSH-remote session.
    command: String,
    combined: CombinedPerformer,
    parser: vte::Parser,
    /// Whether this pane's program was taken over from an earlier process image
    /// and has not yet been asked to redraw (see [`Pane::nudge_redraw`]).
    adopted: bool,
    /// The rest of a frame the program began with `CSI ? 2026 h`, kept unparsed
    /// until it ends so the grid never shows the frame half drawn.
    sync_hold: Option<SyncHold>,
    /// Row remaps from grid resizes since the last [`Pane::take_row_remaps`]:
    /// the app's own anchors (image blocks, WebView tiles, selections) must
    /// be pushed through them, the same way the pane's internal ones already
    /// were inside [`Pane::apply_grid_resize`].
    row_remaps: Vec<winter_render::RowRemap>,
    transport: PaneTransport,
}

/// The bytes of a synchronized frame held back, and since when.
struct SyncHold {
    bytes: Vec<u8>,
    since: Instant,
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

impl Terminal {
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
        let effective_cwd = cwd.map(std::path::PathBuf::from).or_else(|| {
            #[cfg(target_os = "windows")]
            {
                if let Ok(cur) = std::env::current_dir() {
                    let cur_str = cur.to_string_lossy();
                    if cur_str.ends_with("System32") || cur_str.ends_with("system32") {
                        return crate::model::path::home_dir();
                    }
                    return Some(cur);
                }
                crate::model::path::home_dir()
            }
            #[cfg(not(target_os = "windows"))]
            {
                None
            }
        });
        if let Some(dir) = effective_cwd {
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
        if let Some(editor) = git_editor_command() {
            command.env("GIT_EDITOR", editor);
            // Git announces that it is waiting for an editor it does not know
            // to be a terminal one. Winter's is in a tab of its own, so the
            // line only tells a reader what the tabs already show.
            for (name, value) in quiet_editor_config(std::env::var("GIT_CONFIG_COUNT").ok().as_deref())
            {
                command.env(name, value);
            }
        }

        let pty_system = NativePtySystem::default();
        let pair = pty_system.openpty(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let child = pair.slave.spawn_command(command)?;
        drop(pair.slave);

        Self::from_pty(cols, rows, child, pair.master, max_scrollback, command_str)
    }

    /// A pane over a pty and the child on its slave side, with the thread that
    /// reads the master and hands what it reads to [`Pane::drain_output`].
    fn from_pty(
        cols: usize,
        rows: usize,
        child: Box<dyn portable_pty::Child + Send>,
        master: Box<dyn portable_pty::MasterPty + Send>,
        max_scrollback: usize,
        command_str: String,
    ) -> anyhow::Result<Self> {
        let reader = master.try_clone_reader()?;
        let writer = master.take_writer()?;

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
            row_remaps: Vec::new(),
            sync_hold: None,
            adopted: false,
            transport: PaneTransport::Local {
                child,
                master,
                rx,
                writer,
                _read_thread: Some(read_thread),
            },
        })
    }

    /// A pane over a pty and child that an earlier incarnation of this process
    /// started and handed on across `exec` (see [`Pane::hand_over`]): `fd` is
    /// the pty's master end, still open here, and `pid` the child on the other
    /// side, still this process's child.
    #[cfg(unix)]
    pub fn adopt(
        cols: usize,
        rows: usize,
        fd: std::os::fd::RawFd,
        pid: u32,
        max_scrollback: usize,
        command: String,
    ) -> anyhow::Result<Self> {
        let master = portable_pty::unix::adopt_master(fd)?;
        let child = Box::new(portable_pty::unix::AdoptedChild::new(pid));
        Self::from_pty(cols, rows, child, master, max_scrollback, command)
    }

    /// What it takes to adopt this pane's program in a new process image: the
    /// master descriptor, made to survive `exec`, and the child's process id.
    /// `None` for a pane that is not a local pty.
    #[cfg(unix)]
    pub fn hand_over(&self) -> Option<(std::os::fd::RawFd, u32)> {
        let PaneTransport::Local { master, child, .. } = &self.transport else {
            return None;
        };
        let fd = master.as_raw_fd()?;
        portable_pty::unix::keep_across_exec(fd).ok()?;
        Some((fd, child.process_id()?))
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
            row_remaps: Vec::new(),
            sync_hold: None,
            adopted: false,
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
            row_remaps: Vec::new(),
            sync_hold: None,
            adopted: false,
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
                        // The server owns the session's real geometry: the
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
        let stale_released = self.release_stale_sync_hold();
        let got_any = !chunks.is_empty() || session_geometry.is_some() || stale_released;
        for chunk in &chunks {
            for &byte in chunk {
                self.feed_byte(byte);
            }
        }
        if got_any {
            // A block whose reservation was somehow missed still needs a band;
            // anchor it at the cursor with the default height.
            let fallback = BlockBand {
                abs_row: self.combined.grid().absolute_cursor_row(),
                rows: BLOCK_RESERVE_ROWS,
            };
            let anchors = self.combined.take_block_anchors();
            self.block_queue
                .update(self.combined.scrollback(), fallback, &anchors);
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

    /// The terminal output that rebuilds this pane's screen in a fresh pane of
    /// the same size: the grid (see [`Grid::dump_ansi`]) and the keyboard
    /// protocol the program on it negotiated, which a program asks for once
    /// at startup and would not ask for again.
    pub fn dump_screen(&self) -> Vec<u8> {
        let mut bytes = self.combined.grid().dump_ansi();
        let kitty = self.combined.kitty_flags();
        if kitty != 0 {
            bytes.extend_from_slice(format!("\x1b[>{kitty}u").as_bytes());
        }
        if let Some(mode) = self.combined.modify_other_keys() {
            bytes.extend_from_slice(format!("\x1b[>4;{mode}m").as_bytes());
        }
        bytes
    }

    /// Build this pane's screen from a [`Pane::dump_screen`] taken elsewhere.
    /// It goes straight to the parser, ahead of anything the program says next.
    pub fn restore_screen(&mut self, dump: &[u8]) {
        self.parser.advance(&mut self.combined, dump);
        // The dump asks nothing and anchors no blocks; clear what parsing it
        // might have queued so none of it reaches the program or the host.
        let _ = self.combined.take_pending_responses();
        let _ = self.combined.take_block_anchors();
    }

    /// Note that this pane's program was taken over from an earlier process
    /// image, so [`Pane::take_adopted`] reports it once.
    pub fn mark_adopted(&mut self) {
        self.adopted = true;
    }

    /// Whether the program was taken over and still owes a redraw. True once.
    pub fn take_adopted(&mut self) -> bool {
        std::mem::take(&mut self.adopted)
    }

    /// Make the program on this pane redraw, by changing the pty's size and
    /// changing it back: the program is signalled (`SIGWINCH`) either way, and
    /// a same-size resize alone signals nothing.
    pub fn nudge_redraw(&mut self) {
        let (cols, rows) = (self.combined.grid().cols(), self.combined.grid().rows());
        self.transport_resize(cols.saturating_sub(1).max(1), rows);
        self.transport_resize(cols, rows);
    }

    /// Take one byte of program output into the parser, or into the frame being
    /// held back when the program is in the middle of a synchronized update.
    fn feed_byte(&mut self, byte: u8) {
        if let Some(hold) = self.sync_hold.as_mut() {
            hold.bytes.push(byte);
            if hold.bytes.ends_with(SYNC_OUTPUT_END) || hold.bytes.len() > SYNC_OUTPUT_MAX_BYTES {
                self.release_sync_hold();
            }
            return;
        }
        match self.combined.apc_filter(byte) {
            ApcDecision::Drop => {}
            ApcDecision::Pass => self.parser.advance(&mut self.combined, &[byte]),
            ApcDecision::ReplayEscThenByte(b) => {
                self.parser.advance(&mut self.combined, b"\x1b");
                self.parser.advance(&mut self.combined, &[b]);
            }
        }
        if self.combined.grid().synchronized_output() {
            self.sync_hold = Some(SyncHold {
                bytes: Vec::new(),
                since: Instant::now(),
            });
        }
    }

    /// Show the frame held back: its bytes go to the parser now, all within
    /// one drain, so a repaint sees the finished frame. A frame that begins
    /// another inside what was held is held again from there.
    fn release_sync_hold(&mut self) {
        let Some(hold) = self.sync_hold.take() else {
            return;
        };
        // The held bytes belong to the frame already begun, so they are not to
        // begin it again.
        self.combined
            .grid_mut()
            .set_private_mode(SYNC_OUTPUT_MODE, false);
        for byte in hold.bytes {
            self.feed_byte(byte);
        }
    }

    /// Show a frame that has been held longer than a frame should take.
    /// Returns `true` if a stale held frame was released.
    fn release_stale_sync_hold(&mut self) -> bool {
        let stale = self
            .sync_hold
            .as_ref()
            .is_some_and(|hold| hold.since.elapsed() >= SYNC_OUTPUT_TIMEOUT);
        if stale {
            self.release_sync_hold();
            true
        } else {
            false
        }
    }

    /// The deadline when any currently held synchronized output frame will
    /// expire and be forcibly shown, if a frame is currently being held.
    pub fn sync_hold_deadline(&self) -> Option<Instant> {
        self.sync_hold.as_ref().map(|hold| hold.since + SYNC_OUTPUT_TIMEOUT)
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
        let remap = self.combined.resize(cols, rows);
        // The reflow rebuilt the live screen; the queued block anchors and
        // the app's anchors (see [`Pane::take_row_remaps`]) must follow the
        // rows their bands now occupy, or a re-wrapped line above a block
        // leaves the block drawn over it.
        if let Some(remap) = &remap {
            self.block_queue.remap_rows(remap);
            self.row_remaps.push(remap.clone());
        }
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

    /// The grid column where the editable command line begins on the row
    /// holding the shell cursor, per OSC 133's `B` mark: the shell's own
    /// prompt decoration sits to its left. `None` when the boundary is
    /// unknown (no shell integration, or mid-draw of a fresh prompt).
    pub fn prompt_line_start(&self) -> Option<usize> {
        self.combined.prompt_line_start()
    }

    /// Advance the parser with bytes as if the program on the PTY had written
    /// them, through the same path [`Pane::drain_output`] uses. Test-only:
    /// lets tests fake shell-integration output without a real shell.
    #[cfg(test)]
    pub(crate) fn feed_program_output(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.combined, bytes);
    }

    /// Whether a command the shell started is running on the primary screen:
    /// the shell's own integration marks say so, so a shell without them, and
    /// a full-screen program that stays up by design, never read as busy.
    pub fn command_running(&self) -> bool {
        self.scrollback().shell_integration_active()
            && !self.scrollback().is_at_prompt()
            && !self.combined.grid().is_alt_screen()
    }

    /// True when no full-screen process is running. Uses the alternate screen as
    /// a proxy: full-screen apps (vim, fzf, less) enter it; the shell prompt does
    /// not. When the shell emits OSC 133 integration marks, the mark-driven
    /// phase refines this: a command owns the pane even on the primary screen
    /// (see [`Self::is_at_prompt`]'s integration branch).
    pub fn is_at_prompt(&self) -> bool {
        // Shell-integration marks (OSC 133), when the shell emits them, are
        // authoritative: the prompt and command phases mean the shell itself
        // is waiting for input, the output phase means a command owns the
        // pane even on the primary screen - a REPL, an agent CLI, an
        // `exec`ed program sharing the shell's process group, a mux pane
        // whose process table lives on the server. Without marks, fall back
        // to the alt-screen heuristic.
        if self.scrollback().shell_integration_active() {
            return !self.combined.grid().is_alt_screen() && self.scrollback().is_at_prompt();
        }
        !self.combined.grid().is_alt_screen()
    }

    /// True while a foreground process other than the shell itself owns the
    /// pane: a full-screen program (via [`Self::is_at_prompt`]) or, on Linux,
    /// any other foreground process group leader (via
    /// [`Self::foreground_process_name`]). A bare Escape in Normal mode, with
    /// nothing of Winter's to put away, is forwarded to it.
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

    /// Take the flag raised by an `OSC 52 ; c ; ?` clipboard read query,
    /// the app layer answers it from the OS clipboard.
    pub fn take_clipboard_read(&mut self) -> bool {
        self.combined.take_clipboard_read()
    }

    /// Whether a bell character was received since the last check.
    pub fn take_bell(&mut self) -> bool {
        self.combined.take_bell()
    }

    /// The rich blocks this pane has emitted.
    pub fn block_queue(&self) -> &BlockQueue {
        &self.block_queue
    }

    /// Mutable access to the emitted block list.
    pub fn block_queue_mut(&mut self) -> &mut BlockQueue {
        &mut self.block_queue
    }

    /// Indices of live blocks patched since the last drain.
    pub fn drain_live_patches(&mut self) -> Vec<usize> {
        let blocks = self.combined.scrollback().blocks().to_vec();
        self.block_queue.drain_patched_live(&blocks)
    }

    /// `(block_index, segment_index)` pairs of blocks elided by the
    /// scrollback's retention budget since the last call (see
    /// [`BlockQueue::take_elided`]). The rendered block for each — GPU
    /// image, WebView tile — can never show content again and should be
    /// dropped by the owner.
    pub fn drain_elided_blocks(&mut self) -> Vec<(usize, usize)> {
        self.block_queue.take_elided()
    }

    /// Absolute row spans the screen has erased since the last call (see
    /// [`winter_render::Grid::take_erased_spans`]). Any block anchored inside
    /// one has lost the rows it was drawn over and must be dropped.
    pub fn take_erased_spans(&mut self) -> Vec<(usize, usize)> {
        self.combined.grid_mut().take_erased_spans()
    }

    /// Row remaps from grid resizes since the last call (see
    /// [`winter_render::Grid::take_row_remap`]). The app pushes its own
    /// absolute-row anchors through each: the reflow moved the content they
    /// name, and an anchor left behind draws its block over the wrong rows.
    pub fn take_row_remaps(&mut self) -> Vec<winter_render::RowRemap> {
        std::mem::take(&mut self.row_remaps)
    }

    /// Grow a reserved band by inserting `extra` blank rows at absolute row
    /// `abs_row` (the first row past the band): the rows, cursor, and every
    /// block anchor at or below it shift down, so the content beneath a
    /// patched block is overdrawn by neither the block nor the shell.
    pub fn insert_band_rows(&mut self, abs_row: usize, extra: usize) {
        let Some(live_row) = self.live_band_row(abs_row) else {
            return;
        };
        self.combined.grid_mut().insert_rows_at(live_row, extra);
        self.shift_anchors_at_or_below(abs_row, extra as isize);
    }

    /// Hand `spare` rows of a reserved band back at absolute row `abs_row`
    /// (the first row the block does not need): the rows below close the gap,
    /// and the cursor and every anchor below follow them up, so a block that
    /// reserved more room than its content needed does not leave dead space
    /// above the next prompt.
    pub fn remove_band_rows(&mut self, abs_row: usize, spare: usize) {
        let Some(live_row) = self.live_band_row(abs_row) else {
            return;
        };
        self.combined.grid_mut().remove_rows_at(live_row, spare);
        self.shift_anchors_at_or_below(abs_row, -(spare as isize));
    }

    /// The live grid row absolute `abs_row` names, or `None` when it is not a
    /// live row at all: the grid can only insert into or delete from its live
    /// rows, so a band whose tail has scrolled into history is no longer
    /// resizable and the block is clipped to what it has instead.
    fn live_band_row(&self, abs_row: usize) -> Option<usize> {
        let grid = self.combined.grid();
        let live_row = abs_row.checked_sub(grid.absolute_live_top())?;
        (live_row < grid.rows()).then_some(live_row)
    }

    /// Move every anchor at or below absolute `row` by `delta`, keeping the
    /// pending anchors and the queued entries pointing at the same content.
    fn shift_anchors_at_or_below(&mut self, row: usize, delta: isize) {
        self.combined.shift_block_anchors(row, delta);
        self.block_queue.shift_rows_at_or_below(row, delta);
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
        if let Some(cwd) = self.combined.cwd() {
            return Some(cwd);
        }
        #[cfg(target_os = "windows")]
        {
            let PaneTransport::Local { child, .. } = &self.transport else {
                return None;
            };
            let pid = child.process_id()?;
            win_process::query_process_cwd(pid)
        }
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
        #[cfg(all(not(target_os = "windows"), not(target_os = "linux")))]
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

impl Drop for Terminal {
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
// GIT_EDITOR
// ========================================================================

/// What Linux appends to the path of a running binary whose file was removed
/// or replaced.
const DELETED_EXE_NOTE: &str = " (deleted)";

/// The `GIT_EDITOR` that makes `git commit` in a pane edit its message in
/// Winter's own editor: this binary's `--edit-wait`, quoted for the shell git
/// runs it through. `None` when the binary's path is not known.
pub(crate) fn git_editor_command() -> Option<String> {
    let exe = runnable_current_exe()?;
    Some(format!(
        "{} --edit-wait",
        shell_quote(&exe.to_string_lossy())
    ))
}

/// The environment that tells git not to print its "waiting for your editor"
/// hint, as one more entry after the `existing` count of config entries the
/// environment already carries, so a user's own are kept. Git before 2.31
/// does not read these and prints the hint anyway.
fn quiet_editor_config(existing: Option<&str>) -> Vec<(String, String)> {
    let count = existing.and_then(|n| n.trim().parse::<usize>().ok()).unwrap_or(0);
    vec![
        ("GIT_CONFIG_COUNT".to_string(), (count + 1).to_string()),
        (format!("GIT_CONFIG_KEY_{count}"), "advice.waitingForEditor".to_string()),
        (format!("GIT_CONFIG_VALUE_{count}"), "false".to_string()),
    ]
}

/// The path of the running binary as something that can be run again. When the
/// binary was replaced on disk after this process started (an install over a
/// running window), Linux reports the old file as `<path> (deleted)`, which
/// names nothing; the path without the note is where the replacement now is.
pub(crate) fn runnable_current_exe() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(std::path::PathBuf::from(replaced_exe_path(
        &exe.to_string_lossy(),
    )))
}

fn replaced_exe_path(path: &str) -> String {
    path.strip_suffix(DELETED_EXE_NOTE)
        .unwrap_or(path)
        .to_string()
}

/// `text` as one word for a POSIX shell.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    #[test]
    fn test_the_editor_hint_is_switched_off_after_the_users_own_config() {
        // The new entry goes after any already in the environment, or it
        // would replace the first of them.
        let env = quiet_editor_config(Some("2"));
        assert_eq!(env[0], ("GIT_CONFIG_COUNT".into(), "3".into()));
        assert_eq!(env[1], ("GIT_CONFIG_KEY_2".into(), "advice.waitingForEditor".into()));
        assert_eq!(env[2], ("GIT_CONFIG_VALUE_2".into(), "false".into()));
        assert_eq!(quiet_editor_config(None)[0].1, "1");
        assert_eq!(quiet_editor_config(Some("junk"))[1].0, "GIT_CONFIG_KEY_0");
    }

    use super::*;

    #[test]
    fn test_a_pane_rebuilt_from_a_screen_dump_has_the_text_and_the_keyboard_protocol() {
        let mut old = quiet_pane();
        feed(
            &mut old,
            b"history one\r\nhistory two\r\n\x1b[>7u\x1b[>4;2m\x1b[1mprompt\x1b[0m> typed",
        );
        let dump = old.dump_screen();

        let mut new = quiet_pane();
        new.restore_screen(&dump);

        assert_eq!(new.grid().to_text(), old.grid().to_text());
        assert_eq!(new.grid().cursor(), old.grid().cursor());
        assert_eq!(new.kitty_flags(), old.kitty_flags());
        assert_eq!(new.kitty_flags(), 7);
        assert_eq!(new.modify_other_keys(), Some(2));
        assert_eq!(new.grid().scrollback_len(), old.grid().scrollback_len());
    }

    #[cfg(unix)]
    #[test]
    fn test_a_pane_adopted_from_a_raw_pty_reads_and_writes_it_and_tracks_its_child() {
        // Stand in for the old process image: a pty and a child on it, of
        // which only the master descriptor and the child's id are carried on.
        let pair = NativePtySystem::default()
            .openpty(PtySize {
                rows: 4,
                cols: 20,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let child = pair
            .slave
            .spawn_command(CommandBuilder::new("cat"))
            .expect("spawn");
        let pid = child.process_id().expect("pid");
        let raw = pair.master.as_raw_fd().expect("master fd");
        // The old image's handle is forgotten, not closed: the descriptor is
        // the adopting pane's from here, as it is after an `exec`.
        std::mem::forget(pair.master);
        drop(pair.slave);

        let mut terminal = Terminal::adopt(20, 4, raw, pid, MAX_SCROLLBACK, "cat".into()).expect("adopts");
        assert!(terminal.is_alive(), "the adopted child is still running");
        terminal.write(b"hello\n");
        let deadline = std::time::Instant::now() + PTY_WAIT;
        let mut text = String::new();
        while std::time::Instant::now() < deadline {
            terminal.drain_output();
            text = terminal.grid().to_text();
            if text.contains("hello") {
                break;
            }
            thread::sleep(POLL_STEP);
        }
        assert!(text.contains("hello"), "the program answered: {text}");
        assert_eq!(terminal.hand_over().map(|(_, id)| id), Some(pid));
    }

    #[test]
    fn test_a_binary_replaced_while_running_is_run_from_its_path_not_its_deleted_note() {
        // Regression: after an install over the running window, new panes got
        // `GIT_EDITOR='/usr/bin/winter (deleted)' --edit-wait`, which git could
        // not run, so `git commit` never reached the commit editor.
        assert_eq!(
            replaced_exe_path("/usr/bin/winter (deleted)"),
            "/usr/bin/winter"
        );
        assert_eq!(replaced_exe_path("/usr/bin/winter"), "/usr/bin/winter");
    }

    #[test]
    fn test_shell_quote_keeps_spaces_and_single_quotes_in_one_word() {
        assert_eq!(shell_quote("/opt/my app/winter"), "'/opt/my app/winter'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    /// How long a test waits for a child process to produce what it was asked
    /// for. Generous, because every wait below breaks out as soon as the output
    /// arrives: the ceiling only has to survive a machine under load.
    const PTY_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

    /// How long to sleep between polls, so waiting does not spin a core.
    const POLL_STEP: std::time::Duration = std::time::Duration::from_millis(20);

    /// A pane over `cat`, which stays quiet on its own so a test can feed it
    /// the bytes a program would have written.
    fn quiet_pane() -> Terminal {
        Terminal::with_command(20, 4, CommandBuilder::new("cat"), MAX_SCROLLBACK)
            .expect("test pane spawn")
    }

    fn feed(terminal: &mut Terminal, bytes: &[u8]) {
        for &byte in bytes {
            terminal.feed_byte(byte);
        }
    }

    #[test]
    fn test_a_synchronized_frame_is_shown_whole_when_it_ends_not_while_it_is_drawn() {
        let mut terminal = quiet_pane();
        feed(&mut terminal, b"old prompt");

        // The frame clears the screen and starts redrawing it.
        feed(&mut terminal, b"\x1b[?2026h\x1b[2J\x1b[Hnew");
        assert!(
            terminal.grid().to_text().contains("old prompt"),
            "the half drawn frame is not shown"
        );
        assert!(!terminal.grid().to_text().contains("new"));

        feed(&mut terminal, b" frame\x1b[?2026l");
        let text = terminal.grid().to_text();
        assert!(
            text.contains("new frame"),
            "the finished frame shows: {text}"
        );
        assert!(!text.contains("old prompt"));
        assert!(!terminal.grid().synchronized_output());
    }

    #[test]
    fn test_two_frames_in_one_read_are_each_applied_in_order() {
        let mut terminal = quiet_pane();

        feed(
            &mut terminal,
            b"\x1b[?2026h\x1b[Hone\x1b[?2026l\x1b[?2026h\x1b[2J\x1b[Htwo\x1b[?2026l",
        );

        let text = terminal.grid().to_text();
        assert!(text.contains("two") && !text.contains("one"), "got: {text}");
        assert!(terminal.sync_hold.is_none());
    }

    #[test]
    fn test_a_frame_that_never_ends_is_shown_after_the_timeout() {
        let mut terminal = quiet_pane();
        feed(&mut terminal, b"\x1b[?2026hstuck");
        assert!(!terminal.grid().to_text().contains("stuck"));

        terminal.sync_hold.as_mut().expect("held").since -= SYNC_OUTPUT_TIMEOUT;
        let drained = terminal.drain_output();

        assert!(drained, "draining a timed-out frame must report new output");
        assert!(terminal.grid().to_text().contains("stuck"));
        assert!(terminal.sync_hold.is_none());
        assert_eq!(terminal.sync_hold_deadline(), None);
    }

    #[test]
    fn test_pane_echo() {
        let mut terminal = Terminal::with_command(40, 10, CommandBuilder::new("bash"), MAX_SCROLLBACK)
            .expect("test pane spawn");
        terminal.write(b"echo hello\n");
        // Polled rather than slept through once: a shell on a loaded machine
        // takes far longer than any fixed beat to hand its output back.
        let deadline = std::time::Instant::now() + PTY_WAIT;
        let mut text = String::new();
        while std::time::Instant::now() < deadline {
            terminal.drain_output();
            text = terminal.grid().to_text();
            if text.contains("hello") {
                break;
            }
            thread::sleep(POLL_STEP);
        }
        assert!(
            text.contains("hello"),
            "expected 'hello' in output, got: {text}"
        );
    }

    #[test]
    fn test_shell_integration_marks_drive_prompt_state_and_line_start() {
        // A shell emitting OSC 133: the pane's prompt state follows the marks
        // rather than the alt-screen heuristic, and the B mark records where
        // the editable command line begins.
        let mut terminal = Terminal::with_command(
            40,
            10,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        assert!(
            terminal.is_at_prompt(),
            "no marks yet: primary-screen heuristic applies"
        );
        assert_eq!(terminal.prompt_line_start(), None);

        terminal.feed_program_output(b"\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\");
        assert!(terminal.is_at_prompt(), "shell is drawing/accepting its prompt");
        assert_eq!(
            terminal.prompt_line_start(),
            Some(2),
            "command text starts after \"$ \""
        );

        // The C mark means a command owns the pane even though it never
        // entered the alt screen - a REPL or agent CLI on the primary screen.
        terminal.feed_program_output(b"ls\x1b]133;C\x1b\\");
        assert!(!terminal.is_at_prompt());
        // Still mid-command: the D mark ends it, and the next A starts a
        // fresh prompt whose line-start boundary is unknown until its B.
        terminal.feed_program_output(b"out\r\n\x1b]133;D;0\x1b\\\r\n");
        assert!(!terminal.is_at_prompt(), "between D and the next prompt");
        terminal.feed_program_output(b"\x1b]133;A\x1b\\");
        assert!(terminal.is_at_prompt());
        assert_eq!(
            terminal.prompt_line_start(),
            None,
            "A clears the stale boundary"
        );
        terminal.feed_program_output(b"$ \x1b]133;B\x1b\\");
        assert_eq!(terminal.prompt_line_start(), Some(2));
    }

    #[test]
    fn test_with_command_returns_an_error_instead_of_panicking_on_a_bad_command() {
        // Regression: PTY spawn failures used to be `.expect()`-ed, crashing
        // the whole app on an ordinary misconfiguration (a bad shell path,
        // fd exhaustion, ...). A nonexistent executable must surface as an
        // `Err`, not a panic, so callers can fall back or show an error.
        let result = Terminal::with_command(
            20,
            5,
            CommandBuilder::new("/nonexistent/winter-test-executable-xyz"),
            MAX_SCROLLBACK,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_pane_resize_signals_pty() {
        let mut terminal = Terminal::with_command(20, 5, CommandBuilder::new("bash"), MAX_SCROLLBACK)
            .expect("test pane spawn");
        terminal.resize(40, 10);
        thread::sleep(std::time::Duration::from_millis(50));
        assert!(terminal.is_alive());
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

        let mut terminal = Terminal::new_mux_at(&socket_path, 40, 10, "work", MAX_SCROLLBACK).unwrap();
        assert_eq!(terminal.mux_session(), Some("work"));

        let deadline = std::time::Instant::now() + PTY_WAIT;
        let mut text = String::new();
        while std::time::Instant::now() < deadline {
            terminal.drain_output();
            text = terminal.grid().to_text();
            if text.contains("replayed") && text.contains("live") {
                break;
            }
            thread::sleep(POLL_STEP);
        }
        assert!(
            text.contains("replayed"),
            "scrollback must replay, got: {text}"
        );
        assert!(
            text.contains("live"),
            "live output must render, got: {text}"
        );
        assert!(terminal.is_alive());

        terminal.write(b"echo hi\n");
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
        // attached clients). A pane whose layout is larger must follow it,
        // letterboxing within the layout: instead of rendering a stream
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

        let mut terminal = Terminal::new_mux_at(&socket_path, 30, 10, "s", MAX_SCROLLBACK).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            terminal.drain_output();
            if (terminal.grid().cols(), terminal.grid().rows()) == (10, 3) {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            (terminal.grid().cols(), terminal.grid().rows()),
            (10, 3),
            "the grid must follow the server-arbitrated session geometry"
        );
        assert!(terminal.grid().to_text().contains("hello"));

        let _ = server.join();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_new_mux_reports_missing_server() {
        assert!(Terminal::new_mux_at(
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
        let mut terminal = Terminal::with_command(40, 10, CommandBuilder::new("bash"), MAX_SCROLLBACK)
            .expect("test pane spawn");
        for _ in 0..100 {
            terminal.drain_output();
            if !terminal.has_foreground_process() {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            !terminal.has_foreground_process(),
            "an idle shell prompt has no foreground process"
        );

        terminal.write(b"sleep 2\n");
        let mut detected = false;
        for _ in 0..100 {
            terminal.drain_output();
            if terminal.has_foreground_process() {
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
