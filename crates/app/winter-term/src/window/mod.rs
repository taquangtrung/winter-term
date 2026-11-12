//! The native application: winit event loop, GPU renderer, and tabs wired
//! together. This is the `Winter` binary's core runtime: keyboard input drives
//! the PTY, PTY output drives the cell grid, and the grid is rendered to the
//! GPU surface every frame.
//!
//! This file holds the `App` struct and the state it carries. Every
//! responsibility, the winit event loop included, lives in a submodule that carries its own `impl App`,
//! grouped by what it is about:
//!
//! - [`appearance`]: font size, themes, display toggles.
//! - [`editor`]: editor tabs: opening, closing, and reopening them, their keys,
//!   and the Vim-style cursor over their rows.
//! - [`frame`]: composing each frame for the renderer: the panes' cell grids,
//!   the canvas's images, the blink clock.
//! - [`input`]: where keys and the pointer go: key and mouse routing, action
//!   dispatch, multi-key sequences, the overlay holding the keyboard, focus,
//!   and input methods.
//! - [`layout`]: panes and the tabs in them, and the geometry they occupy.
//! - [`menus`]: the menubar and the right-click context menu.
//! - [`notices`]: notifications: toasts, the bell, and their history.
//! - [`picker`]: the command palette and the pickers built on it.
//! - [`runtime`]: bootstrap, lifecycle, session restore, the window events, and
//!   the threads that wake the loop.
//! - [`settings`]: the settings page.
//! - [`status`]: the status bar and its AI quotas.
//! - [`strip`]: the strip of tabs along each pane, and their busy spinners.
//! - [`tabbar`]: the top tabbar and the window title.
//! - [`terminal`]: what terminal tabs offer: blocks, prompt edits, next-error,
//!   navigation, the pointer, and the yank flash.
//! - [`workspace`]: the workspace the window is in, and switching to another.

mod appearance;
pub(crate) mod commands;
mod editor;
mod extension_host;
#[cfg(feature = "wasm-extensions")]
mod extension_installs;
mod frame;
mod input;
mod layout;
mod macro_record;
mod menus;
pub(in crate::window) mod misty_theme;
mod notices;
mod picker;
mod recent;
pub(crate) mod runtime;

pub use runtime::init::prefer_integrated_gpu;
mod settings;
mod status;
mod strip;
mod tabbar;
pub(crate) mod terminal;
#[cfg(test)]
mod test_support;
mod workspace;

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::config::Config;
use winter_base::wake::Waker;
use winter_model::input::{FindChar, Key, Keymap, VisualKind};
use winter_model::layout::TabId;
use winter_model::notify::Notifier;
use winter_model::picture::Content;
use winter_render::renderer::GpuRenderer;

use self::editor::cursor::EditorCursor;
use self::editor::prompt::TransientCard;
use self::frame::blink::Blinking;
use self::frame::canvas::Fit;
use self::frame::canvas::Textures;
use self::input::focus::WindowFocus;
use self::input::ime::ImeState;
use self::input::overlay::Overlay;
use self::input::sequence::PendingKeys;
use self::layout::open_tabs::OpenTabs;
use self::layout::tabs::TabsState;
use self::menus::MenuState;
use self::picker::PickerMemory;

/// The key tables of the pickers' sources' actions, for the Keys editor's
/// one list.
pub(crate) const PICKER_KEY_TABLES: &[&dyn winter_model::input::table::Listed] =
    picker::sources::KEY_TABLES;

/// The keys every picker answers, for the Keys editor's one list.
pub(crate) const PICKER_FRAMEWORK_KEY_TABLES: &[&dyn winter_model::input::table::Listed] =
    picker::keys::KEY_TABLES;
use self::recent::RecentLists;
use self::runtime::jobs::Jobs;
use self::runtime::lifecycle::ConfigWatch;
use self::runtime::lifecycle::ControlChannel;
use self::runtime::lifecycle::ExitState;
use self::strip::busy::Busy;
use self::terminal::errors::ErrorWalk;
use self::terminal::navigation::{SearchState, VimState};
use self::terminal::pointer::{PointerState, SelectionState};
use self::workspace::WorkspaceState;

// ========================================================================
// Constants
// ========================================================================

