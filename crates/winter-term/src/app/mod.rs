//! The native application: winit event loop, GPU renderer, and tabs wired
//! together. This is the `Winter` binary's core runtime: keyboard input drives
//! the PTY, PTY output drives the cell grid, and the grid is rendered to the
//! GPU surface every frame.
//!
//! This file holds the `App` struct and the winit event-loop plumbing. Every
//! other responsibility lives in a submodule that carries its own `impl App`:
//!
//! - [`actions`]: keyboard action dispatch.
//! - [`appearance`]: font size, themes, display toggles.
//! - [`blink`]: the blink clock for cursors and blinking text.
//! - [`block_text`]: a structured block's plain text, when drawn natively.
//! - [`blocks`]: block fold, yank, and focus operations.
//! - [`busy`]: which tabs are busy, and their strip spinner.
//! - [`canvas`]: content drawn in a pane rather than painted as cells.
//! - [`errors`]: next-error, walking the `path:line` references a command printed.
//! - [`focus`]: which tab has the keyboard, and telling programs that asked.
//! - [`frame_grid`]: painting an editor's styled rows into a cell grid.
//! - [`geometry`]: chrome insets, viewports, pane hit-testing.
//! - [`ime`]: input-method composition at the cursor.
//! - [`init`]: GPU and window bootstrap on `resumed`.
//! - [`jobs`]: slow work an editor asked for, run off the event-loop thread.
//! - [`lifecycle`]: construction, state persistence, quit and reload.
//! - [`live_search`]: the project searched as you type, in the palette.
//! - [`navigation`]: vim-style cursor motions, search, quick-select.
//! - [`notices`]: notifications: toasts, the bell, and their history.
//! - [`open_tabs`]: every open tab, terminal or editor, and what it holds.
//! - [`open_with`]: opening the focused tab's file in another editor.
//! - [`overlay`]: what holds the keyboard over the panes.
//! - [`palette`]: command palette and the pickers built on it.
//! - [`panes`]: pane creation, splitting, closing, per-frame upkeep.
//! - [`pointer`]: mouse hit-testing, selection, clipboard, PTY mouse forwarding.
//! - [`project_files`]: the project file picker.
//! - [`prompt_edit`]: Vim delete operators on the last prompt, as shell edits.
//! - [`quota`]: which AI quotas are asked, and the Z.ai API key.
//! - [`recent`]: the recent list of tabs, files, and directories.
//! - [`remote`]: the file browser as root, in a container, or over SSH.
//! - [`render`]: frame composition.
//! - [`sequence`]: multi-key sequences and the which-key hint.
//! - [`session_restore`]: rebuilding a saved session's layout and PTYs.
//! - [`settings`]: the settings page.
//! - [`settings_grid`]: painting the settings page into a cell grid.
//! - [`status_bar`]: the status bar and its configurable segments.
//! - [`strip`]: the strip of tabs along the top of every pane.
//! - [`tabbar`]: the top tabbar and its clicks.
//! - [`tabs`]: tab creation, activation order, the most-recently-used ring.
//! - [`title`]: window title composition and the yank highlight.
//! - [`editor`]: editor tabs: opening, closing, and reopening them, and their keys.
//! - [`editor_cursor`]: a Vim-style text cursor over an editor's rows.
//! - [`wake`]: waking the event loop from the threads that feed it.
//! - [`window_event`]: handlers for individual winit window events.

pub mod actions;
mod appearance;
mod blink;
mod block_text;
mod blocks;
mod busy;
mod canvas;
mod editor;
mod editor_cursor;
mod errors;
mod focus;
mod frame_grid;
mod geometry;
mod ime;
mod init;
mod jobs;
mod lifecycle;
mod live_search;
pub(crate) mod navigation;
mod notices;
mod open_tabs;
mod open_with;
mod overlay;
mod palette;
mod panes;
mod pointer;
mod project_files;
mod prompt_edit;
mod quota;
mod recent;
mod remote;
mod render;
mod sequence;
mod session_restore;
mod settings;
mod settings_grid;
mod status_bar;
mod strip;
mod tabbar;
mod tabs;
#[cfg(test)]
mod test_support;
mod title;
pub(crate) mod wake;
mod window_event;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::{Window, WindowId};

use crate::config::Config;
use crate::control::{ControlRequest, SocketFile};
use crate::model::input::{self, KeyCode, Keymap, PendingPrefix, VisualKind};
use crate::model::layout::TabId;
use crate::model::mode::Mode;
use crate::model::notify::Notifier;
use crate::model::paint::IconKind;
use crate::model::picture::Content;
use crate::model::textedit::recall::Answers;
use winter_render::renderer::GpuRenderer;
use winter_render::Theme;

use self::canvas::Fit;
use self::navigation::{SearchFrom, SearchState, VimState};
use self::open_tabs::OpenTabs;
use self::overlay::Overlay;
use self::pointer::{PointerState, SelectionState};
use self::sequence::is_hint_redraw_due;
use self::tabbar::MenuState;
use self::tabs::TabsState;

// ========================================================================
// Constants
// ========================================================================