/// Debounce delay before executing incremental search or filter updates.
pub(in crate::window) const SEARCH_DEBOUNCE_MILLIS: u64 = 150;
/// How long the window's size and place must hold still before they are
/// written to the state file.
const GEOMETRY_SAVE_SETTLE: Duration = Duration::from_millis(500);
/// Period of one cursor blink phase (on or off). Two phases = one full blink cycle.
const CURSOR_BLINK_PERIOD: Duration = Duration::from_millis(530);
const SPLIT_RATIO: f32 = 0.5;
/// How much of the window's height the dock takes when it opens.
pub(in crate::window) const DOCK_SHARE: f32 = 0.3;
/// Cell rows reserved at the bottom of the surface for the status bar.
pub(in crate::window) const STATUS_BAR_ROWS: usize = 1;
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
pub(in crate::window) const SCROLLBAR_CLICK_WIDTH: f32 = 10.0;
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

pub(in crate::window) const DEFAULT_COLS: u32 = 80;
pub(in crate::window) const DEFAULT_ROWS: u32 = 24;
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
pub(in crate::window) fn content_rows(full_rows: usize) -> usize {
    full_rows.saturating_sub(STATUS_BAR_ROWS).max(1)
}

// ========================================================================
// Data Structures
// ========================================================================

/// The application state, driven by winit's `ApplicationHandler` trait.
pub struct App {
    /// What terminal tabs keep beyond their sessions: the `/` search and the selection, live search, next-error, swoop, the yank flash, and their image blocks.
    pub(in crate::window) term: TerminalState,
    /// What the keyboard and the pointer are in the middle of: modifiers held, hover and drags, the wheel, keys pending, a macro being recorded, input methods, focus, and the clipboards.
    pub(in crate::window) input: InputState,
    /// The event loop's own housekeeping: watching the settings, the control channel, quitting, children left by a reload, and its clocks.
    pub(in crate::window) runtime: RuntimeState,
    /// What the window keeps for its editors beyond the tabs themselves:
    /// their jobs, questions, builds, language servers, extensions, and the Vim-style cursor over one of them.
    pub(in crate::window) host: EditorHost,
    /// The furniture over the panes: menus, the overlay holding the
    /// keyboard, jump labels, the status bar's hover, and notifications.
    pub(in crate::window) chrome: Chrome,
    pub(in crate::window) config: Config,
    pub(in crate::window) dirty: bool,
    /// The notifier's tasks standing for the long work under way.
    pub(in crate::window) progress: runtime::progress::ProgressTasks,
    /// Every open tab, terminal or editor, by id.
    pub(in crate::window) open_tabs: OpenTabs,
    /// The AI providers' quotas the status bar shows: started with the
    /// first frame that draws the bar, so a window without one asks nothing.
    pub(in crate::window) quota: Option<crate::quota::Monitor>,
    pub(in crate::window) renderer: Option<GpuRenderer>,
    /// Next free globally-unique pane id; allocated by [`Self::alloc_tab_id`].
    pub(in crate::window) next_tab_id: u64,
    /// The window, as Winter's code asks things of it; `None` until misty
    /// has opened it.
    pub(in crate::window) window: Option<runtime::handle::WindowHandle>,
    /// The GPU misty draws with, which the renderer draws on; `None` until
    /// the window is open.
    pub(in crate::window) gpu: Option<misty::Gpu>,
    /// Configurable split/close/focus key bindings (the `window` keybindings
    /// block), resolved against in Normal mode.
    pub(in crate::window) keymap: Keymap,
    /// The bindings that apply only in a context, looked up ahead of every
    /// other binding (see `input::when`).
    pub(in crate::window) when_keys: winter_model::input::WhenMap,
    pub(in crate::window) window_title: String,
    /// What this session has changed over the config: a zoom, a toggle.
    pub(in crate::window) overrides: appearance::SessionOverrides,
    /// Per-pane vim state: jumplist, changelist, marks, registers (see
    /// [`VimState`]).
    pub(in crate::window) vim: VimState,
    /// Every open tab and the state of the bar across the top of them (see [`TabsState`]).
    pub(in crate::window) tabs: TabsState,
    /// Which tabs are busy, and the spinner their strip titles wear.
    pub(in crate::window) busy: Busy,
    /// The cursor's blink phase and the clock blinking text keeps.
    pub(in crate::window) blink: Blinking,
    /// The GPU textures the canvas has uploaded, and the ids that name them.
    pub(in crate::window) textures: Textures,
    /// What the pickers keep: their queries, their rows' use, the session to
    /// resume.
    pub(in crate::window) pickers: PickerMemory,
    /// The files and directories opened lately.
    pub(in crate::window) recent: RecentLists,
    /// The workspace the window is in, and where the workspaces are kept.
    pub(in crate::window) workspace: WorkspaceState,
}

/// What the event loop keeps for itself rather than for any tab: what it watches, what it reaps, and when things last happened.
pub(in crate::window) struct RuntimeState {
    /// The config file's watcher, the reload it has scheduled, and the error the
    /// last load left to report.
    pub(in crate::window) config_watch: ConfigWatch,
    /// Requests from `winter` commands run elsewhere, and the socket they come through.
    pub(in crate::window) control: ControlChannel,
    /// Whether the window is on its way out, and which questions about it
    /// have been answered.
    pub(in crate::window) exit: ExitState,
    /// When the window's size and place, changed and since settled, are due
    /// to be written to the state file; `None` with nothing to write. A drag
    /// moves or resizes the window dozens of times a second, and each write
    /// is a flush to disk on this thread.
    pub(in crate::window) geometry_save_due: Option<Instant>,
    /// When PTY output, a window event, or any other activity was last
    /// observed. Drives the `about_to_wait` idle back-off (see
    /// `PTY_ACTIVE_WINDOW`).
    pub(in crate::window) last_activity: Instant,
    /// When the last frame was drawn, so a hint that fell due is given the
    /// one frame that shows it rather than one every tick.
    pub(in crate::window) last_frame_at: Option<Instant>,
    /// Children a reload's previous process image left behind that no tab
    /// took over (a put-aside workspace's shells, language servers, jobs):
    /// their reapers went with that image, so they are reaped here as they
    /// end, or each would stay a zombie until the window quits.
    pub(in crate::window) strays: Vec<u32>,
    /// Wakes the event loop from the threads that feed it: jobs, the control
    /// socket, the config watch.
    pub(in crate::window) waker: Waker,
}

/// What input is in the middle of, between one event and the next.
pub(in crate::window) struct InputState {
    /// The system clipboard and what this window copied lately.
    pub(in crate::window) clipboards: terminal::pointer::Clipboards,
    /// The keys pressed since a describe-key began, to be said rather than
    /// run; `None` while no key is being described.
    pub(in crate::window) describing: Option<Vec<Key>>,
    /// Whether the window has the keyboard, which tab was last told so, and the
    /// flash a focus change leaves.
    pub(in crate::window) focus: WindowFocus,
    /// What the input method is composing, and where its candidate window
    /// was last anchored.
    pub(in crate::window) ime: ImeState,
    /// Keys typed so far that have not resolved yet, and presses whose release
    /// is owed to a tab.
    pub(in crate::window) keys: PendingKeys,
    /// Keyboard macro recording and replay state.
    pub(in crate::window) macro_state: macro_record::MacroState,
    pub(in crate::window) modifiers: winit::event::Modifiers,
    /// Transient pointer input: hover, drags, and click timing (see
    /// [`PointerState`]).
    pub(in crate::window) pointer: PointerState,
    /// The wheel's rows still to reach an editor, a frame at a time, and
    /// the part of a row a touchpad has scrolled short of a whole one.
    pub(in crate::window) wheel: input::mouse::Wheel,
}