/// Debounce delay before executing incremental search or filter updates.
pub(crate) const SEARCH_DEBOUNCE_MILLIS: u64 = 150;
const PTY_POLL_INTERVAL: Duration = Duration::from_millis(16);
/// Poll interval used once `PTY_ACTIVE_WINDOW` has passed with no output, input,
/// or other window activity. `about_to_wait` still has no way to be woken
/// directly by a pane's PTY-reader thread, so it can't stop polling outright;
/// backing off to this from the 16ms cadence once idle is what keeps a
/// sitting-there terminal from holding the CPU out of its deeper idle states
/// around the clock.
const PTY_POLL_INTERVAL_IDLE: Duration = Duration::from_millis(250);
/// How long since the last output/input/window activity before `about_to_wait`
/// backs off to `PTY_POLL_INTERVAL_IDLE`. Long enough that normal think-time
/// between commands doesn't visibly change responsiveness, short enough that
/// a command finishing and printing while the user has stepped away still
/// shows up promptly once they're back.
const PTY_ACTIVE_WINDOW: Duration = Duration::from_secs(2);
/// How long the window's size and place must hold still before they are
/// written to the state file.
const GEOMETRY_SAVE_SETTLE: Duration = Duration::from_millis(500);
/// Period of one cursor blink phase (on or off). Two phases = one full blink cycle.
const CURSOR_BLINK_PERIOD: Duration = Duration::from_millis(530);
const SPLIT_RATIO: f32 = 0.5;
/// Cell rows reserved at the bottom of the surface for the status bar.
pub(crate) const STATUS_BAR_ROWS: usize = 1;
/// How long the span a yank took stays lit. Long enough to register as
/// "that is what was copied", short enough to be gone before the next
/// keystroke's motion, which is vim-highlightedyank's own default.
const YANK_FLASH_DURATION: Duration = Duration::from_millis(150);
/// Pixel margin from the top/bottom of the content viewport within which a
/// held-button selection drag auto-scrolls the pane's scrollback.
const AUTO_SCROLL_EDGE_MARGIN: f32 = 24.0;
/// How many history lines one auto-scroll step moves at full edge depth (the
/// pointer at or past the viewport's own edge). Barely inside the margin still
/// crawls at one line per step: the speed scales linearly in between, so
/// holding deeper into the edge scrolls proportionally faster.
const AUTO_SCROLL_MAX_LINES_PER_TICK: usize = 4;
/// Minimum time between auto-scroll steps while the pointer sits in the edge
/// margin, so history advances at a readable pace rather than once per
/// `about_to_wait` tick.
const AUTO_SCROLL_INTERVAL: Duration = Duration::from_millis(50);

/// Pixel width of the scrollbar hit region at the right edge of each pane.
const SCROLLBAR_CLICK_WIDTH: f32 = 8.0;
/// Pixel margin from the window's outer edge within which a press starts an
/// OS-driven `drag_resize_window`. Only applies to the Modern title bar: with
/// OS decorations off, nothing else offers a resize border.
const WINDOW_RESIZE_BORDER_PX: f32 = 6.0;
/// Bounds and step for the settings page's font-size and opacity rows. The
/// opacity range matches the clamp in [`App::apply_setting`].
const MIN_FONT_SIZE: f32 = 6.0;
const MAX_FONT_SIZE: f32 = 72.0;
const FONT_SIZE_STEP: f32 = 1.0;
const MIN_OPACITY: f32 = 0.1;
const MAX_OPACITY: f32 = 1.0;
const OPACITY_STEP: f32 = 0.05;
const MIN_SCROLLBACK: f32 = 100.0;
const MAX_SCROLLBACK: f32 = 100_000.0;
const SCROLLBACK_STEP: f32 = 1_000.0;
/// Bounds for the pane divider. One logical pixel is the thinnest line that
/// still draws, and past a few the divider reads as a gap rather than a rule.
const MIN_PANE_BORDER: f32 = 1.0;
const MAX_PANE_BORDER: f32 = 8.0;
const PANE_BORDER_STEP: f32 = 1.0;

pub(crate) const DEFAULT_COLS: u32 = 80;
pub(crate) const DEFAULT_ROWS: u32 = 24;
const APPROX_CELL_WIDTH: u32 = 9;
const APPROX_CELL_HEIGHT: u32 = 20;
/// Terminal rows scrolled per wheel notch (one `LineDelta` unit). Windows
/// and X11 report exactly 1.0 per notch regardless of the OS "lines to
/// scroll" setting, so without this multiplier a single click only moves
/// one row; this matches the ~3-line default most terminals and browsers use.
const SCROLL_LINES_PER_WHEEL_NOTCH: f32 = 3.0;
/// Floor below which a window is unusable; also the clamp applied to a
/// persisted size, so a stale `state.json` (e.g. saved while minimized) can
/// never restore an unreadably tiny window.
const MIN_COLS: u32 = 20;
const MIN_ROWS: u32 = 4;

// ========================================================================
// Free functions
// ========================================================================

/// Rows available to panes after reserving the status bar row, never below one.
pub(crate) fn content_rows(full_rows: usize) -> usize {
    full_rows.saturating_sub(STATUS_BAR_ROWS).max(1)
}

/// Whether `about_to_wait` should back off to `PTY_POLL_INTERVAL_IDLE`:
/// `PTY_ACTIVE_WINDOW` has elapsed since `last_activity` as of `now`.
fn is_poll_idle(last_activity: Instant, now: Instant) -> bool {
    now.saturating_duration_since(last_activity) >= PTY_ACTIVE_WINDOW
}

/// Whether the blink timer should be running: it is configured on *and* the
/// window has focus. An unfocused window draws its cursor in the steady "not
/// receiving keystrokes" form, which does not consult the blink phase, so
/// flipping it there cannot change a pixel and every redraw it asked for was
/// waste a backgrounded window paid around the clock.
fn is_blink_active(blink_configured: bool, window_focused: bool) -> bool {
    blink_configured && window_focused
}

// ========================================================================
// Data Structures
// ========================================================================

/// The application state, driven by winit's `ApplicationHandler` trait.
pub struct App {
    pub(crate) config: Config,
    /// Filesystem-event receiver for the config directory: drained each
    /// `about_to_wait` tick to hot-reload as soon as a config/theme file is
    /// saved, rather than on a poll interval. `None` if the platform's watch
    /// backend failed to start, in which case hot-reload is simply disabled.
    pub(crate) config_watch_rx: Option<mpsc::Receiver<()>>,
    /// When the config directory, changed and since quiet, is due to be read
    /// again; `None` with nothing pending. See `reload_config_if_changed`.
    pub(crate) config_reload_due: Option<Instant>,
    /// When the window's size and place, changed and since settled, are due
    /// to be written to the state file; `None` with nothing to write. A drag
    /// moves or resizes the window dozens of times a second, and each write
    /// is a flush to disk on this thread.
    pub(crate) geometry_save_due: Option<Instant>,
    /// Owns the filesystem watch backing `config_watch_rx`; never read again
    /// after construction, but must stay alive for as long as the app runs,
    /// dropping it stops event delivery.
    pub(crate) _config_watcher: Option<notify::RecommendedWatcher>,
    pub(crate) dirty: bool,
    /// When the last frame was drawn, so a hint that fell due is given the
    /// one frame that shows it rather than one every tick.
    pub(crate) last_frame_at: Option<Instant>,
    /// What has been said to the user: the toasts on screen and the
    /// session's history behind the bell.
    pub(crate) notifier: Notifier,
    /// How to stop the focused tab's running program, shown in the status
    /// bar until it finishes; see [`App::job_progress_line`].
    pub(crate) job_progress: Option<String>,
    /// When each tab that is doing something last started: a job running for
    /// it, or a command in its shell. Drives the strip's spinner.
    pub(crate) busy_since: HashMap<TabId, Instant>,
    /// The tabs busy long enough to show a spinner, and the frame it is on.
    pub(crate) spinning: HashSet<TabId>,
    pub(crate) spinner_frame: usize,
    /// When the app started, the epoch every spinner frame is counted from.
    pub(crate) started: Instant,
    /// The span a yank just took and the instant its highlight burns out. The
    /// span is kept so the expiry only clears a selection still showing what
    /// the yank copied, never one made since.
    pub(crate) yank_flash: Option<(Selection, Instant)>,
    /// A config diagnostic from startup, held until the window exists.
    /// A notice raised in `App::new` would start its expiry clock before GPU
    /// init and could lapse before the first frame ever paints.
    pub(crate) pending_config_error: Option<String>,
    /// Whether settings.kdl failed to parse when last read, so what is in
    /// force is not what the file says and the settings page must not save
    /// over it.
    pub(crate) settings_rejected: bool,
    /// A single long-lived clipboard handle, created on first use. On Linux the
    /// process must keep the clipboard owner alive to serve the contents to
    /// other apps; a fresh handle dropped right after writing loses them, so
    /// every copy/paste reuses this one instance.
    pub(crate) clipboard: Option<arboard::Clipboard>,
    /// The blocks in the scrollback drawn on the canvas.
    pub(crate) image_blocks: Vec<ImageBlock>,
    /// Set when the window gains focus; cleared on the next `ModifiersChanged`
    /// event. While set, `KeyboardInput::Pressed` events are swallowed because
    /// winit synthesizes presses for all physically-held keys on `XI_FocusIn`,
    /// and those would otherwise be forwarded to the PTY (e.g. Tab from M-TAB).
    pub(crate) suppress_synthesized_keys: bool,
    /// Where the keyboard was when focus events were last reported, so the
    /// next report goes only to the programs whose focus changed.
    pub(crate) focus_reported: Option<focus::FocusSpot>,
    /// The line lit in the tab that just took the keyboard, while it fades.
    pub(crate) focus_flash: Option<focus::FocusFlash>,
    /// The part of the status bar's right side under the pointer, drawn on a
    /// hover background.
    pub(crate) status_hover: Option<winter_render::StatusHover>,
    /// Whether the OS window currently has keyboard focus. `false` while the
    /// user has switched away (alt-tab, another app clicked to front), which
    /// draws the focused pane's cursor in its unfocused form, a hollow block
    /// outline whatever its shape, matching the convention most terminals use to signal
    /// "not receiving your keystrokes right now".
    pub(crate) window_focused: bool,
    /// The last char-search (`f`/`F`/`t`/`T`), repeated by `;` and `,`.
    pub(crate) last_find: Option<input::FindChar>,
    pub(crate) modifiers: winit::event::Modifiers,
    /// Set when the custom window-close control is clicked, drained by the mouse
    /// handler into the same quit path as a native close request.
    pub(crate) exit_requested: bool,
    /// Whether quitting has been agreed to, so asking about what it would
    /// lose is not repeated when the quit is carried out.
    pub(crate) quit_confirmed: bool,
    /// Whether a reload has been agreed to over what it would lose, so the
    /// reload that follows the answer does not ask again.
    pub(crate) reload_confirmed: bool,
    /// Set by the "reload" command or an incoming [`crate::control::ControlMessage::Reload`],
    /// drained in `about_to_wait` into [`App::reload`] instead of [`App::quit`].
    pub(crate) pending_reload: bool,
    /// Background threads' control-channel messages (see [`crate::control`]),
    /// polled in `about_to_wait`. `None` when no control socket could be
    /// bound.
    pub(crate) control_rx: Option<mpsc::Receiver<ControlRequest>>,
    /// This window's own control socket file, removed when the app is
    /// dropped.
    pub(crate) _control_socket: Option<SocketFile>,
    /// Set after a Vim prompt edit so the nav cursor is re-seeded to the shell
    /// cursor once the resulting PTY echo is drained.
    pub(crate) nav_resync_pending: bool,
    pub(crate) next_image_id: u64,
    /// GPU texture ids for icons already rasterized, keyed by icon name and the
    /// pixel box they were drawn for. `None` records an icon that failed to
    /// rasterize, so a broken document is not retried every frame.
    ///
    /// Cleared when the cell size changes, because every entry was rasterized
    /// for the old one: see [`App::icon_texture`].
    pub(crate) icon_textures: HashMap<(String, u32, u32), Option<u64>>,
    /// The pixel box `icon_textures` was filled for.
    pub(crate) icon_texture_box: (u32, u32),
    /// What the GPU holds for what each editor shows over its pane, by tab
    /// and the key the editor gave it.
    pub(crate) editor_textures: HashMap<(TabId, u64), canvas::EditorTexture>,
    /// Each icon the open palette's entries name, resolved to its SVG once
    /// rather than on every frame the palette is drawn. Emptied whenever no
    /// palette is open, so it never outgrows one list.
    pub(crate) palette_icons: HashMap<IconKind, Option<Arc<[u8]>>>,
    /// Every open tab, terminal or editor, by id.
    pub(crate) open_tabs: OpenTabs,
    /// For each commit-message editor tab, the pane beside it showing what is
    /// staged, which closes with it.
    pub(crate) commit_companions: HashMap<TabId, TabId>,
    /// Panes and editors that were closed, oldest first, for reopening. A pane
    /// or a tab closed with an editor on it used to drop the editor with no way
    /// back, which for an editor meant dropping edits nothing had asked
    /// about.
    pub(crate) closed: Vec<editor::Closed>,
    /// The id the next closed thing is given, so a row of the list of them
    /// stands for one editor or pane rather than for a position in a list that
    /// moves.
    pub(crate) next_closed_id: u64,
    /// Whether this session has said yet that a closed tab can be brought
    /// back. Said once, on the first close, and never again.
    pub(crate) reopen_hint_shown: bool,
    /// Whether editors wrap rows wider than their pane onto the next screen
    /// row, toggled by `M-z` in every editor at once.
    pub(crate) editor_wrap: bool,
    /// Slow work editors have asked for, and the answers coming back.
    pub(crate) jobs: jobs::Jobs,
    /// The AI providers' quotas the status bar shows: started with the
    /// first frame that draws the bar, so a window without one asks nothing.
    pub(crate) quota: Option<crate::quota::Monitor>,
    /// The project search behind the palette, while it is open.
    pub(crate) live_search: Option<live_search::LiveSearch>,
    /// Debounce timer for active prompt input changes.
    pub(crate) prompt_debounce: Option<(Instant, TabId)>,
    /// Debounce timer for live search query input.
    pub(crate) search_debounce: Option<(Instant, TabId)>,
    /// Where next-error has got to in the last command's file references.
    pub(crate) error_walk: Option<errors::ErrorWalk>,
    /// What each editor question was answered with before, for the arrow keys.
    pub(crate) prompt_answers: Answers,
    /// The Vim-style text cursor over an editor, while one is up. At most one
    /// exists: it belongs to whichever editor tab `v` was pressed in.
    pub(crate) editor_cursor: Option<editor_cursor::EditorCursor>,
    /// What holds the keyboard over the panes, if anything: a palette, a
    /// question, a name being typed, the settings page (see [`Overlay`]).
    pub(crate) overlay: Option<Overlay>,
    pub(crate) pending: PendingPrefix,
    /// When the current `pending` prefix or key sequence was opened, for the
    /// which-key hint delay.
    pub(crate) pending_since: Option<Instant>,
    /// The multi-key sequence typed so far (`M-g` of `M-g M-h`), if one is
    /// open. See `app::sequence`.
    pub(crate) sequence: Option<sequence::PendingSequence>,
    /// The keys whose presses went to a program, and the tab each went to:
    /// the only ones whose releases follow them, and only there.
    pub(crate) forwarded_presses: HashMap<KeyCode, TabId>,
    pub(crate) quick_select: Option<Vec<QuickLabel>>,
    /// The `f`/`F`/`t`/`T` jump overlay: one label per candidate landing spot in the
    /// focused pane, shown when the target character occurs more than once so the
    /// user picks one with a single keystroke instead of repeating `;`.
    pub(crate) find_labels: Option<Vec<FindLabel>>,
    pub(crate) renderer: Option<GpuRenderer>,
    /// The cursor position before Buffer Swoop opened, restored on cancel.
    pub(crate) swoop_initial_cursor: Option<(TabId, (usize, usize))>,
    /// Previously executed palette queries, persisted across sessions.
    pub(crate) palette_history: Vec<String>,
    /// Directories opened in a listing, most recent first: this window's own,
    /// on top of what was on disk when it started (see `recent`).
    pub(crate) recent_dirs: Vec<PathBuf>,
    /// Files opened in an editor or handed to `$EDITOR`, most recent first.
    pub(crate) recent_files: Vec<PathBuf>,
    /// Next free globally-unique pane id; allocated by [`Self::alloc_tab_id`].
    pub(crate) next_tab_id: u64,
    pub(crate) window: Option<Arc<Window>>,
    /// Configurable split/close/focus key bindings (the `window` keybindings
    /// block), resolved against in Normal mode.
    pub(crate) keymap: Keymap,
    pub(crate) window_title: String,
    /// What this session has changed over the config: a zoom, a toggle.
    pub(crate) overrides: appearance::SessionOverrides,
    /// Whether the cursor is in its "visible" blink phase. Toggled by the blink
    /// timer; always `true` while blink is disabled.
    pub(crate) blink_phase: bool,
    /// When the next blink phase flip is due. Updated on every flip and on key
    /// press (which resets the cursor to visible to give feedback).
    pub(crate) blink_next_flip: Instant,
    /// The phase of blinking text (SGR 5): its own clock, running only while
    /// the last frame drew some and the window has focus, whatever the
    /// cursor's blink setting.
    pub(crate) text_blink: blink::BlinkTimer,
    /// What the input method is composing, and where its candidate window
    /// was last anchored.
    pub(crate) ime: ime::ImeState,
    /// When PTY output, a window event, or any other activity was last
    /// observed. Drives the `about_to_wait` idle back-off (see
    /// `PTY_ACTIVE_WINDOW`).
    pub(crate) last_activity: Instant,
    /// The tabbar menus and the right-click context menu (see [`MenuState`]).
    pub(crate) menus: MenuState,
    /// Transient pointer input: hover, drags, and click timing (see
    /// [`PointerState`]).
    pub(crate) pointer: PointerState,
    /// The live `/` search (see [`SearchState`]).
    pub(crate) search: SearchState,
    /// The active selection and the Visual-mode state behind it (see
    /// [`SelectionState`]).
    pub(crate) selection: SelectionState,
    /// Per-pane vim state: jumplist, changelist, marks, registers (see
    /// [`VimState`]).
    pub(crate) vim: VimState,
    /// Every open tab and the state of the bar across the top of them (see [`TabsState`]).
    pub(crate) tabs: TabsState,
    /// Wakes the event loop from the threads that feed it: jobs, the control
    /// socket, the config watch.
    pub(crate) waker: wake::Waker,
}