/// What the window keeps for its terminal tabs beyond each session and the grouped search, selection, and Vim states.
pub(in crate::window) struct TerminalState {
    /// Whether what is typed into the focused terminal goes to every
    /// terminal the panes show (see `terminal::broadcast`).
    pub(in crate::window) broadcast: bool,
    /// Where next-error has got to in the last command's file references.
    pub(in crate::window) error_walk: Option<ErrorWalk>,
    /// The blocks in the scrollback drawn on the canvas.
    pub(in crate::window) image_blocks: Vec<ImageBlock>,
    /// Set after a Vim prompt edit so the nav cursor is re-seeded to the shell
    /// cursor once the resulting PTY echo is drained.
    pub(in crate::window) nav_resync_pending: bool,
    /// The terminal an editor last sent code to, which the next sending
    /// goes to while it is shown.
    pub(in crate::window) sent_to: Option<TabId>,
    /// The live `/` search (see [`SearchState`]).
    pub(in crate::window) search: SearchState,
    /// The active selection and the Visual-mode state behind it (see
    /// [`SelectionState`]).
    pub(in crate::window) selection: SelectionState,
    /// Where a terminal's cursor was before Scout moved it, restored if Scout
    /// is put away.
    pub(in crate::window) scout_origin: Option<(TabId, (usize, usize))>,
    /// The span a yank just took and the instant its highlight burns out. The
    /// span is kept so the expiry only clears a selection still showing what
    /// the yank copied, never one made since.
    pub(in crate::window) yank_flash: Option<(Selection, Instant)>,
}

/// The furniture the window lays over its panes: what the user reads and
/// picks from, rather than what a tab holds.
#[derive(Default)]
pub(in crate::window) struct Chrome {
    /// The labels laid over the focused pane to jump with one key, and the
    /// last `f`/`t` jump.
    pub(in crate::window) labels: JumpLabels,
    /// The tabbar menus and the right-click context menu (see [`MenuState`]).
    pub(in crate::window) menus: MenuState,
    /// What has been said to the user: the toasts on screen and the
    /// session's history behind the bell.
    pub(in crate::window) notifier: Notifier,
    /// The notifier's cards and tasks, as misty draws them in the top-right
    /// corner.
    pub(in crate::window) cards: misty::Toasts,
    /// The status bar misty draws along the window's bottom.
    pub(in crate::window) status_bar: misty::StatusBar,
    /// The title bar misty draws along the window's top, and the shape the
    /// settings gave it; `None` before the first frame.
    pub(in crate::window) title_bar: Option<(frame::title_bar::TitleBarShape, misty::TitleBar)>,
    /// The tooltip misty hangs under what the pointer rests on.
    pub(in crate::window) tooltip: misty::Tooltip,
    /// The token inspector's card the last frame worked out, for misty to
    /// draw.
    pub(in crate::window) drawn_token_inspect: Option<winter_render::TokenInspectView>,
    /// The pane toolbars the last frame worked out, for misty to draw.
    pub(in crate::window) drawn_toolbars: Vec<winter_render::ToolbarView>,
    /// Where misty drew the toolbars' buttons, for the pointer.
    pub(in crate::window) toolbar_hits: Vec<frame::toolbar::ToolbarHits>,
    /// The find card the last frame worked out, for misty to draw.
    pub(in crate::window) drawn_find: Option<winter_render::FindView>,
    /// Where misty drew the find card, for the pointer.
    pub(in crate::window) find_rect: Option<misty::Rect>,
    /// Each pane's cells as the last frame resolved them, for misty to draw.
    pub(in crate::window) drawn_panes: Vec<winter_render::PanePaint>,
    /// The separators between the panes the last frame drew.
    pub(in crate::window) drawn_separators: Vec<winter_render::PaintFill>,
    /// Each pane's strip of tabs, as misty draws it.
    pub(in crate::window) strips: Vec<strip::view::PaneStrip>,
    /// A tab dragged off its strip, while one is.
    pub(in crate::window) tab_drag: Option<strip::view::TabDragging>,
    /// The key menu misty draws, while one is up.
    pub(in crate::window) transient: Option<misty::Transient>,
    /// The title of the key menu up, to tell a new menu from the same one.
    pub(in crate::window) transient_title: Option<String>,
    /// The key menu as the last frame worked it out, for misty to draw and
    /// take the pointer by.
    pub(in crate::window) drawn_transient: Option<winter_render::TransientView>,
    /// The which-key popup misty draws.
    pub(in crate::window) which_key: misty::WhichKey,
    /// The which-key popup as the last frame worked it out, for misty to
    /// draw and take clicks by.
    pub(in crate::window) drawn_which_key: Option<winter_render::WhichKeyView>,
    /// Each pane's popups as the last frame worked them out, for misty to
    /// draw.
    pub(in crate::window) pane_popups: Vec<frame::pane_popups::PanePopups>,
    /// The status bar the last frame built, for misty's bar to draw and
    /// take the pointer by.
    pub(in crate::window) drawn_status: Option<winter_render::StatusBar>,
    /// What holds the keyboard over the panes, if anything: a palette, a
    /// question, a name being typed, the settings page (see [`Overlay`]).
    pub(in crate::window) overlay: Option<Overlay>,
    /// The dialogs of the window's own (an error, a password asked for)
    /// that came up while another was open, oldest first: each opens once
    /// the one before it is answered.
    pub(in crate::window) held: VecDeque<Overlay>,
    /// The focused tab's project and branch as the status bar last read
    /// them off the disk.
    pub(in crate::window) place: status::PlaceCache,
    /// The transient a key just closed, whose card the list its choice asks
    /// for is drawn on.
    pub(in crate::window) transient_card: Option<TransientCard>,
    /// The part of the status bar's right side under the pointer, drawn on a
    /// hover background.
    pub(in crate::window) status_hover: Option<winter_render::StatusHover>,
}