/// An action dispatched from a right-click context menu item.
#[derive(Clone)]
pub(crate) enum ContextAction {
    Copy,
    OpenLink(String),
    Paste,
    /// A line between two groups of entries, which does nothing.
    Separator,
    /// An entry an editor offered, standing for a key that editor binds.
    EditorKey(EditorKeyAction),
}

/// A menu entry an editor offered: what it is called, and the keys of its
/// own the editor is typed when it is chosen.
#[derive(Clone)]
pub(crate) struct EditorKeyAction {
    /// Qualified: `winit::keyboard::Key` is also in scope in this module.
    pub(crate) keys: Vec<input::Key>,
    pub(crate) label: String,
    /// The pane whose editor offered it, since the menu outlives the click and
    /// the focus could move under it.
    pub(crate) tab: TabId,
}

/// A block drawn natively, on the canvas: its content, the texture it is
/// drawn into under `id`, and the band of rows it is placed in.
pub(crate) struct ImageBlock {
    /// The absolute row its band starts at.
    pub abs_row: usize,
    /// Scrollback block-list position of the block this image renders, used
    /// to match live-block patch refreshes to their texture.
    pub block_index: usize,
    /// Set once the live block behind this image closed; dims the placed
    /// quad instead of drawing it at full opacity.
    pub closed: bool,
    /// What it shows.
    pub content: Content,
    /// How the texture was last drawn, `None` before it is or after the
    /// content changed: drawn again whenever the fit it is shown at differs.
    pub drawn: Option<Fit>,
    pub id: u64,
    /// Band height in rows the block is drawn into: matches the rows reserved
    /// for it in the grid, so the following prompt sits flush below.
    pub max_rows: usize,
    /// The content's own size: an image's pixels, an SVG's declared size, or
    /// text as laid out at the last width it was drawn at.
    pub natural: Option<(f32, f32)>,
    /// Segment position within the block's output list, paired with
    /// `block_index` for live-block matching.
    pub segment_index: usize,
    pub tab_id: TabId,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct QuickLabel {
    pub col: usize,
    pub label: char,
    pub row: usize,
}

/// One easymotion-style landing spot for an `f`/`F`/`t`/`T` jump: the label key to
/// press and the viewport cell the cursor lands on (already adjusted for `t`/`T`,
/// which stop one cell short of the target character).
#[derive(Clone, Copy, Debug)]
pub(crate) struct FindLabel {
    pub col: usize,
    pub label: char,
    pub row: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Selection {
    pub block: bool,
    pub end_col: usize,
    pub end_row: usize,
    pub tab: TabId,
    pub start_col: usize,
    pub start_row: usize,
}

/// The last Visual selection, restored by `gv` (`Action::RestoreVisual`).
/// Snapshotting happens at every point a Visual selection ends (see
/// `App::remember_visual`); both ends are absolute so the snapshot survives
/// scrolling and buffer growth.
#[derive(Clone, Copy)]
pub(crate) struct LastVisual {
    /// The selection's anchor end, absolute `(row, col)`.
    pub anchor: (usize, usize),
    /// The selection's cursor end, absolute `(row, col)`.
    pub cursor: (usize, usize),
    /// The Visual selection kind (Block, Char, Line).
    pub kind: VisualKind,
    /// The pane the selection lived in; `gv` in another pane restores nothing.
    pub tab: TabId,
}

// ========================================================================
// App
// ========================================================================

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// ApplicationHandler
// ========================================================================

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        self.init_window(event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // Every window event except a repaint counts as activity for the
        // `about_to_wait` poll back-off. `RedrawRequested` is excluded because
        // the app is what asks for those: the cursor blink alone requests one
        // roughly twice a second, so counting them kept `last_activity`
        // permanently fresher than `PTY_ACTIVE_WINDOW` and pinned the loop to
        // `PTY_POLL_INTERVAL` forever, leaving `PTY_POLL_INTERVAL_IDLE`
        // unreachable with the default config.
        if !matches!(event, WindowEvent::RedrawRequested) {
            self.last_activity = Instant::now();
        }
        match event {
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    let mut size_changed = false;
                    if let Some(renderer) = &mut self.renderer {
                        if let Some(window) = &self.window {
                            let scale_factor = window.scale_factor();
                            if renderer
                                .resize(size.width, size.height, scale_factor)
                                .is_some()
                            {
                                size_changed = true;
                            }
                        }
                    }
                    if size_changed {
                        self.resize_all_panes();
                        self.dirty = true;
                        // Windows reports a bogus small size (the legacy "iconic"
                        // size, not 0x0) when the window is minimized; skip
                        // persisting it so a later restart doesn't restore a
                        // near-unusable window.
                        let minimized = self
                            .window
                            .as_ref()
                            .and_then(|w| w.is_minimized())
                            .unwrap_or(false);
                        if !minimized {
                            self.note_geometry_change(Instant::now());
                        }
                    }
                }
            }