/// What the window keeps for its editors beyond the tabs themselves: the
/// services an editor's outcome asks of the window.
pub(in crate::window) struct EditorHost {
    /// For each commit-message editor tab, the pane beside it showing what is
    /// staged, which closes with it.
    pub(in crate::window) commit_companions: HashMap<TabId, TabId>,
    /// The builds the Compile tabs run, and which was started last.
    pub(in crate::window) compile: editor::compile::Builds,
    /// The Vim-style text cursor over an editor, while one is up. At most one
    /// exists: it belongs to whichever editor tab `v` was pressed in.
    pub(in crate::window) editor_cursor: Option<EditorCursor>,
    /// What the window keeps for its extensions.
    pub(in crate::window) extensions: extension_host::Extensions,
    /// Slow work editors have asked for, and the answers coming back.
    pub(in crate::window) jobs: Jobs,
    /// The language servers running, started when asked.
    pub(in crate::window) lsp: editor::lsp::LspState,
    /// What the window keeps for the questions editors ask.
    pub(in crate::window) prompts: editor::prompt::Prompts,
    /// The tab currently showing the token / scope inspector, if active.
    pub(in crate::window) token_inspect: Option<TabId>,
}

/// A block drawn natively, on the canvas: its content, the texture it is
/// drawn into under `id`, and the band of rows it is placed in.
pub(in crate::window) struct ImageBlock {
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

/// The labels laid over the focused pane to jump with one key, and the last
/// `f`/`t` jump, which `;` and `,` repeat.
#[derive(Default)]
pub(in crate::window) struct JumpLabels {
    /// The `f`/`F`/`t`/`T` jump overlay: one label per candidate landing spot
    /// in the focused pane, shown when the target character occurs more than
    /// once so the user picks one with a single keystroke instead of
    /// repeating `;`.
    pub(in crate::window) find: Option<Vec<FindLabel>>,
    /// The last char-search (`f`/`F`/`t`/`T`), repeated by `;` and `,`, in a
    /// terminal or an editor.
    pub(in crate::window) last_find: Option<FindChar>,
    /// Quick Select's labels, one on each thing worth copying.
    pub(in crate::window) quick: Option<Vec<QuickLabel>>,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::window) struct QuickLabel {
    pub col: usize,
    pub label: char,
    pub row: usize,
}

/// One easymotion-style landing spot for an `f`/`F`/`t`/`T` jump: the label key to
/// press and the viewport cell the cursor lands on (already adjusted for `t`/`T`,
/// which stop one cell short of the target character).
#[derive(Clone, Copy, Debug)]
pub(in crate::window) struct FindLabel {
    pub col: usize,
    pub label: char,
    pub row: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct Selection {
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
pub(in crate::window) struct LastVisual {
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
// JumpLabels
// ========================================================================

impl JumpLabels {
    /// Whether labels are up over the pane, waiting for a key.
    pub(in crate::window) fn are_up(&self) -> bool {
        self.find.is_some() || self.quick.is_some()
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
}