            WindowEvent::Moved(_) => {
                let minimized = self
                    .window
                    .as_ref()
                    .and_then(|w| w.is_minimized())
                    .unwrap_or(false);
                if !minimized {
                    self.note_geometry_change(Instant::now());
                }
            }

            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let size = self.window.as_ref().map(|w| w.inner_size());
                if let Some(size) = size {
                    let mut changed = false;
                    if let Some(renderer) = &mut self.renderer {
                        if renderer
                            .resize(size.width, size.height, scale_factor)
                            .is_some()
                        {
                            changed = true;
                        }
                    }
                    if changed {
                        self.resize_all_panes();
                        self.dirty = true;
                    }
                }
            }

            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods;
                // winit always fires ModifiersChanged after the synthesized
                // KeyboardInput::Pressed events it issues on XI_FocusIn, so
                // this is the right moment to stop suppressing. X11-only quirk:
                // on other platforms `suppress_synthesized_keys` is never set,
                // so this reset is a no-op there.
                #[cfg(target_os = "linux")]
                {
                    self.suppress_synthesized_keys = false;
                }
            }

            WindowEvent::KeyboardInput { event, .. } => self.on_keyboard_input(event, event_loop),
            WindowEvent::Ime(ime) => self.on_ime(ime, event_loop),

            WindowEvent::MouseInput { state, button, .. } => {
                self.on_mouse_input(state, button, event_loop)
            }

            WindowEvent::CursorMoved { position, .. } => self.on_cursor_moved(position),

            WindowEvent::MouseWheel { delta, .. } => self.on_mouse_wheel(delta),

            WindowEvent::RedrawRequested => {
                self.render_frame();
            }

            WindowEvent::CloseRequested => {
                self.quit(event_loop);
            }

            WindowEvent::Focused(focused) => {
                // Suppressing synthesized presses on focus-in is an X11-only
                // workaround (see the `ModifiersChanged` arm above): winit's
                // Windows and macOS backends never synthesize those presses,
                // and never fire a compensating `ModifiersChanged` either, so
                // setting this flag there would swallow every keystroke until
                // a modifier key happened to be pressed.
                #[cfg(target_os = "linux")]
                if focused {
                    self.suppress_synthesized_keys = true;
                }
                if !focused {
                    self.pending = input::PendingPrefix::None;
                    self.clear_sequence();
                    self.forwarded_presses.clear();
                    self.tabs.drag_start = None;
                }
                if self.window_focused != focused {
                    self.window_focused = focused;
                    // The blink timer is paused while unfocused, so
                    // `blink_next_flip` is stale by the time focus returns.
                    // Restart it from the visible phase, the same way a key
                    // press does, rather than letting the elapsed deadline
                    // blink the cursor off the instant the window is clicked.
                    if self.config.cursor.blink && focused {
                        self.blink_phase = true;
                        self.blink_next_flip = Instant::now() + CURSOR_BLINK_PERIOD;
                    }
                    self.dirty = true;
                }
            }

            WindowEvent::ThemeChanged(theme) => {
                if let Some(renderer) = &mut self.renderer {
                    if self.config.theme == crate::config::ThemeSetting::Auto {
                        let mut colors = match theme {
                            winit::window::Theme::Dark => Theme::dark(),
                            winit::window::Theme::Light => Theme::light(),
                        };
                        self.config.colors.apply(&mut colors);
                        renderer.set_theme(colors);
                        self.sync_translucency();
                        self.dirty = true;
                    }
                }
            }

            _ => {}
        }
        // A key, a click, or the window's own focus may have moved the
        // keyboard; the programs that asked hear where it went.
        self.sync_focus_reports();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.reload_config_if_changed() {
            self.dirty = true;
        }

        self.serve_control_requests();
        if self.pending_reload {
            self.pending_reload = false;
            self.reload(event_loop);
            return;
        }

        self.reap_dead_panes();
        // A tab closed on its own hands the keyboard on.
        self.sync_focus_reports();
        if self.exit_requested {
            self.exit_requested = false;
            self.quit(event_loop);
            return;
        }
        let any_output = self.drain_all_panes();

        if any_output {
            self.last_activity = Instant::now();
            // A Vim prompt edit just echoed back: re-seed the nav cursor onto the
            // shell's new cursor position so it tracks the edited line.
            if self.nav_resync_pending {
                let focused = self.layout().focused();
                if self.open_tabs.mode(&focused) == Some(Mode::Normal) {
                    self.init_nav_cursor(focused);
                }
                self.nav_resync_pending = false;
            }
            self.dirty = true;
            self.update_window_title();
        }

        // Toasts whose time is up come down; the loop wakes for the next.
        let toast_tick = self.expire_toasts(Instant::now());

        // Blinking text runs on its own timer, for as long as some is on
        // screen.
        if self
            .text_blink
            .tick(Instant::now(), self.is_text_blink_active())
        {
            self.dirty = true;
        }

        // Flip cursor blink phase when the timer fires, requesting a redraw to
        // show the updated state.
        if is_blink_active(self.config.cursor.blink, self.window_focused) {
            let now = Instant::now();
            if now >= self.blink_next_flip {
                self.blink_phase = !self.blink_phase;
                self.blink_next_flip = now + CURSOR_BLINK_PERIOD;
                self.dirty = true;
            }
        }

        // One frame to show the which-key hint once its delay is up, and no
        // more: the frame that drew it is what shows it from then on.
        if let Some(deadline) = self.hint_deadline() {
            if is_hint_redraw_due(deadline, Instant::now(), self.last_frame_at) {
                self.dirty = true;
            }
        }

        self.auto_scroll_selection();
        if self.take_geometry_save_due(Instant::now()) {
            self.save_app_state();
        }

        // Answers from editor jobs. Each worker wakes the loop as it answers,
        // so a running job needs no faster poll to be heard.
        if self.deliver_finished_jobs() {
            self.dirty = true;
        }
        if self.quota.as_mut().is_some_and(|quota| quota.drain()) {
            self.dirty = true;
        }
        // Editors that keep themselves current (the process monitor) ask for
        // their next refresh here, and the loop wakes for the earliest.
        let editor_tick = self.tick_editors(Instant::now());
        let live_search_tick = self.tick_live_search(Instant::now());
        let progress = self.job_progress_line(self.layout().focused());
        if progress != self.job_progress {
            self.job_progress = progress;
            self.dirty = true;
        }
        let spinner_tick = self.update_spinners(Instant::now());

        if let Some((deadline, focused)) = self.search_debounce {
            let now = Instant::now();
            if now >= deadline {
                self.search_debounce = None;
                self.search_step(focused, self.search_start_direction(), SearchFrom::Origin);
                self.dirty = true;
            }
        }
        if let Some((deadline, tab)) = self.prompt_debounce {
            let now = Instant::now();
            if now >= deadline {
                self.prompt_debounce = None;
                if let Some(prompt) = self.editor_prompt() {
                    let query = prompt.field.text().to_string();
                    let tag = prompt.request.tag;
                    if let Some(slot) = self.open_tabs.editor_mut(&tab) {
                        let outcome = slot.editor.on_prompt_input(&query, tag);
                        self.act_on_editor_outcome(tab, outcome);
                    }
                }
                self.dirty = true;
            }
        }

        // Turn the app-wide "what is drawn no longer matches the state" flag
        // into the one redraw request it stands for. This is the only place a
        // redraw is requested: every change sets `dirty` and leaves the frame
        // to this. Taken rather than read, so one request is made per batch
        // of changes; winit coalesces a redraw already pending.
        if let Some(window) = &self.window {
            if std::mem::take(&mut self.dirty) {
                window.request_redraw();
            }
        }

        let now = Instant::now();
        let poll_interval = if is_poll_idle(self.last_activity, now) {
            PTY_POLL_INTERVAL_IDLE
        } else {
            PTY_POLL_INTERVAL
        };
        let next_poll = now + poll_interval;
        let mut wakeup = if is_blink_active(self.config.cursor.blink, self.window_focused) {
            next_poll.min(self.blink_next_flip)
        } else {
            next_poll
        };
        if let Some(deadline) = self.text_blink.deadline(self.is_text_blink_active()) {
            wakeup = wakeup.min(deadline);
        }
        if let Some((deadline, _)) = self.search_debounce {
            wakeup = wakeup.min(deadline);
        }
        if let Some((deadline, _)) = self.prompt_debounce {
            wakeup = wakeup.min(deadline);
        }
        if let Some(deadline) = editor_tick {
            wakeup = wakeup.min(deadline);
        }
        if let Some(deadline) = live_search_tick {
            wakeup = wakeup.min(deadline);
        }
        if let Some(deadline) = spinner_tick {
            wakeup = wakeup.min(deadline);
        }
        if let Some(deadline) = toast_tick {
            wakeup = wakeup.min(deadline);
        }
        // A yank's highlight goes out on its own deadline, and the loop wakes
        // for it rather than leaving it lit until the next PTY poll.
        if let Some(deadline) = self.tick_focus_flash(now) {
            wakeup = wakeup.min(deadline);
        }
        if let Some(deadline) = self.expire_yank_flash() {
            wakeup = wakeup.min(deadline);
        }
        // If any terminal is holding back synchronized output, wake up promptly
        // when its hold deadline expires so the frame is released without lag.
        for (_, terminal) in self.open_tabs.terminals() {
            if let Some(deadline) = terminal.sync_hold_deadline() {
                wakeup = wakeup.min(deadline);
            }
        }
        if let Some(deadline) = self.hint_deadline().filter(|deadline| *deadline > now) {
            wakeup = wakeup.min(deadline);
        }
        if let Some(due) = self.config_reload_due {
            wakeup = wakeup.min(due);
        }
        if let Some(due) = self.geometry_save_due {
            wakeup = wakeup.min(due);
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(wakeup));
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_rows_reserves_status_bar_and_floors_at_one() {
        assert_eq!(content_rows(24), 24 - STATUS_BAR_ROWS);
        assert_eq!(content_rows(1), 1);
        assert_eq!(content_rows(0), 1);
    }

    #[test]
    fn test_blink_defaults_on() {
        let app = App::new();
        assert!(app.config.cursor.blink);
        assert!(app.blink_phase);
    }

    #[test]
    fn test_is_poll_idle_false_before_active_window_elapses() {
        let last_activity = Instant::now();
        let now = last_activity + PTY_ACTIVE_WINDOW - Duration::from_millis(1);
        assert!(!is_poll_idle(last_activity, now));
    }

    #[test]
    fn test_is_poll_idle_true_once_active_window_elapses() {
        let last_activity = Instant::now();
        let now = last_activity + PTY_ACTIVE_WINDOW;
        assert!(is_poll_idle(last_activity, now));
    }

    #[test]
    fn test_blink_needs_both_config_and_window_focus() {
        // Dropping either term is the regression this guards. Losing the focus
        // term puts a backgrounded window back on a 530ms redraw it cannot
        // show, which is what kept `PTY_POLL_INTERVAL_IDLE` unreachable;
        // losing the config term blinks for users who turned blinking off.
        assert!(is_blink_active(true, true));
        assert!(!is_blink_active(true, false));
        assert!(!is_blink_active(false, true));
        assert!(!is_blink_active(false, false));
    }
}
